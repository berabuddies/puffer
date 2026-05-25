use axum::http::HeaderMap;
use puffer_connector_core::InboundMessage;
use serde_json::Value;
use std::fmt::Write as _;

use super::super::{header_value, number_or_string, pointer_string, snippet, string_field};

/// Converts a Grafana Alerting webhook payload into an inbound Puffer message.
pub(super) fn grafana_inbound(headers: &HeaderMap, payload: &Value) -> Option<InboundMessage> {
    if !grafana_payload_shape(headers, payload) {
        return None;
    }

    let receiver = string_field(payload, "receiver").unwrap_or("grafana");
    let status = string_field(payload, "status").unwrap_or("alert");
    let state = string_field(payload, "state");
    let alerts = payload.get("alerts")?.as_array()?;
    let group_key = string_field(payload, "groupKey");
    let conversation_id = grafana_conversation_id(receiver, group_key, alerts);
    let text = grafana_message(receiver, status, state, group_key, payload, alerts);

    Some(InboundMessage {
        conversation_id,
        user_id: Some("grafana".to_string()),
        text,
        thread_id: None,
        is_group: false,
        bot_mentioned: true,
        from_bot: false,
    })
}

fn grafana_payload_shape(headers: &HeaderMap, payload: &Value) -> bool {
    let Some(alerts) = payload.get("alerts").and_then(Value::as_array) else {
        return false;
    };
    if alerts.is_empty() {
        return false;
    }
    if !matches!(string_field(payload, "status"), Some("firing" | "resolved")) {
        return false;
    }
    header_value(headers, "x-grafana-alerting-signature").is_some()
        || matches!(string_field(payload, "state"), Some("alerting" | "ok"))
        || payload.get("orgId").and_then(number_or_string).is_some()
}

fn grafana_conversation_id(receiver: &str, group_key: Option<&str>, alerts: &[Value]) -> String {
    let receiver = normalize_grafana_part(receiver);
    if let Some(group_key) = group_key.filter(|value| !value.trim().is_empty()) {
        return format!(
            "grafana:{receiver}:group:{}",
            normalize_grafana_part(group_key)
        );
    }
    if alerts.len() == 1 {
        if let Some(fingerprint) = string_field(&alerts[0], "fingerprint") {
            return format!(
                "grafana:{receiver}:alert:{}",
                normalize_grafana_part(fingerprint)
            );
        }
        if let Some(alertname) = pointer_string(&alerts[0], "/labels/alertname") {
            return format!(
                "grafana:{receiver}:alert:{}",
                normalize_grafana_part(alertname)
            );
        }
    }
    format!("grafana:{receiver}:group:grafana")
}

fn grafana_message(
    receiver: &str,
    status: &str,
    state: Option<&str>,
    group_key: Option<&str>,
    payload: &Value,
    alerts: &[Value],
) -> String {
    let mut lines = vec![
        format!("Grafana {status} notification"),
        format!("Receiver: {receiver}"),
        format!("Alerts: {}", alerts.len()),
    ];
    if let Some(state) = state {
        lines.push(format!("State: {state}"));
    }
    if let Some(group_key) = group_key {
        lines.push(format!("Group: {group_key}"));
    }
    if let Some(labels) = grafana_map_summary(payload.get("commonLabels"), 6) {
        lines.push(format!("Common labels: {labels}"));
    }
    if let Some(title) = string_field(payload, "title") {
        lines.push(format!("Title: {}", snippet(title)));
    }
    if let Some(message) = string_field(payload, "message") {
        lines.push(String::new());
        lines.push(snippet(message));
    }
    for (index, alert) in alerts.iter().take(5).enumerate() {
        lines.push(String::new());
        lines.push(grafana_alert_line(index + 1, alert));
        if let Some(summary) = grafana_annotation(alert, "summary")
            .or_else(|| grafana_annotation(alert, "description"))
        {
            lines.push(format!("Summary: {}", snippet(&summary)));
        }
        if let Some(url) = grafana_alert_url(alert) {
            lines.push(format!("Source: {url}"));
        }
    }
    if alerts.len() > 5 {
        lines.push(format!("... {} more alert(s)", alerts.len() - 5));
    }
    if let Some(truncated) = payload.get("truncatedAlerts").and_then(number_or_string) {
        if truncated != "0" {
            lines.push(format!("Truncated alerts: {truncated}"));
        }
    }
    if let Some(external_url) = string_field(payload, "externalURL") {
        lines.push(format!("Grafana: {external_url}"));
    }
    lines.join("\n")
}

fn grafana_alert_line(index: usize, alert: &Value) -> String {
    let status = string_field(alert, "status").unwrap_or("alert");
    let name = pointer_string(alert, "/labels/alertname")
        .or_else(|| pointer_string(alert, "/annotations/summary"))
        .or_else(|| string_field(alert, "fingerprint"))
        .unwrap_or("unnamed");
    let labels = grafana_map_summary(alert.get("labels"), 4);
    match labels {
        Some(labels) => format!("Alert {index}: {} {status} {labels}", snippet(name)),
        None => format!("Alert {index}: {} {status}", snippet(name)),
    }
}

fn grafana_annotation(alert: &Value, field: &str) -> Option<String> {
    alert
        .get("annotations")
        .and_then(|annotations| string_field(annotations, field))
        .map(str::to_string)
}

fn grafana_alert_url(alert: &Value) -> Option<String> {
    string_field(alert, "dashboardURL")
        .filter(|value| !value.is_empty())
        .or_else(|| string_field(alert, "panelURL").filter(|value| !value.is_empty()))
        .or_else(|| string_field(alert, "generatorURL").filter(|value| !value.is_empty()))
        .map(str::to_string)
}

fn grafana_map_summary(value: Option<&Value>, limit: usize) -> Option<String> {
    let object = value?.as_object()?;
    if object.is_empty() {
        return None;
    }
    let mut parts = object
        .iter()
        .filter_map(|(key, value)| grafana_value_label(value).map(|value| format!("{key}={value}")))
        .take(limit + 1)
        .collect::<Vec<_>>();
    if parts.is_empty() {
        return None;
    }
    if parts.len() > limit {
        parts.truncate(limit);
        parts.push("...".to_string());
    }
    Some(parts.join(" "))
}

fn grafana_value_label(value: &Value) -> Option<String> {
    match value {
        Value::String(value) if !value.trim().is_empty() => Some(value.clone()),
        Value::Number(_) => number_or_string(value),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

fn normalize_grafana_part(value: &str) -> String {
    let mut normalized = String::new();
    for ch in value.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            normalized.push(ch.to_ascii_lowercase());
        } else {
            let _ = write!(&mut normalized, "_{:x}", ch as u32);
        }
    }
    let normalized = normalized.trim_matches('_');
    if normalized.is_empty() {
        "grafana".to_string()
    } else {
        normalized.chars().take(96).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grafana_alert_group_payload_maps_to_inbound_message() {
        let mut headers = HeaderMap::new();
        headers.insert("x-grafana-alerting-signature", "fake".parse().unwrap());
        let payload = serde_json::json!({
            "receiver": "My Super Webhook",
            "status": "firing",
            "orgId": 1,
            "alerts": [
                {
                    "status": "firing",
                    "labels": {
                        "alertname": "High memory usage",
                        "team": "blue",
                        "zone": "us-1"
                    },
                    "annotations": {
                        "description": "The system has high memory usage",
                        "runbook_url": "https://myrunbook.com/runbook/1234",
                        "summary": "This alert was triggered for zone us-1"
                    },
                    "startsAt": "2021-10-12T09:51:03.157076+02:00",
                    "generatorURL": "https://play.grafana.org/alerting/1afz29v7z/edit",
                    "fingerprint": "c6eadffa33fcdf37"
                },
                {
                    "status": "firing",
                    "labels": {
                        "alertname": "High CPU usage",
                        "team": "blue",
                        "zone": "eu-1"
                    },
                    "annotations": {
                        "description": "The system has high CPU usage"
                    },
                    "generatorURL": "https://play.grafana.org/alerting/d1rdpdv7k/edit",
                    "fingerprint": "bc97ff14869b13e3"
                }
            ],
            "groupLabels": {},
            "commonLabels": {
                "team": "blue"
            },
            "commonAnnotations": {},
            "externalURL": "https://play.grafana.org/",
            "version": "1",
            "groupKey": "{}:{}",
            "truncatedAlerts": 0,
            "title": "[FIRING:2]  (blue)",
            "state": "alerting",
            "message": "**Firing**\n\nLabels:\n - alertname = High memory usage"
        });

        let inbound = grafana_inbound(&headers, &payload).expect("grafana inbound");

        assert_eq!(
            inbound.conversation_id,
            "grafana:my_20super_20webhook:group:7b_7d_3a_7b_7d"
        );
        assert_eq!(inbound.user_id.as_deref(), Some("grafana"));
        assert!(inbound.text.contains("Grafana firing notification"));
        assert!(inbound.text.contains("Common labels: team=blue"));
        assert!(inbound.text.contains("Alert 1: High memory usage firing"));
        assert!(inbound
            .text
            .contains("This alert was triggered for zone us-1"));
    }

    #[test]
    fn grafana_single_alert_without_group_uses_fingerprint_thread() {
        let headers = HeaderMap::new();
        let payload = serde_json::json!({
            "receiver": "Ops Alerts",
            "status": "resolved",
            "orgId": 1,
            "alerts": [
                {
                    "status": "resolved",
                    "labels": {
                        "alertname": "Disk full",
                        "team": "infra"
                    },
                    "annotations": {
                        "summary": "Disk usage returned below threshold"
                    },
                    "fingerprint": "abc123"
                }
            ],
            "state": "ok"
        });

        let inbound = grafana_inbound(&headers, &payload).expect("grafana inbound");

        assert_eq!(inbound.conversation_id, "grafana:ops_20alerts:alert:abc123");
        assert!(inbound.text.contains("Grafana resolved notification"));
        assert!(inbound.text.contains("State: ok"));
    }

    #[test]
    fn grafana_shape_does_not_claim_generic_alertmanager_payloads() {
        let headers = HeaderMap::new();
        let payload = serde_json::json!({
            "receiver": "webhook",
            "status": "firing",
            "alerts": [
                {
                    "status": "firing",
                    "labels": {"alertname": "Generic alert"}
                }
            ],
            "groupKey": "{}:{}"
        });

        assert!(grafana_inbound(&headers, &payload).is_none());
    }

    #[test]
    fn grafana_shape_requires_alert_array() {
        let mut headers = HeaderMap::new();
        headers.insert("x-grafana-alerting-signature", "fake".parse().unwrap());
        let payload = serde_json::json!({
            "receiver": "webhook",
            "status": "firing",
            "alerts": "not an array"
        });

        assert!(grafana_inbound(&headers, &payload).is_none());
    }
}

use axum::http::HeaderMap;
use puffer_connector_core::InboundMessage;
use serde_json::Value;
use std::fmt::Write as _;

use super::super::{number_or_string, pointer_string, snippet, string_field};

/// Converts a Prometheus Alertmanager webhook payload into an inbound Puffer message.
pub(super) fn alertmanager_inbound(
    _headers: &HeaderMap,
    payload: &Value,
) -> Option<InboundMessage> {
    if !alertmanager_payload_shape(payload) {
        return None;
    }

    let receiver = string_field(payload, "receiver").unwrap_or("alertmanager");
    let status = string_field(payload, "status").unwrap_or("alert");
    let alerts = payload.get("alerts")?.as_array()?;
    let group_key = string_field(payload, "groupKey");
    let conversation_id = alertmanager_conversation_id(receiver, group_key, alerts);
    let text = alertmanager_message(receiver, status, group_key, payload, alerts);

    Some(InboundMessage {
        conversation_id,
        user_id: Some("alertmanager".to_string()),
        text,
        thread_id: None,
        is_group: false,
        bot_mentioned: true,
        from_bot: false,
    })
}

fn alertmanager_payload_shape(payload: &Value) -> bool {
    let Some(alerts) = payload.get("alerts").and_then(Value::as_array) else {
        return false;
    };
    if alerts.is_empty() {
        return false;
    }
    if !matches!(string_field(payload, "status"), Some("firing" | "resolved")) {
        return false;
    }
    if string_field(payload, "receiver").is_none() {
        return false;
    }
    if payload.get("orgId").is_some() || string_field(payload, "state").is_some() {
        return false;
    }
    matches!(
        payload.get("version").and_then(number_or_string).as_deref(),
        Some("4")
    ) || payload.get("groupKey").and_then(Value::as_str).is_some()
}

fn alertmanager_conversation_id(
    receiver: &str,
    group_key: Option<&str>,
    alerts: &[Value],
) -> String {
    let receiver = normalize_alertmanager_part(receiver);
    if let Some(group_key) = group_key.filter(|value| !value.trim().is_empty()) {
        return format!(
            "alertmanager:{receiver}:group:{}",
            normalize_alertmanager_part(group_key)
        );
    }
    if alerts.len() == 1 {
        if let Some(fingerprint) = string_field(&alerts[0], "fingerprint") {
            return format!(
                "alertmanager:{receiver}:alert:{}",
                normalize_alertmanager_part(fingerprint)
            );
        }
        if let Some(alertname) = pointer_string(&alerts[0], "/labels/alertname") {
            return format!(
                "alertmanager:{receiver}:alert:{}",
                normalize_alertmanager_part(alertname)
            );
        }
    }
    format!("alertmanager:{receiver}:group:alertmanager")
}

fn alertmanager_message(
    receiver: &str,
    status: &str,
    group_key: Option<&str>,
    payload: &Value,
    alerts: &[Value],
) -> String {
    let mut lines = vec![
        format!("Alertmanager {status} notification"),
        format!("Receiver: {receiver}"),
        format!("Alerts: {}", alerts.len()),
    ];
    if let Some(group_key) = group_key {
        lines.push(format!("Group: {group_key}"));
    }
    if let Some(labels) = alertmanager_map_summary(payload.get("groupLabels"), 6) {
        lines.push(format!("Group labels: {labels}"));
    }
    if let Some(labels) = alertmanager_map_summary(payload.get("commonLabels"), 6) {
        lines.push(format!("Common labels: {labels}"));
    }
    if let Some(annotations) = alertmanager_map_summary(payload.get("commonAnnotations"), 4) {
        lines.push(format!("Common annotations: {annotations}"));
    }
    for (index, alert) in alerts.iter().take(5).enumerate() {
        lines.push(String::new());
        lines.push(alertmanager_alert_line(index + 1, alert));
        if let Some(summary) = alertmanager_annotation(alert, "summary")
            .or_else(|| alertmanager_annotation(alert, "description"))
        {
            lines.push(format!("Summary: {}", snippet(&summary)));
        }
        if let Some(url) = alertmanager_alert_url(alert) {
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
        lines.push(format!("Alertmanager: {external_url}"));
    }
    lines.join("\n")
}

fn alertmanager_alert_line(index: usize, alert: &Value) -> String {
    let status = string_field(alert, "status").unwrap_or("alert");
    let name = pointer_string(alert, "/labels/alertname")
        .or_else(|| pointer_string(alert, "/annotations/summary"))
        .or_else(|| string_field(alert, "fingerprint"))
        .unwrap_or("unnamed");
    let labels = alertmanager_map_summary(alert.get("labels"), 4);
    match labels {
        Some(labels) => format!("Alert {index}: {} {status} {labels}", snippet(name)),
        None => format!("Alert {index}: {} {status}", snippet(name)),
    }
}

fn alertmanager_annotation(alert: &Value, field: &str) -> Option<String> {
    alert
        .get("annotations")
        .and_then(|annotations| string_field(annotations, field))
        .map(str::to_string)
}

fn alertmanager_alert_url(alert: &Value) -> Option<String> {
    string_field(alert, "generatorURL")
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn alertmanager_map_summary(value: Option<&Value>, limit: usize) -> Option<String> {
    let object = value?.as_object()?;
    if object.is_empty() {
        return None;
    }
    let mut parts = object
        .iter()
        .filter_map(|(key, value)| {
            alertmanager_value_label(value).map(|value| format!("{key}={value}"))
        })
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

fn alertmanager_value_label(value: &Value) -> Option<String> {
    match value {
        Value::String(value) if !value.trim().is_empty() => Some(value.clone()),
        Value::Number(_) => number_or_string(value),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

fn normalize_alertmanager_part(value: &str) -> String {
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
        "alertmanager".to_string()
    } else {
        normalized.chars().take(96).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alertmanager_group_payload_maps_to_inbound_message() {
        let headers = HeaderMap::new();
        let payload = serde_json::json!({
            "version": "4",
            "groupKey": "{}/{severity=\"critical\"}:{alertname=\"InstanceDown\"}",
            "truncatedAlerts": 0,
            "status": "firing",
            "receiver": "Ops Alerts",
            "groupLabels": {
                "alertname": "InstanceDown"
            },
            "commonLabels": {
                "severity": "critical",
                "team": "platform"
            },
            "commonAnnotations": {
                "runbook_url": "https://runbooks.example.com/instance-down"
            },
            "externalURL": "https://alertmanager.example.com",
            "alerts": [
                {
                    "status": "firing",
                    "labels": {
                        "alertname": "InstanceDown",
                        "instance": "db-01",
                        "severity": "critical"
                    },
                    "annotations": {
                        "summary": "db-01 is down"
                    },
                    "startsAt": "2026-05-25T12:00:00Z",
                    "endsAt": "0001-01-01T00:00:00Z",
                    "generatorURL": "https://prometheus.example.com/graph?g0.expr=up",
                    "fingerprint": "abc123"
                },
                {
                    "status": "firing",
                    "labels": {
                        "alertname": "InstanceDown",
                        "instance": "db-02",
                        "severity": "critical"
                    },
                    "annotations": {
                        "description": "db-02 has not reported metrics"
                    },
                    "generatorURL": "https://prometheus.example.com/graph?g0.expr=up",
                    "fingerprint": "def456"
                }
            ]
        });

        let inbound = alertmanager_inbound(&headers, &payload).expect("alertmanager inbound");

        assert_eq!(
            inbound.conversation_id,
            "alertmanager:ops_20alerts:group:7b_7d_2f_7bseverity_3d_22critical_22_7d_3a_7balertname_3d_22instancedown_22_7d"
        );
        assert_eq!(inbound.user_id.as_deref(), Some("alertmanager"));
        assert!(inbound.text.contains("Alertmanager firing notification"));
        assert!(inbound
            .text
            .contains("Common labels: severity=critical team=platform"));
        assert!(inbound.text.contains("Alert 1: InstanceDown firing"));
        assert!(inbound.text.contains("Summary: db-01 is down"));
        assert!(inbound
            .text
            .contains("Alertmanager: https://alertmanager.example.com"));
    }

    #[test]
    fn alertmanager_single_alert_without_group_uses_fingerprint_thread() {
        let headers = HeaderMap::new();
        let payload = serde_json::json!({
            "version": "4",
            "status": "resolved",
            "receiver": "SRE",
            "alerts": [
                {
                    "status": "resolved",
                    "labels": {
                        "alertname": "DiskFull",
                        "instance": "web-01"
                    },
                    "annotations": {
                        "summary": "Disk usage returned below threshold"
                    },
                    "fingerprint": "disk123"
                }
            ]
        });

        let inbound = alertmanager_inbound(&headers, &payload).expect("alertmanager inbound");

        assert_eq!(inbound.conversation_id, "alertmanager:sre:alert:disk123");
        assert!(inbound.text.contains("Alertmanager resolved notification"));
    }

    #[test]
    fn alertmanager_shape_does_not_claim_grafana_payloads() {
        let headers = HeaderMap::new();
        let payload = serde_json::json!({
            "receiver": "webhook",
            "status": "firing",
            "orgId": 1,
            "state": "alerting",
            "alerts": [
                {
                    "status": "firing",
                    "labels": {"alertname": "Grafana alert"}
                }
            ],
            "groupKey": "{}:{}"
        });

        assert!(alertmanager_inbound(&headers, &payload).is_none());
    }

    #[test]
    fn alertmanager_shape_requires_alert_array() {
        let headers = HeaderMap::new();
        let payload = serde_json::json!({
            "version": "4",
            "receiver": "webhook",
            "status": "firing",
            "alerts": "not an array"
        });

        assert!(alertmanager_inbound(&headers, &payload).is_none());
    }
}

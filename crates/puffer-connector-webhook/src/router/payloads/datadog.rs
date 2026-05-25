use axum::http::HeaderMap;
use puffer_connector_core::InboundMessage;
use serde_json::Value;
use std::fmt::Write as _;

use super::super::{header_value, number_or_string, snippet, string_field};

/// Converts a Datadog webhook payload into an inbound Puffer message.
pub(super) fn datadog_inbound(headers: &HeaderMap, payload: &Value) -> Option<InboundMessage> {
    if !datadog_payload_shape(headers, payload) {
        return None;
    }

    let title = datadog_title(payload).unwrap_or_else(|| "Datadog notification".to_string());
    let conversation_id = datadog_conversation_id(payload, &title);
    let actor = datadog_actor(payload).unwrap_or_else(|| "datadog".to_string());
    let text = datadog_message(payload, &title);

    Some(InboundMessage {
        conversation_id,
        user_id: Some(actor),
        text,
        thread_id: None,
        is_group: false,
        bot_mentioned: true,
        from_bot: false,
    })
}

fn datadog_payload_shape(headers: &HeaderMap, payload: &Value) -> bool {
    let source_hint = datadog_source_hint(headers, payload);
    let has_message = datadog_title(payload).is_some()
        || datadog_message_body(payload).is_some()
        || datadog_alert_status(payload).is_some();
    if source_hint {
        return has_message;
    }

    let has_alert_hint = datadog_alert_transition(payload).is_some()
        || datadog_alert_cycle_key(payload).is_some()
        || (datadog_alert_id(payload).is_some()
            && (datadog_alert_metric(payload).is_some()
                || datadog_alert_query(payload).is_some()
                || has_message));
    let known_event = datadog_event_type(payload)
        .as_deref()
        .is_some_and(datadog_known_event_type);
    let datadog_link = datadog_link(payload).is_some_and(|link| {
        let link = link.to_ascii_lowercase();
        link.contains("datadoghq.com") || link.contains("datadoghq.eu")
    });

    has_alert_hint || (known_event && has_message) || (datadog_link && has_message)
}

fn datadog_source_hint(headers: &HeaderMap, payload: &Value) -> bool {
    header_value(headers, "x-datadog-webhook").is_some()
        || ["source", "provider", "puffer_provider", "integration"]
            .iter()
            .filter_map(|field| string_field(payload, field))
            .any(|value| value.eq_ignore_ascii_case("datadog"))
}

fn datadog_known_event_type(value: &str) -> bool {
    matches!(
        value,
        "ci_pipelines_alert"
            | "ci_tests_alert"
            | "composite_monitor"
            | "error_tracking_alert"
            | "event_alert"
            | "event_v2_alert"
            | "log_alert"
            | "monitor_slo_alert"
            | "metric_slo_alert"
            | "outlier_monitor"
            | "process_alert"
            | "query_alert_monitor"
            | "rum_alert"
            | "service_check"
            | "synthetics_alert"
            | "trace_analytics_alert"
    )
}

fn datadog_conversation_id(payload: &Value, title: &str) -> String {
    if let Some(cycle_key) = datadog_alert_cycle_key(payload) {
        return format!("datadog:alert_cycle:{}", normalize_datadog_part(&cycle_key));
    }
    if let Some(alert_id) = datadog_alert_id(payload) {
        return format!("datadog:alert:{}", normalize_datadog_part(&alert_id));
    }
    if let Some(incident) = datadog_incident_id(payload) {
        return format!("datadog:incident:{}", normalize_datadog_part(&incident));
    }
    if let Some(event_id) = datadog_event_id(payload) {
        return format!("datadog:event:{}", normalize_datadog_part(&event_id));
    }
    format!("datadog:event:{}", normalize_datadog_part(title))
}

fn datadog_message(payload: &Value, title: &str) -> String {
    let headline = datadog_alert_transition(payload)
        .or_else(|| datadog_alert_type(payload))
        .or_else(|| datadog_event_type(payload))
        .unwrap_or_else(|| "event".to_string());
    let mut lines = vec![
        format!("Datadog {} notification", snippet(&headline)),
        format!("Title: {}", snippet(title)),
    ];

    if let Some(status) = datadog_alert_status(payload) {
        lines.push(format!("Status: {}", snippet(&status)));
    }
    if let Some(priority) = datadog_alert_priority(payload) {
        lines.push(format!("Priority: {}", snippet(&priority)));
    }
    if let Some(hostname) = datadog_hostname(payload) {
        lines.push(format!("Host: {}", snippet(&hostname)));
    }
    if let Some(scope) = datadog_alert_scope(payload) {
        lines.push(format!("Scope: {}", snippet(&scope)));
    }
    if let Some(metric) = datadog_alert_metric(payload) {
        lines.push(format!("Metric: {}", snippet(&metric)));
    }
    if let Some(query) = datadog_alert_query(payload) {
        lines.push(format!("Query: {}", snippet(&query)));
    }
    if let Some(body) = datadog_message_body(payload) {
        lines.push(format!("Message: {}", snippet(&body)));
    }
    if let Some(tags) = datadog_tags(payload) {
        lines.push(format!("Tags: {}", snippet(&tags)));
    }
    if let Some(logs_sample) = datadog_logs_sample(payload) {
        lines.push(format!("Logs sample: {}", snippet(&logs_sample)));
    }
    if let Some(snapshot) = datadog_snapshot(payload) {
        lines.push(format!("Snapshot: {snapshot}"));
    }
    if let Some(link) = datadog_link(payload) {
        lines.push(format!("Datadog: {link}"));
    }
    lines.join("\n")
}

fn datadog_title(payload: &Value) -> Option<String> {
    datadog_alert_field(payload, &["title", "alert_title"])
        .or_else(|| datadog_event_field(payload, &["title", "event_title"]))
        .or_else(|| datadog_field(payload, &["title", "summary", "event_title", "alert_title"]))
}

fn datadog_message_body(payload: &Value) -> Option<String> {
    datadog_event_field(payload, &["message", "msg", "event_msg"])
        .or_else(|| {
            datadog_field(
                payload,
                &["message", "body", "text_only_msg", "text only message"],
            )
        })
        .or_else(|| datadog_alert_field(payload, &["status", "alert_status"]))
}

fn datadog_alert_cycle_key(payload: &Value) -> Option<String> {
    datadog_alert_field(payload, &["cycle_key", "cycle key", "alert_cycle_key"])
        .or_else(|| datadog_field(payload, &["alert_cycle_key", "cycle_key"]))
}

fn datadog_alert_id(payload: &Value) -> Option<String> {
    datadog_parent_field(payload, "alert", &["id", "alert_id", "monitor_id"])
        .or_else(|| datadog_field(payload, &["alert_id", "monitor_id"]))
}

fn datadog_incident_id(payload: &Value) -> Option<String> {
    datadog_parent_field(
        payload,
        "incident",
        &["uuid", "public_id", "public id", "id"],
    )
    .or_else(|| datadog_field(payload, &["incident_uuid", "incident_public_id"]))
}

fn datadog_event_id(payload: &Value) -> Option<String> {
    datadog_event_field(payload, &["id", "event_id"]).or_else(|| datadog_field(payload, &["id"]))
}

fn datadog_alert_transition(payload: &Value) -> Option<String> {
    datadog_alert_field(payload, &["transition", "alert_transition"])
        .or_else(|| datadog_field(payload, &["alert_transition", "transition"]))
}

fn datadog_alert_status(payload: &Value) -> Option<String> {
    datadog_alert_field(payload, &["status", "alert_status"])
        .or_else(|| datadog_field(payload, &["alert_status", "status"]))
}

fn datadog_alert_type(payload: &Value) -> Option<String> {
    datadog_alert_field(payload, &["type", "alert_type"])
        .or_else(|| datadog_field(payload, &["alert_type"]))
}

fn datadog_event_type(payload: &Value) -> Option<String> {
    datadog_event_field(payload, &["type", "event_type"])
        .or_else(|| datadog_field(payload, &["event_type"]))
}

fn datadog_alert_priority(payload: &Value) -> Option<String> {
    datadog_alert_field(payload, &["priority", "alert_priority"])
        .or_else(|| datadog_field(payload, &["alert_priority", "priority"]))
}

fn datadog_alert_metric(payload: &Value) -> Option<String> {
    datadog_alert_field(payload, &["metric", "alert_metric"])
        .or_else(|| datadog_field(payload, &["alert_metric", "metric"]))
}

fn datadog_alert_query(payload: &Value) -> Option<String> {
    datadog_alert_field(payload, &["query", "alert_query"])
        .or_else(|| datadog_field(payload, &["alert_query", "query"]))
}

fn datadog_alert_scope(payload: &Value) -> Option<String> {
    datadog_alert_field(payload, &["scope", "alert_scope"])
        .or_else(|| datadog_field(payload, &["alert_scope", "scope"]))
}

fn datadog_hostname(payload: &Value) -> Option<String> {
    datadog_field(payload, &["hostname", "host"])
}

fn datadog_tags(payload: &Value) -> Option<String> {
    datadog_field(payload, &["tags"])
}

fn datadog_logs_sample(payload: &Value) -> Option<String> {
    datadog_field(payload, &["logs_sample", "logs sample"])
}

fn datadog_snapshot(payload: &Value) -> Option<String> {
    datadog_field(payload, &["snapshot"])
}

fn datadog_link(payload: &Value) -> Option<String> {
    datadog_field(payload, &["link", "url", "event_url", "datadog_url"])
        .or_else(|| datadog_event_field(payload, &["url", "link"]))
        .or_else(|| datadog_parent_field(payload, "incident", &["url"]))
}

fn datadog_actor(payload: &Value) -> Option<String> {
    datadog_field(payload, &["user", "username", "email"])
}

fn datadog_alert_field(payload: &Value, fields: &[&str]) -> Option<String> {
    datadog_parent_field(payload, "alert", fields)
        .or_else(|| datadog_field(payload, fields))
        .or_else(|| {
            let prefixed = fields
                .iter()
                .map(|field| format!("alert_{}", field.replace(' ', "_")))
                .collect::<Vec<_>>();
            let prefixed = prefixed.iter().map(String::as_str).collect::<Vec<_>>();
            datadog_field(payload, &prefixed)
        })
}

fn datadog_event_field(payload: &Value, fields: &[&str]) -> Option<String> {
    datadog_parent_field(payload, "event", fields)
        .or_else(|| datadog_field(payload, fields))
        .or_else(|| {
            let prefixed = fields
                .iter()
                .map(|field| format!("event_{}", field.replace(' ', "_")))
                .collect::<Vec<_>>();
            let prefixed = prefixed.iter().map(String::as_str).collect::<Vec<_>>();
            datadog_field(payload, &prefixed)
        })
}

fn datadog_parent_field(payload: &Value, parent: &str, fields: &[&str]) -> Option<String> {
    payload
        .get(parent)
        .and_then(|value| datadog_field(value, fields))
}

fn datadog_field(value: &Value, fields: &[&str]) -> Option<String> {
    fields
        .iter()
        .find_map(|field| value.get(*field).and_then(datadog_value_label))
}

fn datadog_value_label(value: &Value) -> Option<String> {
    match value {
        Value::String(value) if !value.trim().is_empty() => Some(value.clone()),
        Value::Number(_) => number_or_string(value),
        Value::Bool(value) => Some(value.to_string()),
        Value::Array(values) => {
            let joined = values
                .iter()
                .filter_map(datadog_value_label)
                .collect::<Vec<_>>()
                .join(", ");
            (!joined.is_empty()).then_some(joined)
        }
        Value::Object(object) => {
            for field in ["title", "name", "message", "id", "url", "uuid"] {
                if let Some(value) = object.get(field).and_then(datadog_value_label) {
                    return Some(value);
                }
            }
            serde_json::to_string(value).ok()
        }
        _ => None,
    }
}

fn normalize_datadog_part(value: &str) -> String {
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
        "datadog".to_string()
    } else {
        normalized.chars().take(96).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn datadog_nested_monitor_payload_maps_to_inbound_message() {
        let headers = HeaderMap::new();
        let payload = serde_json::json!({
            "source": "datadog",
            "aggregation_key": "9bd4ac313a4d1e8fae2482df7b77628",
            "alert": {
                "cycle_key": "cycle-1",
                "id": "1234",
                "metric": "system.load.1",
                "priority": "P1",
                "query": "avg(last_5m):avg:system.load.1{host:web-01} > 2",
                "scope": "host:web-01, env:prod",
                "status": "system.load.1 over host:web-01 was > 2 during the last 5m",
                "title": "[Triggered on {host:web-01}] Host is Down",
                "transition": "Triggered",
                "type": "error"
            },
            "event": {
                "message": "CPU and load are above threshold",
                "title": "[Triggered] Host is Down",
                "type": "query_alert_monitor"
            },
            "hostname": "web-01",
            "link": "https://app.datadoghq.com/monitors/1234",
            "tags": "monitor, env:prod, service:web",
            "text_only_msg": "Host web-01 is down"
        });

        let inbound = datadog_inbound(&headers, &payload).expect("datadog inbound");

        assert_eq!(inbound.conversation_id, "datadog:alert_cycle:cycle_2d1");
        assert_eq!(inbound.user_id.as_deref(), Some("datadog"));
        assert!(inbound.text.contains("Datadog Triggered notification"));
        assert!(inbound
            .text
            .contains("Title: [Triggered on {host:web-01}] Host is Down"));
        assert!(inbound.text.contains("Priority: P1"));
        assert!(inbound.text.contains("Host: web-01"));
        assert!(inbound.text.contains("Metric: system.load.1"));
        assert!(inbound
            .text
            .contains("Datadog: https://app.datadoghq.com/monitors/1234"));
    }

    #[test]
    fn datadog_flat_payload_uses_alert_id_thread() {
        let headers = HeaderMap::new();
        let payload = serde_json::json!({
            "alert_id": 5678,
            "alert_transition": "Recovered",
            "alert_title": "[Recovered] API latency",
            "alert_status": "avg latency returned below threshold",
            "event_type": "query_alert_monitor",
            "hostname": "api-01",
            "tags": ["source:alert", "service:api", "env:prod"]
        });

        let inbound = datadog_inbound(&headers, &payload).expect("datadog inbound");

        assert_eq!(inbound.conversation_id, "datadog:alert:5678");
        assert!(inbound.text.contains("Datadog Recovered notification"));
        assert!(inbound
            .text
            .contains("Tags: source:alert, service:api, env:prod"));
    }

    #[test]
    fn datadog_event_payload_uses_known_event_type() {
        let headers = HeaderMap::new();
        let payload = serde_json::json!({
            "event": {
                "type": "synthetics_alert",
                "title": "[Triggered] Checkout test failed",
                "message": "First failing step: submit order"
            },
            "id": "event-42",
            "link": "https://app.datadoghq.com/synthetics/details/abc"
        });

        let inbound = datadog_inbound(&headers, &payload).expect("datadog inbound");

        assert_eq!(inbound.conversation_id, "datadog:event:event_2d42");
        assert!(inbound
            .text
            .contains("Datadog synthetics_alert notification"));
        assert!(inbound
            .text
            .contains("Message: First failing step: submit order"));
    }

    #[test]
    fn datadog_shape_does_not_claim_generic_event_payloads() {
        let headers = HeaderMap::new();
        let payload = serde_json::json!({
            "event_type": "issue",
            "title": "Generic issue event",
            "message": "not from Datadog"
        });

        assert!(datadog_inbound(&headers, &payload).is_none());
    }
}

use axum::http::HeaderMap;
use puffer_connector_core::InboundMessage;
use serde_json::Value;
use std::fmt::Write as _;

use super::super::{header_value, number_or_string, snippet, string_field};

/// Converts a New Relic webhook payload into an inbound Puffer message.
pub(super) fn newrelic_inbound(headers: &HeaderMap, payload: &Value) -> Option<InboundMessage> {
    if !newrelic_payload_shape(headers, payload) {
        return None;
    }

    let title = newrelic_title(payload).unwrap_or_else(|| "New Relic issue".to_string());
    let conversation_id = newrelic_conversation_id(payload, &title);
    let actor = newrelic_actor(payload).unwrap_or_else(|| "newrelic".to_string());
    let text = newrelic_message(payload, &title);

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

fn newrelic_payload_shape(headers: &HeaderMap, payload: &Value) -> bool {
    let has_message = newrelic_title(payload).is_some()
        || newrelic_body(payload).is_some()
        || newrelic_state(payload).is_some();
    if newrelic_source_hint(headers, payload) {
        return has_message;
    }

    let issue_shape = newrelic_issue_id(payload).is_some()
        && (newrelic_link(payload).is_some()
            || newrelic_trigger(payload).is_some()
            || newrelic_account_id(payload).is_some()
            || (newrelic_state(payload).is_some() && newrelic_priority(payload).is_some()));
    let classic_shape = newrelic_incident_id(payload).is_some()
        && (newrelic_condition(payload).is_some()
            || newrelic_link(payload).is_some()
            || (newrelic_event_type(payload)
                .as_deref()
                .is_some_and(|event| event.eq_ignore_ascii_case("incident"))
                && (newrelic_account_id(payload).is_some() || newrelic_policy(payload).is_some())));
    let link_shape = newrelic_link(payload).is_some_and(|link| {
        let link = link.to_ascii_lowercase();
        (link.contains("newrelic.com") || link.contains("newrelic.eu")) && has_message
    });

    issue_shape || classic_shape || link_shape
}

fn newrelic_source_hint(headers: &HeaderMap, payload: &Value) -> bool {
    header_value(headers, "x-newrelic-webhook").is_some()
        || [
            "source",
            "provider",
            "puffer_provider",
            "integration",
            "service",
        ]
        .iter()
        .filter_map(|field| string_field(payload, field))
        .any(is_newrelic_value)
}

fn is_newrelic_value(value: &str) -> bool {
    let normalized = value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    normalized == "newrelic"
}

fn newrelic_conversation_id(payload: &Value, title: &str) -> String {
    if let Some(issue_id) = newrelic_issue_id(payload) {
        return format!("newrelic:issue:{}", normalize_newrelic_part(&issue_id));
    }
    if let Some(incident_id) = newrelic_incident_id(payload) {
        return format!(
            "newrelic:incident:{}",
            normalize_newrelic_part(&incident_id)
        );
    }
    if let Some(trigger) = newrelic_trigger(payload) {
        return format!("newrelic:event:{}", normalize_newrelic_part(&trigger));
    }
    format!("newrelic:event:{}", normalize_newrelic_part(title))
}

fn newrelic_message(payload: &Value, title: &str) -> String {
    let headline = newrelic_trigger(payload)
        .or_else(|| newrelic_event_type(payload))
        .or_else(|| newrelic_state(payload))
        .unwrap_or_else(|| "issue".to_string());
    let mut lines = vec![
        format!("New Relic {} notification", snippet(&headline)),
        format!("Title: {}", snippet(title)),
    ];

    if let Some(state) = newrelic_state(payload) {
        lines.push(format!("State: {}", snippet(&state)));
    }
    if let Some(priority) = newrelic_priority(payload) {
        lines.push(format!("Priority: {}", snippet(&priority)));
    }
    if let Some(condition) = newrelic_condition(payload) {
        lines.push(format!("Condition: {}", snippet(&condition)));
    }
    if let Some(policy) = newrelic_policy(payload) {
        lines.push(format!("Policy: {}", snippet(&policy)));
    }
    if let Some(entities) = newrelic_entities(payload) {
        lines.push(format!("Entities: {}", snippet(&entities)));
    }
    if let Some(incidents) = newrelic_incident_ids(payload) {
        lines.push(format!("Incident IDs: {}", snippet(&incidents)));
    }
    if let Some(account) = newrelic_account_id(payload) {
        lines.push(format!("Account: {}", snippet(&account)));
    }
    if let Some(body) = newrelic_body(payload) {
        if body != title {
            lines.push(format!("Details: {}", snippet(&body)));
        }
    }
    if let Some(runbook) = newrelic_runbook(payload) {
        lines.push(format!("Runbook: {runbook}"));
    }
    if let Some(chart) = newrelic_chart(payload) {
        lines.push(format!("Chart: {chart}"));
    }
    if let Some(link) = newrelic_link(payload) {
        lines.push(format!("New Relic: {link}"));
    }
    lines.join("\n")
}

fn newrelic_issue_id(payload: &Value) -> Option<String> {
    newrelic_root_field(payload, &["issueId", "issue_id"])
}

fn newrelic_incident_id(payload: &Value) -> Option<String> {
    newrelic_root_field(payload, &["incident_id", "incidentId"])
        .or_else(|| newrelic_root_field(payload, &["incidentIds"]))
}

fn newrelic_incident_ids(payload: &Value) -> Option<String> {
    newrelic_root_field(payload, &["incidentIds", "incident_ids"])
        .or_else(|| newrelic_incident_id(payload))
}

fn newrelic_title(payload: &Value) -> Option<String> {
    newrelic_root_field(payload, &["issueTitle", "title", "details"])
        .or_else(|| newrelic_nested_field(payload, &["annotations"], &["title"]))
        .or_else(|| newrelic_condition(payload))
}

fn newrelic_body(payload: &Value) -> Option<String> {
    newrelic_root_field(
        payload,
        &[
            "condition_description",
            "description",
            "message",
            "VIOLATION DESCRIPTION",
        ],
    )
    .or_else(|| newrelic_nested_field(payload, &["annotations"], &["description"]))
    .or_else(|| newrelic_nested_field(payload, &["accumulations"], &["conditionDescription"]))
}

fn newrelic_state(payload: &Value) -> Option<String> {
    newrelic_root_field(payload, &["current_state", "stateText", "state", "status"])
}

fn newrelic_priority(payload: &Value) -> Option<String> {
    newrelic_root_field(payload, &["priorityText", "priority", "severity"])
}

fn newrelic_trigger(payload: &Value) -> Option<String> {
    newrelic_root_field(payload, &["triggerEvent", "trigger_event", "eventType"])
}

fn newrelic_event_type(payload: &Value) -> Option<String> {
    newrelic_root_field(payload, &["event_type", "eventType", "type"])
}

fn newrelic_condition(payload: &Value) -> Option<String> {
    newrelic_root_field(payload, &["condition_name", "conditionName"])
        .or_else(|| newrelic_nested_field(payload, &["accumulations"], &["conditionName"]))
}

fn newrelic_policy(payload: &Value) -> Option<String> {
    newrelic_root_field(payload, &["policy_name", "policyName"])
        .or_else(|| newrelic_nested_field(payload, &["accumulations"], &["policyName"]))
}

fn newrelic_entities(payload: &Value) -> Option<String> {
    newrelic_root_field(
        payload,
        &["entities", "entity", "entity_name", "target_name"],
    )
    .or_else(|| newrelic_nested_field(payload, &["entitiesData"], &["names", "entities"]))
    .or_else(|| newrelic_nested_field(payload, &["targets"], &["name", "id"]))
}

fn newrelic_account_id(payload: &Value) -> Option<String> {
    newrelic_root_field(payload, &["nrAccountId", "account_id", "accountId"])
}

fn newrelic_actor(payload: &Value) -> Option<String> {
    newrelic_root_field(payload, &["owner", "acknowledgedBy", "closedBy"])
}

fn newrelic_runbook(payload: &Value) -> Option<String> {
    newrelic_root_field(payload, &["runbook_url", "runbookUrl"])
        .or_else(|| newrelic_nested_field(payload, &["accumulations"], &["runbookUrl"]))
}

fn newrelic_chart(payload: &Value) -> Option<String> {
    newrelic_root_field(payload, &["violation_chart_url", "violationChartUrl"])
}

fn newrelic_link(payload: &Value) -> Option<String> {
    newrelic_root_field(
        payload,
        &[
            "issuePageUrl",
            "incident_url",
            "violation_callback_url",
            "url",
            "link",
            "newrelic_url",
            "new_relic_url",
        ],
    )
}

fn newrelic_root_field(payload: &Value, fields: &[&str]) -> Option<String> {
    newrelic_field(payload, fields).or_else(|| {
        payload
            .get("payload")
            .and_then(|value| newrelic_field(value, fields))
    })
}

fn newrelic_nested_field(payload: &Value, parents: &[&str], fields: &[&str]) -> Option<String> {
    parents
        .iter()
        .find_map(|parent| {
            payload
                .get(*parent)
                .and_then(|value| newrelic_field(value, fields))
        })
        .or_else(|| {
            payload.get("payload").and_then(|nested| {
                parents.iter().find_map(|parent| {
                    nested
                        .get(*parent)
                        .and_then(|value| newrelic_field(value, fields))
                })
            })
        })
}

fn newrelic_field(value: &Value, fields: &[&str]) -> Option<String> {
    if let Some(found) = fields
        .iter()
        .find_map(|field| value.get(*field).and_then(newrelic_value_label))
    {
        return Some(found);
    }
    if let Value::Array(values) = value {
        return values.iter().find_map(|item| newrelic_field(item, fields));
    }
    None
}

fn newrelic_value_label(value: &Value) -> Option<String> {
    match value {
        Value::String(value) if !value.trim().is_empty() => Some(value.clone()),
        Value::Number(_) => number_or_string(value),
        Value::Bool(value) => Some(value.to_string()),
        Value::Array(values) => {
            let joined = values
                .iter()
                .filter_map(newrelic_value_label)
                .collect::<Vec<_>>()
                .join(", ");
            (!joined.is_empty()).then_some(joined)
        }
        Value::Object(object) => {
            for field in ["title", "name", "message", "id", "url", "guid"] {
                if let Some(value) = object.get(field).and_then(newrelic_value_label) {
                    return Some(value);
                }
            }
            serde_json::to_string(value).ok()
        }
        _ => None,
    }
}

fn normalize_newrelic_part(value: &str) -> String {
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
        "newrelic".to_string()
    } else {
        normalized.chars().take(96).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newrelic_issue_payload_maps_to_inbound_message() {
        let headers = HeaderMap::new();
        let payload = serde_json::json!({
            "source": "new_relic",
            "issueId": "NR-123",
            "issueTitle": "Checkout API latency is high",
            "priority": "CRITICAL",
            "state": "ACTIVATED",
            "stateText": "active",
            "triggerEvent": "STATE_CHANGE",
            "issuePageUrl": "https://one.newrelic.com/alerts-ai/issues/NR-123",
            "incidentIds": ["111", "112"],
            "entitiesData": {
                "names": ["checkout-api", "payments-api"]
            },
            "accumulations": {
                "conditionName": ["Latency greater than 500ms"],
                "conditionDescription": ["p95 latency breached for 10 minutes"],
                "policyName": ["Production APIs"],
                "runbookUrl": ["https://runbooks.example.com/checkout-latency"]
            },
            "nrAccountId": 456
        });

        let inbound = newrelic_inbound(&headers, &payload).expect("newrelic inbound");

        assert_eq!(inbound.conversation_id, "newrelic:issue:nr_2d123");
        assert_eq!(inbound.user_id.as_deref(), Some("newrelic"));
        assert!(inbound.text.contains("New Relic STATE_CHANGE notification"));
        assert!(inbound.text.contains("Title: Checkout API latency is high"));
        assert!(inbound.text.contains("Priority: CRITICAL"));
        assert!(inbound
            .text
            .contains("Condition: Latency greater than 500ms"));
        assert!(inbound
            .text
            .contains("Entities: checkout-api, payments-api"));
        assert!(inbound.text.contains("Incident IDs: 111, 112"));
        assert!(inbound
            .text
            .contains("New Relic: https://one.newrelic.com/alerts-ai/issues/NR-123"));
    }

    #[test]
    fn newrelic_classic_migrated_payload_uses_incident_thread() {
        let headers = HeaderMap::new();
        let payload = serde_json::json!({
            "account_id": 456,
            "condition_name": "Error rate greater than 5%",
            "condition_description": "Errors exceeded the alert threshold.",
            "current_state": "open",
            "details": "Checkout error rate is high",
            "event_type": "INCIDENT",
            "incident_id": 987,
            "incident_url": "https://one.newrelic.com/alerts-ai/incidents/987",
            "policy_name": "Production APIs",
            "severity": "CRITICAL",
            "targets": [{
                "id": "entity-1",
                "name": "checkout-api",
                "type": "APM Application"
            }]
        });

        let inbound = newrelic_inbound(&headers, &payload).expect("newrelic inbound");

        assert_eq!(inbound.conversation_id, "newrelic:incident:987");
        assert!(inbound.text.contains("New Relic INCIDENT notification"));
        assert!(inbound.text.contains("State: open"));
        assert!(inbound.text.contains("Policy: Production APIs"));
        assert!(inbound.text.contains("Entities: checkout-api"));
    }

    #[test]
    fn newrelic_nested_payload_is_supported() {
        let headers = HeaderMap::new();
        let payload = serde_json::json!({
            "payload": {
                "issueId": "issue-42",
                "details": "Queue depth breached",
                "current_state": "closed",
                "triggerEvent": "INCIDENT_CLOSED",
                "incident_url": "https://one.newrelic.com/alerts-ai/issues/issue-42"
            }
        });

        let inbound = newrelic_inbound(&headers, &payload).expect("newrelic inbound");

        assert_eq!(inbound.conversation_id, "newrelic:issue:issue_2d42");
        assert!(inbound
            .text
            .contains("New Relic INCIDENT_CLOSED notification"));
        assert!(inbound.text.contains("Title: Queue depth breached"));
    }

    #[test]
    fn newrelic_shape_does_not_claim_generic_incident_payloads() {
        let headers = HeaderMap::new();
        let payload = serde_json::json!({
            "incident_id": "42",
            "current_state": "open",
            "details": "Generic incident from another tool"
        });

        assert!(newrelic_inbound(&headers, &payload).is_none());
    }
}

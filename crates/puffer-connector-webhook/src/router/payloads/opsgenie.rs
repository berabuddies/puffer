use axum::http::HeaderMap;
use puffer_connector_core::InboundMessage;
use serde_json::Value;

use super::super::{header_value, number_or_string, snippet, string_field};

/// Converts an Opsgenie webhook payload into an inbound Puffer message.
pub(super) fn opsgenie_inbound(headers: &HeaderMap, payload: &Value) -> Option<InboundMessage> {
    let delivery = opsgenie_delivery(payload)?;
    if !opsgenie_payload_shape(headers, delivery.container, delivery.alert) {
        return None;
    }

    let action = opsgenie_action(delivery.container).unwrap_or_else(|| "AlertAction".to_string());
    let actor = opsgenie_actor(delivery.container, delivery.alert)
        .unwrap_or_else(|| "opsgenie".to_string());
    let conversation_id = opsgenie_conversation_id(delivery.container, delivery.alert, &action);
    let text = opsgenie_message(delivery.container, delivery.alert, &action);

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

struct OpsgenieDelivery<'a> {
    container: &'a Value,
    alert: &'a Value,
}

fn opsgenie_delivery(payload: &Value) -> Option<OpsgenieDelivery<'_>> {
    if let Some(alert) = payload.get("alert").filter(|value| value.is_object()) {
        return Some(OpsgenieDelivery {
            container: payload,
            alert,
        });
    }

    ["/data", "/payload", "/event"]
        .iter()
        .filter_map(|pointer| payload.pointer(pointer))
        .find_map(|container| {
            container
                .get("alert")
                .filter(|alert| alert.is_object())
                .map(|alert| OpsgenieDelivery { container, alert })
        })
}

fn opsgenie_payload_shape(headers: &HeaderMap, container: &Value, alert: &Value) -> bool {
    let has_action = opsgenie_action(container).is_some();
    let has_message = opsgenie_alert_field(alert, &["message"]).is_some()
        || opsgenie_alert_field(alert, &["description"]).is_some();
    if header_value(headers, "x-opsgenie-webhook").is_some() {
        return has_action && has_message;
    }

    let has_alert_id = opsgenie_alert_field(alert, &["alertId", "alert_id", "id"]).is_some();
    let has_alert_identity = opsgenie_alert_field(
        alert,
        &[
            "alias", "tinyId", "tiny_id", "userId", "user_id", "username",
        ],
    )
    .is_some();
    let has_integration_hint = opsgenie_container_field(
        container,
        &["integrationName", "integrationId", "integrationType"],
    )
    .is_some();

    has_action && has_message && has_alert_id && (has_alert_identity || has_integration_hint)
}

fn opsgenie_conversation_id(container: &Value, alert: &Value, action: &str) -> String {
    if let Some(alert_id) = opsgenie_alert_field(alert, &["alertId", "alert_id", "id"]) {
        return format!("opsgenie:alert:{}", normalize_opsgenie_part(&alert_id));
    }
    if let Some(alias) = opsgenie_alert_field(alert, &["alias"]) {
        return format!("opsgenie:alias:{}", normalize_opsgenie_part(&alias));
    }
    if let Some(tiny_id) = opsgenie_alert_field(alert, &["tinyId", "tiny_id"]) {
        return format!("opsgenie:tiny:{}", normalize_opsgenie_part(&tiny_id));
    }
    if let Some(integration_id) = opsgenie_container_field(container, &["integrationId"]) {
        return format!(
            "opsgenie:integration:{}:{}",
            normalize_opsgenie_part(&integration_id),
            normalize_opsgenie_part(action)
        );
    }
    format!("opsgenie:event:{}", normalize_opsgenie_part(action))
}

fn opsgenie_message(container: &Value, alert: &Value, action: &str) -> String {
    let mut lines = vec![format!("Opsgenie {action}")];
    if let Some(integration) = opsgenie_integration(container) {
        lines.push(format!("Integration: {integration}"));
    }
    if let Some(message) = opsgenie_alert_field(alert, &["message"]) {
        lines.push(format!("Message: {}", snippet(&message)));
    }
    if let Some(alert_id) = opsgenie_alert_field(alert, &["alertId", "alert_id", "id"]) {
        lines.push(format!("Alert: {alert_id}"));
    }
    if let Some(alias) = opsgenie_alert_field(alert, &["alias"]) {
        lines.push(format!("Alias: {}", snippet(&alias)));
    }
    if let Some(tiny_id) = opsgenie_alert_field(alert, &["tinyId", "tiny_id"]) {
        lines.push(format!("Tiny ID: {tiny_id}"));
    }
    if let Some(priority) = opsgenie_alert_field(alert, &["priority"]) {
        lines.push(format!("Priority: {priority}"));
    }
    if let Some(old_priority) = opsgenie_alert_field(alert, &["oldPriority", "old_priority"]) {
        lines.push(format!("Previous priority: {old_priority}"));
    }
    if let Some(status) = opsgenie_alert_field(alert, &["status"]) {
        lines.push(format!("Status: {}", snippet(&status)));
    }
    if let Some(entity) = opsgenie_alert_field(alert, &["entity"]) {
        if !entity.trim().is_empty() {
            lines.push(format!("Entity: {}", snippet(&entity)));
        }
    }
    if let Some(source) = opsgenie_source(container, alert) {
        lines.push(format!("Source: {}", snippet(&source)));
    }
    if let Some(username) = opsgenie_alert_field(alert, &["username"]) {
        lines.push(format!("User: {}", snippet(&username)));
    }
    if let Some(tags) = opsgenie_list_field(alert, "tags") {
        lines.push(format!("Tags: {tags}"));
    }
    if let Some(teams) = opsgenie_list_field(alert, "teams") {
        lines.push(format!("Teams: {teams}"));
    }
    if let Some(recipients) = opsgenie_list_field(alert, "recipients") {
        lines.push(format!("Recipients: {recipients}"));
    }
    if let Some(description) = opsgenie_alert_field(alert, &["description"]) {
        lines.push(format!("Description: {}", snippet(&description)));
    }
    if let Some(details) = opsgenie_details(alert) {
        lines.push(format!("Details: {details}"));
    }
    if let Some(created) = opsgenie_alert_field(alert, &["createdAt", "created_at"]) {
        lines.push(format!("Created: {created}"));
    }
    if let Some(updated) = opsgenie_alert_field(alert, &["updatedAt", "updated_at"]) {
        lines.push(format!("Updated: {updated}"));
    }
    lines.join("\n")
}

fn opsgenie_action(container: &Value) -> Option<String> {
    opsgenie_container_field(container, &["action", "alertAction", "alert_action"])
}

fn opsgenie_actor(container: &Value, alert: &Value) -> Option<String> {
    opsgenie_alert_field(alert, &["username", "userId", "user_id"])
        .or_else(|| opsgenie_source(container, alert))
}

fn opsgenie_integration(container: &Value) -> Option<String> {
    let name = opsgenie_container_field(container, &["integrationName"]);
    let kind = opsgenie_container_field(container, &["integrationType"]);
    match (name, kind) {
        (Some(name), Some(kind)) => Some(format!("{} ({})", snippet(&name), snippet(&kind))),
        (Some(name), None) => Some(snippet(&name)),
        (None, Some(kind)) => Some(snippet(&kind)),
        (None, None) => None,
    }
}

fn opsgenie_source(container: &Value, alert: &Value) -> Option<String> {
    if let Some(source) = container.get("source").and_then(opsgenie_value_label) {
        return Some(source);
    }
    opsgenie_alert_field(alert, &["source"])
}

fn opsgenie_details(alert: &Value) -> Option<String> {
    let details = alert.get("details")?;
    match details {
        Value::Object(map) if !map.is_empty() => {
            let mut entries = map
                .iter()
                .filter_map(|(key, value)| {
                    opsgenie_value_label(value).map(|value| format!("{key}={}", snippet(&value)))
                })
                .collect::<Vec<_>>();
            entries.sort();
            Some(entries.into_iter().take(8).collect::<Vec<_>>().join(", "))
        }
        Value::String(value) if !value.trim().is_empty() => Some(snippet(value)),
        _ => None,
    }
}

fn opsgenie_list_field(alert: &Value, field: &str) -> Option<String> {
    match alert.get(field)? {
        Value::Array(values) => {
            let values = values
                .iter()
                .filter_map(opsgenie_value_label)
                .take(8)
                .collect::<Vec<_>>();
            (!values.is_empty()).then(|| values.join(", "))
        }
        value => opsgenie_value_label(value),
    }
}

fn opsgenie_alert_field(alert: &Value, fields: &[&str]) -> Option<String> {
    fields.iter().find_map(|field| {
        alert
            .get(*field)
            .and_then(opsgenie_value_label)
            .filter(|value| !value.trim().is_empty())
    })
}

fn opsgenie_container_field(container: &Value, fields: &[&str]) -> Option<String> {
    fields.iter().find_map(|field| {
        container
            .get(*field)
            .and_then(opsgenie_value_label)
            .filter(|value| !value.trim().is_empty())
    })
}

fn opsgenie_value_label(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(_) => number_or_string(value),
        Value::Bool(value) => Some(value.to_string()),
        Value::Object(_) => ["name", "username", "email", "id", "type"]
            .iter()
            .find_map(|field| string_field(value, field).map(str::to_string)),
        _ => None,
    }
}

fn normalize_opsgenie_part(value: &str) -> String {
    let normalized = value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    normalized.trim_matches('_').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;
    use serde_json::json;

    #[test]
    fn opsgenie_create_payload_maps_to_inbound_message() {
        let headers = HeaderMap::new();
        let payload = json!({
            "source": {"name": "mytool", "type": "api"},
            "alert": {
                "message": "testing webhooks",
                "username": "fili@opsgenie.com",
                "alertId": "44f717bf-44bd-11c9-44c7-2d0cf1d07b23",
                "alias": "webhooktest",
                "tinyId": "454",
                "entity": "checkout-api",
                "userId": "4caaaa77-9222-4322-8622-d3522fbd7dda"
            },
            "action": "Create"
        });

        let inbound = opsgenie_inbound(&headers, &payload).expect("opsgenie inbound");

        assert_eq!(
            inbound.conversation_id,
            "opsgenie:alert:44f717bf-44bd-11c9-44c7-2d0cf1d07b23"
        );
        assert_eq!(inbound.user_id.as_deref(), Some("fili@opsgenie.com"));
        assert!(inbound.text.contains("Opsgenie Create"));
        assert!(inbound.text.contains("Message: testing webhooks"));
        assert!(inbound.text.contains("Alias: webhooktest"));
        assert!(inbound.text.contains("Tiny ID: 454"));
        assert!(inbound.text.contains("Source: mytool"));
    }

    #[test]
    fn opsgenie_edge_update_payload_includes_alert_details() {
        let headers = HeaderMap::new();
        let payload = json!({
            "action": "UpdatePriority",
            "alert": {
                "alertId": "8809ea18-89ea-4e4e-9cca-8037fd745e102343",
                "message": "database replica lag",
                "tags": ["database", "prod"],
                "tinyId": "418",
                "source": "user@opsgenie.com",
                "entity": "postgres-primary",
                "alias": "db-replica-lag",
                "createdAt": 1512047424512_u64,
                "updatedAt": 1512559548447_u64,
                "username": "user@opsgenie.com",
                "description": "replica lag exceeded threshold",
                "details": {"region": "us-east-1", "service": "billing"},
                "priority": "P1",
                "oldPriority": "P3"
            },
            "source": {"name": "", "type": "web"},
            "integrationId": "1a423289-568a-468c-8a11-b1404edf0a832334534344",
            "integrationName": "Webhook1",
            "integrationType": "Webhook"
        });

        let inbound = opsgenie_inbound(&headers, &payload).expect("opsgenie inbound");

        assert_eq!(
            inbound.conversation_id,
            "opsgenie:alert:8809ea18-89ea-4e4e-9cca-8037fd745e102343"
        );
        assert!(inbound.text.contains("Integration: Webhook1 (Webhook)"));
        assert!(inbound.text.contains("Priority: P1"));
        assert!(inbound.text.contains("Previous priority: P3"));
        assert!(inbound.text.contains("Tags: database, prod"));
        assert!(inbound
            .text
            .contains("Details: region=us-east-1, service=billing"));
    }

    #[test]
    fn opsgenie_nested_payload_is_supported() {
        let headers = HeaderMap::new();
        let payload = json!({
            "payload": {
                "action": "Close",
                "integrationName": "Webhook1",
                "alert": {
                    "alertId": "052652ac-5d1c-464a-812a-7dd18bbfba8c",
                    "message": "recovered",
                    "alias": "aliastest",
                    "tinyId": "23",
                    "username": "fili@ifountain.com"
                }
            }
        });

        let inbound = opsgenie_inbound(&headers, &payload).expect("opsgenie inbound");

        assert_eq!(
            inbound.conversation_id,
            "opsgenie:alert:052652ac-5d1c-464a-812a-7dd18bbfba8c"
        );
        assert!(inbound.text.contains("Opsgenie Close"));
    }

    #[test]
    fn opsgenie_shape_does_not_claim_generic_alert_payloads() {
        let headers = HeaderMap::new();
        let payload = json!({
            "action": "Create",
            "alert": {
                "message": "generic alert",
                "severity": "warning"
            }
        });

        assert!(opsgenie_inbound(&headers, &payload).is_none());
    }
}

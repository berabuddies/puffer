use axum::http::HeaderMap;
use puffer_connector_core::InboundMessage;
use serde_json::Value;

use super::super::{header_value, pointer_string, snippet, string_field};

/// Returns whether an Asana webhook payload is a heartbeat with no events.
pub(super) fn asana_payload_is_heartbeat(headers: &HeaderMap, payload: &Value) -> bool {
    let Some(events) = payload.get("events").and_then(Value::as_array) else {
        return false;
    };
    events.is_empty() && asana_delivery_hint(headers, payload)
}

/// Converts an Asana webhook payload into an inbound Puffer message.
pub(super) fn asana_inbound(headers: &HeaderMap, payload: &Value) -> Option<InboundMessage> {
    let events = payload.get("events").and_then(Value::as_array)?;
    if events.is_empty() || !asana_payload_shape(headers, payload, events) {
        return None;
    }

    let first = events.first()?;
    let subject = event_resource(first).or_else(|| event_parent(first));
    let conversation_id = asana_conversation_id(subject, first);
    let actor = events.iter().find_map(event_actor).unwrap_or("asana");
    let text = asana_message(events);

    Some(InboundMessage {
        conversation_id,
        user_id: Some(actor.to_string()),
        text,
        thread_id: None,
        is_group: false,
        bot_mentioned: true,
        from_bot: false,
    })
}

fn asana_payload_shape(headers: &HeaderMap, payload: &Value, events: &[Value]) -> bool {
    asana_delivery_hint(headers, payload) || events.iter().any(asana_event_shape)
}

fn asana_delivery_hint(headers: &HeaderMap, payload: &Value) -> bool {
    header_value(headers, "x-hook-signature").is_some()
        || header_value(headers, "x-hook-secret").is_some()
        || payload
            .get("events")
            .and_then(Value::as_array)
            .is_some_and(|events| events.iter().any(asana_event_shape))
}

fn asana_event_shape(event: &Value) -> bool {
    string_field(event, "action").is_some()
        && event_resource(event)
            .or_else(|| event_parent(event))
            .and_then(|resource| resource.gid)
            .is_some()
}

fn asana_conversation_id(subject: Option<AsanaRef<'_>>, event: &Value) -> String {
    if let Some(subject) = subject {
        if let (Some(resource_type), Some(gid)) = (subject.resource_type, subject.gid) {
            return format!("asana:{}:{gid}", normalize_asana_part(resource_type));
        }
    }
    let created_at = string_field(event, "created_at")
        .or_else(|| string_field(event, "createdAt"))
        .unwrap_or("event");
    format!("asana:event:{}", normalize_asana_part(created_at))
}

fn asana_message(events: &[Value]) -> String {
    let mut lines = vec![match events.len() {
        1 => "Asana webhook event".to_string(),
        count => format!("Asana webhook events ({count})"),
    }];
    for event in events.iter().take(12) {
        lines.push(format!("- {}", asana_event_line(event)));
    }
    if events.len() > 12 {
        lines.push(format!("... {} more events", events.len() - 12));
    }
    lines.join("\n")
}

fn asana_event_line(event: &Value) -> String {
    let action = string_field(event, "action").unwrap_or("changed");
    let resource = event_resource(event);
    let subject = resource
        .as_ref()
        .map(asana_ref_label)
        .unwrap_or_else(|| "resource".to_string());
    let mut parts = vec![format!("{subject} {action}")];
    if let Some(change) = asana_change(event) {
        parts.push(change);
    }
    if let Some(parent) = event_parent(event).map(|value| asana_ref_label(&value)) {
        parts.push(format!("parent {parent}"));
    }
    if let Some(actor) = event_actor(event) {
        parts.push(format!("by {actor}"));
    }
    if let Some(created_at) =
        string_field(event, "created_at").or_else(|| string_field(event, "createdAt"))
    {
        parts.push(format!("at {created_at}"));
    }
    parts.join(" | ")
}

fn asana_change(event: &Value) -> Option<String> {
    let change = event.get("change")?;
    let action = string_field(change, "action");
    let field = string_field(change, "field").or_else(|| string_field(change, "name"));
    match (field, action) {
        (Some(field), Some(action)) => Some(format!("change {field} {action}")),
        (Some(field), None) => Some(format!("change {field}")),
        (None, Some(action)) => Some(format!("change {action}")),
        (None, None) => change.as_str().map(snippet),
    }
}

#[derive(Clone, Copy)]
struct AsanaRef<'a> {
    gid: Option<&'a str>,
    resource_type: Option<&'a str>,
    name: Option<&'a str>,
}

fn event_resource(event: &Value) -> Option<AsanaRef<'_>> {
    asana_ref(event.get("resource")?)
}

fn event_parent(event: &Value) -> Option<AsanaRef<'_>> {
    asana_ref(event.get("parent")?)
}

fn event_actor(event: &Value) -> Option<&str> {
    event
        .get("user")
        .and_then(asana_ref)
        .and_then(|value| value.name.or(value.gid))
        .or_else(|| pointer_string(event, "/created_by/name"))
        .or_else(|| pointer_string(event, "/created_by/gid"))
}

fn asana_ref(value: &Value) -> Option<AsanaRef<'_>> {
    match value {
        Value::Object(_) => Some(AsanaRef {
            gid: string_field(value, "gid").or_else(|| string_field(value, "id")),
            resource_type: string_field(value, "resource_type")
                .or_else(|| string_field(value, "type")),
            name: string_field(value, "name"),
        }),
        _ => None,
    }
}

fn asana_ref_label(value: &AsanaRef<'_>) -> String {
    let resource_type = value.resource_type.unwrap_or("resource");
    let gid = value.gid.unwrap_or("unknown");
    match value.name {
        Some(name) => format!("{resource_type} {gid} {}", snippet(name)),
        None => format!("{resource_type} {gid}"),
    }
}

fn normalize_asana_part(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .replace(':', "_")
        .replace(' ', "_")
        .replace('-', "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asana_task_payload_maps_to_inbound_message() {
        let mut headers = HeaderMap::new();
        headers.insert("x-hook-signature", "signature".parse().unwrap());
        let payload = serde_json::json!({
            "events": [
                {
                    "created_at": "2026-05-25T10:30:00.000Z",
                    "action": "changed",
                    "resource": {
                        "gid": "1200000000000001",
                        "resource_type": "task",
                        "name": "Improve workflow UX"
                    },
                    "parent": {
                        "gid": "1200000000000002",
                        "resource_type": "project",
                        "name": "Puffer"
                    },
                    "user": {
                        "gid": "1200000000000003",
                        "resource_type": "user",
                        "name": "Tony"
                    },
                    "change": {
                        "field": "completed",
                        "action": "changed"
                    }
                }
            ]
        });

        let inbound = asana_inbound(&headers, &payload).expect("asana inbound");

        assert_eq!(inbound.conversation_id, "asana:task:1200000000000001");
        assert_eq!(inbound.user_id.as_deref(), Some("Tony"));
        assert!(inbound.text.contains("Asana webhook event"));
        assert!(inbound
            .text
            .contains("task 1200000000000001 Improve workflow UX changed"));
        assert!(inbound.text.contains("change completed changed"));
        assert!(inbound
            .text
            .contains("parent project 1200000000000002 Puffer"));
    }

    #[test]
    fn asana_empty_events_with_signature_are_heartbeat() {
        let mut headers = HeaderMap::new();
        headers.insert("x-hook-signature", "signature".parse().unwrap());
        let payload = serde_json::json!({ "events": [] });

        assert!(asana_payload_is_heartbeat(&headers, &payload));
        assert!(asana_inbound(&headers, &payload).is_none());
    }
}

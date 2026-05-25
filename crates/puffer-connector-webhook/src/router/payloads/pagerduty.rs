use axum::http::HeaderMap;
use puffer_connector_core::InboundMessage;
use serde_json::Value;

use super::super::{header_value, number_or_string, pointer_string, snippet, string_field};

/// Converts a PagerDuty webhook payload into an inbound Puffer message.
pub(super) fn pagerduty_inbound(headers: &HeaderMap, payload: &Value) -> Option<InboundMessage> {
    let event = payload.get("event")?;
    if !event.is_object() {
        return None;
    }
    if !pagerduty_payload_shape(headers, event) {
        return None;
    }

    let event_type = string_field(event, "event_type").unwrap_or("pagerduty.event");
    let resource_type = string_field(event, "resource_type")
        .or_else(|| event.pointer("/data/type").and_then(Value::as_str))
        .unwrap_or_else(|| event_type.split('.').next().unwrap_or("event"));
    let subject = pagerduty_subject(event, resource_type);
    let service = subject
        .as_ref()
        .and_then(|subject| subject.service.clone())
        .or_else(|| pagerduty_service(event))
        .unwrap_or_else(|| "pagerduty".to_string());
    let delivery = string_field(event, "id")
        .map(str::to_string)
        .or_else(|| header_value(headers, "x-webhook-subscription").map(str::to_string));
    let actor = pagerduty_actor(event).unwrap_or_else(|| "pagerduty".to_string());
    let conversation_id =
        pagerduty_conversation_id(&service, event_type, delivery.as_deref(), subject.as_ref());
    let text = pagerduty_message(
        &service,
        event_type,
        resource_type,
        delivery.as_deref(),
        event,
        subject.as_ref(),
    );

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

fn pagerduty_payload_shape(headers: &HeaderMap, event: &Value) -> bool {
    header_value(headers, "user-agent").is_some_and(|value| value.contains("PagerDuty-Webhook/"))
        || header_value(headers, "x-webhook-subscription").is_some()
        || header_value(headers, "x-pagerduty-signature").is_some()
        || string_field(event, "event_type")
            .is_some_and(|value| value.starts_with("incident.") || value.starts_with("service."))
        || (string_field(event, "resource_type").is_some_and(pagerduty_resource_type_hint)
            && string_field(event, "occurred_at").is_some()
            && event.get("data").is_some())
}

fn pagerduty_resource_type_hint(value: &str) -> bool {
    matches!(value, "incident" | "service" | "pagey")
}

#[derive(Clone)]
struct PagerDutySubject {
    kind: &'static str,
    conversation_kind: &'static str,
    id: Option<String>,
    title: Option<String>,
    status: Option<String>,
    urgency: Option<String>,
    priority: Option<String>,
    service: Option<String>,
    url: Option<String>,
    body: Option<String>,
}

fn pagerduty_subject(event: &Value, resource_type: &str) -> Option<PagerDutySubject> {
    let data = event.get("data")?;
    if resource_type == "incident"
        || string_field(data, "type") == Some("incident")
        || data.get("incident").is_some()
    {
        return Some(pagerduty_incident_subject(data));
    }
    if resource_type == "service" || string_field(data, "type") == Some("service") {
        return Some(pagerduty_service_subject(data));
    }
    Some(PagerDutySubject {
        kind: "event",
        conversation_kind: resource_type_kind(resource_type),
        id: pagerduty_field(data, &["id", "incident_id"]),
        title: pagerduty_title(data),
        status: string_field(data, "status").map(str::to_string),
        urgency: string_field(data, "urgency").map(str::to_string),
        priority: pagerduty_priority(data),
        service: pagerduty_service(event),
        url: pagerduty_url(data),
        body: pagerduty_body(data),
    })
}

fn pagerduty_incident_subject(data: &Value) -> PagerDutySubject {
    let incident = if string_field(data, "type") == Some("incident") {
        data
    } else {
        data.get("incident").unwrap_or(data)
    };
    PagerDutySubject {
        kind: "incident",
        conversation_kind: "incident",
        id: pagerduty_field(incident, &["id", "incident_id", "incident_number"]),
        title: pagerduty_title(incident),
        status: string_field(incident, "status").map(str::to_string),
        urgency: string_field(incident, "urgency").map(str::to_string),
        priority: pagerduty_priority(incident),
        service: pagerduty_service_value(incident)
            .or_else(|| pagerduty_service_value(data))
            .or_else(|| pagerduty_field(data, &["service_id"])),
        url: pagerduty_url(incident).or_else(|| pagerduty_url(data)),
        body: pagerduty_body(data).or_else(|| pagerduty_body(incident)),
    }
}

fn pagerduty_service_subject(data: &Value) -> PagerDutySubject {
    PagerDutySubject {
        kind: "service",
        conversation_kind: "service",
        id: pagerduty_field(data, &["id"]),
        title: pagerduty_title(data),
        status: string_field(data, "status").map(str::to_string),
        urgency: None,
        priority: None,
        service: pagerduty_service_value(data).or_else(|| pagerduty_title(data)),
        url: pagerduty_url(data),
        body: pagerduty_body(data),
    }
}

fn pagerduty_conversation_id(
    service: &str,
    event_type: &str,
    delivery: Option<&str>,
    subject: Option<&PagerDutySubject>,
) -> String {
    let service = normalize_pagerduty_part(service);
    if let Some(subject) = subject {
        if let Some(id) = &subject.id {
            return format!(
                "pagerduty:{service}:{}:{}",
                subject.conversation_kind,
                normalize_pagerduty_part(id)
            );
        }
    }
    let fallback = delivery
        .map(normalize_pagerduty_part)
        .unwrap_or_else(|| normalize_pagerduty_part(event_type));
    format!("pagerduty:{service}:event:{fallback}")
}

fn pagerduty_message(
    service: &str,
    event_type: &str,
    resource_type: &str,
    delivery: Option<&str>,
    event: &Value,
    subject: Option<&PagerDutySubject>,
) -> String {
    let mut lines = vec![
        format!("PagerDuty {event_type}"),
        format!("Resource: {resource_type}"),
        format!("Service: {service}"),
    ];
    if let Some(subject) = subject {
        lines.push(pagerduty_subject_line(subject));
        if let Some(url) = &subject.url {
            lines.push(format!("URL: {url}"));
        }
        if let Some(body) = &subject.body {
            lines.push(String::new());
            lines.push(body.to_string());
        }
    }
    if let Some(occurred_at) = string_field(event, "occurred_at") {
        lines.push(format!("Occurred: {occurred_at}"));
    }
    if let Some(delivery) = delivery {
        lines.push(format!("Delivery: {delivery}"));
    }
    lines.join("\n")
}

fn pagerduty_subject_line(subject: &PagerDutySubject) -> String {
    let mut details = Vec::new();
    if let Some(id) = &subject.id {
        details.push(id.to_string());
    }
    if let Some(title) = &subject.title {
        details.push(snippet(title));
    }
    if let Some(status) = &subject.status {
        details.push(status.to_string());
    }
    if let Some(urgency) = &subject.urgency {
        details.push(urgency.to_string());
    }
    if let Some(priority) = &subject.priority {
        details.push(priority.to_string());
    }
    if details.is_empty() {
        format!("Subject: {}", subject.kind)
    } else {
        format!("Subject: {} {}", subject.kind, details.join(" "))
    }
}

fn pagerduty_actor(event: &Value) -> Option<String> {
    event
        .get("agent")
        .and_then(pagerduty_value_label)
        .or_else(|| event.get("client").and_then(pagerduty_value_label))
}

fn pagerduty_service(event: &Value) -> Option<String> {
    event
        .pointer("/data/service")
        .and_then(pagerduty_value_label)
        .or_else(|| {
            event
                .pointer("/data/incident/service")
                .and_then(pagerduty_value_label)
        })
        .or_else(|| pagerduty_field(event.get("data")?, &["service_id"]))
}

fn pagerduty_service_value(value: &Value) -> Option<String> {
    value.get("service").and_then(pagerduty_value_label)
}

fn pagerduty_priority(value: &Value) -> Option<String> {
    value
        .get("priority")
        .and_then(pagerduty_value_label)
        .or_else(|| string_field(value, "priority").map(str::to_string))
}

fn pagerduty_title(value: &Value) -> Option<String> {
    pagerduty_field(value, &["title", "summary", "message", "name"])
}

fn pagerduty_url(value: &Value) -> Option<String> {
    pagerduty_field(value, &["html_url", "url", "self"])
}

fn pagerduty_body(value: &Value) -> Option<String> {
    pagerduty_field(value, &["description", "message", "body", "content"])
        .or_else(|| pointer_string(value, "/note/content").map(str::to_string))
        .or_else(|| pointer_string(value, "/status_update/message").map(str::to_string))
        .map(|value| snippet(&value))
}

fn pagerduty_value_label(value: &Value) -> Option<String> {
    match value {
        Value::String(value) if !value.trim().is_empty() => Some(value.clone()),
        Value::Number(_) => number_or_string(value),
        Value::Object(_) => pagerduty_field(value, &["summary", "name", "id", "email", "type"]),
        _ => None,
    }
}

fn pagerduty_field(value: &Value, fields: &[&str]) -> Option<String> {
    fields.iter().find_map(|field| match value.get(*field)? {
        Value::String(value) if !value.trim().is_empty() => Some(value.clone()),
        Value::Number(_) => value.get(*field).and_then(number_or_string),
        Value::Object(_) => value.get(*field).and_then(pagerduty_value_label),
        _ => None,
    })
}

fn resource_type_kind(resource_type: &str) -> &'static str {
    if resource_type == "service" {
        "service"
    } else {
        "event"
    }
}

fn normalize_pagerduty_part(value: &str) -> String {
    let normalized = value
        .trim()
        .to_ascii_lowercase()
        .replace("https://", "")
        .replace("http://", "")
        .replace(':', "_")
        .replace('/', "_")
        .replace(' ', "_")
        .replace('-', "_")
        .replace('.', "_");
    if normalized.is_empty() {
        "pagerduty".to_string()
    } else {
        normalized
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pagerduty_incident_payload_maps_to_inbound_message() {
        let mut headers = HeaderMap::new();
        headers.insert("user-agent", "PagerDuty-Webhook/V3.0".parse().unwrap());
        headers.insert("x-webhook-subscription", "sub-123".parse().unwrap());
        let payload = serde_json::json!({
            "event": {
                "id": "01DXQ7Y7JZ7R6J3M4K1R",
                "event_type": "incident.triggered",
                "resource_type": "incident",
                "occurred_at": "2026-05-25T12:00:00Z",
                "agent": {"summary": "PagerDuty"},
                "data": {
                    "id": "P123456",
                    "type": "incident",
                    "summary": "Checkout API latency",
                    "status": "triggered",
                    "urgency": "high",
                    "priority": {"summary": "P1"},
                    "service": {"summary": "checkout-api"},
                    "html_url": "https://example.pagerduty.com/incidents/P123456",
                    "description": "p95 latency crossed the paging threshold"
                }
            }
        });

        let inbound = pagerduty_inbound(&headers, &payload).expect("pagerduty inbound");

        assert_eq!(
            inbound.conversation_id,
            "pagerduty:checkout_api:incident:p123456"
        );
        assert_eq!(inbound.user_id.as_deref(), Some("PagerDuty"));
        assert!(inbound.text.contains("PagerDuty incident.triggered"));
        assert!(inbound.text.contains("Service: checkout-api"));
        assert!(inbound
            .text
            .contains("Subject: incident P123456 Checkout API latency triggered high P1"));
        assert!(inbound
            .text
            .contains("p95 latency crossed the paging threshold"));
    }

    #[test]
    fn pagerduty_nested_incident_event_uses_incident_thread() {
        let mut headers = HeaderMap::new();
        headers.insert("x-pagerduty-signature", "v1=fake".parse().unwrap());
        let payload = serde_json::json!({
            "event": {
                "id": "evt-status",
                "event_type": "incident.status_update_published",
                "resource_type": "incident",
                "occurred_at": "2026-05-25T12:05:00Z",
                "data": {
                    "type": "status_update",
                    "message": "Mitigation is in progress.",
                    "incident": {
                        "id": "PABCDEF",
                        "summary": "Checkout API latency",
                        "status": "acknowledged",
                        "service": {"summary": "checkout-api"}
                    }
                }
            }
        });

        let inbound = pagerduty_inbound(&headers, &payload).expect("pagerduty inbound");

        assert_eq!(
            inbound.conversation_id,
            "pagerduty:checkout_api:incident:pabcdef"
        );
        assert!(inbound
            .text
            .contains("PagerDuty incident.status_update_published"));
        assert!(inbound.text.contains("Mitigation is in progress."));
    }

    #[test]
    fn pagerduty_test_payload_maps_to_event_thread() {
        let headers = HeaderMap::new();
        let payload = serde_json::json!({
            "event": {
                "id": "01CH754SM17TWPE2V2H4VPBRO7",
                "event_type": "pagey.ping",
                "resource_type": "pagey",
                "occurred_at": "2021-12-08T22:58:53.510Z",
                "data": {
                    "message": "Hello from your friend Pagey!",
                    "type": "ping"
                }
            }
        });

        let inbound = pagerduty_inbound(&headers, &payload).expect("pagerduty inbound");

        assert_eq!(
            inbound.conversation_id,
            "pagerduty:pagerduty:event:01ch754sm17twpe2v2h4vpbro7"
        );
        assert!(inbound.text.contains("PagerDuty pagey.ping"));
        assert!(inbound.text.contains("Hello from your friend Pagey!"));
    }

    #[test]
    fn pagerduty_shape_does_not_claim_generic_event_payloads() {
        let headers = HeaderMap::new();
        let payload = serde_json::json!({
            "event": {
                "event_type": "user.created",
                "data": {"id": "user-1"}
            }
        });

        assert!(pagerduty_inbound(&headers, &payload).is_none());
    }

    #[test]
    fn pagerduty_shape_requires_event_object() {
        let mut headers = HeaderMap::new();
        headers.insert("user-agent", "PagerDuty-Webhook/V3.0".parse().unwrap());
        let payload = serde_json::json!({
            "event": "not an object"
        });

        assert!(pagerduty_inbound(&headers, &payload).is_none());
    }
}

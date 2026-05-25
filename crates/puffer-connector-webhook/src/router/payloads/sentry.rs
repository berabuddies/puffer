use axum::http::HeaderMap;
use puffer_connector_core::InboundMessage;
use serde_json::Value;

use super::super::{header_value, number_or_string, pointer_string, snippet, string_field};

/// Converts a Sentry webhook payload into an inbound Puffer message.
pub(super) fn sentry_inbound(headers: &HeaderMap, payload: &Value) -> Option<InboundMessage> {
    if !sentry_payload_shape(headers, payload) {
        return None;
    }

    let resource = sentry_resource(headers, payload);
    let action = string_field(payload, "action")
        .or_else(|| string_field(payload, "type"))
        .unwrap_or("received");
    let subject = sentry_subject(payload);
    let project = subject
        .as_ref()
        .and_then(|subject| subject.project.clone())
        .or_else(|| sentry_project(payload))
        .unwrap_or_else(|| "sentry".to_string());
    let delivery = sentry_delivery(headers, payload);
    let actor = sentry_actor(payload).unwrap_or_else(|| "sentry".to_string());
    let conversation_id = sentry_conversation_id(
        &project,
        &resource,
        action,
        delivery.as_deref(),
        subject.as_ref(),
    );
    let text = sentry_message(
        &project,
        &resource,
        action,
        delivery.as_deref(),
        payload,
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

fn sentry_payload_shape(headers: &HeaderMap, payload: &Value) -> bool {
    if header_value(headers, "sentry-hook-resource").is_some()
        || header_value(headers, "sentry-hook-signature").is_some()
        || header_value(headers, "x-sentry-event").is_some()
    {
        return true;
    }

    sentry_installation_payload_hint(payload)
        || payload
            .pointer("/data/issue")
            .is_some_and(sentry_issue_payload_hint)
        || payload
            .pointer("/data/event")
            .is_some_and(sentry_event_payload_hint)
        || sentry_alert_name(payload).is_some()
}

fn sentry_installation_payload_hint(payload: &Value) -> bool {
    payload.get("installation").is_some()
        && (payload.get("data").is_some() || string_field(payload, "action").is_some())
}

fn sentry_issue_payload_hint(issue: &Value) -> bool {
    sentry_field(
        issue,
        &["shortId", "short_id", "issue_id", "permalink", "web_url"],
    )
    .is_some()
        || issue.get("metadata").is_some()
        || issue.get("culprit").is_some()
}

fn sentry_event_payload_hint(event: &Value) -> bool {
    event.get("event_id").is_some()
        || event.get("eventID").is_some()
        || sentry_field(event, &["web_url", "culprit", "transaction"]).is_some()
        || (sentry_field(event, &["platform"]).is_some() && sentry_project_value(event).is_some())
}

fn sentry_resource(headers: &HeaderMap, payload: &Value) -> String {
    header_value(headers, "sentry-hook-resource")
        .or_else(|| header_value(headers, "x-sentry-event"))
        .or_else(|| string_field(payload, "resource"))
        .or_else(|| string_field(payload, "event"))
        .map(str::to_string)
        .unwrap_or_else(|| {
            if payload.pointer("/data/issue").is_some() || payload.get("issue").is_some() {
                "issue".to_string()
            } else if sentry_alert_name(payload).is_some() {
                "event.alert".to_string()
            } else {
                "event".to_string()
            }
        })
}

fn sentry_delivery(headers: &HeaderMap, payload: &Value) -> Option<String> {
    header_value(headers, "sentry-hook-timestamp")
        .or_else(|| header_value(headers, "x-sentry-event-id"))
        .map(str::to_string)
        .or_else(|| string_field(payload, "event_id").map(str::to_string))
        .or_else(|| pointer_string(payload, "/data/event/event_id").map(str::to_string))
        .or_else(|| string_field(payload, "id").map(str::to_string))
}

#[derive(Clone)]
struct SentrySubject {
    kind: &'static str,
    conversation_kind: &'static str,
    id: Option<String>,
    title: Option<String>,
    status: Option<String>,
    level: Option<String>,
    culprit: Option<String>,
    url: Option<String>,
    project: Option<String>,
    event_id: Option<String>,
    environment: Option<String>,
    body: Option<String>,
}

fn sentry_subject(payload: &Value) -> Option<SentrySubject> {
    if let Some(issue) = payload
        .pointer("/data/issue")
        .or_else(|| payload.get("issue"))
    {
        return Some(sentry_issue_subject(issue, payload));
    }
    if let Some(event) = payload
        .pointer("/data/event")
        .or_else(|| payload.get("event"))
    {
        return Some(sentry_event_subject(event, payload));
    }
    if let Some(alert) = payload
        .pointer("/data/metric_alert")
        .or_else(|| payload.get("metric_alert"))
        .or_else(|| payload.get("alert"))
    {
        return Some(sentry_alert_subject(alert, payload));
    }
    sentry_alert_name(payload).map(|name| SentrySubject {
        kind: "alert",
        conversation_kind: "alert",
        id: string_field(payload, "id").map(str::to_string),
        title: Some(name),
        status: string_field(payload, "status").map(str::to_string),
        level: string_field(payload, "level").map(str::to_string),
        culprit: None,
        url: sentry_url(payload),
        project: sentry_project(payload),
        event_id: string_field(payload, "event_id").map(str::to_string),
        environment: sentry_environment(payload),
        body: string_field(payload, "message").map(snippet),
    })
}

fn sentry_issue_subject(issue: &Value, payload: &Value) -> SentrySubject {
    SentrySubject {
        kind: "issue",
        conversation_kind: "issue",
        id: sentry_issue_id(issue),
        title: sentry_title(issue),
        status: string_field(issue, "status").map(str::to_string),
        level: string_field(issue, "level")
            .or_else(|| pointer_string(issue, "/metadata/type"))
            .map(str::to_string),
        culprit: string_field(issue, "culprit").map(str::to_string),
        url: sentry_url(issue).or_else(|| sentry_url(payload)),
        project: sentry_project_value(issue).or_else(|| sentry_project(payload)),
        event_id: string_field(issue, "event_id").map(str::to_string),
        environment: sentry_environment(issue).or_else(|| sentry_environment(payload)),
        body: string_field(issue, "message")
            .or_else(|| pointer_string(issue, "/metadata/value"))
            .map(snippet),
    }
}

fn sentry_event_subject(event: &Value, payload: &Value) -> SentrySubject {
    SentrySubject {
        kind: "event",
        conversation_kind: "event",
        id: sentry_event_id(event),
        title: sentry_title(event),
        status: string_field(event, "status").map(str::to_string),
        level: string_field(event, "level").map(str::to_string),
        culprit: string_field(event, "culprit")
            .or_else(|| string_field(event, "transaction"))
            .map(str::to_string),
        url: sentry_url(event).or_else(|| sentry_url(payload)),
        project: sentry_project_value(event).or_else(|| sentry_project(payload)),
        event_id: sentry_event_id(event),
        environment: sentry_environment(event).or_else(|| sentry_environment(payload)),
        body: string_field(event, "message").map(snippet),
    }
}

fn sentry_alert_subject(alert: &Value, payload: &Value) -> SentrySubject {
    SentrySubject {
        kind: "alert",
        conversation_kind: "alert",
        id: sentry_field(alert, &["id", "identifier"]),
        title: sentry_title(alert).or_else(|| sentry_alert_name(payload)),
        status: string_field(alert, "status").map(str::to_string),
        level: string_field(alert, "level").map(str::to_string),
        culprit: None,
        url: sentry_url(alert).or_else(|| sentry_url(payload)),
        project: sentry_project_value(alert).or_else(|| sentry_project(payload)),
        event_id: string_field(payload, "event_id").map(str::to_string),
        environment: sentry_environment(alert).or_else(|| sentry_environment(payload)),
        body: string_field(alert, "description")
            .or_else(|| string_field(payload, "message"))
            .map(snippet),
    }
}

fn sentry_conversation_id(
    project: &str,
    resource: &str,
    action: &str,
    delivery: Option<&str>,
    subject: Option<&SentrySubject>,
) -> String {
    let project = normalize_sentry_part(project);
    if let Some(subject) = subject {
        if let Some(id) = &subject.id {
            return format!(
                "sentry:{project}:{}:{}",
                subject.conversation_kind,
                normalize_sentry_part(id)
            );
        }
    }
    let resource = normalize_sentry_resource(resource);
    let fallback = delivery
        .map(normalize_sentry_part)
        .unwrap_or_else(|| normalize_sentry_part(action));
    format!("sentry:{project}:{resource}:{fallback}")
}

fn sentry_message(
    project: &str,
    resource: &str,
    action: &str,
    delivery: Option<&str>,
    payload: &Value,
    subject: Option<&SentrySubject>,
) -> String {
    let mut lines = vec![
        format!("Sentry {resource} {action}"),
        format!("Project: {project}"),
    ];
    if let Some(rule) = sentry_alert_name(payload) {
        lines.push(format!("Rule: {rule}"));
    }
    if let Some(subject) = subject {
        lines.push(sentry_subject_line(subject));
        if let Some(culprit) = &subject.culprit {
            lines.push(format!("Culprit: {culprit}"));
        }
        if let Some(environment) = &subject.environment {
            lines.push(format!("Environment: {environment}"));
        }
        if let Some(url) = &subject.url {
            lines.push(format!("URL: {url}"));
        }
        if let Some(event_id) = &subject.event_id {
            lines.push(format!("Event: {event_id}"));
        }
        if let Some(body) = &subject.body {
            lines.push(String::new());
            lines.push(body.to_string());
        }
    }
    if let Some(delivery) = delivery {
        lines.push(format!("Delivery: {delivery}"));
    }
    lines.join("\n")
}

fn sentry_subject_line(subject: &SentrySubject) -> String {
    let mut details = Vec::new();
    if let Some(id) = &subject.id {
        details.push(id.to_string());
    }
    if let Some(title) = &subject.title {
        details.push(snippet(title));
    }
    if let Some(level) = &subject.level {
        details.push(level.to_string());
    }
    if let Some(status) = &subject.status {
        details.push(status.to_string());
    }
    if details.is_empty() {
        format!("Subject: {}", subject.kind)
    } else {
        format!("Subject: {} {}", subject.kind, details.join(" "))
    }
}

fn sentry_issue_id(issue: &Value) -> Option<String> {
    sentry_field(issue, &["shortId", "short_id", "issue_id", "id"])
}

fn sentry_event_id(event: &Value) -> Option<String> {
    sentry_field(event, &["event_id", "eventID", "id"])
}

fn sentry_title(value: &Value) -> Option<String> {
    sentry_field(value, &["title", "name", "message", "short_id"])
}

fn sentry_url(value: &Value) -> Option<String> {
    sentry_field(value, &["permalink", "web_url", "url", "issue_url"])
}

fn sentry_alert_name(payload: &Value) -> Option<String> {
    sentry_field(payload, &["triggered_rule", "rule", "rule_name"])
        .or_else(|| {
            sentry_field(
                payload.pointer("/data").unwrap_or(payload),
                &["triggered_rule"],
            )
        })
        .or_else(|| {
            payload
                .pointer("/data/metric_alert/name")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
}

fn sentry_project(payload: &Value) -> Option<String> {
    sentry_field(payload, &["project", "project_slug"])
        .or_else(|| {
            payload
                .pointer("/data/project")
                .and_then(sentry_value_label)
        })
        .or_else(|| payload.pointer("/project").and_then(sentry_value_label))
        .or_else(|| {
            payload
                .pointer("/data/issue/project")
                .and_then(sentry_value_label)
        })
        .or_else(|| {
            payload
                .pointer("/data/event/project")
                .and_then(sentry_value_label)
        })
}

fn sentry_project_value(value: &Value) -> Option<String> {
    value
        .get("project")
        .and_then(sentry_value_label)
        .or_else(|| sentry_field(value, &["project_slug"]))
}

fn sentry_environment(value: &Value) -> Option<String> {
    sentry_field(value, &["environment"]).or_else(|| {
        value.get("tags").and_then(|tags| {
            tags.as_array()?.iter().find_map(|tag| match tag {
                Value::Array(values) if values.len() >= 2 && values[0] == "environment" => {
                    values[1].as_str().map(str::to_string)
                }
                Value::Object(map)
                    if map.get("key").and_then(Value::as_str) == Some("environment") =>
                {
                    map.get("value").and_then(Value::as_str).map(str::to_string)
                }
                _ => None,
            })
        })
    })
}

fn sentry_actor(payload: &Value) -> Option<String> {
    payload
        .get("actor")
        .and_then(sentry_value_label)
        .or_else(|| {
            payload
                .pointer("/installation/app/slug")
                .and_then(sentry_value_label)
        })
        .or_else(|| payload.pointer("/data/actor").and_then(sentry_value_label))
}

fn sentry_value_label(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(_) => number_or_string(value),
        Value::Object(_) => sentry_field(value, &["slug", "name", "id", "username", "email"]),
        _ => None,
    }
}

fn sentry_field(value: &Value, fields: &[&str]) -> Option<String> {
    fields.iter().find_map(|field| match value.get(*field)? {
        Value::String(value) if !value.trim().is_empty() => Some(value.clone()),
        Value::Number(_) => value.get(*field).and_then(number_or_string),
        Value::Object(_) => value.get(*field).and_then(sentry_value_label),
        _ => None,
    })
}

fn normalize_sentry_resource(value: &str) -> String {
    normalize_sentry_part(&value.replace('.', "_"))
}

fn normalize_sentry_part(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .replace("https://", "")
        .replace("http://", "")
        .replace(':', "_")
        .replace('/', "_")
        .replace(' ', "_")
        .replace('-', "_")
        .replace('.', "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentry_issue_payload_maps_to_inbound_message() {
        let mut headers = HeaderMap::new();
        headers.insert("sentry-hook-resource", "issue".parse().unwrap());
        headers.insert("sentry-hook-timestamp", "1716652800".parse().unwrap());
        let payload = serde_json::json!({
            "action": "created",
            "actor": {"name": "Sentry"},
            "data": {
                "issue": {
                    "id": "123456",
                    "shortId": "PUF-1",
                    "title": "TypeError: undefined is not a function",
                    "status": "unresolved",
                    "level": "error",
                    "culprit": "checkout.views.pay",
                    "permalink": "https://sentry.io/organizations/acme/issues/123456/",
                    "project": {"slug": "puffer-api"},
                    "metadata": {"value": "undefined is not a function"},
                    "tags": [["environment", "production"]]
                }
            }
        });

        let inbound = sentry_inbound(&headers, &payload).expect("sentry inbound");

        assert_eq!(inbound.conversation_id, "sentry:puffer_api:issue:puf_1");
        assert_eq!(inbound.user_id.as_deref(), Some("Sentry"));
        assert!(inbound.text.contains("Sentry issue created"));
        assert!(inbound.text.contains("Project: puffer-api"));
        assert!(inbound.text.contains(
            "Subject: issue PUF-1 TypeError: undefined is not a function error unresolved"
        ));
        assert!(inbound.text.contains("Culprit: checkout.views.pay"));
        assert!(inbound.text.contains("Environment: production"));
        assert!(inbound.text.contains("undefined is not a function"));
    }

    #[test]
    fn sentry_event_alert_payload_uses_event_thread() {
        let mut headers = HeaderMap::new();
        headers.insert("sentry-hook-resource", "event_alert".parse().unwrap());
        let payload = serde_json::json!({
            "action": "triggered",
            "data": {
                "triggered_rule": "High checkout error rate",
                "event": {
                    "event_id": "9f3d8c2e7b7642d3a7e98a9f1a2b3c4d",
                    "title": "Checkout failed",
                    "message": "Payment provider timed out.",
                    "level": "error",
                    "project": "web",
                    "web_url": "https://sentry.io/organizations/acme/issues/7/events/9f3d/",
                    "environment": "production"
                }
            }
        });

        let inbound = sentry_inbound(&headers, &payload).expect("sentry inbound");

        assert_eq!(
            inbound.conversation_id,
            "sentry:web:event:9f3d8c2e7b7642d3a7e98a9f1a2b3c4d"
        );
        assert_eq!(inbound.user_id.as_deref(), Some("sentry"));
        assert!(inbound.text.contains("Sentry event_alert triggered"));
        assert!(inbound.text.contains("Rule: High checkout error rate"));
        assert!(inbound
            .text
            .contains("Subject: event 9f3d8c2e7b7642d3a7e98a9f1a2b3c4d Checkout failed error"));
        assert!(inbound.text.contains("Payment provider timed out."));
    }

    #[test]
    fn sentry_shape_requires_sentry_header_or_payload_hint() {
        let headers = HeaderMap::new();
        let payload = serde_json::json!({"id": "evt_123", "message": "plain event"});

        assert!(sentry_inbound(&headers, &payload).is_none());
    }

    #[test]
    fn sentry_shape_does_not_claim_generic_data_event_payloads() {
        let headers = HeaderMap::new();
        let payload = serde_json::json!({
            "data": {
                "event": {
                    "id": "evt_123",
                    "message": "plain event"
                }
            }
        });

        assert!(sentry_inbound(&headers, &payload).is_none());
    }
}

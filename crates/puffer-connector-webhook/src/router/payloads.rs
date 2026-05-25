use axum::http::HeaderMap;
use puffer_connector_core::InboundMessage;
use serde_json::Value;

use super::{header_value, number_or_string, pointer_string, snippet, string_field};

mod alertmanager;
mod asana;
mod bitbucket;
mod datadog;
mod grafana;
mod newrelic;
mod opsgenie;
mod pagerduty;
mod sentry;
mod shopify;
mod trello;

/// Returns whether an Asana webhook payload is a heartbeat with no events.
pub(super) fn asana_payload_is_heartbeat(headers: &HeaderMap, payload: &Value) -> bool {
    asana::asana_payload_is_heartbeat(headers, payload)
}

/// Converts an Asana webhook payload into an inbound Puffer message.
pub(super) fn asana_inbound(headers: &HeaderMap, payload: &Value) -> Option<InboundMessage> {
    asana::asana_inbound(headers, payload)
}

/// Converts a known provider webhook payload into an inbound Puffer message.
pub(super) fn provider_inbound(headers: &HeaderMap, payload: &Value) -> Option<InboundMessage> {
    asana_inbound(headers, payload)
        .or_else(|| bitbucket_inbound(headers, payload))
        .or_else(|| jira_inbound(headers, payload))
        .or_else(|| grafana_inbound(headers, payload))
        .or_else(|| alertmanager_inbound(headers, payload))
        .or_else(|| datadog_inbound(headers, payload))
        .or_else(|| newrelic_inbound(headers, payload))
        .or_else(|| opsgenie_inbound(headers, payload))
        .or_else(|| pagerduty_inbound(headers, payload))
        .or_else(|| sentry_inbound(headers, payload))
        .or_else(|| shopify_inbound(headers, payload))
        .or_else(|| stripe_inbound(headers, payload))
        .or_else(|| trello_inbound(headers, payload))
        .or_else(|| gitlab_inbound(headers, payload))
}

/// Converts a Bitbucket Cloud webhook payload into an inbound Puffer message.
pub(super) fn bitbucket_inbound(headers: &HeaderMap, payload: &Value) -> Option<InboundMessage> {
    bitbucket::bitbucket_inbound(headers, payload)
}

/// Converts a Prometheus Alertmanager webhook payload into an inbound Puffer message.
pub(super) fn alertmanager_inbound(headers: &HeaderMap, payload: &Value) -> Option<InboundMessage> {
    alertmanager::alertmanager_inbound(headers, payload)
}

/// Converts a Datadog webhook payload into an inbound Puffer message.
pub(super) fn datadog_inbound(headers: &HeaderMap, payload: &Value) -> Option<InboundMessage> {
    datadog::datadog_inbound(headers, payload)
}

/// Converts a New Relic webhook payload into an inbound Puffer message.
pub(super) fn newrelic_inbound(headers: &HeaderMap, payload: &Value) -> Option<InboundMessage> {
    newrelic::newrelic_inbound(headers, payload)
}

/// Converts an Opsgenie webhook payload into an inbound Puffer message.
pub(super) fn opsgenie_inbound(headers: &HeaderMap, payload: &Value) -> Option<InboundMessage> {
    opsgenie::opsgenie_inbound(headers, payload)
}

/// Converts a Grafana Alerting webhook payload into an inbound Puffer message.
pub(super) fn grafana_inbound(headers: &HeaderMap, payload: &Value) -> Option<InboundMessage> {
    grafana::grafana_inbound(headers, payload)
}

/// Converts a Trello webhook payload into an inbound Puffer message.
pub(super) fn trello_inbound(headers: &HeaderMap, payload: &Value) -> Option<InboundMessage> {
    trello::trello_inbound(headers, payload)
}

/// Converts a PagerDuty webhook payload into an inbound Puffer message.
pub(super) fn pagerduty_inbound(headers: &HeaderMap, payload: &Value) -> Option<InboundMessage> {
    pagerduty::pagerduty_inbound(headers, payload)
}

/// Converts a Sentry webhook payload into an inbound Puffer message.
pub(super) fn sentry_inbound(headers: &HeaderMap, payload: &Value) -> Option<InboundMessage> {
    sentry::sentry_inbound(headers, payload)
}

/// Converts a Shopify webhook payload into an inbound Puffer message.
pub(super) fn shopify_inbound(headers: &HeaderMap, payload: &Value) -> Option<InboundMessage> {
    shopify::shopify_inbound(headers, payload)
}

/// Converts a Jira webhook payload into an inbound Puffer message.
pub(super) fn jira_inbound(headers: &HeaderMap, payload: &Value) -> Option<InboundMessage> {
    let event = string_field(payload, "webhookEvent")
        .or_else(|| string_field(payload, "webhook_event"))
        .or_else(|| header_value(headers, "x-atlassian-webhook-event"))?;
    if !jira_payload_shape(event, payload) {
        return None;
    }

    let event_type = string_field(payload, "issue_event_type_name").unwrap_or(event);
    let project = jira_project(payload).unwrap_or_else(|| "jira".to_string());
    let actor = jira_actor(payload);
    let delivery = header_value(headers, "x-atlassian-webhook-identifier")
        .map(str::to_string)
        .or_else(|| payload.get("timestamp").and_then(number_or_string));
    let subject = jira_subject(payload);
    let conversation_id =
        jira_conversation_id(&project, event, delivery.as_deref(), subject.as_ref());
    let text = jira_message(
        &project,
        event,
        event_type,
        actor,
        payload,
        subject.as_ref(),
    );

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

struct JiraSubject {
    kind: &'static str,
    conversation_kind: &'static str,
    key: Option<String>,
    title: Option<String>,
    body: Option<String>,
    url: Option<String>,
}

fn jira_payload_shape(event: &str, payload: &Value) -> bool {
    event.starts_with("jira:")
        || event.starts_with("comment_")
        || payload.get("issue").is_some()
        || payload.get("comment").is_some()
        || payload.get("project").is_some()
        || string_field(payload, "issue_event_type_name").is_some()
}

fn jira_subject(payload: &Value) -> Option<JiraSubject> {
    if let Some(comment) = payload.get("comment") {
        let issue = payload.get("issue");
        return Some(JiraSubject {
            kind: "comment",
            conversation_kind: "issue",
            key: issue
                .and_then(|value| string_field(value, "key"))
                .map(str::to_string)
                .or_else(|| comment.get("id").and_then(number_or_string)),
            title: issue
                .and_then(|value| value.pointer("/fields/summary"))
                .and_then(Value::as_str)
                .map(str::to_string),
            body: comment.get("body").and_then(jira_text_value),
            url: string_field(comment, "self").map(str::to_string),
        });
    }
    if let Some(issue) = payload.get("issue") {
        return Some(JiraSubject {
            kind: "issue",
            conversation_kind: "issue",
            key: string_field(issue, "key").map(str::to_string),
            title: issue
                .pointer("/fields/summary")
                .and_then(Value::as_str)
                .map(str::to_string),
            body: issue
                .pointer("/fields/description")
                .and_then(jira_text_value),
            url: string_field(issue, "self").map(str::to_string),
        });
    }
    payload.get("project").map(|project| JiraSubject {
        kind: "project",
        conversation_kind: "project",
        key: string_field(project, "key")
            .or_else(|| string_field(project, "id"))
            .map(str::to_string),
        title: string_field(project, "name").map(str::to_string),
        body: None,
        url: string_field(project, "self").map(str::to_string),
    })
}

fn jira_conversation_id(
    project: &str,
    event: &str,
    delivery: Option<&str>,
    subject: Option<&JiraSubject>,
) -> String {
    if let Some(subject) = subject {
        if let Some(key) = &subject.key {
            return format!("jira:{project}:{}:{key}", subject.conversation_kind);
        }
    }
    format!(
        "jira:{project}:{}:{}",
        normalize_jira_event(event),
        delivery.unwrap_or("event")
    )
}

fn jira_message(
    project: &str,
    event: &str,
    event_type: &str,
    actor: &str,
    payload: &Value,
    subject: Option<&JiraSubject>,
) -> String {
    let mut lines = vec![
        jira_event_line(project, event, event_type),
        format!("Actor: {actor}"),
    ];
    if let Some(subject) = subject {
        let marker = subject
            .key
            .as_ref()
            .map(|key| format!("{key} "))
            .unwrap_or_default();
        let title = subject.title.as_deref().unwrap_or_default();
        if subject.kind == "comment" {
            lines.push(format!("Subject: comment on issue {marker}{title}"));
        } else {
            lines.push(format!("Subject: {} {marker}{title}", subject.kind));
        }
        if let Some(url) = &subject.url {
            lines.push(format!("URL: {url}"));
        }
        if let Some(body) = &subject.body {
            lines.push(String::new());
            lines.push(body.clone());
        }
    }
    append_jira_changelog(&mut lines, payload);
    lines.join("\n")
}

fn jira_event_line(project: &str, event: &str, event_type: &str) -> String {
    let suffix = if event_type == event {
        String::new()
    } else {
        format!(" ({event_type})")
    };
    if project == "jira" {
        format!("Jira {event}{suffix}")
    } else {
        format!("Jira {event}{suffix} in {project}")
    }
}

fn append_jira_changelog(lines: &mut Vec<String>, payload: &Value) {
    let Some(items) = payload
        .pointer("/changelog/items")
        .and_then(Value::as_array)
    else {
        return;
    };
    let fields = items
        .iter()
        .filter_map(|item| string_field(item, "field"))
        .take(8)
        .collect::<Vec<_>>();
    if !fields.is_empty() {
        lines.push(format!("Updated fields: {}", fields.join(", ")));
    }
}

fn jira_project(payload: &Value) -> Option<String> {
    pointer_string(payload, "/issue/fields/project/key")
        .or_else(|| pointer_string(payload, "/issue/fields/project/name"))
        .or_else(|| pointer_string(payload, "/project/key"))
        .or_else(|| pointer_string(payload, "/project/name"))
        .map(str::to_string)
}

fn jira_actor(payload: &Value) -> &str {
    pointer_string(payload, "/user/displayName")
        .or_else(|| pointer_string(payload, "/user/accountId"))
        .or_else(|| pointer_string(payload, "/user/name"))
        .or_else(|| pointer_string(payload, "/user/key"))
        .or_else(|| pointer_string(payload, "/comment/author/displayName"))
        .unwrap_or("jira")
}

fn jira_text_value(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return Some(snippet(text));
    }
    let mut parts = Vec::new();
    collect_jira_text(value, &mut parts);
    (!parts.is_empty()).then(|| snippet(&parts.join(" ")))
}

fn collect_jira_text(value: &Value, parts: &mut Vec<String>) {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_jira_text(value, parts);
            }
        }
        Value::Object(map) => {
            if let Some(text) = map.get("text").and_then(Value::as_str) {
                parts.push(text.to_string());
            }
            for value in map.values() {
                collect_jira_text(value, parts);
            }
        }
        _ => {}
    }
}

fn normalize_jira_event(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .replace(':', "_")
        .replace(' ', "_")
        .replace('-', "_")
}

/// Converts a Stripe webhook payload into an inbound Puffer message.
pub(super) fn stripe_inbound(headers: &HeaderMap, payload: &Value) -> Option<InboundMessage> {
    if !stripe_payload_shape(headers, payload) {
        return None;
    }
    let event_id = string_field(payload, "id")?;
    let event_type = string_field(payload, "type")?;
    let object = payload.pointer("/data/object")?;
    let object_type = string_field(object, "object").unwrap_or("object");
    let object_id = stripe_ref(object, "id").unwrap_or_else(|| event_id.to_string());
    let account =
        string_field(payload, "account").or_else(|| header_value(headers, "stripe-account"));
    let conversation_id = stripe_conversation_id(account, object_type, &object_id);
    let text = stripe_message(
        event_type,
        event_id,
        account,
        object_type,
        &object_id,
        payload,
        object,
    );

    Some(InboundMessage {
        conversation_id,
        user_id: Some(account.unwrap_or("stripe").to_string()),
        text,
        thread_id: None,
        is_group: false,
        bot_mentioned: true,
        from_bot: false,
    })
}

fn stripe_payload_shape(headers: &HeaderMap, payload: &Value) -> bool {
    header_value(headers, "stripe-signature").is_some()
        || (string_field(payload, "object") == Some("event")
            && payload.pointer("/data/object").is_some())
}

fn stripe_conversation_id(account: Option<&str>, object_type: &str, object_id: &str) -> String {
    let scope = account
        .map(|value| format!("stripe:{value}"))
        .unwrap_or_else(|| "stripe".to_string());
    format!("{scope}:{}:{object_id}", normalize_stripe_part(object_type))
}

fn stripe_message(
    event_type: &str,
    event_id: &str,
    account: Option<&str>,
    object_type: &str,
    object_id: &str,
    payload: &Value,
    object: &Value,
) -> String {
    let mut lines = vec![format!("Stripe {event_type}")];
    if let Some(account) = account {
        lines.push(format!("Account: {account}"));
    }
    lines.push(stripe_mode_line(object));
    lines.push(stripe_subject_line(object_type, object_id, object));
    for (label, field) in [
        ("Customer", "customer"),
        ("Subscription", "subscription"),
        ("Invoice", "invoice"),
        ("Payment intent", "payment_intent"),
    ] {
        if let Some(value) = stripe_ref(object, field) {
            lines.push(format!("{label}: {value}"));
        }
    }
    if let Some(amount) = stripe_amount(object) {
        lines.push(format!("Amount: {amount}"));
    }
    if let Some(url) = stripe_url(object) {
        lines.push(format!("URL: {url}"));
    }
    if let Some(request) =
        pointer_string(payload, "/request/id").or_else(|| pointer_string(payload, "/request"))
    {
        lines.push(format!("Request: {request}"));
    }
    lines.push(format!("Event: {event_id}"));
    lines.join("\n")
}

fn stripe_mode_line(object: &Value) -> String {
    if object
        .get("livemode")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        "Mode: live".to_string()
    } else {
        "Mode: test".to_string()
    }
}

fn stripe_subject_line(object_type: &str, object_id: &str, object: &Value) -> String {
    let mut suffixes = Vec::new();
    if let Some(number) = string_field(object, "number") {
        suffixes.push(number.to_string());
    }
    if let Some(status) =
        string_field(object, "status").or_else(|| string_field(object, "payment_status"))
    {
        suffixes.push(status.to_string());
    }
    if let Some(description) = string_field(object, "description") {
        suffixes.push(snippet(description));
    }
    if suffixes.is_empty() {
        format!("Subject: {object_type} {object_id}")
    } else {
        format!("Subject: {object_type} {object_id} {}", suffixes.join(" "))
    }
}

fn stripe_amount(object: &Value) -> Option<String> {
    let amount = [
        "amount_total",
        "amount_paid",
        "amount_due",
        "amount_received",
        "amount_captured",
        "amount",
    ]
    .iter()
    .find_map(|field| object.get(*field).and_then(number_or_string))?;
    let currency = string_field(object, "currency")
        .map(str::to_ascii_uppercase)
        .unwrap_or_else(|| "minor units".to_string());
    Some(format!("{amount} {currency}"))
}

fn stripe_url(object: &Value) -> Option<&str> {
    string_field(object, "hosted_invoice_url")
        .or_else(|| string_field(object, "receipt_url"))
        .or_else(|| string_field(object, "url"))
        .or_else(|| string_field(object, "invoice_pdf"))
}

fn stripe_ref(object: &Value, field: &str) -> Option<String> {
    match object.get(field)? {
        Value::String(value) => Some(value.clone()),
        Value::Object(_) => object
            .get(field)
            .and_then(|value| string_field(value, "id"))
            .map(str::to_string),
        _ => object.get(field).and_then(number_or_string),
    }
}

fn normalize_stripe_part(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .replace(':', "_")
        .replace(' ', "_")
        .replace('-', "_")
}

/// Converts a GitLab webhook payload into an inbound Puffer message.
pub(super) fn gitlab_inbound(headers: &HeaderMap, payload: &Value) -> Option<InboundMessage> {
    let has_header = header_value(headers, "x-gitlab-event").is_some()
        || header_value(headers, "x-gitlab-event-uuid").is_some();
    let object_kind = string_field(payload, "object_kind")
        .or_else(|| string_field(payload, "event_type"))
        .or_else(|| string_field(payload, "type"));
    if !has_header && object_kind.is_none() {
        return None;
    }

    let event = header_value(headers, "x-gitlab-event").unwrap_or("GitLab Hook");
    let kind = object_kind
        .map(normalize_gitlab_kind)
        .unwrap_or_else(|| normalize_gitlab_kind(event.trim_end_matches(" Hook")));
    let project = gitlab_project(payload)?;
    let action = pointer_string(payload, "/object_attributes/action")
        .or_else(|| string_field(payload, "event_name"))
        .or_else(|| string_field(payload, "event_type"))
        .unwrap_or("received");
    let sender = gitlab_sender(payload);
    let delivery = header_value(headers, "x-gitlab-event-uuid")
        .or_else(|| string_field(payload, "event_uuid"))
        .or_else(|| string_field(payload, "request_id"));
    let subject = gitlab_subject(&kind, payload);
    let conversation_id =
        gitlab_conversation_id(&project, &kind, delivery, payload, subject.as_ref());
    let text = gitlab_message(&project, event, action, sender, payload, subject.as_ref());

    Some(InboundMessage {
        conversation_id,
        user_id: Some(sender.to_string()),
        text,
        thread_id: None,
        is_group: false,
        bot_mentioned: true,
        from_bot: false,
    })
}

struct GitlabSubject {
    kind: &'static str,
    conversation_kind: String,
    key: Option<String>,
    title: Option<String>,
    body: Option<String>,
    url: Option<String>,
}

fn gitlab_subject(kind: &str, payload: &Value) -> Option<GitlabSubject> {
    let attrs = payload.get("object_attributes")?;
    if kind == "note" {
        return Some(gitlab_note_subject(attrs, payload));
    }
    if kind == "issue" || kind == "merge_request" || kind == "work_item" {
        let conversation_kind = if kind == "merge_request" {
            "merge-request".to_string()
        } else {
            kind.replace('_', "-")
        };
        return Some(GitlabSubject {
            kind: if kind == "merge_request" {
                "merge request"
            } else {
                "issue"
            },
            conversation_kind,
            key: gitlab_iid(attrs),
            title: string_field(attrs, "title")
                .or_else(|| string_field(attrs, "name"))
                .map(str::to_string),
            body: string_field(attrs, "description")
                .or_else(|| string_field(attrs, "message"))
                .map(snippet),
            url: string_field(attrs, "url").map(str::to_string),
        });
    }
    None
}

fn gitlab_note_subject(attrs: &Value, payload: &Value) -> GitlabSubject {
    let noteable_type = string_field(attrs, "noteable_type").unwrap_or("note");
    let conversation_kind = gitlab_note_conversation_kind(noteable_type);
    let parent = match conversation_kind.as_str() {
        "merge-request" => payload.get("merge_request"),
        "issue" => payload.get("issue"),
        "commit" => payload.get("commit"),
        _ => None,
    };
    GitlabSubject {
        kind: "comment",
        conversation_kind,
        key: string_field(attrs, "noteable_iid")
            .map(str::to_string)
            .or_else(|| parent.and_then(gitlab_iid))
            .or_else(|| string_field(attrs, "commit_id").map(str::to_string)),
        title: parent.and_then(|value| {
            string_field(value, "title")
                .or_else(|| string_field(value, "message"))
                .map(str::to_string)
        }),
        body: string_field(attrs, "note").map(snippet),
        url: string_field(attrs, "url").map(str::to_string),
    }
}

fn gitlab_note_conversation_kind(noteable_type: &str) -> String {
    let lower = noteable_type.to_ascii_lowercase();
    if lower.contains("merge") {
        "merge-request".to_string()
    } else if lower.contains("issue") {
        "issue".to_string()
    } else if lower.contains("commit") {
        "commit".to_string()
    } else {
        lower.replace('_', "-")
    }
}

fn gitlab_conversation_id(
    project: &str,
    kind: &str,
    delivery: Option<&str>,
    payload: &Value,
    subject: Option<&GitlabSubject>,
) -> String {
    if let Some(subject) = subject {
        if let Some(key) = &subject.key {
            return format!("gitlab:{project}:{}:{key}", subject.conversation_kind);
        }
    }
    if let Some(reference) = string_field(payload, "ref") {
        return format!("gitlab:{project}:{kind}:{reference}");
    }
    format!("gitlab:{project}:{kind}:{}", delivery.unwrap_or("event"))
}

fn gitlab_message(
    project: &str,
    event: &str,
    action: &str,
    sender: &str,
    payload: &Value,
    subject: Option<&GitlabSubject>,
) -> String {
    let mut lines = vec![
        format!("GitLab {event} {action} in {project}"),
        format!("Sender: {sender}"),
    ];
    if let Some(subject) = subject {
        let marker = gitlab_subject_marker(&subject.conversation_kind, subject.key.as_deref());
        let title = subject.title.as_deref().unwrap_or_default();
        if subject.kind == "comment" {
            lines.push(format!(
                "Subject: comment on {} {marker}{title}",
                subject.conversation_kind.replace('-', " ")
            ));
        } else {
            lines.push(format!("Subject: {} {marker}{title}", subject.kind));
        }
        if let Some(url) = &subject.url {
            lines.push(format!("URL: {url}"));
        }
        if let Some(body) = &subject.body {
            lines.push(String::new());
            lines.push(body.clone());
        }
    } else if normalize_gitlab_kind(event) == "push" || string_field(payload, "ref").is_some() {
        append_gitlab_push_summary(&mut lines, payload);
    }
    lines.join("\n")
}

fn append_gitlab_push_summary(lines: &mut Vec<String>, payload: &Value) {
    if let Some(reference) = string_field(payload, "ref") {
        lines.push(format!("Ref: {reference}"));
    }
    let commits = payload
        .get("commits")
        .and_then(Value::as_array)
        .map(|commits| commits.iter().take(3).collect::<Vec<_>>())
        .unwrap_or_default();
    if commits.is_empty() {
        return;
    }
    lines.push(String::new());
    lines.push("Commits:".to_string());
    for commit in commits {
        let id = string_field(commit, "id")
            .map(|value| value.chars().take(8).collect::<String>())
            .unwrap_or_else(|| "commit".to_string());
        let message = string_field(commit, "message")
            .map(snippet)
            .unwrap_or_default();
        lines.push(format!("- {id}: {message}"));
    }
}

fn gitlab_project(payload: &Value) -> Option<String> {
    pointer_string(payload, "/project/path_with_namespace")
        .or_else(|| pointer_string(payload, "/project/name"))
        .or_else(|| string_field(payload, "project_path"))
        .or_else(|| string_field(payload, "path_with_namespace"))
        .map(str::to_string)
}

fn gitlab_sender(payload: &Value) -> &str {
    pointer_string(payload, "/user/username")
        .or_else(|| pointer_string(payload, "/user/name"))
        .or_else(|| pointer_string(payload, "/user/id"))
        .or_else(|| string_field(payload, "user_username"))
        .or_else(|| string_field(payload, "user_name"))
        .unwrap_or("gitlab")
}

fn gitlab_iid(value: &Value) -> Option<String> {
    value
        .get("iid")
        .and_then(number_or_string)
        .or_else(|| value.get("id").and_then(number_or_string))
}

fn gitlab_subject_marker(kind: &str, key: Option<&str>) -> String {
    let Some(key) = key else {
        return String::new();
    };
    match kind {
        "issue" | "work-item" => format!("#{key} "),
        "merge-request" => format!("!{key} "),
        _ => format!("{key} "),
    }
}

fn normalize_gitlab_kind(value: &str) -> String {
    value
        .trim()
        .trim_end_matches(" Hook")
        .to_ascii_lowercase()
        .replace(' ', "_")
        .replace('-', "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jira_issue_payload_maps_to_inbound_message() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-atlassian-webhook-identifier",
            "jira-delivery-1".parse().unwrap(),
        );
        let payload = serde_json::json!({
            "webhookEvent": "jira:issue_updated",
            "issue_event_type_name": "issue_generic",
            "issue": {
                "id": "99291",
                "self": "https://example.atlassian.net/rest/api/2/issue/99291",
                "key": "JRA-20002",
                "fields": {
                    "summary": "Make workflow connector search faster",
                    "description": "The connector picker should feel instant.",
                    "project": {"key": "JRA", "name": "Jira"}
                }
            },
            "user": {"displayName": "Tony"},
            "changelog": {
                "items": [
                    {"field": "summary"},
                    {"field": "issuetype"}
                ]
            }
        });

        let inbound = jira_inbound(&headers, &payload).expect("jira inbound");

        assert_eq!(inbound.conversation_id, "jira:JRA:issue:JRA-20002");
        assert_eq!(inbound.user_id.as_deref(), Some("Tony"));
        assert!(inbound
            .text
            .contains("Jira jira:issue_updated (issue_generic) in JRA"));
        assert!(inbound
            .text
            .contains("Subject: issue JRA-20002 Make workflow connector search faster"));
        assert!(inbound.text.contains("Updated fields: summary, issuetype"));
    }

    #[test]
    fn jira_comment_payload_uses_issue_thread_and_adf_body() {
        let headers = HeaderMap::new();
        let payload = serde_json::json!({
            "webhookEvent": "comment_created",
            "issue": {
                "key": "PUF-42",
                "fields": {
                    "summary": "Bring connectors into Corbina",
                    "project": {"key": "PUF"}
                }
            },
            "comment": {
                "id": "10003",
                "self": "https://example.atlassian.net/rest/api/3/issue/PUF-42/comment/10003",
                "body": {
                    "type": "doc",
                    "content": [
                        {
                            "type": "paragraph",
                            "content": [{"type": "text", "text": "Please add Jira."}]
                        }
                    ]
                }
            },
            "user": {"accountId": "user-1"}
        });

        let inbound = jira_inbound(&headers, &payload).expect("jira inbound");

        assert_eq!(inbound.conversation_id, "jira:PUF:issue:PUF-42");
        assert_eq!(inbound.user_id.as_deref(), Some("user-1"));
        assert!(inbound
            .text
            .contains("Subject: comment on issue PUF-42 Bring connectors into Corbina"));
        assert!(inbound.text.contains("Please add Jira."));
    }

    #[test]
    fn stripe_invoice_payload_maps_to_inbound_message() {
        let mut headers = HeaderMap::new();
        headers.insert("stripe-signature", "t=1,v1=test".parse().unwrap());
        let payload = serde_json::json!({
            "id": "evt_invoice_paid",
            "object": "event",
            "type": "invoice.payment_succeeded",
            "account": "acct_123",
            "request": {"id": "req_123"},
            "data": {
                "object": {
                    "id": "in_123",
                    "object": "invoice",
                    "number": "PUF-0001",
                    "status": "paid",
                    "customer": "cus_123",
                    "subscription": "sub_123",
                    "amount_paid": 2000,
                    "currency": "usd",
                    "hosted_invoice_url": "https://invoice.stripe.com/i/acct_123/test",
                    "livemode": false
                }
            }
        });

        let inbound = stripe_inbound(&headers, &payload).expect("stripe inbound");

        assert_eq!(inbound.conversation_id, "stripe:acct_123:invoice:in_123");
        assert_eq!(inbound.user_id.as_deref(), Some("acct_123"));
        assert!(inbound.text.contains("Stripe invoice.payment_succeeded"));
        assert!(inbound
            .text
            .contains("Subject: invoice in_123 PUF-0001 paid"));
        assert!(inbound.text.contains("Customer: cus_123"));
        assert!(inbound.text.contains("Amount: 2000 USD"));
        assert!(inbound.text.contains("Request: req_123"));
    }

    #[test]
    fn stripe_checkout_payload_uses_object_thread_without_account() {
        let headers = HeaderMap::new();
        let payload = serde_json::json!({
            "id": "evt_checkout_completed",
            "object": "event",
            "type": "checkout.session.completed",
            "data": {
                "object": {
                    "id": "cs_test_123",
                    "object": "checkout.session",
                    "payment_status": "paid",
                    "customer": {"id": "cus_nested"},
                    "amount_total": 5000,
                    "currency": "usd",
                    "url": "https://checkout.stripe.com/c/pay/cs_test_123",
                    "livemode": true
                }
            }
        });

        let inbound = stripe_inbound(&headers, &payload).expect("stripe inbound");

        assert_eq!(
            inbound.conversation_id,
            "stripe:checkout.session:cs_test_123"
        );
        assert_eq!(inbound.user_id.as_deref(), Some("stripe"));
        assert!(inbound.text.contains("Mode: live"));
        assert!(inbound
            .text
            .contains("Subject: checkout.session cs_test_123 paid"));
        assert!(inbound.text.contains("Customer: cus_nested"));
    }

    #[test]
    fn gitlab_issue_payload_maps_to_inbound_message() {
        let mut headers = HeaderMap::new();
        headers.insert("x-gitlab-event", "Issue Hook".parse().unwrap());
        headers.insert("x-gitlab-event-uuid", "delivery-1".parse().unwrap());
        let payload = serde_json::json!({
            "object_kind": "issue",
            "project": {"path_with_namespace": "berabuddies/puffer"},
            "user": {"username": "tonykebot"},
            "object_attributes": {
                "iid": 42,
                "action": "open",
                "title": "Improve workflow connector UX",
                "description": "Expose GitLab in the workflow picker.",
                "url": "https://gitlab.com/berabuddies/puffer/-/issues/42"
            }
        });

        let inbound = gitlab_inbound(&headers, &payload).expect("gitlab inbound");

        assert_eq!(
            inbound.conversation_id,
            "gitlab:berabuddies/puffer:issue:42"
        );
        assert_eq!(inbound.user_id.as_deref(), Some("tonykebot"));
        assert!(inbound
            .text
            .contains("GitLab Issue Hook open in berabuddies/puffer"));
        assert!(inbound
            .text
            .contains("Subject: issue #42 Improve workflow connector UX"));
        assert!(inbound
            .text
            .contains("https://gitlab.com/berabuddies/puffer/-/issues/42"));
    }

    #[test]
    fn gitlab_note_payload_uses_parent_thread() {
        let mut headers = HeaderMap::new();
        headers.insert("x-gitlab-event", "Note Hook".parse().unwrap());
        let payload = serde_json::json!({
            "object_kind": "note",
            "project": {"path_with_namespace": "berabuddies/puffer"},
            "user": {"name": "Tony"},
            "merge_request": {
                "iid": 7,
                "title": "Ship workflow UX"
            },
            "object_attributes": {
                "noteable_type": "MergeRequest",
                "note": "Please include this in Puffer.",
                "url": "https://gitlab.com/berabuddies/puffer/-/merge_requests/7#note_1"
            }
        });

        let inbound = gitlab_inbound(&headers, &payload).expect("gitlab inbound");

        assert_eq!(
            inbound.conversation_id,
            "gitlab:berabuddies/puffer:merge-request:7"
        );
        assert_eq!(inbound.user_id.as_deref(), Some("Tony"));
        assert!(inbound
            .text
            .contains("Subject: comment on merge request !7 Ship workflow UX"));
        assert!(inbound.text.contains("Please include this in Puffer."));
    }

    #[test]
    fn gitlab_push_payload_maps_commit_summary() {
        let mut headers = HeaderMap::new();
        headers.insert("x-gitlab-event", "Push Hook".parse().unwrap());
        let payload = serde_json::json!({
            "object_kind": "push",
            "project": {"path_with_namespace": "berabuddies/puffer"},
            "user_username": "tonykebot",
            "ref": "refs/heads/master",
            "commits": [
                {"id": "abcdef1234567890", "message": "Add GitLab workflow preset"}
            ]
        });

        let inbound = gitlab_inbound(&headers, &payload).expect("gitlab inbound");

        assert_eq!(
            inbound.conversation_id,
            "gitlab:berabuddies/puffer:push:refs/heads/master"
        );
        assert!(inbound
            .text
            .contains("GitLab Push Hook received in berabuddies/puffer"));
        assert!(inbound
            .text
            .contains("- abcdef12: Add GitLab workflow preset"));
    }
}

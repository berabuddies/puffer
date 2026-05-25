use axum::http::HeaderMap;
use puffer_connector_core::InboundMessage;
use serde_json::Value;

use super::{header_value, number_or_string, pointer_string, snippet, string_field};

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

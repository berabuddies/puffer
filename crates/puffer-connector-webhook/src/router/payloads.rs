use axum::http::HeaderMap;
use puffer_connector_core::InboundMessage;
use serde_json::Value;

use super::{header_value, number_or_string, pointer_string, snippet, string_field};

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

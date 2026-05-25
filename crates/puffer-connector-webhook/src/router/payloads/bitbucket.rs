use axum::http::HeaderMap;
use puffer_connector_core::InboundMessage;
use serde_json::Value;

use super::super::{header_value, number_or_string, pointer_string, snippet, string_field};

/// Converts a Bitbucket Cloud webhook payload into an inbound Puffer message.
pub(super) fn bitbucket_inbound(headers: &HeaderMap, payload: &Value) -> Option<InboundMessage> {
    let event = bitbucket_event(headers, payload)?;
    if !bitbucket_payload_shape(headers, payload, &event) {
        return None;
    }

    let repository = bitbucket_repository(payload)?;
    let actor = bitbucket_actor(payload);
    let subject = bitbucket_subject(payload);
    let request = header_value(headers, "x-request-uuid")
        .or_else(|| header_value(headers, "x-hook-uuid"))
        .or_else(|| string_field(payload, "uuid"));
    let conversation_id =
        bitbucket_conversation_id(&repository, &event, request, payload, subject.as_ref());
    let text = bitbucket_message(&repository, &event, actor, payload, subject.as_ref());

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

fn bitbucket_event(headers: &HeaderMap, payload: &Value) -> Option<String> {
    header_value(headers, "x-event-key")
        .map(str::to_string)
        .or_else(|| string_field(payload, "event").map(str::to_string))
        .or_else(|| {
            payload
                .get("push")
                .is_some()
                .then(|| "repo:push".to_string())
        })
        .or_else(|| {
            payload.get("pullrequest").is_some().then(|| {
                if payload.get("comment").is_some() {
                    "pullrequest:comment_created".to_string()
                } else {
                    "pullrequest:updated".to_string()
                }
            })
        })
        .or_else(|| {
            payload.get("issue").is_some().then(|| {
                if payload.get("comment").is_some() {
                    "issue:comment_created".to_string()
                } else {
                    "issue:updated".to_string()
                }
            })
        })
}

fn bitbucket_payload_shape(headers: &HeaderMap, payload: &Value, event: &str) -> bool {
    let has_bitbucket_header = header_value(headers, "x-event-key").is_some()
        || header_value(headers, "x-hook-uuid").is_some()
        || header_value(headers, "x-request-uuid").is_some();
    let has_known_event = event.starts_with("repo:")
        || event.starts_with("pullrequest:")
        || event.starts_with("issue:")
        || event.starts_with("commit:")
        || event.starts_with("project:");
    let has_bitbucket_body = payload.get("repository").is_some()
        && (payload.get("push").is_some()
            || payload.get("pullrequest").is_some()
            || payload.get("issue").is_some()
            || payload.get("comment").is_some());

    has_known_event && (has_bitbucket_header || has_bitbucket_body)
}

#[derive(Clone)]
struct BitbucketSubject {
    kind: &'static str,
    conversation_kind: &'static str,
    key: Option<String>,
    title: Option<String>,
    body: Option<String>,
    url: Option<String>,
}

fn bitbucket_subject(payload: &Value) -> Option<BitbucketSubject> {
    if let Some(comment) = payload.get("comment") {
        if let Some(pullrequest) = payload.get("pullrequest") {
            return Some(bitbucket_comment_subject(
                comment,
                pullrequest,
                "pull request",
                "pull-request",
            ));
        }
        if let Some(issue) = payload.get("issue") {
            return Some(bitbucket_comment_subject(comment, issue, "issue", "issue"));
        }
    }
    payload
        .get("pullrequest")
        .map(bitbucket_pullrequest_subject)
        .or_else(|| payload.get("issue").map(bitbucket_issue_subject))
}

fn bitbucket_pullrequest_subject(value: &Value) -> BitbucketSubject {
    BitbucketSubject {
        kind: "pull request",
        conversation_kind: "pull-request",
        key: value.get("id").and_then(number_or_string),
        title: string_field(value, "title").map(str::to_string),
        body: pointer_string(value, "/description").map(snippet),
        url: bitbucket_html_url(value),
    }
}

fn bitbucket_issue_subject(value: &Value) -> BitbucketSubject {
    BitbucketSubject {
        kind: "issue",
        conversation_kind: "issue",
        key: value.get("id").and_then(number_or_string),
        title: string_field(value, "title").map(str::to_string),
        body: bitbucket_content_text(value),
        url: bitbucket_html_url(value),
    }
}

fn bitbucket_comment_subject(
    comment: &Value,
    parent: &Value,
    parent_kind: &'static str,
    conversation_kind: &'static str,
) -> BitbucketSubject {
    BitbucketSubject {
        kind: "comment",
        conversation_kind,
        key: parent.get("id").and_then(number_or_string),
        title: string_field(parent, "title").map(str::to_string),
        body: bitbucket_content_text(comment),
        url: bitbucket_html_url(comment).or_else(|| bitbucket_html_url(parent)),
    }
    .with_parent_kind(parent_kind)
}

trait ParentKind {
    fn with_parent_kind(self, parent_kind: &'static str) -> Self;
}

impl ParentKind for BitbucketSubject {
    fn with_parent_kind(mut self, parent_kind: &'static str) -> Self {
        self.kind = match parent_kind {
            "pull request" => "comment on pull request",
            "issue" => "comment on issue",
            _ => "comment",
        };
        self
    }
}

fn bitbucket_conversation_id(
    repository: &str,
    event: &str,
    request: Option<&str>,
    payload: &Value,
    subject: Option<&BitbucketSubject>,
) -> String {
    if let Some(subject) = subject {
        if let Some(key) = &subject.key {
            return format!("bitbucket:{repository}:{}:{key}", subject.conversation_kind);
        }
    }
    if let Some(reference) = bitbucket_push_reference(payload) {
        return format!(
            "bitbucket:{repository}:push:{}",
            normalize_bitbucket_part(&reference)
        );
    }
    format!(
        "bitbucket:{repository}:{}:{}",
        normalize_bitbucket_part(event),
        request.unwrap_or("event")
    )
}

fn bitbucket_message(
    repository: &str,
    event: &str,
    actor: &str,
    payload: &Value,
    subject: Option<&BitbucketSubject>,
) -> String {
    let mut lines = vec![
        format!("Bitbucket {event} in {repository}"),
        format!("Actor: {actor}"),
    ];
    if let Some(subject) = subject {
        let marker = bitbucket_subject_marker(subject);
        lines.push(format!(
            "Subject: {} {marker}{}",
            subject.kind,
            subject.title.as_deref().unwrap_or_default()
        ));
        append_bitbucket_pullrequest_branches(&mut lines, payload);
        append_bitbucket_state(&mut lines, payload);
        if let Some(url) = &subject.url {
            lines.push(format!("URL: {url}"));
        }
        if let Some(body) = &subject.body {
            lines.push(String::new());
            lines.push(body.clone());
        }
    } else if payload.get("push").is_some() {
        append_bitbucket_push_summary(&mut lines, payload);
    }
    lines.join("\n")
}

fn append_bitbucket_pullrequest_branches(lines: &mut Vec<String>, payload: &Value) {
    let Some(pullrequest) = payload.get("pullrequest") else {
        return;
    };
    if let Some(source) = pointer_string(pullrequest, "/source/branch/name") {
        lines.push(format!("Source branch: {source}"));
    }
    if let Some(destination) = pointer_string(pullrequest, "/destination/branch/name") {
        lines.push(format!("Destination branch: {destination}"));
    }
}

fn append_bitbucket_state(lines: &mut Vec<String>, payload: &Value) {
    payload
        .get("pullrequest")
        .and_then(|value| string_field(value, "state"))
        .or_else(|| {
            payload
                .get("issue")
                .and_then(|value| string_field(value, "state"))
        })
        .map(|state| lines.push(format!("State: {state}")));
}

fn append_bitbucket_push_summary(lines: &mut Vec<String>, payload: &Value) {
    if let Some(reference) = bitbucket_push_reference(payload) {
        lines.push(format!("Ref: {reference}"));
    }
    let commits = payload
        .pointer("/push/changes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|change| change.get("commits").and_then(Value::as_array))
        .flatten()
        .take(3)
        .collect::<Vec<_>>();
    if commits.is_empty() {
        return;
    }
    lines.push(String::new());
    lines.push("Commits:".to_string());
    for commit in commits {
        let hash = string_field(commit, "hash")
            .map(|value| value.chars().take(8).collect::<String>())
            .unwrap_or_else(|| "commit".to_string());
        let message = string_field(commit, "message")
            .map(snippet)
            .unwrap_or_default();
        lines.push(format!("- {hash}: {message}"));
    }
}

fn bitbucket_push_reference(payload: &Value) -> Option<String> {
    payload
        .pointer("/push/changes")
        .and_then(Value::as_array)
        .and_then(|changes| changes.first())
        .and_then(|change| change.get("new").or_else(|| change.get("old")))
        .and_then(|reference| {
            pointer_string(reference, "/links/html/href")
                .or_else(|| string_field(reference, "name"))
                .or_else(|| string_field(reference, "target"))
        })
        .map(str::to_string)
}

fn bitbucket_repository(payload: &Value) -> Option<String> {
    pointer_string(payload, "/repository/full_name")
        .or_else(|| pointer_string(payload, "/repository/name"))
        .map(str::to_string)
}

fn bitbucket_actor(payload: &Value) -> &str {
    pointer_string(payload, "/actor/display_name")
        .or_else(|| pointer_string(payload, "/actor/nickname"))
        .or_else(|| pointer_string(payload, "/actor/account_id"))
        .or_else(|| pointer_string(payload, "/actor/username"))
        .unwrap_or("bitbucket")
}

fn bitbucket_subject_marker(subject: &BitbucketSubject) -> String {
    let Some(key) = &subject.key else {
        return String::new();
    };
    match subject.conversation_kind {
        "pull-request" => format!("#{key} "),
        "issue" => format!("#{key} "),
        _ => format!("{key} "),
    }
}

fn bitbucket_content_text(value: &Value) -> Option<String> {
    pointer_string(value, "/content/raw")
        .or_else(|| pointer_string(value, "/content/html"))
        .or_else(|| string_field(value, "content"))
        .map(snippet)
}

fn bitbucket_html_url(value: &Value) -> Option<String> {
    pointer_string(value, "/links/html/href").map(str::to_string)
}

fn normalize_bitbucket_part(value: &str) -> String {
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
    use serde_json::json;

    #[test]
    fn bitbucket_push_payload_maps_to_inbound_message() {
        let mut headers = HeaderMap::new();
        headers.insert("x-event-key", "repo:push".parse().unwrap());
        headers.insert("x-request-uuid", "request-1".parse().unwrap());
        let payload = json!({
            "actor": {"display_name": "Tony"},
            "repository": {
                "full_name": "berabuddies/puffer",
                "name": "puffer"
            },
            "push": {
                "changes": [
                    {
                        "new": {"type": "branch", "name": "main"},
                        "old": {"type": "branch", "name": "main"},
                        "commits": [
                            {
                                "hash": "abcdef1234567890",
                                "message": "Add Bitbucket workflow preset"
                            }
                        ]
                    }
                ]
            }
        });

        let inbound = bitbucket_inbound(&headers, &payload).expect("bitbucket inbound");

        assert_eq!(
            inbound.conversation_id,
            "bitbucket:berabuddies/puffer:push:main"
        );
        assert_eq!(inbound.user_id.as_deref(), Some("Tony"));
        assert!(inbound
            .text
            .contains("Bitbucket repo:push in berabuddies/puffer"));
        assert!(inbound.text.contains("Ref: main"));
        assert!(inbound
            .text
            .contains("- abcdef12: Add Bitbucket workflow preset"));
    }

    #[test]
    fn bitbucket_pullrequest_payload_maps_to_inbound_message() {
        let mut headers = HeaderMap::new();
        headers.insert("x-event-key", "pullrequest:created".parse().unwrap());
        let payload = json!({
            "actor": {"nickname": "tonykebot"},
            "repository": {"full_name": "berabuddies/puffer"},
            "pullrequest": {
                "id": 42,
                "title": "Improve workflow connector UX",
                "description": "Bring Bitbucket into the connector picker.",
                "state": "OPEN",
                "source": {"branch": {"name": "feature/bitbucket"}},
                "destination": {"branch": {"name": "master"}},
                "links": {"html": {"href": "https://bitbucket.org/berabuddies/puffer/pull-requests/42"}}
            }
        });

        let inbound = bitbucket_inbound(&headers, &payload).expect("bitbucket inbound");

        assert_eq!(
            inbound.conversation_id,
            "bitbucket:berabuddies/puffer:pull-request:42"
        );
        assert_eq!(inbound.user_id.as_deref(), Some("tonykebot"));
        assert!(inbound
            .text
            .contains("Subject: pull request #42 Improve workflow connector UX"));
        assert!(inbound.text.contains("Source branch: feature/bitbucket"));
        assert!(inbound.text.contains("Destination branch: master"));
        assert!(inbound.text.contains("State: OPEN"));
    }

    #[test]
    fn bitbucket_pullrequest_comment_uses_parent_thread() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-event-key",
            "pullrequest:comment_created".parse().unwrap(),
        );
        let payload = json!({
            "actor": {"display_name": "Reviewer"},
            "repository": {"full_name": "berabuddies/puffer"},
            "pullrequest": {
                "id": 7,
                "title": "Ship workflow UX",
                "state": "OPEN"
            },
            "comment": {
                "content": {"raw": "Please include Bitbucket."},
                "links": {"html": {"href": "https://bitbucket.org/berabuddies/puffer/pull-requests/7/_/diff#comment-1"}}
            }
        });

        let inbound = bitbucket_inbound(&headers, &payload).expect("bitbucket inbound");

        assert_eq!(
            inbound.conversation_id,
            "bitbucket:berabuddies/puffer:pull-request:7"
        );
        assert!(inbound
            .text
            .contains("Subject: comment on pull request #7 Ship workflow UX"));
        assert!(inbound.text.contains("Please include Bitbucket."));
    }

    #[test]
    fn bitbucket_shape_does_not_claim_generic_repository_payloads() {
        let headers = HeaderMap::new();
        let payload = json!({
            "repository": {"full_name": "example/repo"},
            "message": "generic repository event"
        });

        assert!(bitbucket_inbound(&headers, &payload).is_none());
    }
}

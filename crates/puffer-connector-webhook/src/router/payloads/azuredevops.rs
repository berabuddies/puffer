use axum::http::HeaderMap;
use puffer_connector_core::InboundMessage;
use serde_json::Value;

use super::super::{number_or_string, pointer_string, snippet, string_field};

/// Converts an Azure DevOps Service Hooks payload into an inbound Puffer message.
pub(super) fn azuredevops_inbound(_headers: &HeaderMap, payload: &Value) -> Option<InboundMessage> {
    let event = string_field(payload, "eventType").or_else(|| string_field(payload, "eventId"))?;
    if !azuredevops_payload_shape(payload, event) {
        return None;
    }

    let project = azuredevops_project(payload);
    let actor = azuredevops_actor(payload);
    let conversation_id = azuredevops_conversation_id(&project, event, payload);
    let text = azuredevops_message(&project, event, actor, payload);

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

fn azuredevops_payload_shape(payload: &Value, event: &str) -> bool {
    let known_event = event.starts_with("git.")
        || event.starts_with("workitem.")
        || event.starts_with("ms.vss-code.");
    if !known_event || payload.get("resource").is_none() {
        return false;
    }
    let publisher = string_field(payload, "publisherId");
    publisher.is_some_and(|value| matches!(value, "tfs" | "azure-devops"))
        || payload.get("resourceContainers").is_some()
        || payload.get("message").is_some()
}

fn azuredevops_conversation_id(project: &str, event: &str, payload: &Value) -> String {
    let project = normalize_azuredevops_part(project);
    if let Some(id) = azuredevops_pull_request_id(payload) {
        return format!("azure-devops:{project}:pull-request:{id}");
    }
    if let Some(id) = azuredevops_work_item_id(payload) {
        return format!("azure-devops:{project}:work-item:{id}");
    }
    if event == "git.push" {
        if let Some(reference) = azuredevops_push_reference(payload) {
            let repository = azuredevops_repository(payload)
                .map(|value| normalize_azuredevops_part(&value))
                .unwrap_or_else(|| "repository".to_string());
            return format!(
                "azure-devops:{project}:push:{repository}:{}",
                normalize_azuredevops_part(&reference)
            );
        }
    }
    let delivery = string_field(payload, "id")
        .or_else(|| string_field(payload, "notificationId"))
        .unwrap_or("event");
    format!(
        "azure-devops:{project}:{}:{}",
        normalize_azuredevops_part(event),
        normalize_azuredevops_part(delivery)
    )
}

fn azuredevops_message(project: &str, event: &str, actor: &str, payload: &Value) -> String {
    let mut lines = vec![
        format!("Azure DevOps {event} in {project}"),
        format!("Actor: {actor}"),
    ];
    if event == "git.push" {
        append_azuredevops_push(&mut lines, payload);
    } else if azuredevops_pull_request_id(payload).is_some() {
        append_azuredevops_pull_request(&mut lines, payload);
    } else if azuredevops_work_item_id(payload).is_some() {
        append_azuredevops_work_item(&mut lines, payload);
    }
    if let Some(text) = azuredevops_detail_text(payload) {
        lines.push(String::new());
        lines.push(text);
    }
    lines.join("\n")
}

fn append_azuredevops_push(lines: &mut Vec<String>, payload: &Value) {
    if let Some(repository) = azuredevops_repository(payload) {
        lines.push(format!("Repository: {repository}"));
    }
    if let Some(reference) = azuredevops_push_reference(payload) {
        lines.push(format!("Ref: {reference}"));
    }
    let commits = payload
        .pointer("/resource/commits")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(3)
        .collect::<Vec<_>>();
    if commits.is_empty() {
        return;
    }
    lines.push(String::new());
    lines.push("Commits:".to_string());
    for commit in commits {
        let id = string_field(commit, "commitId")
            .or_else(|| string_field(commit, "commit_id"))
            .or_else(|| string_field(commit, "id"))
            .map(|value| value.chars().take(8).collect::<String>())
            .unwrap_or_else(|| "commit".to_string());
        let message = string_field(commit, "comment")
            .or_else(|| string_field(commit, "message"))
            .map(snippet)
            .unwrap_or_default();
        lines.push(format!("- {id}: {message}"));
    }
}

fn append_azuredevops_pull_request(lines: &mut Vec<String>, payload: &Value) {
    if let Some(repository) = azuredevops_repository(payload) {
        lines.push(format!("Repository: {repository}"));
    }
    let id = azuredevops_pull_request_id(payload).unwrap_or_else(|| "pull-request".to_string());
    let title = pointer_string(payload, "/resource/title").unwrap_or_default();
    lines.push(format!("Subject: pull request #{id} {title}"));
    for (label, path) in [
        ("Status", "/resource/status"),
        ("Merge status", "/resource/mergeStatus"),
        ("Source branch", "/resource/sourceRefName"),
        ("Target branch", "/resource/targetRefName"),
    ] {
        if let Some(value) = pointer_string(payload, path) {
            lines.push(format!("{label}: {value}"));
        }
    }
    if let Some(url) = azuredevops_web_url(payload) {
        lines.push(format!("URL: {url}"));
    }
    if let Some(comment) = azuredevops_pull_request_comment(payload) {
        lines.push(String::new());
        lines.push(comment);
    }
}

fn append_azuredevops_work_item(lines: &mut Vec<String>, payload: &Value) {
    let id = azuredevops_work_item_id(payload).unwrap_or_else(|| "work-item".to_string());
    let work_type =
        pointer_string(payload, "/resource/fields/System.WorkItemType").unwrap_or("work item");
    let title = pointer_string(payload, "/resource/fields/System.Title").unwrap_or_default();
    lines.push(format!("Subject: {work_type} #{id} {title}"));
    if let Some(state) = pointer_string(payload, "/resource/fields/System.State") {
        lines.push(format!("State: {state}"));
    }
    if let Some(area) = pointer_string(payload, "/resource/fields/System.AreaPath") {
        lines.push(format!("Area: {area}"));
    }
    if let Some(url) = azuredevops_web_url(payload).or_else(|| {
        pointer_string(payload, "/resource/url")
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    }) {
        lines.push(format!("URL: {url}"));
    }
}

fn azuredevops_payload_text(payload: &Value, pointer: &str) -> Option<String> {
    pointer_string(payload, pointer)
        .filter(|value| !value.trim().is_empty())
        .map(snippet)
}

fn azuredevops_detail_text(payload: &Value) -> Option<String> {
    azuredevops_payload_text(payload, "/detailedMessage/text")
        .or_else(|| azuredevops_payload_text(payload, "/message/text"))
}

fn azuredevops_pull_request_comment(payload: &Value) -> Option<String> {
    pointer_string(payload, "/resource/comment/content")
        .or_else(|| pointer_string(payload, "/resource/comment/text"))
        .or_else(|| pointer_string(payload, "/resource/comment"))
        .map(snippet)
}

fn azuredevops_project(payload: &Value) -> String {
    pointer_string(payload, "/resource/repository/project/name")
        .or_else(|| pointer_string(payload, "/resource/project/name"))
        .or_else(|| pointer_string(payload, "/resource/fields/System.TeamProject"))
        .or_else(|| pointer_string(payload, "/resourceContainers/project/id"))
        .map(str::to_string)
        .unwrap_or_else(|| "project".to_string())
}

fn azuredevops_repository(payload: &Value) -> Option<String> {
    pointer_string(payload, "/resource/repository/name")
        .or_else(|| pointer_string(payload, "/resource/repository/id"))
        .map(str::to_string)
}

fn azuredevops_actor(payload: &Value) -> &str {
    pointer_string(payload, "/resource/pushedBy/displayName")
        .or_else(|| pointer_string(payload, "/resource/createdBy/displayName"))
        .or_else(|| pointer_string(payload, "/resource/createdBy/uniqueName"))
        .or_else(|| pointer_string(payload, "/resource/lastMergeSourceCommit/author/name"))
        .or_else(|| pointer_string(payload, "/resource/fields/System.ChangedBy"))
        .or_else(|| pointer_string(payload, "/resource/fields/System.CreatedBy"))
        .unwrap_or("azure-devops")
}

fn azuredevops_pull_request_id(payload: &Value) -> Option<String> {
    payload
        .pointer("/resource/pullRequestId")
        .and_then(number_or_string)
        .or_else(|| payload.pointer("/resource/id").and_then(number_or_string))
        .filter(|_| {
            string_field(payload, "eventType")
                .or_else(|| string_field(payload, "eventId"))
                .is_some_and(|event| {
                    event.contains("pullrequest") || event.contains("pull-request")
                })
        })
}

fn azuredevops_work_item_id(payload: &Value) -> Option<String> {
    payload
        .pointer("/resource/id")
        .and_then(number_or_string)
        .filter(|_| {
            string_field(payload, "eventType")
                .or_else(|| string_field(payload, "eventId"))
                .is_some_and(|event| event.starts_with("workitem."))
        })
}

fn azuredevops_push_reference(payload: &Value) -> Option<String> {
    payload
        .pointer("/resource/refUpdates")
        .and_then(Value::as_array)
        .and_then(|updates| updates.first())
        .and_then(|update| string_field(update, "name"))
        .or_else(|| pointer_string(payload, "/resource/refUpdates/0/refName"))
        .map(str::to_string)
}

fn azuredevops_web_url(payload: &Value) -> Option<String> {
    pointer_string(payload, "/resource/_links/web/href")
        .or_else(|| pointer_string(payload, "/resource/links/web/href"))
        .or_else(|| pointer_string(payload, "/resource/url"))
        .map(str::to_string)
}

fn normalize_azuredevops_part(value: &str) -> String {
    let normalized = value
        .trim()
        .to_ascii_lowercase()
        .replace(':', "_")
        .replace('/', "_")
        .replace(' ', "_")
        .replace('-', "_");
    if normalized.is_empty() {
        "azure_devops".to_string()
    } else {
        normalized
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn azuredevops_push_payload_maps_to_inbound_message() {
        let headers = HeaderMap::new();
        let payload = serde_json::json!({
            "id": "delivery-1",
            "eventType": "git.push",
            "publisherId": "tfs",
            "message": {"text": "Tony pushed updates to branch main."},
            "resource": {
                "repository": {
                    "name": "puffer",
                    "project": {"name": "Workflow UX"}
                },
                "pushedBy": {"displayName": "Tony"},
                "refUpdates": [{"name": "refs/heads/main"}],
                "commits": [
                    {
                        "commitId": "abcdef1234567890",
                        "comment": "Add Azure DevOps workflow preset"
                    }
                ]
            }
        });

        let inbound = azuredevops_inbound(&headers, &payload).expect("azure devops inbound");

        assert_eq!(
            inbound.conversation_id,
            "azure-devops:workflow_ux:push:puffer:refs_heads_main"
        );
        assert_eq!(inbound.user_id.as_deref(), Some("Tony"));
        assert!(inbound
            .text
            .contains("Azure DevOps git.push in Workflow UX"));
        assert!(inbound.text.contains("Repository: puffer"));
        assert!(inbound.text.contains("Ref: refs/heads/main"));
        assert!(inbound
            .text
            .contains("- abcdef12: Add Azure DevOps workflow preset"));
    }

    #[test]
    fn azuredevops_pull_request_payload_maps_to_inbound_message() {
        let headers = HeaderMap::new();
        let payload = serde_json::json!({
            "id": "delivery-2",
            "eventType": "git.pullrequest.created",
            "publisherId": "tfs",
            "resource": {
                "pullRequestId": 42,
                "title": "Improve connector UX",
                "status": "active",
                "mergeStatus": "succeeded",
                "sourceRefName": "refs/heads/feature/ado",
                "targetRefName": "refs/heads/master",
                "repository": {
                    "name": "puffer",
                    "project": {"name": "Puffer"}
                },
                "createdBy": {"displayName": "Mona"},
                "_links": {
                    "web": {
                        "href": "https://dev.azure.com/berabuddies/Puffer/_git/puffer/pullrequest/42"
                    }
                }
            }
        });

        let inbound = azuredevops_inbound(&headers, &payload).expect("azure devops inbound");

        assert_eq!(
            inbound.conversation_id,
            "azure-devops:puffer:pull-request:42"
        );
        assert_eq!(inbound.user_id.as_deref(), Some("Mona"));
        assert!(inbound
            .text
            .contains("Subject: pull request #42 Improve connector UX"));
        assert!(inbound
            .text
            .contains("Source branch: refs/heads/feature/ado"));
        assert!(inbound.text.contains("Target branch: refs/heads/master"));
        assert!(inbound.text.contains("Merge status: succeeded"));
    }

    #[test]
    fn azuredevops_work_item_comment_uses_work_item_thread() {
        let headers = HeaderMap::new();
        let payload = serde_json::json!({
            "id": "delivery-3",
            "eventType": "workitem.commented",
            "publisherId": "tfs",
            "detailedMessage": {
                "text": "Bug #5 commented on by Tony.\nThis is a great new idea"
            },
            "resource": {
                "id": 5,
                "fields": {
                    "System.TeamProject": "Puffer",
                    "System.WorkItemType": "Bug",
                    "System.State": "New",
                    "System.Title": "Improve workflow UX",
                    "System.ChangedBy": "Tony",
                    "System.History": "This is a great new idea"
                },
                "url": "https://dev.azure.com/berabuddies/Puffer/_apis/wit/workItems/5"
            }
        });

        let inbound = azuredevops_inbound(&headers, &payload).expect("azure devops inbound");

        assert_eq!(inbound.conversation_id, "azure-devops:puffer:work-item:5");
        assert_eq!(inbound.user_id.as_deref(), Some("Tony"));
        assert!(inbound.text.contains("Subject: Bug #5 Improve workflow UX"));
        assert!(inbound.text.contains("State: New"));
        assert!(inbound.text.contains("This is a great new idea"));
    }

    #[test]
    fn azuredevops_shape_rejects_unrelated_event_payloads() {
        let headers = HeaderMap::new();
        let payload = serde_json::json!({
            "eventType": "unknown.event",
            "publisherId": "tfs",
            "resource": {"id": "1"}
        });

        assert!(azuredevops_inbound(&headers, &payload).is_none());
    }
}

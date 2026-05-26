use axum::http::HeaderMap;
use puffer_connector_core::InboundMessage;
use serde_json::Value;
use std::collections::BTreeMap;

use super::super::{number_or_string, pointer_string, snippet, string_field};

/// Converts a Figma Webhooks V2 payload into an inbound Puffer message.
pub(super) fn figma_inbound(_headers: &HeaderMap, payload: &Value) -> Option<InboundMessage> {
    let event = string_field(payload, "event_type")?;
    if !figma_payload_shape(event, payload) {
        return None;
    }

    let actor = figma_actor(payload);
    let conversation_id = figma_conversation_id(event, payload);
    let text = figma_message(event, actor, payload);

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

fn figma_payload_shape(event: &str, payload: &Value) -> bool {
    matches!(
        event,
        "PING"
            | "FILE_UPDATE"
            | "FILE_DELETE"
            | "FILE_VERSION_UPDATE"
            | "LIBRARY_PUBLISH"
            | "FILE_COMMENT"
            | "DEV_MODE_STATUS_UPDATE"
    ) && (payload.get("webhook_id").is_some()
        || payload.get("file_key").is_some()
        || payload.get("passcode").is_some())
}

fn figma_conversation_id(event: &str, payload: &Value) -> String {
    if event == "PING" {
        return format!("figma:webhook:{}", figma_delivery(payload));
    }

    let file = string_field(payload, "file_key")
        .map(normalize_figma_part)
        .unwrap_or_else(|| "file".to_string());
    match event {
        "FILE_COMMENT" => {
            let comment = payload
                .get("parent_id")
                .and_then(number_or_string)
                .filter(|value| !value.trim().is_empty())
                .or_else(|| payload.get("comment_id").and_then(number_or_string))
                .unwrap_or_else(|| figma_delivery(payload));
            format!(
                "figma:file:{file}:comment:{}",
                normalize_figma_part(&comment)
            )
        }
        "FILE_VERSION_UPDATE" => {
            let version = string_field(payload, "version_id")
                .map(normalize_figma_part)
                .unwrap_or_else(|| figma_delivery(payload));
            format!("figma:file:{file}:version:{version}")
        }
        "LIBRARY_PUBLISH" => format!("figma:file:{file}:library:{}", figma_delivery(payload)),
        "DEV_MODE_STATUS_UPDATE" => {
            let node = string_field(payload, "node_id")
                .map(normalize_figma_part)
                .unwrap_or_else(|| figma_delivery(payload));
            format!("figma:file:{file}:node:{node}")
        }
        _ => format!("figma:file:{file}:file"),
    }
}

fn figma_message(event: &str, actor: &str, payload: &Value) -> String {
    let file_name = string_field(payload, "file_name").unwrap_or("file");
    let mut lines = vec![format!("Figma {event} for {file_name}")];
    lines.push(format!("Actor: {actor}"));
    if let Some(file_key) = string_field(payload, "file_key") {
        lines.push(format!("File key: {file_key}"));
    }

    match event {
        "FILE_COMMENT" => append_figma_comment(&mut lines, payload),
        "FILE_VERSION_UPDATE" => append_figma_version(&mut lines, payload),
        "LIBRARY_PUBLISH" => append_figma_library_publish(&mut lines, payload),
        "DEV_MODE_STATUS_UPDATE" => append_figma_dev_mode(&mut lines, payload),
        "FILE_DELETE" => lines.push("Status: deleted".to_string()),
        "PING" => lines.push("Status: ping".to_string()),
        _ => {}
    }

    if let Some(timestamp) = string_field(payload, "timestamp") {
        lines.push(format!("Timestamp: {timestamp}"));
    }
    if let Some(url) = figma_url(payload) {
        lines.push(format!("URL: {url}"));
    }

    lines.join("\n")
}

fn append_figma_comment(lines: &mut Vec<String>, payload: &Value) {
    if let Some(comment_id) = payload.get("comment_id").and_then(number_or_string) {
        lines.push(format!("Comment: {comment_id}"));
    }
    if let Some(parent_id) = payload
        .get("parent_id")
        .and_then(number_or_string)
        .filter(|value| !value.trim().is_empty())
    {
        lines.push(format!("Reply to: {parent_id}"));
    }
    if let Some(resolved_at) =
        string_field(payload, "resolved_at").filter(|value| !value.is_empty())
    {
        lines.push(format!("Resolved at: {resolved_at}"));
    }
    if let Some(comment) = figma_comment_text(payload) {
        lines.push(String::new());
        lines.push(comment);
    }
}

fn append_figma_version(lines: &mut Vec<String>, payload: &Value) {
    if let Some(version) = string_field(payload, "version_id") {
        lines.push(format!("Version: {version}"));
    }
    if let Some(label) = string_field(payload, "label") {
        lines.push(format!("Label: {}", snippet(label)));
    }
    if let Some(description) =
        string_field(payload, "description").filter(|value| !value.is_empty())
    {
        lines.push(String::new());
        lines.push(snippet(description));
    }
}

fn append_figma_library_publish(lines: &mut Vec<String>, payload: &Value) {
    if let Some(description) =
        string_field(payload, "description").filter(|value| !value.is_empty())
    {
        lines.push(String::new());
        lines.push(snippet(description));
    }
    let summaries = [
        ("created components", "created_components"),
        ("modified components", "modified_components"),
        ("deleted components", "deleted_components"),
        ("created styles", "created_styles"),
        ("modified styles", "modified_styles"),
        ("deleted styles", "deleted_styles"),
        ("created variables", "created_variables"),
        ("modified variables", "modified_variables"),
        ("deleted variables", "deleted_variables"),
    ]
    .into_iter()
    .filter_map(|(label, field)| {
        let count = payload
            .get(field)
            .and_then(Value::as_array)
            .map(|items| items.len())
            .unwrap_or(0);
        (count > 0).then(|| format!("{label}: {count}"))
    })
    .collect::<Vec<_>>();
    if !summaries.is_empty() {
        lines.push("Library changes:".to_string());
        lines.extend(summaries);
    }
}

fn append_figma_dev_mode(lines: &mut Vec<String>, payload: &Value) {
    if let Some(status) = string_field(payload, "status") {
        lines.push(format!("Status: {status}"));
    }
    if let Some(node_id) = string_field(payload, "node_id") {
        lines.push(format!("Node: {node_id}"));
    }
    if let Some(change) = string_field(payload, "change_message").filter(|value| !value.is_empty())
    {
        lines.push(String::new());
        lines.push(snippet(change));
    }
    let links = payload
        .get("related_links")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(3)
        .filter_map(figma_related_link_label)
        .collect::<Vec<_>>();
    if !links.is_empty() {
        lines.push("Related links:".to_string());
        lines.extend(links.into_iter().map(|link| format!("- {link}")));
    }
}

fn figma_comment_text(payload: &Value) -> Option<String> {
    let fragments = payload.get("comment")?.as_array()?;
    let mentions = figma_mentions(payload);
    let text = fragments
        .iter()
        .filter_map(|fragment| {
            string_field(fragment, "text")
                .map(str::to_string)
                .or_else(|| figma_comment_mention(fragment, &mentions))
        })
        .collect::<Vec<_>>()
        .join("");
    (!text.trim().is_empty()).then(|| snippet(&text))
}

fn figma_mentions(payload: &Value) -> BTreeMap<String, String> {
    payload
        .get("mentions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|mention| {
            let id = string_field(mention, "id")?;
            let handle = string_field(mention, "handle").unwrap_or(id);
            Some((id.to_string(), handle.to_string()))
        })
        .collect()
}

fn figma_comment_mention(fragment: &Value, mentions: &BTreeMap<String, String>) -> Option<String> {
    let id = string_field(fragment, "mention")?;
    let handle = mentions.get(id).map(String::as_str).unwrap_or(id);
    Some(format!("@{handle}"))
}

fn figma_related_link_label(value: &Value) -> Option<String> {
    let name = string_field(value, "name").or_else(|| string_field(value, "id"))?;
    let url = string_field(value, "url");
    Some(
        url.map(|url| format!("{} {url}", snippet(name)))
            .unwrap_or_else(|| snippet(name)),
    )
}

fn figma_actor(payload: &Value) -> &str {
    pointer_string(payload, "/triggered_by/handle")
        .or_else(|| pointer_string(payload, "/triggered_by/id"))
        .unwrap_or("figma")
}

fn figma_url(payload: &Value) -> Option<String> {
    string_field(payload, "file_url")
        .or_else(|| string_field(payload, "url"))
        .map(str::to_string)
        .or_else(|| {
            string_field(payload, "file_key").map(|key| format!("https://www.figma.com/file/{key}"))
        })
}

fn figma_delivery(payload: &Value) -> String {
    payload
        .get("webhook_id")
        .and_then(number_or_string)
        .or_else(|| string_field(payload, "timestamp").map(normalize_figma_part))
        .unwrap_or_else(|| "event".to_string())
}

fn normalize_figma_part(value: &str) -> String {
    let normalized = value
        .trim()
        .to_ascii_lowercase()
        .replace(':', "_")
        .replace('/', "_")
        .replace(' ', "_")
        .replace('-', "_");
    if normalized.is_empty() {
        "figma".to_string()
    } else {
        normalized
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn figma_comment_payload_maps_to_comment_thread() {
        let headers = HeaderMap::new();
        let payload = serde_json::json!({
            "event_type": "FILE_COMMENT",
            "file_key": "zH44k2FUM629Fa4EMShiHL",
            "file_name": "Mockup library",
            "comment": [
                {"text": "TODO: "},
                {"mention": "811724164054158337"},
                {"text": " change selection colors"}
            ],
            "comment_id": "32",
            "parent_id": "",
            "mentions": [{"id": "811724164054158337", "handle": "Evan Wallace"}],
            "passcode": "secretpasscode",
            "timestamp": "2020-02-23T20:27:16Z",
            "triggered_by": {"id": "813845097374535682", "handle": "Dylan Field"},
            "webhook_id": "22"
        });

        let inbound = figma_inbound(&headers, &payload).expect("figma inbound");

        assert_eq!(
            inbound.conversation_id,
            "figma:file:zh44k2fum629fa4emshihl:comment:32"
        );
        assert_eq!(inbound.user_id.as_deref(), Some("Dylan Field"));
        assert!(inbound
            .text
            .contains("Figma FILE_COMMENT for Mockup library"));
        assert!(inbound
            .text
            .contains("TODO: @Evan Wallace change selection colors"));
    }

    #[test]
    fn figma_library_publish_payload_summarizes_asset_counts() {
        let headers = HeaderMap::new();
        let payload = serde_json::json!({
            "event_type": "LIBRARY_PUBLISH",
            "file_key": "zH44k2FUM629Fa4EMShiHL",
            "file_name": "Design system",
            "description": "Publish ready components",
            "created_components": [{"key": "component-1", "name": "Button"}],
            "modified_styles": [{"key": "style-1", "name": "Primary"}],
            "passcode": "secretpasscode",
            "timestamp": "2020-02-23T20:27:16Z",
            "triggered_by": {"handle": "Tony"},
            "webhook_id": "23"
        });

        let inbound = figma_inbound(&headers, &payload).expect("figma inbound");

        assert_eq!(
            inbound.conversation_id,
            "figma:file:zh44k2fum629fa4emshihl:library:23"
        );
        assert!(inbound.text.contains("created components: 1"));
        assert!(inbound.text.contains("modified styles: 1"));
        assert!(inbound.text.contains("Publish ready components"));
    }

    #[test]
    fn figma_dev_mode_payload_uses_node_thread() {
        let headers = HeaderMap::new();
        let payload = serde_json::json!({
            "event_type": "DEV_MODE_STATUS_UPDATE",
            "file_key": "ABzTs1A2aFSy960zBI3nMM",
            "node_id": "43:2",
            "status": "READY_FOR_DEV",
            "change_message": "New rectangle",
            "related_links": [{
                "id": 1118075899259441212u64,
                "name": "Issue BB-8",
                "url": "https://test.atlassian.net/BB-8"
            }],
            "passcode": "secretpasscode",
            "timestamp": "2025-05-14T23:28:40Z",
            "triggered_by": {"handle": "Dylan Field"},
            "webhook_id": "434"
        });

        let inbound = figma_inbound(&headers, &payload).expect("figma inbound");

        assert_eq!(
            inbound.conversation_id,
            "figma:file:abzts1a2afsy960zbi3nmm:node:43_2"
        );
        assert!(inbound.text.contains("Status: READY_FOR_DEV"));
        assert!(inbound.text.contains("Node: 43:2"));
        assert!(inbound
            .text
            .contains("Issue BB-8 https://test.atlassian.net/BB-8"));
    }

    #[test]
    fn figma_shape_rejects_unrelated_payloads() {
        let headers = HeaderMap::new();
        let payload = serde_json::json!({
            "event_type": "SOMETHING_ELSE",
            "webhook_id": "22"
        });

        assert!(figma_inbound(&headers, &payload).is_none());
    }
}

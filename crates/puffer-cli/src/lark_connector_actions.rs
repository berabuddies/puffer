//! Lark direct connector-action helpers.

use anyhow::{bail, Result};
use puffer_config::ConfigPaths;
use puffer_lark::{LarkClient, LarkMediaKind};
use puffer_subscriptions::{ConnectionAuthChecker, ConnectorTemplate};
use serde_json::Value;
use std::path::PathBuf;

pub(crate) struct LarkConnectionAuthChecker {
    pub(crate) paths: ConfigPaths,
}

impl ConnectionAuthChecker for LarkConnectionAuthChecker {
    fn check(
        &self,
        _manager: &puffer_subscriptions::SubscriptionManager,
        template: &ConnectorTemplate,
        connection_slug: &str,
    ) -> Result<Option<bool>> {
        if !is_lark_credential_connector(&template.slug) {
            return Ok(None);
        }
        let path = puffer_lark::credential_path(&self.paths.user_config_dir, connection_slug);
        if !path.exists() {
            return Ok(Some(false));
        }
        let credential = puffer_lark::load_credential(&path)?;
        let client = puffer_lark::LarkClient::new(credential)?;
        client.is_auth_ok().map(Some)
    }
}

pub(crate) fn run_lark_action(
    paths: &ConfigPaths,
    connection_slug: &str,
    action: &str,
    input: &Value,
) -> Result<String> {
    let path = puffer_lark::credential_path(&paths.user_config_dir, connection_slug);
    let credential = puffer_lark::load_credential(&path)?;
    let client = puffer_lark::LarkClient::new(credential)?;
    match action {
        "send_message" => lark_send_message(&client, connection_slug, input),
        "react" | "send_reaction" | "remove_reaction" => {
            lark_react(&client, connection_slug, action, input)
        }
        _ => bail!("unsupported Lark action `{action}`"),
    }
}

pub(crate) fn is_lark_connector(connector_slug: &str) -> bool {
    matches!(connector_slug, "lark" | "lark-app" | "lark-login")
}

fn is_lark_credential_connector(connector_slug: &str) -> bool {
    matches!(connector_slug, "lark-app" | "lark-login")
}

pub(crate) fn is_lark_action(action: &str) -> bool {
    matches!(
        action,
        "send_message" | "react" | "send_reaction" | "remove_reaction"
    )
}

fn lark_send_message(client: &LarkClient, connection_slug: &str, input: &Value) -> Result<String> {
    let reply_to = parse_lark_reply_to(input)?;
    let text = string_from_keys(input, &["message", "text", "caption"]).unwrap_or_default();
    let media = parse_media_attachments(input)?;
    let explicit_content = lark_explicit_content(input)?;
    if text.trim().is_empty() && media.is_empty() && explicit_content.is_none() {
        bail!("Lark send_message requires `message`, `caption`, `content`, or `media`");
    }

    let target = if reply_to.is_some() {
        lark_optional_target(input)?
    } else {
        Some(lark_target(input)?)
    };
    let reply_in_thread = input
        .get("reply_in_thread")
        .or_else(|| input.get("replyInThread"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let idempotency_key = string_from_keys(input, &["idempotency_key", "uuid"]);
    let mut completed = 0usize;
    let mut last_id = String::new();

    if let Some((msg_type, content)) = explicit_content {
        let response = send_lark_content(
            client,
            target.as_ref(),
            reply_to.as_deref(),
            &msg_type,
            &content,
            reply_in_thread,
            idempotency_key.as_deref(),
        )?;
        last_id = response_message_id(&response);
        completed += 1;
    } else if !text.trim().is_empty() {
        let response = send_lark_content(
            client,
            target.as_ref(),
            reply_to.as_deref(),
            "text",
            &serde_json::json!({"text": text}).to_string(),
            reply_in_thread,
            idempotency_key.as_deref(),
        )?;
        last_id = response_message_id(&response);
        completed += 1;
    }

    for attachment in media {
        if let Some(caption) = attachment
            .caption
            .as_deref()
            .map(str::trim)
            .filter(|caption| !caption.is_empty())
        {
            send_lark_content(
                client,
                target.as_ref(),
                reply_to.as_deref(),
                "text",
                &serde_json::json!({"text": caption}).to_string(),
                reply_in_thread,
                None,
            )?;
            completed += 1;
        }
        let (msg_type, content) = resolve_media_content(client, &attachment)?;
        let response = send_lark_content(
            client,
            target.as_ref(),
            reply_to.as_deref(),
            &msg_type,
            &content,
            reply_in_thread,
            None,
        )?;
        last_id = response_message_id(&response);
        completed += 1;
    }

    Ok(format!(
        "sent {completed} Lark message(s) via {connection_slug}{}",
        if last_id.is_empty() {
            String::new()
        } else {
            format!(" ({last_id})")
        }
    ))
}

fn send_lark_content(
    client: &LarkClient,
    target: Option<&LarkTarget>,
    reply_to: Option<&str>,
    msg_type: &str,
    content: &str,
    reply_in_thread: bool,
    uuid: Option<&str>,
) -> Result<Value> {
    if let Some(message_id) = reply_to {
        return client.reply_message(message_id, msg_type, content, reply_in_thread, uuid);
    }
    let target = target.ok_or_else(|| anyhow::anyhow!("Lark send_message requires `to`"))?;
    client.send_message(
        &target.receive_id,
        &target.receive_id_type,
        msg_type,
        content,
        uuid,
    )
}

fn lark_react(
    client: &LarkClient,
    connection_slug: &str,
    action: &str,
    input: &Value,
) -> Result<String> {
    let message_id = string_from_keys(input, &["message_id", "id"])
        .or_else(|| lark_reply_object_string(input, "message_id"))
        .ok_or_else(|| anyhow::anyhow!("Lark reaction requires `message_id`"))?;
    let remove = action == "remove_reaction"
        || input
            .get("remove")
            .and_then(Value::as_bool)
            .unwrap_or(false);
    if remove {
        let reaction_id = string_from_keys(input, &["reaction_id", "id"])
            .ok_or_else(|| anyhow::anyhow!("Lark remove_reaction requires `reaction_id`; Lark deletes reactions by reaction id, not emoji_type"))?;
        client.delete_reaction(&message_id, &reaction_id)?;
        return Ok(format!(
            "removed Lark reaction `{reaction_id}` via {connection_slug} -> {message_id}"
        ));
    }
    let emoji_type = string_from_keys(input, &["emoji_type", "emoji", "reaction"])
        .map(|emoji| normalize_lark_emoji_type(&emoji))
        .ok_or_else(|| {
            anyhow::anyhow!("Lark reaction requires `emoji_type`, `emoji`, or `reaction`")
        })?;
    client.add_reaction(&message_id, &emoji_type)?;
    Ok(format!(
        "reacted `{emoji_type}` via {connection_slug} -> {message_id}"
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LarkTarget {
    receive_id: String,
    receive_id_type: String,
}

fn lark_target(input: &Value) -> Result<LarkTarget> {
    lark_optional_target(input)?.ok_or_else(|| anyhow::anyhow!("Lark send_message requires `to`"))
}

fn lark_optional_target(input: &Value) -> Result<Option<LarkTarget>> {
    if let Some(chat_id) = string_from_keys(input, &["chat_id", "chat", "channel"]) {
        return Ok(Some(LarkTarget {
            receive_id: chat_id,
            receive_id_type: "chat_id".to_string(),
        }));
    }
    if let Some(open_id) = string_from_keys(input, &["open_id", "user_id", "user"]) {
        return Ok(Some(LarkTarget {
            receive_id: open_id,
            receive_id_type: "open_id".to_string(),
        }));
    }
    let Some(receive_id) = string_from_keys(input, &["to", "target", "receive_id"]) else {
        return Ok(None);
    };
    if let Some(receive_id_type) = string_from_keys(input, &["receive_id_type"]) {
        return Ok(Some(LarkTarget {
            receive_id,
            receive_id_type,
        }));
    }
    if receive_id.starts_with("oc_") {
        return Ok(Some(LarkTarget {
            receive_id,
            receive_id_type: "chat_id".to_string(),
        }));
    }
    if receive_id.starts_with("ou_") {
        return Ok(Some(LarkTarget {
            receive_id,
            receive_id_type: "open_id".to_string(),
        }));
    }
    if receive_id.starts_with("on_") {
        return Ok(Some(LarkTarget {
            receive_id,
            receive_id_type: "union_id".to_string(),
        }));
    }
    bail!(
        "Lark target `{receive_id}` is ambiguous; resolve it first with lark search-chats or lark search-users, or pass receive_id_type explicitly"
    )
}

fn parse_lark_reply_to(input: &Value) -> Result<Option<String>> {
    if let Some(value) = string_from_keys(input, &["reply_to_message_id", "message_id"]) {
        return Ok(Some(value));
    }
    if let Some(value) = input.get("reply_to") {
        if value.is_null() {
            return Ok(None);
        }
        if let Some(message_id) = value
            .as_str()
            .map(str::trim)
            .filter(|message_id| !message_id.is_empty())
        {
            return Ok(Some(message_id.to_string()));
        }
        if let Some(message_id) = value
            .get("message_id")
            .or_else(|| value.get("id"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|message_id| !message_id.is_empty())
        {
            return Ok(Some(message_id.to_string()));
        }
        bail!("Lark reply_to must be a message_id string or object with message_id");
    }
    Ok(None)
}

fn lark_explicit_content(input: &Value) -> Result<Option<(String, String)>> {
    let Some(content) = input.get("content") else {
        return Ok(None);
    };
    let msg_type = string_from_keys(input, &["msg_type", "message_type"]).unwrap_or_else(|| {
        if content.is_string() {
            "text".to_string()
        } else {
            "interactive".to_string()
        }
    });
    let content = match content.as_str() {
        Some(raw) if msg_type == "text" && !looks_like_json(raw) => {
            serde_json::json!({"text": raw}).to_string()
        }
        Some(raw) => raw.to_string(),
        None => serde_json::to_string(content)?,
    };
    Ok(Some((msg_type, content)))
}

fn looks_like_json(value: &str) -> bool {
    let trimmed = value.trim_start();
    trimmed.starts_with('{') || trimmed.starts_with('[')
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LarkMediaAttachment {
    path: String,
    caption: Option<String>,
    kind: LarkMediaKind,
}

fn parse_media_attachments(input: &Value) -> Result<Vec<LarkMediaAttachment>> {
    let mut media = Vec::new();
    for key in ["media", "attachments", "files"] {
        if let Some(value) = input.get(key) {
            parse_media_value(value, &mut media)?;
        }
    }
    for key in ["file", "path", "image"] {
        if let Some(value) = input.get(key) {
            parse_media_value(value, &mut media)?;
        }
    }
    Ok(media)
}

fn parse_media_value(value: &Value, media: &mut Vec<LarkMediaAttachment>) -> Result<()> {
    if value.is_null() {
        return Ok(());
    }
    if let Some(items) = value.as_array() {
        for item in items {
            media.push(parse_media_attachment(item)?);
        }
        return Ok(());
    }
    media.push(parse_media_attachment(value)?);
    Ok(())
}

fn parse_media_attachment(value: &Value) -> Result<LarkMediaAttachment> {
    if let Some(path) = value.as_str() {
        return Ok(LarkMediaAttachment {
            path: path.to_string(),
            caption: None,
            kind: LarkMediaKind::Auto,
        });
    }
    let Some(object) = value.as_object() else {
        bail!("Lark media attachment must be a string path/key or object");
    };
    let path = object
        .get("path")
        .or_else(|| object.get("file"))
        .or_else(|| object.get("image"))
        .or_else(|| object.get("source"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if path.trim().is_empty() {
        bail!("Lark media attachment object requires `path`, `file`, or `image`");
    }
    let caption = object
        .get("caption")
        .or_else(|| object.get("message"))
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let kind = object
        .get("kind")
        .or_else(|| object.get("type"))
        .or_else(|| object.get("media_type"))
        .and_then(Value::as_str)
        .map(parse_media_kind)
        .transpose()?
        .unwrap_or(LarkMediaKind::Auto);
    Ok(LarkMediaAttachment {
        path: path.to_string(),
        caption,
        kind,
    })
}

fn parse_media_kind(kind: &str) -> Result<LarkMediaKind> {
    match kind.trim().to_lowercase().as_str() {
        "" | "auto" => Ok(LarkMediaKind::Auto),
        "photo" | "image" => Ok(LarkMediaKind::Image),
        "document" | "doc" | "file" => Ok(LarkMediaKind::File),
        "audio" | "voice" => Ok(LarkMediaKind::Audio),
        "video" | "media" => Ok(LarkMediaKind::Video),
        other => bail!("unsupported Lark media kind `{other}`"),
    }
}

fn resolve_media_content(
    client: &LarkClient,
    attachment: &LarkMediaAttachment,
) -> Result<(String, String)> {
    let source = attachment.path.trim();
    if source.starts_with("http://") || source.starts_with("https://") {
        bail!("Lark media upload requires a local file path or existing img_/file_ key; got URL `{source}`");
    }
    if source.starts_with("img_") {
        return Ok((
            "image".to_string(),
            serde_json::json!({"image_key": source}).to_string(),
        ));
    }
    if source.starts_with("file_") {
        return Ok((
            lark_msg_type_for_file_kind(attachment.kind).to_string(),
            serde_json::json!({"file_key": source}).to_string(),
        ));
    }
    let path = PathBuf::from(source);
    if attachment.kind == LarkMediaKind::Image
        || (attachment.kind == LarkMediaKind::Auto && is_image_path(source))
    {
        let image_key = client.upload_image(&path)?;
        return Ok((
            "image".to_string(),
            serde_json::json!({"image_key": image_key}).to_string(),
        ));
    }
    let upload_kind = infer_lark_media_kind(source, attachment.kind);
    if upload_kind == LarkMediaKind::Audio && !is_audio_path(source) {
        bail!("Lark audio messages require a local .opus or .ogg file; send other audio formats as kind=file after conversion or as a generic file");
    }
    let file_key = client.upload_file(&path, upload_kind)?;
    Ok((
        lark_msg_type_for_file_kind(upload_kind).to_string(),
        serde_json::json!({"file_key": file_key}).to_string(),
    ))
}

fn infer_lark_media_kind(path: &str, kind: LarkMediaKind) -> LarkMediaKind {
    if kind != LarkMediaKind::Auto {
        return kind;
    }
    if is_audio_path(path) {
        return LarkMediaKind::Audio;
    }
    if is_video_path(path) {
        return LarkMediaKind::Video;
    }
    LarkMediaKind::Auto
}

fn lark_msg_type_for_file_kind(kind: LarkMediaKind) -> &'static str {
    match kind {
        LarkMediaKind::Audio => "audio",
        LarkMediaKind::Image => "image",
        LarkMediaKind::Video => "media",
        LarkMediaKind::Auto | LarkMediaKind::File => "file",
    }
}

fn is_image_path(path: &str) -> bool {
    let lower = path.to_lowercase();
    [".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp"]
        .iter()
        .any(|suffix| lower.ends_with(suffix))
}

fn is_audio_path(path: &str) -> bool {
    let lower = path.to_lowercase();
    [".opus", ".ogg"]
        .iter()
        .any(|suffix| lower.ends_with(suffix))
}

fn is_video_path(path: &str) -> bool {
    let lower = path.to_lowercase();
    [".mp4", ".mov", ".avi", ".mkv", ".webm"]
        .iter()
        .any(|suffix| lower.ends_with(suffix))
}

fn response_message_id(response: &Value) -> String {
    response
        .get("message_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn normalize_lark_emoji_type(emoji: &str) -> String {
    match emoji.trim().trim_matches(':').to_lowercase().as_str() {
        "+1" | "thumbsup" | "thumbs_up" | "thumbs-up" => "THUMBSUP".to_string(),
        "smile" => "SMILE".to_string(),
        "ok" | "white_check_mark" | "check" => "OK".to_string(),
        other
            if other
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_') =>
        {
            other.to_ascii_uppercase()
        }
        _ => emoji.trim().to_string(),
    }
}

fn string_from_keys(input: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .filter_map(|key| input.get(*key))
        .find_map(|value| {
            value
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
        })
}

fn lark_reply_object_string(input: &Value, key: &str) -> Option<String> {
    input.get("reply_to").and_then(|value| {
        value
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
    })
}

#[cfg(test)]
mod tests {
    use super::{
        infer_lark_media_kind, is_lark_action, is_lark_connector, is_lark_credential_connector,
        lark_explicit_content, lark_msg_type_for_file_kind, lark_target, normalize_lark_emoji_type,
        parse_lark_reply_to, parse_media_attachments,
    };
    use puffer_lark::LarkMediaKind;
    use serde_json::json;

    #[test]
    fn lark_matchers_cover_builtin_slugs_and_actions() {
        assert!(is_lark_connector("lark-app"));
        assert!(is_lark_connector("lark-login"));
        assert!(is_lark_credential_connector("lark-app"));
        assert!(is_lark_credential_connector("lark-login"));
        assert!(is_lark_action("send_message"));
        assert!(is_lark_action("remove_reaction"));
        assert!(!is_lark_action("vote_poll"));
    }

    #[test]
    fn target_infers_lark_id_types() {
        assert_eq!(
            lark_target(&json!({"to": "oc_123"}))
                .unwrap()
                .receive_id_type,
            "chat_id"
        );
        assert_eq!(
            lark_target(&json!({"to": "ou_123"}))
                .unwrap()
                .receive_id_type,
            "open_id"
        );
        assert!(lark_target(&json!({"to": "Tony"})).is_err());
    }

    #[test]
    fn reply_to_accepts_string_and_object() {
        assert_eq!(
            parse_lark_reply_to(&json!({"reply_to": {"message_id": "om_1"}})).unwrap(),
            Some("om_1".to_string())
        );
        assert_eq!(
            parse_lark_reply_to(&json!({"reply_to": "om_2"})).unwrap(),
            Some("om_2".to_string())
        );
    }

    #[test]
    fn media_attachments_accept_path_and_kind() {
        let media = parse_media_attachments(&json!({
            "media": [{"path": "/tmp/report.pdf", "kind": "file", "caption": "report"}]
        }))
        .unwrap();

        assert_eq!(media.len(), 1);
        assert_eq!(media[0].path, "/tmp/report.pdf");
        assert_eq!(media[0].kind, LarkMediaKind::File);
        assert_eq!(media[0].caption.as_deref(), Some("report"));
    }

    #[test]
    fn media_attachments_preserve_video_kind() {
        let media = parse_media_attachments(&json!({
            "media": [{"path": "/tmp/clip.mp4", "kind": "video"}]
        }))
        .unwrap();

        assert_eq!(media[0].kind, LarkMediaKind::Video);
        assert_eq!(lark_msg_type_for_file_kind(media[0].kind), "media");
    }

    #[test]
    fn auto_media_kind_infers_audio_and_video_paths() {
        assert_eq!(
            infer_lark_media_kind("/tmp/voice.ogg", LarkMediaKind::Auto),
            LarkMediaKind::Audio
        );
        assert_eq!(
            infer_lark_media_kind("/tmp/clip.mp4", LarkMediaKind::Auto),
            LarkMediaKind::Video
        );
        assert_eq!(
            infer_lark_media_kind("/tmp/report.pdf", LarkMediaKind::Auto),
            LarkMediaKind::Auto
        );
    }

    #[test]
    fn explicit_text_content_wraps_plain_text() {
        let (_, content) = lark_explicit_content(&json!({"content": "hello"}))
            .unwrap()
            .unwrap();

        assert_eq!(content, json!({"text": "hello"}).to_string());
    }

    #[test]
    fn emoji_aliases_are_normalized() {
        assert_eq!(normalize_lark_emoji_type(":thumbsup:"), "THUMBSUP");
        assert_eq!(normalize_lark_emoji_type("smile"), "SMILE");
    }
}

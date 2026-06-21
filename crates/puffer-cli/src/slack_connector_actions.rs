//! Slack direct connector-action helpers.

use anyhow::{bail, Result};
use puffer_config::ConfigPaths;
use puffer_subscriptions::{ConnectionAuthChecker, ConnectionAuthStatus, ConnectorTemplate};
use serde_json::Value;
use std::path::PathBuf;

pub(crate) struct SlackConnectionAuthChecker {
    pub(crate) paths: ConfigPaths,
}

impl ConnectionAuthChecker for SlackConnectionAuthChecker {
    fn check(
        &self,
        _manager: &puffer_subscriptions::SubscriptionManager,
        template: &ConnectorTemplate,
        connection_slug: &str,
    ) -> Result<Option<ConnectionAuthStatus>> {
        if !is_slack_credential_connector(&template.slug) {
            return Ok(None);
        }
        let path = puffer_slack::credential_path(&self.paths.user_config_dir, connection_slug);
        if !path.exists() {
            return Ok(Some(ConnectionAuthStatus::Broken));
        }
        let credential = puffer_slack::load_credential(&path)?;
        let client = puffer_slack::SlackClient::new(credential)?;
        Ok(Some(if client.is_auth_ok()? {
            ConnectionAuthStatus::Healthy
        } else {
            ConnectionAuthStatus::Broken
        }))
    }
}

pub(crate) fn run_slack_action(
    paths: &ConfigPaths,
    connection_slug: &str,
    action: &str,
    input: &Value,
) -> Result<String> {
    let path = puffer_slack::credential_path(&paths.user_config_dir, connection_slug);
    let credential = puffer_slack::load_credential(&path)?;
    let client = puffer_slack::SlackClient::new(credential)?;
    match action {
        "send_message" => slack_send_message(&client, connection_slug, input),
        "react" | "send_reaction" | "remove_reaction" => {
            slack_react(&client, connection_slug, action, input)
        }
        _ => bail!("unsupported Slack action `{action}`"),
    }
}

pub(crate) fn is_slack_connector(connector_slug: &str) -> bool {
    matches!(connector_slug, "slack" | "slack-app" | "slack-login")
}

fn is_slack_credential_connector(connector_slug: &str) -> bool {
    matches!(connector_slug, "slack-app" | "slack-login")
}

pub(crate) fn is_slack_action(action: &str) -> bool {
    matches!(
        action,
        "send_message" | "react" | "send_reaction" | "remove_reaction"
    )
}

fn slack_send_message(
    client: &puffer_slack::SlackClient,
    connection_slug: &str,
    input: &Value,
) -> Result<String> {
    let target = string_from_keys(input, &["to", "target", "channel", "user"])
        .ok_or_else(|| anyhow::anyhow!("Slack send_message requires `to` or `channel`"))?;
    let text = string_from_keys(input, &["message", "text", "caption"]).unwrap_or_default();
    let media = parse_media_attachments(input)?;
    if text.trim().is_empty() && media.is_empty() {
        bail!("Slack send_message requires `message`, `caption`, or `media`");
    }
    let thread_ts = parse_slack_thread_ts(input)?;
    let channel = slack_channel_for_target(client, &target)?;
    if media.is_empty() {
        let response = client.post_message(&channel, &text, thread_ts.as_deref())?;
        let ts = response
            .get("ts")
            .and_then(Value::as_str)
            .unwrap_or("unknown-ts");
        return Ok(format!(
            "sent via {connection_slug} -> slack:{channel} ({ts})"
        ));
    }
    let mut uploaded = 0usize;
    for (index, attachment) in media.into_iter().enumerate() {
        if attachment.path.starts_with("http://") || attachment.path.starts_with("https://") {
            bail!(
                "Slack file upload requires a local file path; got URL `{}`",
                attachment.path
            );
        }
        let path = PathBuf::from(&attachment.path);
        let caption = attachment
            .caption
            .as_deref()
            .or_else(|| (index == 0 && !text.trim().is_empty()).then_some(text.as_str()));
        client.upload_file(&channel, &path, caption, thread_ts.as_deref())?;
        uploaded += 1;
    }
    Ok(format!(
        "uploaded {uploaded} file(s) via {connection_slug} -> slack:{channel}"
    ))
}

fn slack_react(
    client: &puffer_slack::SlackClient,
    connection_slug: &str,
    action: &str,
    input: &Value,
) -> Result<String> {
    let channel = string_from_keys(input, &["channel", "to", "target"])
        .ok_or_else(|| anyhow::anyhow!("Slack reaction requires `channel`"))?;
    let timestamp = string_from_keys(input, &["timestamp", "ts", "message_ts", "message_id"])
        .or_else(|| slack_reply_object_string(input, "ts"))
        .ok_or_else(|| anyhow::anyhow!("Slack reaction requires message `ts`"))?;
    let emoji = string_from_keys(input, &["emoji", "reaction"])
        .ok_or_else(|| anyhow::anyhow!("Slack reaction requires `emoji` or `reaction`"))?;
    let remove = action == "remove_reaction"
        || input
            .get("remove")
            .and_then(Value::as_bool)
            .unwrap_or(false);
    if remove {
        client.remove_reaction(&channel, &timestamp, &emoji)?;
        return Ok(format!(
            "removed Slack reaction `{}` via {connection_slug} -> {channel}:{timestamp}",
            emoji.trim().trim_matches(':')
        ));
    }
    client.add_reaction(&channel, &timestamp, &emoji)?;
    Ok(format!(
        "reacted `{}` via {connection_slug} -> {channel}:{timestamp}",
        emoji.trim().trim_matches(':')
    ))
}

fn slack_channel_for_target(client: &puffer_slack::SlackClient, target: &str) -> Result<String> {
    let target = target.trim();
    if target.is_empty() {
        bail!("Slack target is empty");
    }
    if target.starts_with('U') || target.starts_with('W') {
        return client.open_conversation(target);
    }
    if target.starts_with('@') || target.starts_with('#') {
        bail!(
            "Slack target `{target}` is ambiguous; resolve it first with slack search-users or slack search-conversations"
        );
    }
    Ok(target.to_string())
}

fn parse_slack_thread_ts(input: &Value) -> Result<Option<String>> {
    if let Some(value) = string_from_keys(
        input,
        &[
            "thread_ts",
            "reply_to_message_id",
            "message_ts",
            "ts",
            "timestamp",
        ],
    ) {
        return Ok(Some(value));
    }
    if let Some(value) = input.get("reply_to") {
        if value.is_null() {
            return Ok(None);
        }
        if let Some(ts) = value.as_str().map(str::trim).filter(|ts| !ts.is_empty()) {
            return Ok(Some(ts.to_string()));
        }
        if let Some(ts) = value
            .get("thread_ts")
            .or_else(|| value.get("ts"))
            .or_else(|| value.get("message_ts"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|ts| !ts.is_empty())
        {
            return Ok(Some(ts.to_string()));
        }
        bail!("Slack reply_to must be a timestamp string or object with ts/thread_ts");
    }
    Ok(None)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SlackMediaAttachment {
    path: String,
    caption: Option<String>,
}

fn parse_media_attachments(input: &Value) -> Result<Vec<SlackMediaAttachment>> {
    let mut media = Vec::new();
    for key in ["media", "attachments", "files"] {
        if let Some(value) = input.get(key) {
            parse_media_value(value, &mut media)?;
        }
    }
    for key in ["file", "path"] {
        if let Some(value) = input.get(key) {
            parse_media_value(value, &mut media)?;
        }
    }
    Ok(media)
}

fn parse_media_value(value: &Value, media: &mut Vec<SlackMediaAttachment>) -> Result<()> {
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

fn parse_media_attachment(value: &Value) -> Result<SlackMediaAttachment> {
    if let Some(path) = value.as_str() {
        return Ok(SlackMediaAttachment {
            path: path.to_string(),
            caption: None,
        });
    }
    let Some(object) = value.as_object() else {
        bail!("Slack media attachment must be a string path or object");
    };
    let path = object
        .get("path")
        .or_else(|| object.get("file"))
        .or_else(|| object.get("source"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if path.trim().is_empty() {
        bail!("Slack media attachment object requires `path` or `file`");
    }
    let caption = object
        .get("caption")
        .or_else(|| object.get("message"))
        .and_then(Value::as_str)
        .map(ToString::to_string);
    Ok(SlackMediaAttachment {
        path: path.to_string(),
        caption,
    })
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

fn slack_reply_object_string(input: &Value, key: &str) -> Option<String> {
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
        is_slack_action, is_slack_connector, is_slack_credential_connector, parse_slack_thread_ts,
    };
    use serde_json::json;

    #[test]
    fn slack_matchers_cover_builtin_slugs_and_actions() {
        assert!(is_slack_connector("slack-app"));
        assert!(is_slack_connector("slack-login"));
        assert!(!is_slack_connector("slack-bot"));
        assert!(is_slack_credential_connector("slack-app"));
        assert!(is_slack_credential_connector("slack-login"));
        assert!(!is_slack_credential_connector("slack-bot"));
        assert!(is_slack_action("send_message"));
        assert!(is_slack_action("remove_reaction"));
        assert!(!is_slack_action("vote_poll"));
    }

    #[test]
    fn thread_ts_accepts_reply_object() {
        let input = json!({"reply_to": {"thread_ts": "1700000000.000100"}});

        assert_eq!(
            parse_slack_thread_ts(&input).unwrap(),
            Some("1700000000.000100".to_string())
        );
    }
}

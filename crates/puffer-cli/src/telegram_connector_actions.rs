//! Telegram subscriber-backed connector-action helpers.

use anyhow::Result;
use puffer_config::ConfigPaths;
use puffer_subscriber_runtime::{EventEnvelope, Manifest, StateSpec, SubscriberCommand};
use puffer_subscriptions::{
    find_subscriber_manifest, ConnectionAuthChecker, ConnectorTemplate, SubscriberManifestRoots,
    SubscriptionManager,
};
use std::time::Duration;

const TELEGRAM_SUBSCRIBER_TOPIC: &str = "telegram-user";
const TELEGRAM_AUTH_TIMEOUT: Duration = Duration::from_secs(15);
const TELEGRAM_ACTION_TIMEOUT: Duration = Duration::from_secs(60);

pub(crate) struct TelegramConnectionAuthChecker;

impl ConnectionAuthChecker for TelegramConnectionAuthChecker {
    fn check(
        &self,
        manager: &SubscriptionManager,
        template: &ConnectorTemplate,
        connection_slug: &str,
    ) -> Result<Option<bool>> {
        if !is_telegram_connector(&template.slug) {
            return Ok(None);
        }
        if !manager
            .subscriber_ids()
            .iter()
            .any(|subscriber_id| subscriber_id == connection_slug)
        {
            return Ok(None);
        }
        let command = SubscriberCommand::TelegramAuthOk;
        let envelope = match manager.send_command_and_wait(
            connection_slug,
            connection_slug,
            &command,
            &["auth_ok", "login_error"],
            TELEGRAM_AUTH_TIMEOUT,
        ) {
            Ok(envelope) => envelope,
            Err(_) => {
                // A subscriber restart or command timeout says the probe was
                // unavailable, not that Telegram rejected the saved session.
                return Ok(None);
            }
        };
        Ok(Some(
            envelope.event.kind == "auth_ok"
                && envelope
                    .event
                    .payload
                    .get("ok")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
        ))
    }
}

pub(crate) fn telegram_action_via_subscriber(
    manager: &SubscriptionManager,
    paths: &ConfigPaths,
    connector_slug: &str,
    connection_slug: &str,
    action: &str,
    input: &serde_json::Value,
) -> Result<String> {
    let subscriber_id =
        telegram_subscriber_for_action(manager, paths, connector_slug, connection_slug)?
            .ok_or_else(|| {
                anyhow::anyhow!("no subscriber is configured for connector `{connector_slug}`")
            })?;
    let command = SubscriberCommand::Custom {
        op: "telegram_act".to_string(),
        args: serde_json::json!({
            "action": action,
            "input": input,
        }),
    };
    let event = manager.send_command_and_wait(
        &subscriber_id,
        &subscriber_id,
        &command,
        &["telegram_act_complete", "telegram_act_error", "login_error"],
        TELEGRAM_ACTION_TIMEOUT,
    )?;
    telegram_action_event_summary(&event, &subscriber_id, connector_slug, action)
}

pub(crate) fn telegram_subscriber_for_action(
    manager: &SubscriptionManager,
    paths: &ConfigPaths,
    connector_slug: &str,
    connection_slug: &str,
) -> Result<Option<String>> {
    if !is_telegram_connector(connector_slug) {
        return Ok(None);
    }
    let subscriber_id = telegram_subscriber_id(connection_slug);
    ensure_telegram_subscriber_running(manager, paths, &subscriber_id)?;
    Ok(Some(subscriber_id))
}

pub(crate) fn telegram_subscriber_for_platform(platform: &str) -> Option<&'static str> {
    is_telegram_connector(platform).then_some(TELEGRAM_SUBSCRIBER_TOPIC)
}

pub(crate) fn ensure_telegram_subscriber_running(
    manager: &SubscriptionManager,
    paths: &ConfigPaths,
    connection_slug: &str,
) -> Result<()> {
    validate_telegram_connection_slug(connection_slug)?;
    if manager
        .subscriber_ids()
        .iter()
        .any(|subscriber_id| subscriber_id == connection_slug)
    {
        return Ok(());
    }
    let manifest = telegram_connection_manifest(paths, connection_slug)?;
    manager.start_subscriber(manifest)?;
    Ok(())
}

pub(crate) fn telegram_subscriber_id(connection_slug: &str) -> String {
    if connection_slug.trim().is_empty() || connection_slug == "telegram-login" {
        return TELEGRAM_SUBSCRIBER_TOPIC.to_string();
    }
    connection_slug.to_string()
}

pub(crate) fn validate_telegram_connection_slug(connection_slug: &str) -> Result<()> {
    if connection_slug.is_empty()
        || !connection_slug
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        anyhow::bail!("Telegram connection slug must be non-empty kebab-case ASCII");
    }
    Ok(())
}

pub(crate) fn is_telegram_connector(connector_slug: &str) -> bool {
    matches!(
        connector_slug,
        "telegram" | "telegram-user" | "telegram-login"
    )
}

pub(crate) fn is_telegram_action(action: &str) -> bool {
    matches!(
        action,
        "vote_poll"
            | "edit_message"
            | "delete_message"
            | "delete_messages"
            | "forward_message"
            | "forward_messages"
            | "pin_message"
            | "unpin_message"
            | "unpin_all_messages"
            | "react"
            | "send_reaction"
            | "mark_read"
            | "clear_mentions"
            | "send_typing"
            | "send_chat_action"
            | "join_chat"
            | "leave_chat"
            | "kick_participant"
            | "ban_participant"
            | "unban_participant"
            | "invite_users"
            | "add_chat_users"
            | "update_profile"
            | "update_username"
            | "update_avatar"
            | "upload_avatar"
            | "update_group_title"
            | "update_group_name"
            | "update_group_username"
            | "update_group_photo"
            | "send_story"
    )
}

fn telegram_connection_manifest(paths: &ConfigPaths, connection_slug: &str) -> Result<Manifest> {
    let roots = subscriber_manifest_roots(paths);
    let dir = find_subscriber_manifest(&roots, TELEGRAM_SUBSCRIBER_TOPIC)
        .ok_or_else(|| anyhow::anyhow!("telegram-user subscriber manifest not found"))?;
    let mut manifest = Manifest::load(&dir)?;
    manifest.spec.id = connection_slug.to_string();
    manifest.spec.topic = Some(connection_slug.to_string());
    manifest.spec.display_name = Some(format!("Telegram ({connection_slug})"));
    manifest.spec.state = Some(StateSpec {
        dir: paths
            .user_config_dir
            .join("telegram-accounts")
            .join(connection_slug)
            .to_string_lossy()
            .to_string(),
    });
    Ok(manifest)
}

fn telegram_action_event_summary(
    event: &EventEnvelope,
    subscriber_id: &str,
    connector_slug: &str,
    action: &str,
) -> Result<String> {
    match event.event.kind.as_str() {
        "telegram_act_complete" => {
            let summary = event
                .event
                .payload
                .get("summary")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("completed");
            Ok(format!(
                "Telegram action `{action}` via {subscriber_id} -> {connector_slug}: {summary}"
            ))
        }
        "telegram_act_error" | "login_error" => anyhow::bail!(
            "Telegram action `{action}` via {subscriber_id} failed: {}",
            event_error(event)
        ),
        other => {
            anyhow::bail!("Telegram action `{action}` returned unexpected event `{other}`")
        }
    }
}

fn event_error(event: &EventEnvelope) -> String {
    event
        .event
        .payload
        .get("error")
        .and_then(serde_json::Value::as_str)
        .filter(|message| !message.trim().is_empty())
        .unwrap_or_else(|| {
            if event.event.text.trim().is_empty() {
                "unknown error"
            } else {
                event.event.text.as_str()
            }
        })
        .to_string()
}

fn subscriber_manifest_roots(paths: &ConfigPaths) -> SubscriberManifestRoots {
    SubscriberManifestRoots::new(
        paths.workspace_config_dir.clone(),
        paths.user_config_dir.clone(),
        paths.builtin_resources_dir.clone(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_connection_manifest_uses_account_state_dir() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ConfigPaths {
            workspace_root: temp.path().join("workspace"),
            workspace_config_dir: temp.path().join("workspace/.puffer"),
            user_config_dir: temp.path().join("home/.puffer"),
            builtin_resources_dir: temp.path().join("resources"),
        };
        let manifest_dir = paths
            .builtin_resources_dir
            .join("subscribers/telegram-user");
        std::fs::create_dir_all(&manifest_dir).unwrap();
        std::fs::write(
            manifest_dir.join("manifest.toml"),
            "manifest_version = 1\nid = \"telegram-user\"\nkind = \"subscriber\"\ntopic = \"telegram-user\"\n[run]\ncmd = [\"puffer\", \"__subscriber\", \"telegram-user\"]\n[state]\ndir = \"state\"\n",
        )
        .unwrap();

        let manifest = telegram_connection_manifest(&paths, "telegram-user").unwrap();

        assert_eq!(manifest.spec.id, "telegram-user");
        assert_eq!(manifest.topic(), "telegram-user");
        assert_eq!(
            manifest.spec.state.unwrap().dir,
            paths
                .user_config_dir
                .join("telegram-accounts/telegram-user")
                .to_string_lossy()
        );
    }
}

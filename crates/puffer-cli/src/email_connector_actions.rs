//! Email and Gmail-browser subscriber-backed connector-action helpers.

use anyhow::Result;
use puffer_config::ConfigPaths;
use puffer_subscriber_runtime::{EventEnvelope, Manifest, StateSpec, SubscriberCommand};
use puffer_subscriptions::{
    connection_subscriber_manifest, direct_subscriber_manifest, ConnectionRecord,
    SubscriberManifestRoots, SubscriptionManager,
};
use serde_json::Value;
use std::time::Duration;

const EMAIL_SUBSCRIBER_TOPIC: &str = "email";
const EMAIL_ACTION_TIMEOUT: Duration = Duration::from_secs(60);

/// Sends an email-style connector action to the subscriber that owns it.
pub(crate) fn email_action_via_subscriber(
    manager: &SubscriptionManager,
    paths: &ConfigPaths,
    connector_slug: &str,
    connection_slug: &str,
    action: &str,
    input: &Value,
) -> Result<String> {
    let subscriber_id =
        email_subscriber_for_action(manager, paths, connector_slug, connection_slug)?.ok_or_else(
            || anyhow::anyhow!("no subscriber is configured for connector `{connector_slug}`"),
        )?;
    let op = if is_gmail_browser_connector(connector_slug) {
        "gmail_browser_act"
    } else {
        "email_act"
    };
    let command = SubscriberCommand::Custom {
        op: op.to_string(),
        args: serde_json::json!({
            "action": action,
            "input": input,
        }),
    };
    let terminal_events = if is_gmail_browser_connector(connector_slug) {
        [
            "gmail_browser_action_complete",
            "gmail_browser_action_error",
            "command_ignored",
        ]
    } else {
        [
            "email_action_complete",
            "email_action_error",
            "command_ignored",
        ]
    };
    let event = manager.send_command_and_wait(
        &subscriber_id,
        &subscriber_id,
        &command,
        &terminal_events,
        EMAIL_ACTION_TIMEOUT,
    )?;
    email_action_event_summary(&event, &subscriber_id, connector_slug, action)
}

/// Resolves and starts the subscriber used for one email connector action.
pub(crate) fn email_subscriber_for_action(
    manager: &SubscriptionManager,
    paths: &ConfigPaths,
    connector_slug: &str,
    connection_slug: &str,
) -> Result<Option<String>> {
    if connector_slug == "email" {
        ensure_direct_email_subscriber_running(manager, paths)?;
        return Ok(Some(EMAIL_SUBSCRIBER_TOPIC.to_string()));
    }
    if !is_gmail_browser_connector(connector_slug) {
        return Ok(None);
    }
    let subscriber_id = gmail_browser_subscriber_id(connection_slug);
    ensure_gmail_browser_subscriber_running(manager, paths, connector_slug, &subscriber_id)?;
    Ok(Some(subscriber_id))
}

/// Returns the legacy direct subscriber id for platform-based email sends.
pub(crate) fn email_subscriber_for_platform(platform: &str) -> Option<&'static str> {
    (platform == "email").then_some(EMAIL_SUBSCRIBER_TOPIC)
}

/// Returns whether `connector_slug` is handled by the email action router.
pub(crate) fn is_email_connector(connector_slug: &str) -> bool {
    connector_slug == "email" || is_gmail_browser_connector(connector_slug)
}

/// Returns whether `action` belongs to the email action surface.
pub(crate) fn is_email_action(action: &str) -> bool {
    matches!(
        action,
        "list_emails"
            | "list_inbox"
            | "list_category"
            | "search_emails"
            | "mark_read"
            | "draft_reply"
            | "draft_forward"
            | "send_email"
            | "delete"
    )
}

fn ensure_direct_email_subscriber_running(
    manager: &SubscriptionManager,
    paths: &ConfigPaths,
) -> Result<()> {
    if manager
        .subscriber_ids()
        .iter()
        .any(|subscriber_id| subscriber_id == EMAIL_SUBSCRIBER_TOPIC)
    {
        return Ok(());
    }
    let manifest =
        direct_subscriber_manifest(&subscriber_manifest_roots(paths), EMAIL_SUBSCRIBER_TOPIC)?
            .ok_or_else(|| anyhow::anyhow!("email subscriber manifest not found"))?;
    manager.start_subscriber(manifest)?;
    Ok(())
}

fn ensure_gmail_browser_subscriber_running(
    manager: &SubscriptionManager,
    paths: &ConfigPaths,
    connector_slug: &str,
    subscriber_id: &str,
) -> Result<()> {
    if manager
        .subscriber_ids()
        .iter()
        .any(|id| id == subscriber_id)
    {
        return Ok(());
    }
    let template = manager
        .connector_store()
        .get(connector_slug)
        .ok_or_else(|| anyhow::anyhow!("connector `{connector_slug}` not found"))?;
    let connection = manager
        .connection_store()
        .get(subscriber_id)
        .unwrap_or_else(|| {
            ConnectionRecord::authenticated(subscriber_id, connector_slug, subscriber_id)
        });
    if let Some(manifest) =
        connection_subscriber_manifest(&subscriber_manifest_roots(paths), &connection, &template)?
    {
        manager.start_subscriber(manifest)?;
        return Ok(());
    }
    let manifest = gmail_browser_fallback_manifest(paths, subscriber_id)?;
    manager.start_subscriber(manifest)?;
    Ok(())
}

fn gmail_browser_fallback_manifest(paths: &ConfigPaths, subscriber_id: &str) -> Result<Manifest> {
    let mut manifest =
        direct_subscriber_manifest(&subscriber_manifest_roots(paths), "gmail-browser")?
            .ok_or_else(|| anyhow::anyhow!("gmail-browser subscriber manifest not found"))?;
    manifest.spec.id = subscriber_id.to_string();
    manifest.spec.topic = Some(subscriber_id.to_string());
    manifest.spec.display_name = Some(format!("Gmail Browser ({subscriber_id})"));
    manifest.spec.state = Some(StateSpec {
        dir: paths
            .user_config_dir
            .join(crate::gmail_browser::STATE_ROOT)
            .join(subscriber_id)
            .to_string_lossy()
            .to_string(),
    });
    Ok(manifest)
}

fn gmail_browser_subscriber_id(connection_slug: &str) -> String {
    if connection_slug.trim().is_empty() {
        crate::gmail_browser::DEFAULT_CONNECTION.to_string()
    } else {
        connection_slug.to_string()
    }
}

fn is_gmail_browser_connector(connector_slug: &str) -> bool {
    connector_slug == crate::gmail_browser::CONNECTOR_SLUG
}

fn email_action_event_summary(
    event: &EventEnvelope,
    subscriber_id: &str,
    connector_slug: &str,
    action: &str,
) -> Result<String> {
    match event.event.kind.as_str() {
        "email_action_complete" | "gmail_browser_action_complete" => {
            let summary = event
                .event
                .payload
                .get("summary")
                .and_then(Value::as_str)
                .unwrap_or("completed");
            let payload = serde_json::to_string_pretty(&event.event.payload)
                .unwrap_or_else(|_| "{}".to_string());
            Ok(format!(
                "Email action `{action}` via {subscriber_id} -> {connector_slug}: {summary}\n{payload}"
            ))
        }
        "email_action_error" | "gmail_browser_action_error" | "command_ignored" => {
            anyhow::bail!(
                "Email action `{action}` via {subscriber_id} failed: {}",
                event_error(event)
            )
        }
        other => anyhow::bail!("Email action `{action}` returned unexpected event `{other}`"),
    }
}

fn event_error(event: &EventEnvelope) -> String {
    event
        .event
        .payload
        .get("error")
        .and_then(Value::as_str)
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
    fn recognizes_email_action_surface() {
        assert!(is_email_action("list_emails"));
        assert!(is_email_action("send_email"));
        assert!(is_email_action("delete"));
        assert!(!is_email_action("vote_poll"));
    }

    #[test]
    fn recognizes_email_connector_surface() {
        assert!(is_email_connector("email"));
        assert!(is_email_connector("gmail-browser"));
        assert!(!is_email_connector("telegram-login"));
    }
}

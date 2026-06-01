//! Google Calendar browser subscriber-backed connector-action helpers.

use anyhow::Result;
use puffer_config::ConfigPaths;
use puffer_subscriber_runtime::{EventEnvelope, Manifest, StateSpec, SubscriberCommand};
use puffer_subscriptions::{
    connection_subscriber_manifest, direct_subscriber_manifest, ConnectionRecord,
    SubscriberManifestRoots, SubscriptionManager,
};
use serde_json::Value;
use std::time::Duration;

const GCAL_ACTION_TIMEOUT: Duration = Duration::from_secs(60);

/// Sends a Google Calendar connector action to the subscriber that owns it.
pub(crate) fn gcal_action_via_subscriber(
    manager: &SubscriptionManager,
    paths: &ConfigPaths,
    connector_slug: &str,
    connection_slug: &str,
    action: &str,
    input: &Value,
) -> Result<String> {
    let subscriber_id =
        gcal_subscriber_for_action(manager, paths, connector_slug, connection_slug)?.ok_or_else(
            || anyhow::anyhow!("no subscriber is configured for connector `{connector_slug}`"),
        )?;
    let command = SubscriberCommand::Custom {
        op: "gcal_browser_act".to_string(),
        args: serde_json::json!({
            "action": action,
            "input": input,
        }),
    };
    let terminal_events = [
        "gcal_browser_action_complete",
        "gcal_browser_action_error",
        "command_ignored",
    ];
    let event = manager.send_command_and_wait(
        &subscriber_id,
        &subscriber_id,
        &command,
        &terminal_events,
        GCAL_ACTION_TIMEOUT,
    )?;
    gcal_action_event_summary(&event, &subscriber_id, action)
}

/// Resolves and starts the subscriber used for one Google Calendar action.
pub(crate) fn gcal_subscriber_for_action(
    manager: &SubscriptionManager,
    paths: &ConfigPaths,
    connector_slug: &str,
    connection_slug: &str,
) -> Result<Option<String>> {
    if !is_gcal_connector(connector_slug) {
        return Ok(None);
    }
    let subscriber_id = gcal_browser_subscriber_id(connection_slug);
    ensure_gcal_browser_subscriber_running(manager, paths, connector_slug, &subscriber_id)?;
    Ok(Some(subscriber_id))
}

/// Returns whether `connector_slug` is handled by the Google Calendar action router.
pub(crate) fn is_gcal_connector(connector_slug: &str) -> bool {
    connector_slug == crate::gcal_browser::CONNECTOR_SLUG
}

/// Returns whether `action` belongs to the Google Calendar action surface.
pub(crate) fn is_gcal_action(action: &str) -> bool {
    matches!(
        action,
        "list_events"
            | "list"
            | "list_event"
            | "list_calendar_events"
            | "events"
            | "search"
            | "search_event"
            | "search_events"
            | "get_detail"
            | "get_details"
            | "get_event"
            | "event_detail"
            | "detail"
            | "accept"
            | "accept_event"
            | "yes"
            | "deny"
            | "decline"
            | "reject"
            | "no"
    )
}

fn ensure_gcal_browser_subscriber_running(
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
    let manifest = gcal_browser_fallback_manifest(paths, subscriber_id)?;
    manager.start_subscriber(manifest)?;
    Ok(())
}

fn gcal_browser_fallback_manifest(paths: &ConfigPaths, subscriber_id: &str) -> Result<Manifest> {
    let mut manifest =
        direct_subscriber_manifest(&subscriber_manifest_roots(paths), "gcal-browser")?
            .ok_or_else(|| anyhow::anyhow!("gcal-browser subscriber manifest not found"))?;
    manifest.spec.id = subscriber_id.to_string();
    manifest.spec.topic = Some(subscriber_id.to_string());
    manifest.spec.display_name = Some(format!("Google Calendar Browser ({subscriber_id})"));
    manifest.spec.state = Some(StateSpec {
        dir: paths
            .user_config_dir
            .join(crate::gcal_browser::STATE_ROOT)
            .join(subscriber_id)
            .to_string_lossy()
            .to_string(),
    });
    Ok(manifest)
}

fn gcal_browser_subscriber_id(connection_slug: &str) -> String {
    if connection_slug.trim().is_empty() {
        crate::gcal_browser::DEFAULT_CONNECTION.to_string()
    } else {
        connection_slug.to_string()
    }
}

fn gcal_action_event_summary(
    event: &EventEnvelope,
    subscriber_id: &str,
    action: &str,
) -> Result<String> {
    match event.event.kind.as_str() {
        "gcal_browser_action_complete" => {
            let summary = event
                .event
                .payload
                .get("summary")
                .and_then(Value::as_str)
                .unwrap_or("completed");
            let payload = serde_json::to_string_pretty(&event.event.payload)
                .unwrap_or_else(|_| "{}".to_string());
            Ok(format!(
                "Google Calendar action `{action}` via {subscriber_id}: {summary}\n{payload}"
            ))
        }
        "gcal_browser_action_error" | "command_ignored" => {
            anyhow::bail!(
                "Google Calendar action `{action}` via {subscriber_id} failed: {}",
                event_error(event)
            )
        }
        other => {
            anyhow::bail!("Google Calendar action `{action}` returned unexpected event `{other}`")
        }
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
    fn recognizes_gcal_action_surface() {
        assert!(is_gcal_action("list_events"));
        assert!(is_gcal_action("search_events"));
        assert!(is_gcal_action("get_detail"));
        assert!(is_gcal_action("accept"));
        assert!(is_gcal_action("deny"));
        assert!(is_gcal_action("decline"));
        assert!(!is_gcal_action("draft_reply"));
    }

    #[test]
    fn recognizes_gcal_connector_surface() {
        assert!(is_gcal_connector("gcal-browser"));
        assert!(!is_gcal_connector("gmail-browser"));
    }
}

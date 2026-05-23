//! `SubscriptionCreate` workflow tool — installs a new subscription spec.
//!
//! The agent calls this after translating a user's natural-language
//! request ("watch IoC on telegram, save to sqlite") into the structured
//! spec fields. The tool persists the spec via [`SubscriptionStore::create`]
//! and starts the named subscriber if it is not already running.

use crate::AppState;
use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use puffer_subscriptions::{
    connection_subscriber_manifest, direct_subscriber_manifest, ActionSpec, ConnectorTemplate,
    FilterSpec, SubscriberManifestRoots, SubscriptionSpec, SubscriptionStatus,
};
use serde::Deserialize;
use serde_json::Value;
use std::path::Path;
use time::OffsetDateTime;

use super::subscription_globals;

#[derive(Debug, Deserialize)]
struct CreateInput {
    #[serde(alias = "id")]
    slug: String,
    #[serde(default)]
    description: String,
    #[serde(alias = "source_topic")]
    connection_slug: String,
    #[serde(default)]
    connector_slug: Option<String>,
    #[serde(default)]
    #[serde(alias = "prefilter")]
    filter: Option<FilterSpec>,
    #[serde(default)]
    classify_prompt: Option<String>,
    #[serde(default)]
    classify_model: Option<String>,
    action: ActionSpec,
}

/// Executes `SubscriptionCreate`. Validates the spec, persists it, and
/// (best-effort) starts the matching subscriber if a manifest is on disk
/// at the conventional location.
pub fn execute_subscription_create(
    _state: &mut AppState,
    cwd: &Path,
    input: Value,
) -> Result<String> {
    let parsed: CreateInput =
        serde_json::from_value(input).context("invalid SubscriptionCreate input")?;
    let now_ms = OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000;
    let spec = SubscriptionSpec {
        slug: parsed.slug.clone(),
        description: parsed.description,
        connection_slug: parsed.connection_slug.clone(),
        connector_slug: parsed.connector_slug,
        status: SubscriptionStatus::Enabled,
        filter: parsed.filter,
        classify_prompt: parsed.classify_prompt,
        classify_model: parsed.classify_model,
        action: parsed.action,
        created_at_ms: now_ms,
    };
    let manager = subscription_globals::manager()?;
    manager
        .store()
        .create(spec.clone())
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    manager.refresh_connection_consumers()?;
    let _ = ensure_subscriber_started(&manager, cwd, &parsed.connection_slug);
    Ok(serde_json::to_string_pretty(&spec)?)
}

fn ensure_subscriber_started(
    manager: &puffer_subscriptions::SubscriptionManager,
    cwd: &Path,
    source_topic: &str,
) -> Result<()> {
    if manager.subscriber_ids().iter().any(|id| id == source_topic) {
        return Ok(());
    }
    let roots = subscriber_manifest_roots(cwd);
    if let Some(connection) = manager.connection_store().get(source_topic) {
        if let Some(template) = manager.connector_store().get(&connection.connector_slug) {
            if connector_stream_supported(&template) {
                return Ok(());
            }
            if let Some(manifest) = connection_subscriber_manifest(&roots, &connection, &template)?
            {
                if !manager
                    .subscriber_ids()
                    .iter()
                    .any(|id| id == &manifest.spec.id)
                {
                    manager.start_subscriber(manifest)?;
                }
                return Ok(());
            }
        }
    }
    let Some(manifest) = direct_subscriber_manifest(&roots, source_topic)? else {
        // No manifest installed for this topic; the agent created a spec
        // for a subscriber that has not been wired up. We surface this
        // softly (no events will arrive until a matching manifest exists).
        eprintln!(
            "subscription: no subscriber manifest found for topic `{source_topic}`; spec created but no events will fire"
        );
        return Ok(());
    };
    manager.start_subscriber(manifest)?;
    Ok(())
}

fn connector_stream_supported(template: &ConnectorTemplate) -> bool {
    template.can_subscribe && template.command_argv().is_some()
}

fn subscriber_manifest_roots(cwd: &Path) -> SubscriberManifestRoots {
    let paths = ConfigPaths::discover(cwd);
    SubscriberManifestRoots::new(
        paths.workspace_config_dir,
        paths.user_config_dir,
        paths.builtin_resources_dir,
    )
}

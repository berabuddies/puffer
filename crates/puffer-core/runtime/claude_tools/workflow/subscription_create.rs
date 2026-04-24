//! `SubscriptionCreate` workflow tool — installs a new subscription spec.
//!
//! The agent calls this after translating a user's natural-language
//! request ("watch IoC on telegram, save to sqlite") into the structured
//! spec fields. The tool persists the spec via [`SubscriptionStore::create`]
//! and starts the named subscriber if it is not already running.

use crate::AppState;
use anyhow::{Context, Result};
use puffer_subscriber_runtime::Manifest;
use puffer_subscriptions::{ActionSpec, PrefilterSpec, SubscriptionSpec, SubscriptionStatus};
use serde::Deserialize;
use serde_json::Value;
use std::path::{Path, PathBuf};
use time::OffsetDateTime;

use super::subscription_globals;

#[derive(Debug, Deserialize)]
struct CreateInput {
    id: String,
    #[serde(default)]
    description: String,
    source_topic: String,
    #[serde(default)]
    prefilter: Option<PrefilterSpec>,
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
    _cwd: &Path,
    input: Value,
) -> Result<String> {
    let parsed: CreateInput =
        serde_json::from_value(input).context("invalid SubscriptionCreate input")?;
    let now_ms = OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000;
    let spec = SubscriptionSpec {
        id: parsed.id.clone(),
        description: parsed.description,
        source_topic: parsed.source_topic.clone(),
        status: SubscriptionStatus::Enabled,
        prefilter: parsed.prefilter,
        classify_prompt: parsed.classify_prompt,
        classify_model: parsed.classify_model,
        action: parsed.action,
        created_at_ms: now_ms,
    };
    let manager = subscription_globals::manager()?;
    manager.store().create(spec.clone()).map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let _ = ensure_subscriber_started(&manager, &parsed.source_topic);
    Ok(serde_json::to_string_pretty(&spec)?)
}

fn ensure_subscriber_started(
    manager: &puffer_subscriptions::SubscriptionManager,
    source_topic: &str,
) -> Result<()> {
    if manager.subscriber_ids().iter().any(|id| id == source_topic) {
        return Ok(());
    }
    let manifest_dir = subscriber_manifest_dir(source_topic);
    if !manifest_dir.join("manifest.toml").exists() {
        // No manifest installed for this topic; the agent created a spec
        // for a subscriber that has not been wired up. We surface this
        // softly (no events will arrive until a matching manifest exists).
        eprintln!(
            "subscription: no subscriber manifest found for topic `{source_topic}`; spec created but no events will fire"
        );
        return Ok(());
    }
    let manifest = Manifest::load(&manifest_dir)?;
    manager.start_subscriber(manifest)?;
    Ok(())
}

fn subscriber_manifest_dir(source_topic: &str) -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        let user = PathBuf::from(home)
            .join(".puffer")
            .join("subscribers")
            .join(source_topic);
        if user.join("manifest.toml").exists() {
            return user;
        }
    }
    PathBuf::from("resources/subscribers").join(source_topic)
}

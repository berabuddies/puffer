//! Per-monitor completion phrases.
//!
//! The self-report judge ([`super::monitor_self_report`]) is handed a list of
//! "completion phrase" hints — words that often mean the user just finished a
//! task ("搞定了", "done", ...). A multilingual default set is built in; this
//! module adds an optional *per-connection* override so a user can teach one
//! monitor extra phrases ("快递拿了", "稿子发了") without affecting others.
//!
//! Phrases are only hints handed to the LLM, never a hard gate — so an empty or
//! missing override simply means "defaults only", and the feature degrades to
//! the built-in behaviour with zero configuration.
//!
//! Storage is a single small JSON map `{ connection_slug: [phrase, ...] }` kept
//! next to the monitor task store, so reading it in the judge's cheap gate adds
//! no new subsystem.

use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
struct CompletionPhraseParams {
    #[serde(alias = "connectionSlug")]
    connection_slug: String,
    /// One or more phrases. Accepts a single `phrase` or a `phrases` array.
    #[serde(default)]
    phrases: Vec<String>,
    #[serde(default)]
    phrase: Option<String>,
}

/// Adds one or more completion phrases to a monitor connection and returns a
/// refreshed editor snapshot.
pub(crate) fn handle_monitor_completion_phrase_add(
    paths: &ConfigPaths,
    params: &Value,
) -> Result<Value> {
    let params = parse_params(params)?;
    let connection = valid_connection_slug(&params.connection_slug)?;
    let additions = requested_phrases(&params);
    if additions.is_empty() {
        anyhow::bail!("completion phrase must not be empty");
    }
    let mut store = load_store(paths)?;
    let entry = store.entry(connection.to_string()).or_default();
    for phrase in additions {
        if !entry.iter().any(|existing| existing == &phrase) {
            entry.push(phrase);
        }
    }
    save_store(paths, &store)?;
    super::handle_workflow_list(paths)
}

/// Removes one or more completion phrases from a monitor connection and returns
/// a refreshed editor snapshot.
pub(crate) fn handle_monitor_completion_phrase_delete(
    paths: &ConfigPaths,
    params: &Value,
) -> Result<Value> {
    let params = parse_params(params)?;
    let connection = valid_connection_slug(&params.connection_slug)?;
    let removals: BTreeSet<String> = requested_phrases(&params).into_iter().collect();
    let mut store = load_store(paths)?;
    if let Some(entry) = store.get_mut(connection) {
        entry.retain(|phrase| !removals.contains(phrase));
        if entry.is_empty() {
            store.remove(connection);
        }
    }
    save_store(paths, &store)?;
    super::handle_workflow_list(paths)
}

/// Returns the configured completion phrases for a connection (empty when none).
/// Used by the self-report judge to merge with the built-in defaults.
pub(super) fn load_completion_phrases(paths: &ConfigPaths, connection: &str) -> Vec<String> {
    load_store(paths)
        .ok()
        .and_then(|store| store.get(connection).cloned())
        .unwrap_or_default()
}

fn parse_params(params: &Value) -> Result<CompletionPhraseParams> {
    serde_json::from_value(params.clone()).context("invalid completion phrase params")
}

fn requested_phrases(params: &CompletionPhraseParams) -> Vec<String> {
    let mut phrases = Vec::new();
    let mut seen = BTreeSet::new();
    for raw in params
        .phrase
        .iter()
        .cloned()
        .chain(params.phrases.iter().cloned())
    {
        let trimmed = raw.trim();
        if trimmed.is_empty() || !seen.insert(trimmed.to_string()) {
            continue;
        }
        phrases.push(trimmed.to_string());
    }
    phrases
}

type PhraseStore = std::collections::BTreeMap<String, Vec<String>>;

fn load_store(paths: &ConfigPaths) -> Result<PhraseStore> {
    let path = store_path(paths);
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(PhraseStore::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", path.display()));
        }
    };
    if raw.trim().is_empty() {
        return Ok(PhraseStore::new());
    }
    let value: Value = serde_json::from_str(&raw)
        .with_context(|| format!("invalid completion phrase store {}", path.display()))?;
    Ok(phrase_store_from_value(&value))
}

fn phrase_store_from_value(value: &Value) -> PhraseStore {
    let Some(object) = value.as_object() else {
        return PhraseStore::new();
    };
    object
        .iter()
        .map(|(connection, phrases)| (connection.clone(), phrase_list(phrases)))
        .filter(|(_, phrases)| !phrases.is_empty())
        .collect()
}

fn phrase_list(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|phrase| !phrase.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn save_store(paths: &ConfigPaths, store: &PhraseStore) -> Result<()> {
    let path = store_path(paths);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let mut object = Map::new();
    for (connection, phrases) in store {
        object.insert(
            connection.clone(),
            Value::Array(phrases.iter().cloned().map(Value::String).collect()),
        );
    }
    fs::write(&path, serde_json::to_string_pretty(&Value::Object(object))?)
        .with_context(|| format!("failed to write {}", path.display()))
}

fn store_path(paths: &ConfigPaths) -> PathBuf {
    paths
        .workspace_config_dir
        .join("runtime")
        .join("claude_workflow")
        .join("monitor_completion_phrases.json")
}

fn valid_connection_slug(slug: &str) -> Result<&str> {
    let trimmed = slug.trim();
    if trimmed.is_empty()
        || trimmed == "."
        || trimmed == ".."
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.contains('\0')
    {
        anyhow::bail!("invalid monitor completion phrase connection slug");
    }
    Ok(trimmed)
}

#[cfg(test)]
#[path = "monitor_completion_phrases_tests.rs"]
mod tests;

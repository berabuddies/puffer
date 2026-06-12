//! Desktop contact RPCs and local contact persistence.

use crate::daemon::DaemonState;
use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use puffer_subscriptions::{
    connector_slug_accepts_contact_id, contact_display_name_from_payload, contact_id_prefix,
    contact_ids_from_payload, normalize_contact_id, normalize_contact_ids, ConnectorContact,
    ContactContext, SavedContact,
};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

#[path = "daemon_contacts_infer.rs"]
mod daemon_contacts_infer;
#[path = "daemon_contacts_params.rs"]
mod daemon_contacts_params;
#[path = "daemon_contacts_store.rs"]
mod daemon_contacts_store;
#[path = "daemon_contacts_telegram.rs"]
mod daemon_contacts_telegram;
#[path = "daemon_contacts_trace.rs"]
mod daemon_contacts_trace;
use daemon_contacts_infer::{candidate_trace_sample, contact_infer_system_prompt, infer_proposals};
use daemon_contacts_params::{
    ContactContextParams, ContactDeleteParams, ContactInferParams, ContactListParams,
    ContactSaveParams,
};
use daemon_contacts_store::{
    load_proposals, load_store, prune_proposals_for_contact_ids, save_proposals, save_store,
};
use daemon_contacts_telegram::{
    collect_telegram_candidates, read_telegram_peer_avatars, read_telegram_peer_names,
    recent_telegram_contacts, refresh_telegram_peer_caches,
};
use daemon_contacts_trace::ContactInferTrace;

const DEFAULT_LIMIT: usize = 30;
const MAX_LIMIT: usize = 120;
const TELEGRAM_CONTEXT_LIMIT: usize = 100;
const MIN_CONTEXT_MESSAGES_PER_CANDIDATE: usize = 20;
const TELEGRAM_RECENT_CONTEXT_LIMIT: usize = MIN_CONTEXT_MESSAGES_PER_CANDIDATE;
const TELEGRAM_INTERACTION_CONTEXT_LIMIT: usize = 10;
const GOOGLE_CONTEXT_LIMIT: usize = 10;
const INFERENCE_CONTEXT_LIMIT: usize =
    TELEGRAM_RECENT_CONTEXT_LIMIT + (TELEGRAM_INTERACTION_CONTEXT_LIMIT * 2);
const HISTORY_CANDIDATE_LIMIT: usize = 1_000;
const DAY_MS: i128 = 86_400_000;

pub(crate) fn cached_telegram_peer_avatars(paths: &ConfigPaths) -> HashMap<String, String> {
    read_telegram_peer_avatars(paths)
}

/// Cached Telegram peer display names, keyed like the avatar map (contact-id
/// aliases). Used to surface sender names on monitor task source contexts.
pub(crate) fn cached_telegram_peer_names(paths: &ConfigPaths) -> HashMap<String, String> {
    read_telegram_peer_names(paths)
}

#[derive(Debug, Clone)]
struct Candidate {
    id: String,
    name: Option<String>,
    avatar: Option<String>,
    score: f64,
    last_message_at_ms: Option<i128>,
    context: Vec<ContactContext>,
}

#[derive(Debug, Clone, Copy)]
struct CandidateContextOptions {
    telegram_limit: usize,
    other_limit: usize,
    include_payload: bool,
    include_connector_commands: bool,
}

impl CandidateContextOptions {
    fn none() -> Self {
        Self {
            telegram_limit: 0,
            other_limit: 0,
            include_payload: false,
            include_connector_commands: false,
        }
    }

    fn full() -> Self {
        Self {
            telegram_limit: TELEGRAM_CONTEXT_LIMIT,
            other_limit: GOOGLE_CONTEXT_LIMIT,
            include_payload: true,
            include_connector_commands: true,
        }
    }

    fn inference() -> Self {
        Self {
            telegram_limit: INFERENCE_CONTEXT_LIMIT,
            other_limit: INFERENCE_CONTEXT_LIMIT,
            include_payload: false,
            include_connector_commands: false,
        }
    }

    fn limit_for_id(self, id: &str) -> usize {
        if id.starts_with("telegram@") || id.starts_with("telegram-user-id@") {
            self.telegram_limit
        } else {
            self.other_limit
        }
    }

    fn payload(self, payload: &Value) -> Value {
        if self.include_payload {
            payload.clone()
        } else {
            Value::Null
        }
    }

    fn uses_connector_commands(self) -> bool {
        self.include_connector_commands
    }
}

/// Lists saved contacts plus ranked connector candidates.
pub(crate) fn handle_contacts_list(paths: &ConfigPaths, params: &Value) -> Result<Value> {
    let params: ContactListParams =
        serde_json::from_value(params.clone()).context("invalid contact list params")?;
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let query = params
        .query
        .as_deref()
        .map(str::trim)
        .filter(|q| !q.is_empty());
    let store = load_store(paths)?;
    let proposals = load_proposals(paths)?;
    let mut saved = filtered_saved_contacts(store.contacts, query);
    enrich_saved_contact_avatars(paths, &mut saved);
    let (candidates, ready) = if is_recent_telegram_request(&params) {
        let mut snapshot = recent_telegram_contacts(paths, limit)?;
        reject_bot_candidates(&mut snapshot.candidates);
        (
            snapshot
                .candidates
                .into_iter()
                .map(Candidate::into_preview_contact)
                .collect(),
            snapshot.ready,
        )
    } else {
        (filtered_candidates(paths, limit, query)?, true)
    };
    Ok(json!({
        "contacts": saved,
        "candidates": candidates,
        "proposals": proposals,
        "ready": ready,
    }))
}

/// Searches connector contact ids for autocomplete.
pub(crate) fn handle_contacts_search(paths: &ConfigPaths, params: &Value) -> Result<Value> {
    let params: ContactListParams =
        serde_json::from_value(params.clone()).context("invalid contact search params")?;
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let query = params
        .query
        .as_deref()
        .map(str::trim)
        .filter(|q| !q.is_empty());
    let store = load_store(paths)?;
    let proposals = load_proposals(paths)?;
    let mut saved = filtered_saved_contacts(store.contacts, query);
    enrich_saved_contact_avatars(paths, &mut saved);
    let candidates = searched_candidates(paths, limit, query)?;
    Ok(json!({
        "contacts": saved,
        "candidates": candidates,
        "proposals": proposals,
    }))
}

/// Refreshes connector-backed contact caches and returns the current contact snapshot.
pub(crate) fn handle_contacts_refresh(paths: &ConfigPaths, params: &Value) -> Result<Value> {
    let parsed: ContactListParams =
        serde_json::from_value(params.clone()).context("invalid contact refresh params")?;
    refresh_telegram_peer_caches(paths)?;
    let has_query = parsed
        .query
        .as_deref()
        .map(str::trim)
        .is_some_and(|query| !query.is_empty());
    if has_query {
        handle_contacts_search(paths, params)
    } else {
        handle_contacts_list(paths, params)
    }
}

fn is_recent_telegram_request(params: &ContactListParams) -> bool {
    params
        .connector
        .as_deref()
        .is_some_and(|value| value.eq_ignore_ascii_case("telegram"))
        && params
            .sort
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case("recent"))
}

/// Saves a user-curated contact and returns the refreshed contact snapshot.
pub(crate) fn handle_contacts_save(paths: &ConfigPaths, params: &Value) -> Result<Value> {
    let params: ContactSaveParams =
        serde_json::from_value(params.clone()).context("invalid contact save params")?;
    let name = params.name.trim();
    if name.is_empty() {
        anyhow::bail!("contact name must not be empty");
    }
    let contact_ids = normalize_contact_ids(params.contact_ids);
    if contact_ids.is_empty() {
        anyhow::bail!("contact must contain at least one valid contact id");
    }
    let mut contact_ids = contact_ids;
    let mut store = load_store(paths)?;
    let id = params
        .id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    contact_ids = merged_overlapping_contact_ids(&store.contacts, &id, contact_ids);
    let saved_contact_ids = contact_ids.clone();
    store.contacts.retain(|contact| {
        contact.id != id && !contact_ids_overlap(&contact.contact_ids, &contact_ids)
    });
    let saved = SavedContact {
        id: id.clone(),
        name: name.to_string(),
        description: params.description.trim().to_string(),
        avatar: params
            .avatar
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        contact_ids,
    };
    store.contacts.push(saved);
    store.contacts.sort_by(|left, right| {
        left.name
            .to_ascii_lowercase()
            .cmp(&right.name.to_ascii_lowercase())
    });
    save_store(paths, &store)?;
    prune_proposals_for_contact_ids(paths, &saved_contact_ids)?;
    handle_contacts_list(paths, &json!({ "limit": DEFAULT_LIMIT }))
}

/// Deletes a user-curated contact and returns the refreshed contact snapshot.
pub(crate) fn handle_contacts_delete(paths: &ConfigPaths, params: &Value) -> Result<Value> {
    let params: ContactDeleteParams =
        serde_json::from_value(params.clone()).context("invalid contact delete params")?;
    let id = params.id.trim();
    if id.is_empty() {
        anyhow::bail!("contact id must not be empty");
    }
    let mut store = load_store(paths)?;
    let before = store.contacts.len();
    store.contacts.retain(|contact| contact.id != id);
    if store.contacts.len() == before {
        anyhow::bail!("contact `{}` not found", id);
    }
    save_store(paths, &store)?;
    handle_contacts_list(paths, &json!({ "limit": DEFAULT_LIMIT }))
}

/// Returns recent context for one or more connector contact ids.
pub(crate) fn handle_contacts_context(paths: &ConfigPaths, params: &Value) -> Result<Value> {
    let params: ContactContextParams =
        serde_json::from_value(params.clone()).context("invalid contact context params")?;
    let contact_ids = normalize_contact_ids(params.contact_ids);
    if contact_ids.is_empty() {
        anyhow::bail!("contact context requires at least one valid contact id");
    }
    let limit = params
        .limit
        .unwrap_or(TELEGRAM_CONTEXT_LIMIT)
        .clamp(1, MAX_LIMIT);
    let contexts = contact_contexts(paths, &contact_ids, limit)?;
    Ok(json!({ "contact_ids": contact_ids, "context": contexts }))
}

/// Infers contact proposals from top connector candidates.
pub(crate) fn handle_contacts_infer(state: &DaemonState, params: &Value) -> Result<Value> {
    let params: ContactInferParams =
        serde_json::from_value(params.clone()).context("invalid contact infer params")?;
    let limit = params
        .limit
        .unwrap_or(DEFAULT_LIMIT)
        .clamp(1, DEFAULT_LIMIT);
    let paths = state.config_paths();
    let trace = ContactInferTrace::new(state, params.trace_id.as_deref());
    trace.message(
        "assistant",
        "Contact inference",
        "Collecting ranked connector candidates before asking the model to create contacts.",
    );
    trace.message("system", "System prompt", contact_infer_system_prompt());
    let collect_call = trace.tool_id("CollectContacts");
    trace.tool_event(
        &collect_call,
        "CollectContacts",
        "running",
        "Collecting ranked connector candidates.",
        json!({
            "limit": DEFAULT_LIMIT,
            "context_limit": INFERENCE_CONTEXT_LIMIT,
            "include_payload": false,
            "live_connector_commands": false,
        }),
        Value::Null,
    );
    let candidates = match inference_candidates(paths) {
        Ok(candidates) => {
            trace.tool_event(
                &collect_call,
                "CollectContacts",
                "completed",
                "Collected ranked connector candidates.",
                json!({
                    "limit": DEFAULT_LIMIT,
                    "context_limit": INFERENCE_CONTEXT_LIMIT,
                    "include_payload": false,
                    "live_connector_commands": false,
                }),
                json!({
                    "candidate_count": candidates.len(),
                    "sample": candidate_trace_sample(&candidates),
                }),
            );
            candidates
        }
        Err(err) => {
            trace.tool_event(
                &collect_call,
                "CollectContacts",
                "failed",
                "Failed to collect connector candidates.",
                json!({
                    "limit": DEFAULT_LIMIT,
                    "context_limit": INFERENCE_CONTEXT_LIMIT,
                    "include_payload": false,
                    "live_connector_commands": false,
                }),
                json!({ "error": err.to_string() }),
            );
            return Err(err);
        }
    };
    let proposals = infer_proposals(state, &candidates, limit, params.model.as_deref(), &trace)?;
    let proposals = save_proposals(paths, proposals)?;
    trace.message(
        "assistant",
        "Inference complete",
        format!("Inferred {} contact proposal(s).", proposals.len()),
    );
    let mut snapshot = handle_contacts_list(paths, &json!({ "limit": DEFAULT_LIMIT }))?;
    if let Some(object) = snapshot.as_object_mut() {
        object.insert("candidates".to_string(), json!(candidates));
        object.insert("proposals".to_string(), json!(proposals));
    }
    Ok(snapshot)
}

fn filtered_candidates(
    paths: &ConfigPaths,
    limit: usize,
    query: Option<&str>,
) -> Result<Vec<ConnectorContact>> {
    let mut candidates = ranked_candidates(paths, CandidateContextOptions::none())?;
    if let Some(query) = query {
        let query = query.to_ascii_lowercase();
        candidates.retain(|candidate| {
            candidate.id.to_ascii_lowercase().contains(&query)
                || candidate
                    .name
                    .as_deref()
                    .unwrap_or_default()
                    .to_ascii_lowercase()
                    .contains(&query)
        });
    }
    candidates.truncate(limit);
    Ok(candidates
        .into_iter()
        .map(Candidate::into_preview_contact)
        .collect())
}

fn searched_candidates(
    paths: &ConfigPaths,
    limit: usize,
    query: Option<&str>,
) -> Result<Vec<ConnectorContact>> {
    let Some(query) = query else {
        return filtered_candidates(paths, limit, None);
    };
    let mut by_id: HashMap<String, Candidate> = HashMap::new();
    let context_options = CandidateContextOptions::none();
    collect_telegram_candidates(paths, &mut by_id, context_options)?;
    collect_history_candidates(paths, &mut by_id, context_options);
    let query = query.to_ascii_lowercase();
    let mut candidates = by_id
        .into_values()
        .filter(|candidate| !candidate_is_bot_like(candidate))
        .filter(|candidate| {
            candidate.id.to_ascii_lowercase().contains(&query)
                || candidate
                    .name
                    .as_deref()
                    .unwrap_or_default()
                    .to_ascii_lowercase()
                    .contains(&query)
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.id.cmp(&right.id))
    });
    candidates.truncate(limit);
    Ok(candidates
        .into_iter()
        .map(Candidate::into_preview_contact)
        .collect())
}

fn filtered_saved_contacts(
    mut contacts: Vec<SavedContact>,
    query: Option<&str>,
) -> Vec<SavedContact> {
    let Some(query) = query else {
        return contacts;
    };
    let query = query.to_ascii_lowercase();
    contacts.retain(|contact| {
        contact.name.to_ascii_lowercase().contains(&query)
            || contact.description.to_ascii_lowercase().contains(&query)
            || contact
                .contact_ids
                .iter()
                .any(|id| id.to_ascii_lowercase().contains(&query))
    });
    contacts
}

fn contact_ids_overlap(left: &[String], right: &[String]) -> bool {
    let left = normalize_contact_ids(left);
    let right = normalize_contact_ids(right);
    left.iter().any(|id| right.contains(id))
}

fn merged_overlapping_contact_ids(
    contacts: &[SavedContact],
    saved_id: &str,
    contact_ids: Vec<String>,
) -> Vec<String> {
    let mut merged = contact_ids.clone();
    for contact in contacts {
        if contact.id == saved_id || !contact_ids_overlap(&contact.contact_ids, &contact_ids) {
            continue;
        }
        merged.extend(contact.contact_ids.iter().cloned());
    }
    normalize_contact_ids(merged)
}

fn enrich_saved_contact_avatars(paths: &ConfigPaths, contacts: &mut [SavedContact]) {
    if contacts.iter().all(|contact| {
        contact
            .avatar
            .as_deref()
            .map(str::trim)
            .is_some_and(|value| !value.is_empty())
    }) {
        return;
    }
    let avatars = read_telegram_peer_avatars(paths);
    if avatars.is_empty() {
        return;
    }
    for contact in contacts {
        if contact
            .avatar
            .as_deref()
            .map(str::trim)
            .is_some_and(|value| !value.is_empty())
        {
            continue;
        }
        contact.avatar = contact
            .contact_ids
            .iter()
            .find_map(|id| avatars.get(id).cloned());
    }
}

fn ranked_candidates(
    paths: &ConfigPaths,
    context_options: CandidateContextOptions,
) -> Result<Vec<Candidate>> {
    let mut by_id: HashMap<String, Candidate> = HashMap::new();
    collect_telegram_candidates(paths, &mut by_id, context_options)?;
    if context_options.uses_connector_commands() {
        collect_connector_method_candidates(&mut by_id, context_options);
    }
    collect_history_candidates(paths, &mut by_id, context_options);
    let mut candidates = by_id.into_values().collect::<Vec<_>>();
    reject_bot_candidates(&mut candidates);
    candidates.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(candidates)
}

fn inference_candidates(paths: &ConfigPaths) -> Result<Vec<ConnectorContact>> {
    Ok(sample_inference_candidates(
        ranked_candidates(paths, CandidateContextOptions::inference())?,
        DEFAULT_LIMIT,
    )
    .into_iter()
    .map(Candidate::into_contact)
    .collect())
}

fn sample_inference_candidates(candidates: Vec<Candidate>, per_connector: usize) -> Vec<Candidate> {
    let mut buckets: HashMap<String, Vec<Candidate>> = HashMap::new();
    for candidate in candidates {
        if candidate_is_bot_like(&candidate) {
            continue;
        }
        let prefix = contact_id_prefix(&candidate.id).unwrap_or("unknown");
        buckets
            .entry(prefix.to_string())
            .or_default()
            .push(candidate);
    }
    let mut prefixes = buckets.keys().cloned().collect::<Vec<_>>();
    prefixes.sort();
    let mut sampled = Vec::new();
    for prefix in prefixes {
        if let Some(mut bucket) = buckets.remove(&prefix) {
            bucket.sort_by(|left, right| {
                right
                    .score
                    .partial_cmp(&left.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| left.id.cmp(&right.id))
            });
            sampled.extend(bucket.into_iter().take(per_connector));
        }
    }
    sampled
}

fn collect_connector_method_candidates(
    by_id: &mut HashMap<String, Candidate>,
    context_options: CandidateContextOptions,
) {
    let Ok(manager) = puffer_core::subscription_manager() else {
        return;
    };
    for connection in manager.connection_store().list() {
        let Ok(Some(contacts)) =
            manager.list_connector_contacts(&connection.slug, None, Some(DEFAULT_LIMIT))
        else {
            continue;
        };
        merge_connector_contacts(by_id, &connection.connector_slug, contacts, context_options);
    }
}

fn merge_connector_contacts(
    by_id: &mut HashMap<String, Candidate>,
    connector_slug: &str,
    contacts: Vec<ConnectorContact>,
    context_options: CandidateContextOptions,
) {
    for contact in contacts {
        let Some(id) = normalize_contact_id(&contact.id) else {
            continue;
        };
        if !connector_slug_accepts_contact_id(connector_slug, &id) {
            continue;
        }
        let entry = by_id.entry(id.clone()).or_insert_with(|| Candidate {
            id: id.clone(),
            name: contact.name.clone().or_else(|| Some(id.clone())),
            avatar: contact.avatar.clone(),
            score: 0.0,
            last_message_at_ms: contact.last_message_at_ms,
            context: Vec::new(),
        });
        entry.score += contact.score.max(0.01);
        merge_candidate_last_message_at_ms(
            &mut entry.last_message_at_ms,
            contact.last_message_at_ms,
        );
        if entry.name.is_none() {
            entry.name = contact.name;
        }
        if entry.avatar.is_none() {
            entry.avatar = contact.avatar;
        }
        let context_limit = context_options.limit_for_id(&id);
        if context_limit == 0 || candidate_has_enriched_telegram_context(entry) {
            continue;
        }
        for mut context in contact.context {
            if !context_options.include_payload {
                context.payload = Value::Null;
            }
            push_context(entry, context, context_limit);
        }
    }
}

fn collect_history_candidates(
    paths: &ConfigPaths,
    by_id: &mut HashMap<String, Candidate>,
    context_options: CandidateContextOptions,
) {
    let Ok(manager) = puffer_core::subscription_manager() else {
        return;
    };
    let telegram_peer_names = read_telegram_peer_names(paths);
    let telegram_peer_avatars = read_telegram_peer_avatars(paths);
    manager
        .history_store()
        .visit_recent(HISTORY_CANDIDATE_LIMIT, |run| {
            merge_history_candidate_run(
                run,
                by_id,
                context_options,
                &telegram_peer_names,
                &telegram_peer_avatars,
            );
        });
}

fn merge_history_candidate_run(
    run: &puffer_subscriptions::WorkflowBindingRun,
    by_id: &mut HashMap<String, Candidate>,
    context_options: CandidateContextOptions,
    telegram_peer_names: &HashMap<String, String>,
    telegram_peer_avatars: &HashMap<String, String>,
) {
    let payload = run.trigger_info.get("payload");
    let text = run
        .trigger_info
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if text.trim().is_empty() {
        return;
    }
    let timestamp_ms = run
        .trigger_info
        .get("received_at_ms")
        .and_then(Value::as_i64)
        .map(i128::from);
    let score = entropy_score(&text) / days_since(timestamp_ms.unwrap_or(run.started_at_ms));
    for id in payload.map(contact_ids_from_payload).unwrap_or_default() {
        if contact_id_is_bot_like(&id) || payload.is_some_and(payload_has_bot_flag) {
            continue;
        }
        let candidate_name = history_candidate_name(&id, payload, telegram_peer_names);
        let candidate_avatar = telegram_peer_avatars.get(&id).cloned();
        let entry = by_id.entry(id.clone()).or_insert_with(|| Candidate {
            id: id.clone(),
            name: candidate_name.clone().or_else(|| Some(id.clone())),
            avatar: candidate_avatar.clone(),
            score: 0.0,
            last_message_at_ms: timestamp_ms,
            context: Vec::new(),
        });
        merge_candidate_name(&mut entry.name, candidate_name.as_deref());
        merge_candidate_last_message_at_ms(&mut entry.last_message_at_ms, timestamp_ms);
        if entry.avatar.is_none() {
            entry.avatar = candidate_avatar;
        }
        entry.score += score.max(0.01);
        let context_limit = context_options.limit_for_id(&id);
        if context_limit > 0 && !candidate_has_enriched_telegram_context(entry) {
            push_context(
                entry,
                ContactContext {
                    kind: "history_message".to_string(),
                    text: history_context_text(payload, &text),
                    timestamp_ms,
                    payload: payload
                        .map(|payload| context_options.payload(payload))
                        .unwrap_or(Value::Null),
                },
                context_limit,
            );
        }
    }
}

fn history_candidate_name(
    id: &str,
    payload: Option<&Value>,
    telegram_peer_names: &HashMap<String, String>,
) -> Option<String> {
    if id.starts_with("telegram@") {
        if let Some(name) = telegram_peer_names
            .get(id)
            .map(String::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Some(name.to_string());
        }
    }
    payload.and_then(name_from_payload)
}

fn history_context_text(payload: Option<&Value>, text: &str) -> String {
    let Some(payload) = payload else {
        return text.to_string();
    };
    if payload.get("chat_kind").and_then(Value::as_str).is_none() {
        return text.to_string();
    }
    format!(
        "[history][dest: {}][from: {}] {}",
        history_destination_label(payload),
        history_sender_label(payload),
        text
    )
}

fn history_destination_label(payload: &Value) -> String {
    if payload.get("chat_kind").and_then(Value::as_str) == Some("user") {
        if let Some(name) = name_from_payload(payload) {
            return name;
        }
    }
    history_payload_string(payload, "chat_title")
        .or_else(|| {
            history_payload_string(payload, "chat_username").map(|username| format!("@{username}"))
        })
        .or_else(|| {
            payload
                .get("chat_id")
                .and_then(Value::as_i64)
                .map(|id| format!("chat {id}"))
        })
        .unwrap_or_else(|| "unknown".to_string())
}

fn history_sender_label(payload: &Value) -> String {
    if payload
        .get("is_outgoing")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return "user".to_string();
    }
    history_payload_string(payload, "sender_name")
        .or_else(|| history_payload_string(payload, "chat_title"))
        .or_else(|| {
            history_payload_string(payload, "sender_username")
                .map(|username| format!("@{username}"))
        })
        .unwrap_or_else(|| "contact".to_string())
}

fn history_payload_string(payload: &Value, key: &str) -> Option<String> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn merge_candidate_name(existing: &mut Option<String>, candidate: Option<&str>) {
    let Some(candidate) = candidate.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };
    if candidate_name_is_more_complete(existing.as_deref(), candidate) {
        *existing = Some(candidate.to_string());
    }
}

fn candidate_name_is_more_complete(existing: Option<&str>, candidate: &str) -> bool {
    let Some(existing) = existing.map(str::trim).filter(|value| !value.is_empty()) else {
        return true;
    };
    let existing_is_contact_id = name_looks_like_contact_id(existing);
    let candidate_is_contact_id = name_looks_like_contact_id(candidate);
    if existing_is_contact_id != candidate_is_contact_id {
        return existing_is_contact_id && !candidate_is_contact_id;
    }
    let existing_parts = existing.split_whitespace().count();
    let candidate_parts = candidate.split_whitespace().count();
    candidate_parts > existing_parts
        || (candidate_parts == existing_parts && candidate.len() > existing.len())
}

fn merge_candidate_last_message_at_ms(existing: &mut Option<i128>, candidate: Option<i128>) {
    let Some(candidate) = candidate else {
        return;
    };
    *existing = Some(existing.map_or(candidate, |current| current.max(candidate)));
}

fn name_looks_like_contact_id(name: &str) -> bool {
    let value = name.trim().to_ascii_lowercase();
    value.starts_with("telegram@")
        || value.starts_with("telegram-user-id@")
        || value.starts_with("google@")
        || value.starts_with("gmail@")
        || value.starts_with("lark@")
        || value.starts_with("slack@")
}

fn reject_bot_candidates(candidates: &mut Vec<Candidate>) {
    candidates.retain(|candidate| !candidate_is_bot_like(candidate));
}

fn candidate_has_enriched_telegram_context(candidate: &Candidate) -> bool {
    is_telegram_contact_id(&candidate.id)
        && candidate
            .context
            .iter()
            .any(|context| context.kind.starts_with("telegram_"))
}

fn is_telegram_contact_id(id: &str) -> bool {
    id.starts_with("telegram@") || id.starts_with("telegram-user-id@")
}

fn candidate_is_bot_like(candidate: &Candidate) -> bool {
    contact_id_is_bot_like(&candidate.id)
        || candidate.name.as_deref().is_some_and(name_is_bot_like)
        || candidate
            .context
            .iter()
            .any(|context| payload_has_bot_flag(&context.payload))
}

fn contact_id_is_bot_like(id: &str) -> bool {
    let Some((prefix, suffix)) = id.split_once('@') else {
        return false;
    };
    let prefix = prefix.to_ascii_lowercase();
    let suffix = suffix.to_ascii_lowercase();
    if prefix == "telegram" {
        return suffix.ends_with("bot");
    }
    let local = suffix.split('@').next().unwrap_or(suffix.as_str());
    has_bot_suffix(local) || has_bot_suffix(&suffix)
}

fn name_is_bot_like(name: &str) -> bool {
    let lower = name.trim().to_ascii_lowercase();
    lower == "bot"
        || lower.ends_with(" bot")
        || lower.contains(" support bot")
        || lower.contains(" sales bot")
        || lower.contains(" assistant bot")
}

fn payload_has_bot_flag(payload: &Value) -> bool {
    for path in [
        "/chat_is_bot",
        "/sender_is_bot",
        "/is_bot",
        "/bot",
        "/message/sender_is_bot",
        "/message/is_bot",
    ] {
        if payload.pointer(path).and_then(Value::as_bool) == Some(true) {
            return true;
        }
    }
    false
}

fn has_bot_suffix(value: &str) -> bool {
    value == "bot" || value.ends_with("-bot") || value.ends_with("_bot") || value.ends_with(".bot")
}

fn contact_contexts(
    paths: &ConfigPaths,
    contact_ids: &[String],
    limit: usize,
) -> Result<Vec<ContactContext>> {
    let wanted = contact_ids.iter().cloned().collect::<BTreeSet<_>>();
    let mut contexts = Vec::new();
    let mut enriched_ids = HashSet::new();
    for candidate in ranked_candidates(paths, CandidateContextOptions::full())? {
        if wanted.is_empty() || wanted.contains(&candidate.id) {
            let cap = if is_telegram_contact_id(&candidate.id) {
                TELEGRAM_CONTEXT_LIMIT.min(limit)
            } else {
                GOOGLE_CONTEXT_LIMIT.min(limit)
            };
            if !candidate.context.is_empty() {
                enriched_ids.insert(candidate.id.clone());
            }
            contexts.extend(candidate.context.into_iter().take(cap));
        }
    }
    let connector_context_ids = contact_ids
        .iter()
        .filter(|id| !id.starts_with("telegram@") || !enriched_ids.contains(*id))
        .cloned()
        .collect::<Vec<_>>();
    contexts.extend(connector_method_contexts(&connector_context_ids, limit));
    contexts.sort_by(|left, right| right.timestamp_ms.cmp(&left.timestamp_ms));
    contexts.truncate(limit);
    Ok(contexts)
}

fn connector_method_contexts(contact_ids: &[String], limit: usize) -> Vec<ContactContext> {
    if contact_ids.is_empty() {
        return Vec::new();
    }
    let Ok(manager) = puffer_core::subscription_manager() else {
        return Vec::new();
    };
    let mut contexts = Vec::new();
    for connection in manager.connection_store().list() {
        let owned_ids = contact_ids
            .iter()
            .filter(|id| connector_slug_accepts_contact_id(&connection.connector_slug, id))
            .cloned()
            .collect::<Vec<_>>();
        if owned_ids.is_empty() {
            continue;
        }
        let Ok(Some((_ids, items))) =
            manager.connector_contact_context(&connection.slug, owned_ids, Some(limit))
        else {
            continue;
        };
        contexts.extend(items);
    }
    contexts
}

fn name_from_payload(payload: &Value) -> Option<String> {
    if payload.get("chat_kind").and_then(Value::as_str) == Some("user") {
        if let Some(value) = payload
            .pointer("/chat_title")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Some(value.to_string());
        }
    }
    contact_display_name_from_payload(payload)
}

fn push_context(candidate: &mut Candidate, context: ContactContext, limit: usize) {
    if context.text.trim().is_empty() {
        return;
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    context.text.hash(&mut hasher);
    let hash = hasher.finish();
    if candidate.context.iter().any(|existing| {
        let mut existing_hasher = std::collections::hash_map::DefaultHasher::new();
        existing.text.hash(&mut existing_hasher);
        existing_hasher.finish() == hash
    }) {
        return;
    }
    candidate.context.push(context);
    sort_context(&mut candidate.context, limit);
}

fn sort_context(context: &mut Vec<ContactContext>, limit: usize) {
    context.sort_by(|left, right| {
        let entropy_order = entropy_score(&right.text)
            .partial_cmp(&entropy_score(&left.text))
            .unwrap_or(std::cmp::Ordering::Equal);
        entropy_order.then_with(|| right.timestamp_ms.cmp(&left.timestamp_ms))
    });
    context.truncate(limit);
}

fn entropy_score(text: &str) -> f64 {
    let tokens = text
        .split_whitespace()
        .map(|token| {
            token
                .trim_matches(|ch: char| !ch.is_alphanumeric())
                .to_ascii_lowercase()
        })
        .filter(|token| token.len() > 1)
        .collect::<Vec<_>>();
    if tokens.is_empty() {
        return 0.1;
    }
    let unique = tokens.iter().collect::<HashSet<_>>().len() as f64;
    let length = tokens.len() as f64;
    (unique / length) * length.ln_1p().max(1.0)
}

fn days_since(timestamp_ms: i128) -> f64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i128)
        .unwrap_or(timestamp_ms);
    let delta = (now - timestamp_ms).max(DAY_MS);
    (delta as f64) / (DAY_MS as f64)
}

impl Candidate {
    fn into_contact(self) -> ConnectorContact {
        ConnectorContact {
            id: self.id,
            avatar: self.avatar,
            name: self.name,
            context: self.context,
            score: self.score,
            last_message_at_ms: self.last_message_at_ms,
        }
    }

    fn into_preview_contact(self) -> ConnectorContact {
        ConnectorContact {
            id: self.id,
            avatar: None,
            name: self.name,
            context: Vec::new(),
            score: self.score,
            last_message_at_ms: self.last_message_at_ms,
        }
    }
}

#[cfg(test)]
#[path = "daemon_contacts_delete_tests.rs"]
mod delete_tests;

#[cfg(test)]
#[path = "daemon_contacts_save_tests.rs"]
mod save_tests;

#[cfg(test)]
#[path = "daemon_contacts_tests.rs"]
mod tests;

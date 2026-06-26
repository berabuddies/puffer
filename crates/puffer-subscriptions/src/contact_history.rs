//! History-backed contact methods for subscriber-backed connectors.

use crate::contacts::{
    connector_slug_accepts_contact_id, contact_display_name_from_payload,
    contact_ids_for_connector, contact_ids_from_payload, ConnectorContact, ContactContext,
};
use crate::history::{now_ms, WorkflowBindingRun, WorkflowHistoryStore};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::hash::{Hash, Hasher};

const DEFAULT_HISTORY_CONTACT_LIMIT: usize = 30;
const MAX_CONTEXT_LIMIT: usize = 120;
const DAY_MS: i128 = 86_400_000;

#[derive(Debug, Default)]
struct HistoryContact {
    name: Option<String>,
    score: f64,
    context: Vec<ContactContext>,
}

pub(crate) fn list_contacts(
    history_store: &WorkflowHistoryStore,
    connector_slug: &str,
    connection_slug: &str,
    query: Option<&str>,
    limit: Option<usize>,
) -> Vec<ConnectorContact> {
    let mut contacts =
        collect_history_contacts(history_store.list(), connector_slug, connection_slug);
    if let Some(query) = query.map(str::trim).filter(|value| !value.is_empty()) {
        let query = query.to_ascii_lowercase();
        contacts.retain(|id, contact| contact_matches_query(id, contact, &query));
    }
    let mut rows = contacts
        .into_iter()
        .map(|(id, contact)| ConnectorContact {
            id,
            avatar: None,
            name: contact.name,
            display: None,
            context: contact.context,
            score: contact.score,
            last_message_at_ms: None,
        })
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.id.cmp(&right.id))
    });
    rows.truncate(limit.unwrap_or(DEFAULT_HISTORY_CONTACT_LIMIT));
    rows
}

pub(crate) fn contact_context(
    history_store: &WorkflowHistoryStore,
    connector_slug: &str,
    connection_slug: &str,
    contact_ids: &[String],
    limit: Option<usize>,
) -> Option<(Vec<String>, Vec<ContactContext>)> {
    let owned_ids = contact_ids_for_connector(connector_slug, contact_ids);
    if owned_ids.is_empty() {
        return None;
    }
    let wanted = owned_ids.iter().cloned().collect::<BTreeSet<_>>();
    let cap = limit.unwrap_or(MAX_CONTEXT_LIMIT).min(MAX_CONTEXT_LIMIT);
    let contacts = collect_history_contacts(history_store.list(), connector_slug, connection_slug);
    let mut context = contacts
        .into_iter()
        .filter(|(id, _)| wanted.contains(id))
        .flat_map(|(_, contact)| contact.context)
        .collect::<Vec<_>>();
    sort_context(&mut context, cap);
    Some((owned_ids, context))
}

fn collect_history_contacts(
    runs: Vec<WorkflowBindingRun>,
    connector_slug: &str,
    connection_slug: &str,
) -> BTreeMap<String, HistoryContact> {
    let mut contacts = BTreeMap::new();
    for run in runs {
        if run
            .trigger_info
            .get("connection_slug")
            .and_then(Value::as_str)
            != Some(connection_slug)
        {
            continue;
        }
        let payload = run
            .trigger_info
            .get("payload")
            .cloned()
            .unwrap_or(Value::Null);
        let text = run
            .trigger_info
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        let timestamp_ms = run
            .trigger_info
            .get("received_at_ms")
            .and_then(Value::as_i64)
            .map(i128::from)
            .unwrap_or(run.started_at_ms);
        let score = if text.is_empty() {
            0.01
        } else {
            entropy_score(&text) / days_since(timestamp_ms)
        };
        for id in contact_ids_from_payload(&payload) {
            if !connector_slug_accepts_contact_id(connector_slug, &id) {
                continue;
            }
            let entry = contacts.entry(id).or_insert_with(HistoryContact::default);
            if entry.name.is_none() {
                entry.name = name_from_payload(&payload);
            }
            entry.score += score.max(0.01);
            if !text.is_empty() {
                push_context(
                    entry,
                    ContactContext {
                        kind: run
                            .trigger_info
                            .get("kind")
                            .and_then(Value::as_str)
                            .unwrap_or("message")
                            .to_string(),
                        text: text.clone(),
                        timestamp_ms: Some(timestamp_ms),
                        payload: payload.clone(),
                    },
                    MAX_CONTEXT_LIMIT,
                );
            }
        }
    }
    contacts
}

fn contact_matches_query(id: &str, contact: &HistoryContact, query: &str) -> bool {
    id.to_ascii_lowercase().contains(query)
        || contact
            .name
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .contains(query)
        || contact
            .context
            .iter()
            .any(|context| context.text.to_ascii_lowercase().contains(query))
}

fn name_from_payload(payload: &Value) -> Option<String> {
    contact_display_name_from_payload(payload)
}

fn push_context(contact: &mut HistoryContact, context: ContactContext, limit: usize) {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    context.text.hash(&mut hasher);
    let hash = hasher.finish();
    if contact.context.iter().any(|existing| {
        let mut existing_hasher = std::collections::hash_map::DefaultHasher::new();
        existing.text.hash(&mut existing_hasher);
        existing_hasher.finish() == hash
    }) {
        return;
    }
    contact.context.push(context);
    sort_context(&mut contact.context, limit);
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
    let delta = (now_ms() - timestamp_ms).max(DAY_MS);
    (delta as f64) / (DAY_MS as f64)
}

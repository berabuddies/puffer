//! Telegram diagnostics-backed contact ranking.

use super::{
    days_since, entropy_score, push_context, Candidate, CandidateContextOptions,
    TELEGRAM_CONTEXT_LIMIT, TELEGRAM_INTERACTION_CONTEXT_LIMIT, TELEGRAM_RECENT_CONTEXT_LIMIT,
};
use anyhow::{Context, Result};
use grammers_session::Session;
use puffer_config::ConfigPaths;
use puffer_subscriptions::{normalize_contact_id, ContactContext};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

#[path = "daemon_contacts_telegram_peer_cache.rs"]
mod daemon_contacts_telegram_peer_cache;
use daemon_contacts_telegram_peer_cache::{
    collect_telegram_peer_cache_candidates, hydrate_telegram_peer_cache,
    hydrate_telegram_peer_cache_if_needed, TelegramPeerCacheHydrationMode,
};

#[cfg(test)]
pub(super) fn install_test_telegram_peer_cache_hydrator<F>(hydrator: F) -> impl Drop
where
    F: Fn(&ConfigPaths, &Path) -> Result<()> + 'static,
{
    daemon_contacts_telegram_peer_cache::install_test_telegram_peer_cache_hydrator(hydrator)
}

const DEFAULT_LIMIT: usize = 30;
const DAY_MS: i128 = 86_400_000;

/// Normalized Telegram diagnostic message data used for contact ranking.
#[derive(Debug, Clone)]
pub(super) struct TelegramDiagMessage {
    /// Normalized Telegram contact id for the chat or sender.
    pub(super) contact_id: String,
    /// Best available display name from Telegram metadata.
    pub(super) name: Option<String>,
    /// Optional profile avatar as a URL or data URI.
    pub(super) avatar: Option<String>,
    /// Chat destination label, such as a group name or direct-message user.
    pub(super) destination_name: Option<String>,
    /// Chat destination username when Telegram exposes one.
    pub(super) destination_username: Option<String>,
    /// Telegram chat id from the source diagnostic payload.
    pub(super) chat_id: i64,
    /// Telegram chat kind such as `user`, `group`, or `channel`.
    pub(super) chat_kind: String,
    /// Sender username for group messages when available.
    pub(super) sender_username: Option<String>,
    /// Sender display name when Telegram exposes one.
    pub(super) sender_name: Option<String>,
    /// Whether the message was sent by the local Telegram account.
    pub(super) is_outgoing: bool,
    /// Whether this diagnostic row identifies the local Telegram account.
    pub(super) is_self_contact: bool,
    /// Message timestamp in milliseconds since the Unix epoch.
    pub(super) date_ms: i128,
    /// Telegram message id when present in diagnostics.
    pub(super) message_id: Option<i64>,
    /// Message id referenced by a reply, when present.
    pub(super) reply_to_message_id: Option<i64>,
    /// Text excerpt used for scoring and inference context.
    pub(super) text: String,
    /// Original diagnostic payload when payload retention is requested.
    pub(super) payload: Value,
}

#[derive(Debug, Default)]
struct TelegramScore {
    incoming: f64,
    outgoing: f64,
    personal_days: BTreeSet<i128>,
    first_half_count: usize,
    second_half_count: usize,
    reply_count: usize,
}

#[derive(Debug, Default, Deserialize)]
struct TelegramPeerCacheEntry {
    #[serde(default)]
    numeric_id: i64,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    usernames: Vec<String>,
    #[serde(default)]
    first_name: Option<String>,
    #[serde(default)]
    last_name: Option<String>,
    #[serde(default)]
    avatar: Option<String>,
    #[serde(default)]
    is_bot: bool,
}

#[derive(Debug, Clone, Default)]
struct TelegramPeerMetadata {
    name: Option<String>,
    avatar: Option<String>,
}

/// Collects ranked Telegram contacts from cached message diagnostics.
pub(super) fn collect_telegram_candidates(
    paths: &ConfigPaths,
    by_id: &mut HashMap<String, Candidate>,
    context_options: CandidateContextOptions,
) -> Result<()> {
    let root = paths.user_config_dir.join("telegram-accounts");
    if !root.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(&root).with_context(|| format!("read {}", root.display()))? {
        let Ok(entry) = entry else { continue };
        let account_dir = entry.path();
        hydrate_telegram_peer_cache_if_needed(paths, &account_dir);
        collect_telegram_peer_cache_candidates(&account_dir, by_id);
        let path = account_dir.join("message-diagnostics.ndjson");
        if !path.exists() {
            continue;
        }
        let self_user_id = telegram_session_user_id(&account_dir.join("telegram.session"));
        let peer_metadata = read_telegram_peer_metadata_from_account(&account_dir);
        let messages = read_telegram_messages(
            &path,
            context_options.include_payload,
            self_user_id,
            &peer_metadata,
        )?;
        let ranked = rank_telegram_messages(messages, context_options);
        for (id, candidate) in ranked {
            by_id
                .entry(id)
                .and_modify(|existing| {
                    existing.score += candidate.score;
                    merge_telegram_name(&mut existing.name, &candidate.name);
                    if existing.avatar.is_none() {
                        existing.avatar = candidate.avatar.clone();
                    }
                    for context in &candidate.context {
                        push_context(existing, context.clone(), TELEGRAM_CONTEXT_LIMIT);
                    }
                })
                .or_insert(candidate);
        }
    }
    Ok(())
}

/// Forces a best-effort refresh of Telegram peer caches for contact pickers.
pub(super) fn refresh_telegram_peer_caches(paths: &ConfigPaths) -> Result<()> {
    let root = paths.user_config_dir.join("telegram-accounts");
    if !root.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(&root).with_context(|| format!("read {}", root.display()))? {
        let Ok(entry) = entry else { continue };
        hydrate_telegram_peer_cache(paths, &entry.path(), TelegramPeerCacheHydrationMode::Force);
    }
    Ok(())
}

fn read_telegram_messages(
    path: &Path,
    include_payload: bool,
    self_user_id: Option<i64>,
    peer_metadata: &HashMap<String, TelegramPeerMetadata>,
) -> Result<Vec<TelegramDiagMessage>> {
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut messages = Vec::new();
    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { continue };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(payload) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if payload.get("stage").and_then(Value::as_str) != Some("emitted") {
            continue;
        }
        if payload
            .get("notification_muted")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            continue;
        }
        let text = payload
            .get("text_prefix")
            .or_else(|| payload.get("text"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        if text.is_empty() {
            continue;
        }
        let chat_kind = payload
            .get("chat_kind")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if chat_kind != "user" {
            continue;
        }
        let Some(contact_id) = telegram_contact_id(&payload, &chat_kind) else {
            continue;
        };
        let chat_id = payload.get("chat_id").and_then(Value::as_i64).unwrap_or(0);
        let sender_id = payload.get("sender_id").and_then(Value::as_i64);
        let sender_username = payload
            .get("sender_username")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let sender_name = payload
            .get("sender_name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        let is_outgoing = payload
            .get("is_outgoing")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let name = telegram_contact_display_name(&payload, &chat_kind, peer_metadata);
        let avatar = telegram_cached_contact_avatar(&payload, &chat_kind, peer_metadata);
        let destination_name = telegram_destination_name(&payload, &chat_kind, name.as_deref());
        let destination_username = payload
            .get("chat_username")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        let is_self_contact = telegram_payload_is_self_contact(
            &chat_kind,
            chat_id,
            sender_id,
            is_outgoing,
            self_user_id,
        );
        let date_ms = payload
            .get("date_ms")
            .and_then(Value::as_i64)
            .map(i128::from)
            .unwrap_or(0);
        let message_id = payload.get("message_id").and_then(Value::as_i64);
        let reply_to_message_id = payload
            .pointer("/reply_to/message_id")
            .and_then(Value::as_i64);
        let stored_payload = if include_payload {
            payload
        } else {
            Value::Null
        };
        messages.push(TelegramDiagMessage {
            contact_id,
            name,
            avatar,
            destination_name,
            destination_username,
            chat_id,
            chat_kind,
            sender_username,
            sender_name,
            is_outgoing,
            is_self_contact,
            date_ms,
            message_id,
            reply_to_message_id,
            text,
            payload: stored_payload,
        });
    }
    messages.sort_by_key(|message| message.date_ms);
    Ok(messages)
}

fn rank_telegram_messages(
    messages: Vec<TelegramDiagMessage>,
    context_options: CandidateContextOptions,
) -> BTreeMap<String, Candidate> {
    if messages.is_empty() {
        return BTreeMap::new();
    }
    let mut considered = 1000usize.min(messages.len());
    loop {
        let start = messages.len().saturating_sub(considered);
        let result = score_telegram_window_with_options(&messages[start..], context_options);
        if result.len() >= DEFAULT_LIMIT || considered == messages.len() {
            return result;
        }
        considered = (considered + 1000).min(messages.len());
    }
}

#[cfg(test)]
/// Scores an in-memory Telegram diagnostic window for tests.
pub(super) fn score_telegram_window(
    messages: &[TelegramDiagMessage],
) -> BTreeMap<String, Candidate> {
    score_telegram_window_with_options(messages, CandidateContextOptions::full())
}

fn score_telegram_window_with_options(
    messages: &[TelegramDiagMessage],
    context_options: CandidateContextOptions,
) -> BTreeMap<String, Candidate> {
    let midpoint = messages
        .first()
        .zip(messages.last())
        .map(|(first, last)| first.date_ms + (last.date_ms - first.date_ms) / 2)
        .unwrap_or(0);
    let by_id = messages
        .iter()
        .enumerate()
        .map(|(index, message)| (message.message_id, index))
        .collect::<HashMap<_, _>>();
    let mut scores: HashMap<String, TelegramScore> = HashMap::new();
    let mut candidates: BTreeMap<String, Candidate> = BTreeMap::new();
    for (index, message) in messages.iter().enumerate() {
        let text_entropy = entropy_score(&message.text);
        let days = days_since(message.date_ms);
        let score = scores.entry(message.contact_id.clone()).or_default();
        if message.chat_kind == "user" {
            score.personal_days.insert(message.date_ms / DAY_MS);
        }
        if message.date_ms < midpoint {
            score.first_half_count += 1;
        } else {
            score.second_half_count += 1;
        }
        if message.is_outgoing {
            score.outgoing += text_entropy / days;
        } else if let Some(reply_index) = reply_index(messages, &by_id, index, message) {
            let reply = &messages[reply_index];
            let delay_minutes = ((reply.date_ms - message.date_ms).max(60_000) as f64) / 60_000.0;
            score.reply_count += 1;
            score.incoming += text_entropy * entropy_score(&reply.text) / (days * delay_minutes);
        }
        if message.is_self_contact {
            continue;
        }
        let entry = candidates
            .entry(message.contact_id.clone())
            .or_insert_with(|| Candidate {
                id: message.contact_id.clone(),
                name: message.name.clone(),
                avatar: message.avatar.clone(),
                score: 0.0,
                context: Vec::new(),
            });
        merge_telegram_name(&mut entry.name, &message.name);
        if entry.avatar.is_none() {
            entry.avatar = message.avatar.clone();
        }
    }
    for (id, score) in scores {
        if score.reply_count == 0
            && candidates
                .get(&id)
                .is_some_and(|candidate| candidate.id.starts_with("telegram@"))
            && score.personal_days.is_empty()
        {
            candidates.remove(&id);
            continue;
        }
        if let Some(candidate) = candidates.get_mut(&id) {
            let affinity = score.personal_days.len().max(1) as f64 * acceleration(&score);
            candidate.score = (score.incoming + score.outgoing) * affinity;
            let context_limit = context_options.limit_for_id(&candidate.id);
            if context_limit > 0 {
                candidate.context = telegram_context_for_candidate(
                    messages,
                    &by_id,
                    &candidate.id,
                    context_options,
                );
            }
        }
    }
    candidates
}

fn telegram_context_for_candidate(
    messages: &[TelegramDiagMessage],
    by_id: &HashMap<Option<i64>, usize>,
    contact_id: &str,
    context_options: CandidateContextOptions,
) -> Vec<ContactContext> {
    let limit = context_options.limit_for_id(contact_id);
    if limit == 0 {
        return Vec::new();
    }
    let mut context = Vec::new();
    let recent = messages
        .iter()
        .filter(|message| telegram_message_matches_contact(message, contact_id))
        .rev()
        .take(TELEGRAM_RECENT_CONTEXT_LIMIT.min(limit))
        .collect::<Vec<_>>();
    let recent_total = recent.len();
    for (index, message) in recent.into_iter().rev().enumerate() {
        context.push(telegram_context_item(
            message,
            TelegramContextSection::Recent,
            index + 1,
            recent_total,
            None,
            context_options.include_payload,
        ));
    }

    let pair_limit =
        TELEGRAM_INTERACTION_CONTEXT_LIMIT.min(limit.saturating_sub(context.len()) / 2);
    if pair_limit == 0 {
        return context;
    }
    let pairs = messages
        .iter()
        .enumerate()
        .filter(|(_, message)| {
            message.contact_id == contact_id && !message.is_outgoing && !message.is_self_contact
        })
        .filter_map(|(index, message)| {
            reply_index(messages, by_id, index, message).map(|reply_index| (index, reply_index))
        })
        .rev()
        .take(pair_limit)
        .collect::<Vec<_>>();
    let pair_total = pairs.len();
    for (index, (message_index, reply_index)) in pairs.into_iter().rev().enumerate() {
        let pair_number = index + 1;
        let pair_id = telegram_context_pair_id(messages[message_index].message_id, pair_number);
        context.push(telegram_context_item(
            &messages[message_index],
            TelegramContextSection::InteractionContact,
            pair_number,
            pair_total,
            Some(pair_id.clone()),
            context_options.include_payload,
        ));
        context.push(telegram_context_item(
            &messages[reply_index],
            TelegramContextSection::InteractionReply,
            pair_number,
            pair_total,
            Some(pair_id),
            context_options.include_payload,
        ));
    }
    context
}

#[derive(Clone, Copy)]
enum TelegramContextSection {
    Recent,
    InteractionContact,
    InteractionReply,
}

impl TelegramContextSection {
    fn kind(self) -> &'static str {
        match self {
            Self::Recent => "telegram_recent_message",
            Self::InteractionContact => "telegram_interaction_contact_message",
            Self::InteractionReply => "telegram_interaction_user_reply",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Recent => "recent",
            Self::InteractionContact => "interacted/contact",
            Self::InteractionReply => "interacted/user_reply",
        }
    }

    fn payload_label(self) -> &'static str {
        match self {
            Self::Recent => "recent",
            Self::InteractionContact | Self::InteractionReply => "interacted",
        }
    }
}

fn telegram_context_item(
    message: &TelegramDiagMessage,
    section: TelegramContextSection,
    index: usize,
    total: usize,
    pair_id: Option<String>,
    include_raw_payload: bool,
) -> ContactContext {
    let destination = telegram_destination_label(message);
    let sender = telegram_sender_label(message);
    ContactContext {
        kind: section.kind().to_string(),
        text: format!(
            "[{} {}/{}][dest: {}][from: {}] {}",
            section.label(),
            index,
            total.max(1),
            destination,
            sender,
            message.text
        ),
        timestamp_ms: Some(message.date_ms),
        payload: telegram_context_payload(
            message,
            section,
            index,
            total,
            pair_id,
            include_raw_payload,
        ),
    }
}

fn telegram_context_payload(
    message: &TelegramDiagMessage,
    section: TelegramContextSection,
    index: usize,
    total: usize,
    pair_id: Option<String>,
    include_raw_payload: bool,
) -> Value {
    let mut payload = json!({
        "context_section": section.payload_label(),
        "context_role": section.label(),
        "context_index": index,
        "context_total": total,
        "direction": if message.is_outgoing { "outgoing" } else { "incoming" },
        "destination": {
            "kind": message.chat_kind.as_str(),
            "label": telegram_destination_label(message),
            "username": message.destination_username.as_deref(),
            "chat_id": message.chat_id,
        },
        "sender": {
            "label": telegram_sender_label(message),
            "username": message.sender_username.as_deref(),
            "is_user": message.is_outgoing,
        },
        "message_id": message.message_id,
        "reply_to_message_id": message.reply_to_message_id,
        "pair_id": pair_id,
    });
    if include_raw_payload {
        if let Some(map) = payload.as_object_mut() {
            map.insert("raw".to_string(), message.payload.clone());
        }
    }
    payload
}

fn telegram_message_matches_contact(message: &TelegramDiagMessage, contact_id: &str) -> bool {
    message.contact_id == contact_id && !message.is_self_contact
}

fn telegram_context_pair_id(message_id: Option<i64>, fallback: usize) -> String {
    message_id
        .map(|id| format!("telegram-reply-{id}"))
        .unwrap_or_else(|| format!("telegram-reply-{fallback}"))
}

/// Finds the outgoing reply that makes an incoming Telegram message relevant.
pub(super) fn reply_index(
    messages: &[TelegramDiagMessage],
    by_id: &HashMap<Option<i64>, usize>,
    index: usize,
    message: &TelegramDiagMessage,
) -> Option<usize> {
    if message.chat_kind == "user" {
        return messages
            .iter()
            .enumerate()
            .skip(index + 1)
            .find(|(_, candidate)| candidate.chat_id == message.chat_id)
            .and_then(|(index, candidate)| candidate.is_outgoing.then_some(index));
    }
    if let Some(message_id) = message.message_id {
        for candidate in messages.iter().skip(index + 1).take(25) {
            if candidate.chat_id == message.chat_id
                && candidate.is_outgoing
                && candidate.reply_to_message_id == Some(message_id)
            {
                return candidate
                    .message_id
                    .and_then(|id| by_id.get(&Some(id)).copied());
            }
        }
    }
    let Some(username) = message.sender_username.as_deref() else {
        return None;
    };
    let mention = format!("@{}", username.to_ascii_lowercase());
    messages
        .iter()
        .enumerate()
        .skip(index + 1)
        .take(25)
        .find(|(_, candidate)| {
            candidate.chat_id == message.chat_id
                && candidate.is_outgoing
                && candidate.text.to_ascii_lowercase().contains(&mention)
        })
        .map(|(index, _)| index)
}

/// Returns the normalized contact id for a Telegram diagnostic payload.
pub(super) fn telegram_contact_id(payload: &Value, chat_kind: &str) -> Option<String> {
    if chat_kind != "user"
        || payload.get("chat_is_bot").and_then(Value::as_bool) == Some(true)
        || payload_username_looks_like_bot(payload, "chat_username")
    {
        return None;
    }
    if let Some(id) = payload
        .get("chat_username")
        .and_then(Value::as_str)
        .and_then(|username| normalize_contact_id(&format!("telegram@{username}")))
    {
        return Some(id);
    }
    telegram_payload_user_id(payload)
        .and_then(|user_id| normalize_contact_id(&format!("telegram-user-id@{user_id}")))
}

/// Returns the best display name for a Telegram diagnostic payload.
pub(super) fn telegram_contact_name(payload: &Value, chat_kind: &str) -> Option<String> {
    let value = if chat_kind == "user" {
        payload
            .get("chat_title")
            .or_else(|| payload.get("sender_name"))
            .and_then(Value::as_str)
    } else {
        payload.get("sender_name").and_then(Value::as_str)
    };
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn telegram_contact_display_name(
    payload: &Value,
    chat_kind: &str,
    peer_metadata: &HashMap<String, TelegramPeerMetadata>,
) -> Option<String> {
    let name = telegram_cached_contact_name(payload, chat_kind, peer_metadata)
        .or_else(|| telegram_contact_name(payload, chat_kind));
    let username = telegram_contact_username(payload, chat_kind);
    match (name, username) {
        (Some(name), Some(username)) if name_should_include_handle(&name, &username) => {
            Some(format!("{name} (@{username})"))
        }
        (Some(name), _) => Some(name),
        (None, Some(username)) => Some(format!("@{username}")),
        (None, None) => None,
    }
}

fn telegram_destination_name(
    payload: &Value,
    chat_kind: &str,
    contact_name: Option<&str>,
) -> Option<String> {
    if chat_kind == "user" {
        return contact_name
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .or_else(|| telegram_payload_string(payload, "chat_title"))
            .or_else(|| {
                telegram_payload_string(payload, "chat_username")
                    .map(|username| format!("@{}", username.trim_start_matches('@')))
            });
    }
    telegram_payload_string(payload, "chat_title").or_else(|| {
        telegram_payload_string(payload, "chat_username")
            .map(|username| format!("@{}", username.trim_start_matches('@')))
    })
}

fn telegram_payload_string(payload: &Value, key: &str) -> Option<String> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn telegram_destination_label(message: &TelegramDiagMessage) -> String {
    message
        .destination_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            message
                .destination_username
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|username| format!("@{}", username.trim_start_matches('@')))
        })
        .unwrap_or_else(|| format!("chat {}", message.chat_id))
}

fn telegram_sender_label(message: &TelegramDiagMessage) -> String {
    if message.is_outgoing {
        return "user".to_string();
    }
    message
        .sender_name
        .as_deref()
        .or(message.name.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            message
                .sender_username
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|username| format!("@{}", username.trim_start_matches('@')))
        })
        .unwrap_or_else(|| "contact".to_string())
}

fn telegram_cached_contact_name(
    payload: &Value,
    chat_kind: &str,
    peer_metadata: &HashMap<String, TelegramPeerMetadata>,
) -> Option<String> {
    let contact_id = telegram_contact_id(payload, chat_kind)?;
    peer_metadata
        .get(&contact_id)
        .and_then(|metadata| metadata.name.clone())
}

fn telegram_cached_contact_avatar(
    payload: &Value,
    chat_kind: &str,
    peer_metadata: &HashMap<String, TelegramPeerMetadata>,
) -> Option<String> {
    let contact_id = telegram_contact_id(payload, chat_kind)?;
    peer_metadata
        .get(&contact_id)
        .and_then(|metadata| metadata.avatar.clone())
}

/// Reads cached Telegram peer display names from all configured accounts.
pub(super) fn read_telegram_peer_names(paths: &ConfigPaths) -> HashMap<String, String> {
    read_telegram_peer_metadata(paths)
        .into_iter()
        .filter_map(|(id, metadata)| metadata.name.map(|name| (id, name)))
        .collect()
}

/// Reads cached Telegram peer avatars from all configured accounts.
pub(super) fn read_telegram_peer_avatars(paths: &ConfigPaths) -> HashMap<String, String> {
    read_telegram_peer_metadata(paths)
        .into_iter()
        .filter_map(|(id, metadata)| metadata.avatar.map(|avatar| (id, avatar)))
        .collect()
}

fn read_telegram_peer_metadata(paths: &ConfigPaths) -> HashMap<String, TelegramPeerMetadata> {
    let root = paths.user_config_dir.join("telegram-accounts");
    let Ok(entries) = std::fs::read_dir(root) else {
        return HashMap::new();
    };
    let mut metadata = HashMap::new();
    for entry in entries.flatten() {
        for (id, peer_metadata) in read_telegram_peer_metadata_from_account(&entry.path()) {
            merge_peer_metadata(&mut metadata, id, peer_metadata);
        }
    }
    metadata
}

fn read_telegram_peer_metadata_from_account(
    account_dir: &Path,
) -> HashMap<String, TelegramPeerMetadata> {
    let path = account_dir.join("peer-cache.json");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return HashMap::new();
    };
    let Ok(cache) = serde_json::from_str::<Value>(&raw) else {
        return HashMap::new();
    };
    let Some(peers) = cache.get("peers").and_then(Value::as_array) else {
        return HashMap::new();
    };
    let mut metadata = HashMap::new();
    for peer in peers {
        let Ok(peer) = serde_json::from_value::<TelegramPeerCacheEntry>(peer.clone()) else {
            continue;
        };
        if peer.kind != "user" || peer.is_bot {
            continue;
        }
        let peer_metadata = TelegramPeerMetadata {
            name: peer_cache_entry_name(&peer),
            avatar: peer_cache_entry_avatar(&peer),
        };
        if peer_metadata.name.is_none() && peer_metadata.avatar.is_none() {
            continue;
        }
        for id in peer_cache_entry_contact_ids(&peer) {
            merge_peer_metadata(&mut metadata, id, peer_metadata.clone());
        }
    }
    metadata
}

fn merge_peer_metadata(
    metadata: &mut HashMap<String, TelegramPeerMetadata>,
    id: String,
    candidate: TelegramPeerMetadata,
) {
    let entry = metadata.entry(id).or_default();
    if let Some(name) = candidate.name {
        if telegram_name_is_more_complete(entry.name.as_deref(), &name) {
            entry.name = Some(name);
        }
    }
    if entry.avatar.is_none() {
        entry.avatar = candidate.avatar;
    }
}

fn peer_cache_entry_name(peer: &TelegramPeerCacheEntry) -> Option<String> {
    let full_name = [peer.first_name.as_deref(), peer.last_name.as_deref()]
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let mut name = peer
        .title
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    if !full_name.is_empty() && telegram_name_is_more_complete(name.as_deref(), &full_name) {
        name = Some(full_name);
    }
    name
}

fn peer_cache_entry_avatar(peer: &TelegramPeerCacheEntry) -> Option<String> {
    peer.avatar
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn peer_cache_entry_usernames(peer: &TelegramPeerCacheEntry) -> Vec<String> {
    let mut usernames = BTreeSet::new();
    if let Some(username) = peer.username.as_deref() {
        usernames.insert(username.trim().trim_start_matches('@').to_string());
    }
    for username in &peer.usernames {
        usernames.insert(username.trim().trim_start_matches('@').to_string());
    }
    usernames
        .into_iter()
        .filter(|value| !value.is_empty())
        .collect()
}

fn peer_cache_entry_contact_ids(peer: &TelegramPeerCacheEntry) -> Vec<String> {
    let mut ids = Vec::new();
    for username in peer_cache_entry_usernames(peer) {
        if let Some(id) = normalize_contact_id(&format!("telegram@{username}")) {
            ids.push(id);
        }
    }
    if ids.is_empty() && peer.numeric_id > 0 {
        if let Some(id) = normalize_contact_id(&format!("telegram-user-id@{}", peer.numeric_id)) {
            ids.push(id);
        }
    }
    ids
}

fn telegram_payload_user_id(payload: &Value) -> Option<i64> {
    payload
        .get("chat_id")
        .and_then(Value::as_i64)
        .or_else(|| payload.get("sender_id").and_then(Value::as_i64))
        .filter(|id| *id > 0)
}

fn telegram_contact_username(payload: &Value, chat_kind: &str) -> Option<String> {
    let key = if chat_kind == "user" {
        "chat_username"
    } else {
        "sender_username"
    };
    payload
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn name_should_include_handle(name: &str, username: &str) -> bool {
    let trimmed = name.trim();
    if trimmed.split_whitespace().count() > 1 {
        return false;
    }
    let normalized_name = normalize_name_token(trimmed);
    let normalized_username = normalize_name_token(username.trim_start_matches('@'));
    !normalized_name.is_empty() && normalized_name != normalized_username
}

fn normalize_name_token(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn merge_telegram_name(existing: &mut Option<String>, candidate: &Option<String>) {
    let Some(candidate) = candidate
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return;
    };
    if telegram_name_is_more_complete(existing.as_deref(), candidate) {
        *existing = Some(candidate.to_string());
    }
}

fn telegram_name_is_more_complete(existing: Option<&str>, candidate: &str) -> bool {
    let Some(existing) = existing.map(str::trim).filter(|value| !value.is_empty()) else {
        return true;
    };
    let existing_parts = existing.split_whitespace().count();
    let candidate_parts = candidate.split_whitespace().count();
    candidate_parts > existing_parts
        || (candidate_parts == existing_parts && candidate.len() > existing.len())
}

fn payload_username_looks_like_bot(payload: &Value, key: &str) -> bool {
    payload
        .get(key)
        .and_then(Value::as_str)
        .is_some_and(telegram_username_looks_like_bot)
}

fn telegram_username_looks_like_bot(username: &str) -> bool {
    username.to_ascii_lowercase().ends_with("bot")
}

fn telegram_session_user_id(path: &Path) -> Option<i64> {
    Session::load_file(path)
        .ok()?
        .get_user()
        .map(|user| user.id)
}

fn telegram_payload_is_self_contact(
    chat_kind: &str,
    chat_id: i64,
    sender_id: Option<i64>,
    is_outgoing: bool,
    self_user_id: Option<i64>,
) -> bool {
    if chat_kind == "group" && is_outgoing {
        return true;
    }
    let Some(self_user_id) = self_user_id else {
        return false;
    };
    if chat_kind == "group" {
        return sender_id == Some(self_user_id);
    }
    chat_kind == "user" && chat_id == self_user_id
}

fn acceleration(score: &TelegramScore) -> f64 {
    let first = score.first_half_count.max(1) as f64;
    let second = score.second_half_count.max(1) as f64;
    (second / first).clamp(0.25, 4.0)
}

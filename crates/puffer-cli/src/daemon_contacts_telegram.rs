//! Telegram diagnostics-backed contact ranking.

use super::{
    days_since, entropy_score, push_context, sort_context, Candidate, TELEGRAM_CONTEXT_LIMIT,
};
use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use puffer_subscriptions::{normalize_contact_id, ContactContext, TELEGRAM_CONTACT_PREFIX};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

const DEFAULT_LIMIT: usize = 30;
const DAY_MS: i128 = 86_400_000;

#[derive(Debug, Clone)]
pub(super) struct TelegramDiagMessage {
    pub(super) contact_id: String,
    pub(super) name: Option<String>,
    pub(super) chat_id: i64,
    pub(super) chat_kind: String,
    pub(super) sender_username: Option<String>,
    pub(super) is_outgoing: bool,
    pub(super) date_ms: i128,
    pub(super) message_id: Option<i64>,
    pub(super) reply_to_message_id: Option<i64>,
    pub(super) text: String,
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

pub(super) fn collect_telegram_candidates(
    paths: &ConfigPaths,
    by_id: &mut HashMap<String, Candidate>,
) -> Result<()> {
    let root = paths.user_config_dir.join("telegram-accounts");
    if !root.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(&root).with_context(|| format!("read {}", root.display()))? {
        let Ok(entry) = entry else { continue };
        let path = entry.path().join("message-diagnostics.ndjson");
        if !path.exists() {
            continue;
        }
        let messages = read_telegram_messages(&path)?;
        let ranked = rank_telegram_messages(messages);
        for (id, candidate) in ranked {
            by_id
                .entry(id)
                .and_modify(|existing| {
                    existing.score += candidate.score;
                    if existing.name.is_none() {
                        existing.name = candidate.name.clone();
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

fn read_telegram_messages(path: &Path) -> Result<Vec<TelegramDiagMessage>> {
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
        messages.push(TelegramDiagMessage {
            contact_id,
            name: telegram_contact_name(&payload, &chat_kind),
            chat_id: payload.get("chat_id").and_then(Value::as_i64).unwrap_or(0),
            chat_kind,
            sender_username: payload
                .get("sender_username")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            is_outgoing: payload
                .get("is_outgoing")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            date_ms: payload
                .get("date_ms")
                .and_then(Value::as_i64)
                .map(i128::from)
                .unwrap_or(0),
            message_id: payload.get("message_id").and_then(Value::as_i64),
            reply_to_message_id: payload
                .pointer("/reply_to/message_id")
                .and_then(Value::as_i64),
            text,
            payload,
        });
    }
    messages.sort_by_key(|message| message.date_ms);
    Ok(messages)
}

fn rank_telegram_messages(messages: Vec<TelegramDiagMessage>) -> BTreeMap<String, Candidate> {
    if messages.is_empty() {
        return BTreeMap::new();
    }
    let mut considered = 1000usize.min(messages.len());
    loop {
        let start = messages.len().saturating_sub(considered);
        let result = score_telegram_window(&messages[start..]);
        if result.len() >= DEFAULT_LIMIT || considered == messages.len() {
            return result;
        }
        considered = (considered + 1000).min(messages.len());
    }
}

pub(super) fn score_telegram_window(
    messages: &[TelegramDiagMessage],
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
        let entry = candidates
            .entry(message.contact_id.clone())
            .or_insert_with(|| Candidate {
                id: message.contact_id.clone(),
                name: message.name.clone(),
                avatar: None,
                score: 0.0,
                context: Vec::new(),
            });
        push_context(
            entry,
            ContactContext {
                kind: "telegram_message".to_string(),
                text: message.text.clone(),
                timestamp_ms: Some(message.date_ms),
                payload: message.payload.clone(),
            },
            TELEGRAM_CONTEXT_LIMIT,
        );
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
            sort_context(&mut candidate.context, TELEGRAM_CONTEXT_LIMIT);
        }
    }
    candidates
}

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

pub(super) fn telegram_contact_id(payload: &Value, chat_kind: &str) -> Option<String> {
    if chat_kind != "user" || payload.get("chat_is_bot").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    payload
        .get("chat_username")
        .and_then(Value::as_str)
        .and_then(|username| telegram_prefixed_id(TELEGRAM_CONTACT_PREFIX, username))
}

fn telegram_prefixed_id(prefix: &str, suffix: &str) -> Option<String> {
    normalize_contact_id(&format!("{prefix}@{suffix}"))
}

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

fn acceleration(score: &TelegramScore) -> f64 {
    let first = score.first_half_count.max(1) as f64;
    let second = score.second_half_count.max(1) as f64;
    (second / first).clamp(0.25, 4.0)
}

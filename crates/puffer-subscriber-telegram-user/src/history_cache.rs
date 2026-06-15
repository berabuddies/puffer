//! Durable bounded Telegram server-history cache.
//!
//! Monitor reply quality needs recent same-chat context, but the monitor
//! trigger path must not call Telegram. The subscriber owns MTProto access, so
//! it keeps a small local cache of recent server-history messages that the
//! daemon can read synchronously when it builds monitor prompts.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use grammers_client::types::{Chat, Message};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::state::SkillEnv;

const CACHE_VERSION: u32 = 1;
pub const DEFAULT_HISTORY_MESSAGES_PER_CHAT: usize = 32;
const TELEGRAM_SERVICE_CHAT_ID: i64 = 777000;

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct TelegramHistoryCache {
    #[serde(default = "cache_version")]
    pub version: u32,
    #[serde(default = "telegram_server_history_source")]
    pub source: String,
    #[serde(default = "default_limit_per_chat")]
    pub limit_per_chat: usize,
    #[serde(default)]
    pub chats: Vec<TelegramHistoryChat>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Serialize)]
pub struct TelegramHistoryChat {
    pub chat_id: i64,
    pub chat_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_username: Option<String>,
    #[serde(default)]
    pub updated_at_ms: i64,
    #[serde(default)]
    pub messages: Vec<TelegramHistoryMessage>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Serialize)]
pub struct TelegramHistoryMessage {
    pub message_id: i32,
    pub date_ms: i64,
    #[serde(default)]
    pub is_outgoing: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_name: Option<String>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TelegramHistoryContextMessage {
    pub from: String,
    pub direction: String,
    pub sender: TelegramHistoryContextSender,
    pub chat: TelegramHistoryContextChat,
    pub message_id: i32,
    pub date_ms: i64,
    pub ts: i64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TelegramHistoryContextSender {
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    pub is_user: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TelegramHistoryContextChat {
    pub id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

impl Default for TelegramHistoryCache {
    fn default() -> Self {
        Self {
            version: CACHE_VERSION,
            source: telegram_server_history_source(),
            limit_per_chat: DEFAULT_HISTORY_MESSAGES_PER_CHAT,
            chats: Vec::new(),
        }
    }
}

impl TelegramHistoryCache {
    pub fn load_path(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("read Telegram history cache {}", path.display()))?;
        if raw.trim().is_empty() {
            return Ok(Self::default());
        }
        let mut cache: Self = serde_json::from_str(&raw)
            .with_context(|| format!("parse Telegram history cache {}", path.display()))?;
        cache.version = CACHE_VERSION;
        if cache.source.trim().is_empty() {
            cache.source = telegram_server_history_source();
        }
        if cache.limit_per_chat == 0 {
            cache.limit_per_chat = DEFAULT_HISTORY_MESSAGES_PER_CHAT;
        }
        Ok(cache)
    }

    pub(crate) fn load(env: &SkillEnv) -> anyhow::Result<Self> {
        Self::load_path(&env.history_cache_path())
    }

    pub(crate) fn save_if_changed(
        &self,
        env: &SkillEnv,
        original: &TelegramHistoryCache,
    ) -> anyhow::Result<()> {
        if self == original {
            return Ok(());
        }
        self.save(env)
    }

    pub(crate) fn save_merged_with_current(&self, env: &SkillEnv) -> anyhow::Result<()> {
        let original = match Self::load(env) {
            Ok(cache) => cache,
            Err(error) => {
                warn!(
                    error = %error,
                    "failed to load latest Telegram history cache before merged save; rebuilding from startup cache"
                );
                Self::default()
            }
        };
        let mut merged = original.clone();
        merged.merge_from(self);
        merged.save_if_changed(env, &original)
    }

    pub(crate) fn merge_from(&mut self, other: &TelegramHistoryCache) -> bool {
        let mut changed = false;
        if self.source.trim().is_empty() {
            self.source = telegram_server_history_source();
            changed = true;
        }
        self.limit_per_chat = self.limit_per_chat.max(other.limit_per_chat.max(1));
        for chat in &other.chats {
            if chat.chat_kind != "user" {
                continue;
            }
            for message in &chat.messages {
                changed |= self.merge_message(
                    chat.chat_id,
                    &chat.chat_kind,
                    chat.chat_title.clone(),
                    chat.chat_username.clone(),
                    message.clone(),
                    self.limit_per_chat,
                );
            }
        }
        changed
    }

    pub(crate) fn observe_message(&mut self, message: &Message) -> bool {
        let chat = message.chat();
        if !matches!(chat, Chat::User(_)) {
            return false;
        }
        if chat.id() == TELEGRAM_SERVICE_CHAT_ID
            || telegram_chat_is_bot(&chat)
            || message.sender().as_ref().is_some_and(telegram_chat_is_bot)
        {
            return false;
        }
        let Some(text) = nonempty(message.text()) else {
            return false;
        };
        let (chat_title, chat_username) = describe_chat(&chat);
        let sender = message.sender();
        let record = TelegramHistoryMessage {
            message_id: message.id(),
            date_ms: message.date().timestamp_millis(),
            is_outgoing: message.outgoing(),
            sender_id: sender.as_ref().map(Chat::id),
            sender_username: sender
                .as_ref()
                .and_then(|sender| sender.username())
                .and_then(nonempty),
            sender_name: sender
                .as_ref()
                .map(chat_display_name)
                .and_then(|name| nonempty(&name)),
            text,
        };
        self.merge_message(
            chat.id(),
            "user",
            chat_title,
            chat_username,
            record,
            self.limit_per_chat,
        )
    }

    pub fn merge_message(
        &mut self,
        chat_id: i64,
        chat_kind: &str,
        chat_title: Option<String>,
        chat_username: Option<String>,
        message: TelegramHistoryMessage,
        limit_per_chat: usize,
    ) -> bool {
        if chat_kind != "user" || message.text.trim().is_empty() {
            return false;
        }
        let limit_per_chat = limit_per_chat.max(1);
        self.limit_per_chat = self.limit_per_chat.max(limit_per_chat);
        let Some(chat) = self.chats.iter_mut().find(|chat| chat.chat_id == chat_id) else {
            self.chats.push(TelegramHistoryChat {
                chat_id,
                chat_kind: chat_kind.to_string(),
                chat_title,
                chat_username,
                updated_at_ms: now_unix_millis(),
                messages: vec![message],
            });
            self.sort_chats();
            return true;
        };

        let mut changed = merge_optional_fill(&mut chat.chat_title, chat_title);
        changed |= merge_optional_fill(&mut chat.chat_username, chat_username);
        if chat.chat_kind != chat_kind {
            chat.chat_kind = chat_kind.to_string();
            changed = true;
        }
        changed |= upsert_message(&mut chat.messages, message);
        chat.messages
            .sort_by_key(|message| (message.date_ms, message.message_id));
        let trim_count = chat.messages.len().saturating_sub(limit_per_chat);
        if trim_count > 0 {
            chat.messages.drain(0..trim_count);
            changed = true;
        }
        if changed {
            chat.updated_at_ms = now_unix_millis();
        }
        changed
    }

    pub fn prior_context_messages(
        &self,
        chat_id: i64,
        current_message_id: Option<i64>,
        current_date_ms: Option<i64>,
        limit: usize,
    ) -> Vec<TelegramHistoryContextMessage> {
        if limit == 0 {
            return Vec::new();
        }
        let Some(chat) = self
            .chats
            .iter()
            .find(|chat| chat.chat_id == chat_id && chat.chat_kind == "user")
        else {
            return Vec::new();
        };
        let mut messages = chat
            .messages
            .iter()
            .filter(|message| {
                if current_message_id == Some(i64::from(message.message_id)) {
                    return false;
                }
                history_message_precedes_current(
                    message.date_ms,
                    i64::from(message.message_id),
                    current_date_ms,
                    current_message_id,
                )
            })
            .cloned()
            .collect::<Vec<_>>();
        messages.sort_by_key(|message| (message.date_ms, message.message_id));
        let start = messages.len().saturating_sub(limit);
        messages
            .into_iter()
            .skip(start)
            .map(|message| context_message_from_cache(chat, message))
            .collect()
    }

    fn save(&self, env: &SkillEnv) -> anyhow::Result<()> {
        let path = env.history_cache_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("create Telegram history cache parent {}", parent.display())
            })?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)
            .with_context(|| format!("write {}", tmp.display()))?;
        std::fs::rename(&tmp, &path)
            .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))
    }

    fn sort_chats(&mut self) {
        self.chats.sort_by_key(|chat| chat.chat_id);
    }
}

fn upsert_message(
    messages: &mut Vec<TelegramHistoryMessage>,
    candidate: TelegramHistoryMessage,
) -> bool {
    let Some(existing) = messages
        .iter_mut()
        .find(|message| message.message_id == candidate.message_id)
    else {
        messages.push(candidate);
        return true;
    };
    if *existing == candidate {
        return false;
    }
    *existing = candidate;
    true
}

fn context_message_from_cache(
    chat: &TelegramHistoryChat,
    message: TelegramHistoryMessage,
) -> TelegramHistoryContextMessage {
    let direction = if message.is_outgoing {
        "outgoing"
    } else {
        "incoming"
    };
    let from = if message.is_outgoing { "me" } else { "them" };
    let label = message
        .sender_name
        .clone()
        .or_else(|| chat.chat_title.clone())
        .unwrap_or_else(|| {
            if message.is_outgoing {
                "me".to_string()
            } else {
                "sender".to_string()
            }
        });
    TelegramHistoryContextMessage {
        from: from.to_string(),
        direction: direction.to_string(),
        sender: TelegramHistoryContextSender {
            label,
            username: message.sender_username,
            is_user: message.is_outgoing,
        },
        chat: TelegramHistoryContextChat {
            id: chat.chat_id,
            title: chat.chat_title.clone(),
        },
        message_id: message.message_id,
        date_ms: message.date_ms,
        ts: message.date_ms,
        text: message.text,
    }
}

fn history_message_precedes_current(
    date_ms: i64,
    message_id: i64,
    current_date_ms: Option<i64>,
    current_message_id: Option<i64>,
) -> bool {
    if let Some(current_date_ms) = current_date_ms {
        return date_ms < current_date_ms
            || (date_ms == current_date_ms
                && current_message_id.is_some_and(|current| message_id < current));
    }
    current_message_id.map_or(true, |current| message_id < current)
}

fn describe_chat(chat: &Chat) -> (Option<String>, Option<String>) {
    match chat {
        Chat::User(_) => (
            Some(chat_display_name(chat)),
            chat.username().and_then(nonempty),
        ),
        Chat::Group(_) | Chat::Channel(_) => (
            Some(chat.name().to_string()),
            chat.username().and_then(nonempty),
        ),
    }
}

fn chat_display_name(chat: &Chat) -> String {
    match chat {
        Chat::User(user) => user.full_name(),
        Chat::Group(_) | Chat::Channel(_) => chat.name().to_string(),
    }
}

fn telegram_chat_is_bot(chat: &Chat) -> bool {
    matches!(chat, Chat::User(user) if user.raw.bot)
        || chat
            .username()
            .is_some_and(telegram_username_looks_like_bot)
}

fn telegram_username_looks_like_bot(username: &str) -> bool {
    username.to_ascii_lowercase().ends_with("bot")
}

fn nonempty(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn merge_optional_fill(existing: &mut Option<String>, candidate: Option<String>) -> bool {
    if existing
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_none()
    {
        let changed = *existing != candidate;
        *existing = candidate;
        return changed;
    }
    false
}

fn cache_version() -> u32 {
    CACHE_VERSION
}

fn default_limit_per_chat() -> usize {
    DEFAULT_HISTORY_MESSAGES_PER_CHAT
}

fn telegram_server_history_source() -> String {
    "telegram_server_history".to_string()
}

fn now_unix_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_keeps_bounded_ordered_direct_message_history() {
        let mut cache = TelegramHistoryCache::default();
        for id in 1..=4 {
            cache.merge_message(
                42,
                "user",
                Some("Chaofan".to_string()),
                None,
                TelegramHistoryMessage {
                    message_id: id,
                    date_ms: i64::from(id) * 100,
                    is_outgoing: id == 2,
                    sender_name: Some(if id == 2 { "Me" } else { "Chaofan" }.to_string()),
                    text: format!("message {id}"),
                    ..Default::default()
                },
                3,
            );
        }

        let chat = cache.chats.iter().find(|chat| chat.chat_id == 42).unwrap();
        assert_eq!(
            chat.messages
                .iter()
                .map(|message| message.message_id)
                .collect::<Vec<_>>(),
            vec![2, 3, 4]
        );
        let context = cache.prior_context_messages(42, Some(4), Some(400), 8);
        assert_eq!(
            context
                .iter()
                .map(|message| message.text.as_str())
                .collect::<Vec<_>>(),
            vec!["message 2", "message 3"]
        );
        assert_eq!(context[0].from, "me");
        assert_eq!(context[1].from, "them");
    }

    #[test]
    fn cache_ignores_non_user_chats_for_monitor_context() {
        let mut cache = TelegramHistoryCache::default();
        assert!(!cache.merge_message(
            -10042,
            "group",
            Some("Group".to_string()),
            None,
            TelegramHistoryMessage {
                message_id: 1,
                date_ms: 100,
                text: "group context".to_string(),
                ..Default::default()
            },
            8,
        ));

        assert!(cache
            .prior_context_messages(-10042, Some(2), Some(200), 8)
            .is_empty());
    }

    #[test]
    fn cache_duplicate_message_is_not_a_dirty_write() {
        let mut cache = TelegramHistoryCache {
            chats: vec![TelegramHistoryChat {
                chat_id: 42,
                chat_kind: "user".to_string(),
                chat_title: Some("Chaofan".to_string()),
                updated_at_ms: 123,
                messages: vec![TelegramHistoryMessage {
                    message_id: 10,
                    date_ms: 1_000,
                    sender_name: Some("Chaofan".to_string()),
                    text: "server history".to_string(),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };

        assert!(!cache.merge_message(
            42,
            "user",
            Some("Chaofan".to_string()),
            None,
            TelegramHistoryMessage {
                message_id: 10,
                date_ms: 1_000,
                sender_name: Some("Chaofan".to_string()),
                text: "server history".to_string(),
                ..Default::default()
            },
            8,
        ));
        assert_eq!(cache.chats[0].updated_at_ms, 123);
    }

    #[test]
    fn cache_loads_fixture_written_by_subscriber() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("telegram-history-cache.json");
        std::fs::write(
            &path,
            serde_json::to_vec_pretty(&TelegramHistoryCache {
                chats: vec![TelegramHistoryChat {
                    chat_id: 42,
                    chat_kind: "user".to_string(),
                    chat_title: Some("Chaofan".to_string()),
                    messages: vec![TelegramHistoryMessage {
                        message_id: 10,
                        date_ms: 1_000,
                        text: "server history".to_string(),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            })
            .unwrap(),
        )
        .unwrap();

        let cache = TelegramHistoryCache::load_path(&path).unwrap();

        assert_eq!(cache.source, "telegram_server_history");
        assert_eq!(
            cache.prior_context_messages(42, Some(11), Some(2_000), 8)[0].text,
            "server history"
        );
    }

    #[test]
    fn cache_final_startup_save_preserves_resume_writes_on_disk() {
        let temp = tempfile::tempdir().unwrap();
        let env = SkillEnv {
            state_dir: temp.path().to_path_buf(),
            session_path: temp.path().join("telegram.session"),
            topic: "telegram-user".to_string(),
            workspace_config_dir: None,
            live_session_path: None,
        };
        TelegramHistoryCache {
            chats: vec![TelegramHistoryChat {
                chat_id: 200,
                chat_kind: "user".to_string(),
                chat_title: Some("Resume Only".to_string()),
                messages: vec![TelegramHistoryMessage {
                    message_id: 1,
                    date_ms: 1_000,
                    text: "arrived during startup resume".to_string(),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }
        .save(&env)
        .unwrap();
        let startup_backfill_cache = TelegramHistoryCache {
            chats: vec![TelegramHistoryChat {
                chat_id: 42,
                chat_kind: "user".to_string(),
                chat_title: Some("Backfilled".to_string()),
                messages: vec![TelegramHistoryMessage {
                    message_id: 10,
                    date_ms: 2_000,
                    text: "startup server history".to_string(),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };

        startup_backfill_cache
            .save_merged_with_current(&env)
            .unwrap();
        let saved = TelegramHistoryCache::load(&env).unwrap();

        assert_eq!(
            saved
                .prior_context_messages(200, Some(2), Some(2_000), 8)
                .into_iter()
                .map(|message| message.text)
                .collect::<Vec<_>>(),
            vec!["arrived during startup resume"]
        );
        assert_eq!(
            saved
                .prior_context_messages(42, Some(11), Some(3_000), 8)
                .into_iter()
                .map(|message| message.text)
                .collect::<Vec<_>>(),
            vec!["startup server history"]
        );
    }
}

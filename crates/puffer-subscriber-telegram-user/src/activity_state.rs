//! Subscriber-owned Telegram activity state used by monitor TaskCreate gates.
//!
//! This file intentionally stores only ids, directions, reply links, and
//! timestamps. Message text stays in the bounded history cache used for prompt
//! context; the task gate only needs metadata.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use grammers_client::types::{Chat, Message};
use serde::{Deserialize, Serialize};

use crate::history_cache::DEFAULT_HISTORY_MESSAGES_PER_CHAT;
use crate::state::SkillEnv;

const ACTIVITY_VERSION: u32 = 1;
const ACTIVITY_MESSAGES_PER_CHAT: usize = DEFAULT_HISTORY_MESSAGES_PER_CHAT * 4;
const TELEGRAM_SERVICE_CHAT_ID: i64 = 777000;

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub(crate) struct TelegramActivityState {
    #[serde(default = "activity_version")]
    pub version: u32,
    #[serde(default = "activity_source")]
    pub source: String,
    #[serde(default)]
    pub updated_at_ms: i64,
    #[serde(default)]
    pub chats: Vec<TelegramActivityChat>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Serialize)]
pub(crate) struct TelegramActivityChat {
    pub chat_id: i64,
    pub chat_kind: String,
    #[serde(default)]
    pub updated_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_inbox_max_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_inbox_updated_at_ms: Option<i64>,
    #[serde(default)]
    pub agent_sent_message_ids: Vec<i64>,
    #[serde(default)]
    pub messages: Vec<TelegramActivityMessage>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Serialize)]
pub(crate) struct TelegramActivityMessage {
    pub message_id: i64,
    #[serde(default)]
    pub date_ms: i64,
    #[serde(default)]
    pub is_outgoing: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to_message_id: Option<i64>,
    #[serde(default)]
    pub agent_originated: bool,
}

impl Default for TelegramActivityState {
    fn default() -> Self {
        Self {
            version: ACTIVITY_VERSION,
            source: activity_source(),
            updated_at_ms: now_unix_millis(),
            chats: Vec::new(),
        }
    }
}

impl TelegramActivityState {
    pub(crate) fn load(env: &SkillEnv) -> anyhow::Result<Self> {
        Self::load_path(&env.activity_state_path())
    }

    pub(crate) fn load_path(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("read Telegram activity state {}", path.display()))?;
        if raw.trim().is_empty() {
            return Ok(Self::default());
        }
        let mut state: Self = serde_json::from_str(&raw)
            .with_context(|| format!("parse Telegram activity state {}", path.display()))?;
        state.version = ACTIVITY_VERSION;
        if state.source.trim().is_empty() {
            state.source = activity_source();
        }
        Ok(state)
    }

    pub(crate) fn save_if_changed(
        &self,
        env: &SkillEnv,
        original: &TelegramActivityState,
    ) -> anyhow::Result<()> {
        if self == original {
            return Ok(());
        }
        self.save(env)
    }

    pub(crate) fn observe_message(&mut self, message: &Message) -> bool {
        let chat = message.chat();
        if !matches!(chat, Chat::User(_)) || ignored_chat_or_sender(message) {
            return false;
        }
        self.merge_message(
            chat.id(),
            "user",
            TelegramActivityMessage {
                message_id: i64::from(message.id()),
                date_ms: message.date().timestamp_millis(),
                is_outgoing: message.outgoing(),
                reply_to_message_id: message.reply_to_message_id().map(i64::from),
                agent_originated: false,
            },
            ACTIVITY_MESSAGES_PER_CHAT,
        )
    }

    pub(crate) fn record_agent_send(
        &mut self,
        chat_id: i64,
        message_id: i64,
        reply_to_message_id: Option<i64>,
        date_ms: i64,
    ) -> bool {
        let mut changed = self.merge_message(
            chat_id,
            "user",
            TelegramActivityMessage {
                message_id,
                date_ms,
                is_outgoing: true,
                reply_to_message_id,
                agent_originated: true,
            },
            ACTIVITY_MESSAGES_PER_CHAT,
        );
        let chat = self.ensure_chat(chat_id, "user");
        if !chat.agent_sent_message_ids.contains(&message_id) {
            chat.agent_sent_message_ids.push(message_id);
            chat.agent_sent_message_ids.sort_unstable();
            chat.agent_sent_message_ids.dedup();
            trim_i64_vec(&mut chat.agent_sent_message_ids, ACTIVITY_MESSAGES_PER_CHAT);
            chat.updated_at_ms = now_unix_millis();
            changed = true;
        }
        if changed {
            self.updated_at_ms = now_unix_millis();
        }
        changed
    }

    pub(crate) fn record_read_inbox(&mut self, chat_id: i64, max_id: i64) -> bool {
        let chat = self.ensure_chat(chat_id, "user");
        if chat
            .read_inbox_max_id
            .is_some_and(|existing| existing >= max_id)
        {
            return false;
        }
        let now = now_unix_millis();
        chat.read_inbox_max_id = Some(max_id);
        chat.read_inbox_updated_at_ms = Some(now);
        chat.updated_at_ms = now;
        self.updated_at_ms = now;
        true
    }

    fn merge_message(
        &mut self,
        chat_id: i64,
        chat_kind: &str,
        message: TelegramActivityMessage,
        limit_per_chat: usize,
    ) -> bool {
        if chat_kind != "user" {
            return false;
        }
        let chat = self.ensure_chat(chat_id, chat_kind);
        let changed = upsert_message(&mut chat.messages, message);
        chat.messages
            .sort_by_key(|message| (message.date_ms, message.message_id));
        let trim_count = chat.messages.len().saturating_sub(limit_per_chat.max(1));
        if trim_count > 0 {
            chat.messages.drain(0..trim_count);
        }
        if changed || trim_count > 0 {
            let now = now_unix_millis();
            chat.updated_at_ms = now;
            self.updated_at_ms = now;
            return true;
        }
        false
    }

    fn ensure_chat(&mut self, chat_id: i64, chat_kind: &str) -> &mut TelegramActivityChat {
        if let Some(index) = self.chats.iter().position(|chat| chat.chat_id == chat_id) {
            let chat = &mut self.chats[index];
            if chat.chat_kind != chat_kind {
                chat.chat_kind = chat_kind.to_string();
                chat.updated_at_ms = now_unix_millis();
            }
            return chat;
        }
        self.chats.push(TelegramActivityChat {
            chat_id,
            chat_kind: chat_kind.to_string(),
            updated_at_ms: now_unix_millis(),
            ..Default::default()
        });
        self.chats.sort_by_key(|chat| chat.chat_id);
        self.chats
            .iter_mut()
            .find(|chat| chat.chat_id == chat_id)
            .expect("chat inserted")
    }

    fn save(&self, env: &SkillEnv) -> anyhow::Result<()> {
        let path = env.activity_state_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("create Telegram activity state parent {}", parent.display())
            })?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)
            .with_context(|| format!("write {}", tmp.display()))?;
        std::fs::rename(&tmp, &path)
            .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))
    }
}

pub(crate) fn record_message_activity(env: &SkillEnv, message: &Message) -> anyhow::Result<()> {
    let original = TelegramActivityState::load(env).unwrap_or_default();
    let mut state = original.clone();
    state.observe_message(message);
    state.save_if_changed(env, &original)
}

pub(crate) fn record_agent_send_activity(
    env: &SkillEnv,
    chat_id: i64,
    message_id: i64,
    reply_to_message_id: Option<i64>,
    date_ms: i64,
) -> anyhow::Result<()> {
    let original = TelegramActivityState::load(env).unwrap_or_default();
    let mut state = original.clone();
    state.record_agent_send(chat_id, message_id, reply_to_message_id, date_ms);
    state.save_if_changed(env, &original)
}

pub(crate) fn record_read_inbox_activity(
    env: &SkillEnv,
    chat_id: i64,
    max_id: i64,
) -> anyhow::Result<()> {
    let original = TelegramActivityState::load(env).unwrap_or_default();
    let mut state = original.clone();
    state.record_read_inbox(chat_id, max_id);
    state.save_if_changed(env, &original)
}

fn upsert_message(
    messages: &mut Vec<TelegramActivityMessage>,
    mut candidate: TelegramActivityMessage,
) -> bool {
    let Some(existing) = messages
        .iter_mut()
        .find(|message| message.message_id == candidate.message_id)
    else {
        messages.push(candidate);
        return true;
    };
    candidate.agent_originated |= existing.agent_originated;
    if *existing == candidate {
        return false;
    }
    *existing = candidate;
    true
}

fn trim_i64_vec(values: &mut Vec<i64>, limit: usize) {
    let extra = values.len().saturating_sub(limit.max(1));
    if extra > 0 {
        values.drain(0..extra);
    }
}

fn ignored_chat_or_sender(message: &Message) -> bool {
    let chat = message.chat();
    if chat.id() == TELEGRAM_SERVICE_CHAT_ID || telegram_chat_is_bot(&chat) {
        return true;
    }
    message.sender().as_ref().is_some_and(telegram_chat_is_bot)
}

fn telegram_chat_is_bot(chat: &Chat) -> bool {
    matches!(chat, Chat::User(user) if user.raw.bot)
        || chat
            .username()
            .is_some_and(|username| username.to_ascii_lowercase().ends_with("bot"))
}

fn activity_version() -> u32 {
    ACTIVITY_VERSION
}

fn activity_source() -> String {
    "telegram_subscriber_activity".to_string()
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
    fn records_agent_send_id_and_outgoing_reply_metadata() {
        let mut state = TelegramActivityState::default();

        assert!(state.record_agent_send(42, 9001, Some(6836), 1_200));

        let chat = state.chats.iter().find(|chat| chat.chat_id == 42).unwrap();
        assert_eq!(chat.agent_sent_message_ids, vec![9001]);
        assert_eq!(
            chat.messages,
            vec![TelegramActivityMessage {
                message_id: 9001,
                date_ms: 1_200,
                is_outgoing: true,
                reply_to_message_id: Some(6836),
                agent_originated: true,
            }]
        );
    }

    #[test]
    fn read_inbox_tracks_monotonic_max_id() {
        let mut state = TelegramActivityState::default();

        assert!(state.record_read_inbox(42, 10));
        assert!(!state.record_read_inbox(42, 9));
        assert!(state.record_read_inbox(42, 11));

        let chat = state.chats.iter().find(|chat| chat.chat_id == 42).unwrap();
        assert_eq!(chat.read_inbox_max_id, Some(11));
    }

    #[test]
    fn message_upsert_preserves_agent_originated_flag() {
        let mut messages = vec![TelegramActivityMessage {
            message_id: 9001,
            date_ms: 1_200,
            is_outgoing: true,
            reply_to_message_id: Some(6836),
            agent_originated: true,
        }];

        assert!(upsert_message(
            &mut messages,
            TelegramActivityMessage {
                message_id: 9001,
                date_ms: 1_250,
                is_outgoing: true,
                reply_to_message_id: Some(6836),
                agent_originated: false,
            }
        ));

        assert_eq!(
            messages,
            vec![TelegramActivityMessage {
                message_id: 9001,
                date_ms: 1_250,
                is_outgoing: true,
                reply_to_message_id: Some(6836),
                agent_originated: true,
            }]
        );
    }
}

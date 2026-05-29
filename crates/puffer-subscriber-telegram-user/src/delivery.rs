//! Durable delivery cursor for Telegram message events.
//!
//! Telegram's session state tracks the MTProto update position, but Puffer
//! needs its own "last emitted to stdout" cursor so a restart can replay
//! messages that arrived while the subscriber was down instead of trusting
//! Telegram's live update delta alone.

use std::collections::BTreeMap;

use anyhow::Context as _;
use grammers_client::{types::Message, Client};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::events::{build_message_event, emit};
use crate::notifications::NotificationMuteCache;
use crate::state::SkillEnv;

const MAX_CATCH_UP_DIALOGS: usize = 500;
const MAX_CATCH_UP_MESSAGES_PER_CHAT: usize = 5_000;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub(crate) struct DeliveryCursor {
    #[serde(default)]
    initialized: bool,
    #[serde(default)]
    chats: BTreeMap<String, i32>,
}

impl DeliveryCursor {
    pub(crate) fn load(env: &SkillEnv) -> anyhow::Result<Self> {
        let path = env.delivery_cursor_path();
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw =
            std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        if raw.trim().is_empty() {
            return Ok(Self::default());
        }
        serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))
    }

    pub(crate) fn save(&self, env: &SkillEnv) -> anyhow::Result<()> {
        let path = env.delivery_cursor_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create delivery cursor parent {}", parent.display()))?;
        }
        let tmp = path.with_extension("tmp");
        let body = serde_json::to_vec_pretty(self)?;
        std::fs::write(&tmp, body).with_context(|| format!("write {}", tmp.display()))?;
        std::fs::rename(&tmp, &path)
            .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))
    }

    fn is_initialized(&self) -> bool {
        self.initialized
    }

    fn mark_initialized(&mut self) {
        self.initialized = true;
    }

    fn last_id(&self, message: &Message) -> i32 {
        self.chats
            .get(&message_chat_key(message))
            .copied()
            .unwrap_or_default()
    }

    fn is_new(&self, message: &Message) -> bool {
        message.id() > self.last_id(message)
    }

    fn record_seen(&mut self, message: &Message) {
        let key = message_chat_key(message);
        let current = self.chats.entry(key).or_default();
        *current = (*current).max(message.id());
    }
}

pub(crate) async fn catch_up_recent_messages(
    env: &SkillEnv,
    client: &Client,
    cursor: &mut DeliveryCursor,
    notification_mutes: &mut NotificationMuteCache,
) -> anyhow::Result<()> {
    let was_initialized = cursor.is_initialized();
    let mut dialogs = client.iter_dialogs();
    let mut scanned = 0usize;
    let mut emitted = 0usize;

    while scanned < MAX_CATCH_UP_DIALOGS {
        let dialog = match dialogs.next().await {
            Ok(Some(dialog)) => dialog,
            Ok(None) => break,
            Err(error) => {
                warn!(%error, "Telegram catch-up stopped while scanning dialogs");
                break;
            }
        };
        scanned += 1;
        notification_mutes.observe_dialog(&dialog);

        let Some(last_message) = dialog.last_message.as_ref() else {
            continue;
        };
        if !was_initialized {
            cursor.record_seen(last_message);
            continue;
        }
        if !cursor.is_new(last_message) {
            continue;
        }

        let mut pending = Vec::new();
        let mut messages = client
            .iter_messages(dialog.chat().pack())
            .limit(MAX_CATCH_UP_MESSAGES_PER_CHAT);
        loop {
            let message = match messages.next().await {
                Ok(Some(message)) => message,
                Ok(None) => break,
                Err(error) => {
                    warn!(
                        chat = %dialog.chat().id(),
                        %error,
                        "Telegram catch-up skipped chat after message history error",
                    );
                    break;
                }
            };
            if !cursor.is_new(&message) {
                break;
            }
            pending.push(message);
        }
        pending.sort_by_key(Message::id);
        for message in pending {
            if emit_message_if_new(env, client, cursor, notification_mutes, &message).await? {
                emitted += 1;
            }
        }
    }

    if !was_initialized {
        cursor.mark_initialized();
        cursor.save(env)?;
        info!(dialogs = scanned, "initialized Telegram delivery cursor");
    } else if emitted > 0 {
        info!(
            dialogs = scanned,
            messages = emitted,
            "caught up Telegram messages"
        );
    }
    Ok(())
}

pub(crate) async fn emit_message_if_new(
    env: &SkillEnv,
    client: &Client,
    cursor: &mut DeliveryCursor,
    notification_mutes: &mut NotificationMuteCache,
    message: &Message,
) -> anyhow::Result<bool> {
    if !cursor.is_new(message) {
        return Ok(false);
    }
    let notification_muted = notification_mutes.message_chat_muted(client, message).await;
    let notification_silent = message.silent();
    if message_notifications_suppressed(notification_muted, notification_silent) {
        cursor.record_seen(message);
        cursor.save(env)?;
        info!(
            chat = %message.chat().id(),
            message_id = message.id(),
            notification_muted,
            notification_silent,
            "skipped suppressed Telegram message"
        );
        return Ok(false);
    }
    let event = build_message_event(&env.topic, message, notification_muted);
    emit(&event)?;
    cursor.record_seen(message);
    cursor.save(env)?;
    Ok(true)
}

fn message_notifications_suppressed(notification_muted: bool, notification_silent: bool) -> bool {
    notification_muted || notification_silent
}

fn message_chat_key(message: &Message) -> String {
    message.chat().id().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delivery_cursor_defaults_to_uninitialized() {
        let cursor = DeliveryCursor::default();
        assert!(!cursor.is_initialized());
        assert!(cursor.chats.is_empty());
    }

    #[test]
    fn suppressed_notifications_are_not_emitted() {
        assert!(message_notifications_suppressed(true, false));
        assert!(message_notifications_suppressed(false, true));
        assert!(message_notifications_suppressed(true, true));
        assert!(!message_notifications_suppressed(false, false));
    }
}

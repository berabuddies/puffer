//! Live Telegram update adapter and lightweight diagnostics.

use std::time::{SystemTime, UNIX_EPOCH};

use grammers_client::{Client, Update};
use grammers_tl_types as tl;
use serde_json::json;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::activity_state::record_read_inbox_activity;
use crate::delivery::{emit_live_message_if_new, DeliveryCursor};
use crate::notifications::NotificationMuteCache;
use crate::state::SkillEnv;

/// One raw socket update converted for the subscriber runtime loop.
pub(crate) enum LiveUpdateEvent {
    /// A raw update converted into a Grammers high-level update.
    Update {
        /// Converted high-level Grammers update.
        update: Update,
        /// Stable label for the original Telegram raw update kind.
        raw_kind: &'static str,
        /// Subscriber receive timestamp in Unix milliseconds.
        received_at_ms: i128,
    },
    /// A raw update that Grammers could not convert into a high-level update.
    Dropped {
        /// Stable label for the original Telegram raw update kind.
        raw_kind: &'static str,
        /// Subscriber receive timestamp in Unix milliseconds.
        received_at_ms: i128,
    },
    /// Fatal update stream error surfaced to the parent event loop.
    Error(String),
}

/// Spawns the task that reads raw MTProto updates from Grammers.
pub(crate) fn spawn_live_update_task(
    env: SkillEnv,
    client: Client,
) -> (mpsc::UnboundedReceiver<LiveUpdateEvent>, JoinHandle<()>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let handle = tokio::spawn(async move {
        let mut awaiting_logged = false;
        loop {
            if !awaiting_logged {
                let started_at_ms = now_unix_millis();
                append_update_diagnostic(&env, "awaiting_raw", "none", None, started_at_ms);
                awaiting_logged = true;
            }
            let update = match client.next_raw_update().await {
                Ok((raw, chats)) => {
                    let received_at_ms = now_unix_millis();
                    let raw_kind = raw_update_kind(&raw);
                    append_update_diagnostic(&env, "received_raw", raw_kind, None, received_at_ms);
                    awaiting_logged = false;
                    match Update::new(&client, raw, &chats) {
                        Some(update) => {
                            let converted_kind = converted_update_kind(&update);
                            append_update_diagnostic(
                                &env,
                                "converted",
                                raw_kind,
                                Some(converted_kind),
                                received_at_ms,
                            );
                            LiveUpdateEvent::Update {
                                update,
                                raw_kind,
                                received_at_ms,
                            }
                        }
                        None => {
                            append_update_diagnostic(
                                &env,
                                "dropped_unconverted",
                                raw_kind,
                                None,
                                received_at_ms,
                            );
                            LiveUpdateEvent::Dropped {
                                raw_kind,
                                received_at_ms,
                            }
                        }
                    }
                }
                Err(error) => LiveUpdateEvent::Error(error.to_string()),
            };
            if tx.send(update).is_err() {
                break;
            }
        }
    });
    (rx, handle)
}

/// Applies one live update to notification state or message delivery.
pub(crate) async fn handle_live_update(
    env: &SkillEnv,
    delivery_cursor: &mut DeliveryCursor,
    notification_mutes: &mut NotificationMuteCache,
    update: LiveUpdateEvent,
) -> anyhow::Result<()> {
    match update {
        LiveUpdateEvent::Update {
            update: Update::NewMessage(msg),
            raw_kind,
            received_at_ms,
        } => {
            append_update_diagnostic(
                env,
                "handling_new_message",
                raw_kind,
                Some("new_message"),
                received_at_ms,
            );
            emit_live_message_if_new(
                env,
                delivery_cursor,
                notification_mutes,
                &msg,
                Some(received_at_ms),
            )
            .await
            .map(|_| ())?;
        }
        LiveUpdateEvent::Update {
            update: Update::Raw(raw),
            raw_kind,
            received_at_ms,
            ..
        } => {
            append_update_diagnostic(env, "handling_raw", raw_kind, Some("raw"), received_at_ms);
            notification_mutes.apply_raw_update(&raw);
            record_read_history_inbox(env, &raw);
        }
        LiveUpdateEvent::Update {
            raw_kind,
            received_at_ms,
            ..
        } => {
            append_update_diagnostic(
                env,
                "ignored_converted",
                raw_kind,
                Some("non_message"),
                received_at_ms,
            );
        }
        LiveUpdateEvent::Dropped {
            raw_kind,
            received_at_ms,
        } => {
            append_update_diagnostic(env, "ignored_unconverted", raw_kind, None, received_at_ms);
        }
        LiveUpdateEvent::Error(_) => unreachable!("handled before dispatch"),
    }
    Ok(())
}

fn record_read_history_inbox(env: &SkillEnv, update: &tl::enums::Update) {
    let tl::enums::Update::ReadHistoryInbox(update) = update else {
        return;
    };
    let Some(chat_id) = peer_user_chat_id(&update.peer) else {
        return;
    };
    if let Err(error) = record_read_inbox_activity(env, chat_id, i64::from(update.max_id)) {
        tracing::warn!(
            chat = %chat_id,
            max_id = update.max_id,
            %error,
            "failed to record Telegram read inbox activity"
        );
    }
}

fn peer_user_chat_id(peer: &tl::enums::Peer) -> Option<i64> {
    match peer {
        tl::enums::Peer::User(peer) => Some(peer.user_id),
        _ => None,
    }
}

fn append_update_diagnostic(
    env: &SkillEnv,
    stage: &str,
    raw_kind: &str,
    converted_kind: Option<&str>,
    received_at_ms: i128,
) {
    let path = env.state_dir.join("update-diagnostics.ndjson");
    let record = json!({
        "at_ms": now_unix_millis(),
        "stage": stage,
        "raw_kind": raw_kind,
        "converted_kind": converted_kind,
        "received_at_ms": received_at_ms,
    });
    crate::diagnostics::append_ndjson(&path, &record);
}

fn converted_update_kind(update: &Update) -> &'static str {
    match update {
        Update::NewMessage(_) => "new_message",
        Update::MessageEdited(_) => "message_edited",
        Update::MessageDeleted(_) => "message_deleted",
        Update::CallbackQuery(_) => "callback_query",
        Update::InlineQuery(_) => "inline_query",
        Update::InlineSend(_) => "inline_send",
        Update::Raw(_) => "raw",
        _ => "other",
    }
}

fn raw_update_kind(update: &tl::enums::Update) -> &'static str {
    match update {
        tl::enums::Update::NewMessage(_) => "new_message",
        tl::enums::Update::NewChannelMessage(_) => "new_channel_message",
        tl::enums::Update::EditMessage(_) => "edit_message",
        tl::enums::Update::EditChannelMessage(_) => "edit_channel_message",
        tl::enums::Update::DeleteMessages(_) => "delete_messages",
        tl::enums::Update::DeleteChannelMessages(_) => "delete_channel_messages",
        tl::enums::Update::NotifySettings(_) => "notify_settings",
        tl::enums::Update::ChannelTooLong(_) => "channel_too_long",
        tl::enums::Update::Channel(_) => "channel",
        tl::enums::Update::ReadHistoryInbox(_) => "read_history_inbox",
        tl::enums::Update::ReadHistoryOutbox(_) => "read_history_outbox",
        tl::enums::Update::ReadChannelInbox(_) => "read_channel_inbox",
        tl::enums::Update::ReadChannelOutbox(_) => "read_channel_outbox",
        tl::enums::Update::UserTyping(_)
        | tl::enums::Update::ChatUserTyping(_)
        | tl::enums::Update::ChannelUserTyping(_) => "typing",
        _ => "other",
    }
}

fn now_unix_millis() -> i128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i128
}

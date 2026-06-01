//! Telegram startup hydration for the live update stream.
//!
//! Grammers only learns channel/megagroup update state for unknown dialogs
//! while iterating dialogs. We do this once before entering the hot update
//! loop so `next_raw_update` can receive live events for all visible chats.

use anyhow::Context as _;
use grammers_client::{
    types::{Chat, Dialog, Message},
    Client,
};
use tracing::{info, warn};

use crate::delivery::{emit_message_if_new, DeliveryCursor};
use crate::notifications::NotificationMuteCache;
use crate::state::SkillEnv;

const MAX_STARTUP_DIALOGS: usize = 2_000;
const MAX_RESUME_MESSAGES_PER_CHAT: usize = 200;
const DELIVERY_SOURCE_RESUME: &str = "resume";

/// Hydrates dialog state, suppression state, and missed messages before live streaming.
pub(crate) async fn hydrate_dialog_state(
    env: &SkillEnv,
    client: &Client,
    cursor: &mut DeliveryCursor,
    notification_mutes: &mut NotificationMuteCache,
) -> anyhow::Result<()> {
    let was_initialized = cursor.is_initialized();
    let mut dialogs_seen = 0usize;
    let mut suppressed_seen = 0usize;
    let mut chats_backfilled = 0usize;
    let mut messages_emitted = 0usize;
    let mut iter = client.iter_dialogs();
    while dialogs_seen < MAX_STARTUP_DIALOGS {
        let Some(dialog) = iter
            .next()
            .await
            .with_context(|| "iter_dialogs failed during Telegram startup hydration")?
        else {
            break;
        };
        dialogs_seen += 1;
        let suppressed = notification_mutes.observe_dialog(&dialog);
        if suppressed {
            suppressed_seen += 1;
        }
        let Some(last_message) = dialog.last_message.as_ref() else {
            continue;
        };

        if should_mark_seen_without_backfill(cursor, last_message, suppressed, was_initialized) {
            cursor.record_seen(last_message);
            continue;
        }
        if !cursor.is_new(last_message) {
            continue;
        }

        match collect_resume_messages(client, &dialog, cursor).await {
            Ok(mut messages) => {
                if messages.is_empty() {
                    continue;
                }
                chats_backfilled += 1;
                messages.sort_by_key(Message::id);
                for message in messages {
                    if emit_message_if_new(
                        env,
                        cursor,
                        notification_mutes,
                        &message,
                        DELIVERY_SOURCE_RESUME,
                        None,
                    )
                    .await?
                    {
                        messages_emitted += 1;
                    }
                }
            }
            Err(error) => {
                warn!(
                    chat = %dialog.chat().id(),
                    error = %error,
                    "failed to backfill Telegram dialog during startup hydration"
                );
            }
        }
    }

    if !was_initialized {
        cursor.mark_initialized();
    }
    cursor.save(env)?;
    info!(
        dialogs_seen,
        suppressed_seen,
        chats_backfilled,
        messages_emitted,
        initialized = was_initialized,
        "hydrated Telegram startup dialog state"
    );
    Ok(())
}

fn should_mark_seen_without_backfill(
    cursor: &DeliveryCursor,
    last_message: &Message,
    _suppressed: bool,
    was_initialized: bool,
) -> bool {
    !was_initialized || !cursor.has_chat(last_message)
}

async fn collect_resume_messages(
    client: &Client,
    dialog: &Dialog,
    cursor: &DeliveryCursor,
) -> anyhow::Result<Vec<Message>> {
    collect_limited_resume_messages(client, dialog, cursor, MAX_RESUME_MESSAGES_PER_CHAT).await
}

async fn collect_limited_resume_messages(
    client: &Client,
    dialog: &Dialog,
    cursor: &DeliveryCursor,
    limit: usize,
) -> anyhow::Result<Vec<Message>> {
    collect_limited_messages_for_chat(client, dialog.chat(), cursor, limit).await
}

async fn collect_limited_messages_for_chat(
    client: &Client,
    chat: &Chat,
    cursor: &DeliveryCursor,
    limit: usize,
) -> anyhow::Result<Vec<Message>> {
    let mut pending = Vec::new();
    let mut iter = client.iter_messages(chat.pack()).limit(limit);
    while let Some(message) = iter
        .next()
        .await
        .with_context(|| format!("iterate messages for Telegram chat {}", chat.id()))?
    {
        if !cursor.is_new(&message) {
            break;
        }
        pending.push(message);
    }
    Ok(pending)
}

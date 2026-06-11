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
use crate::peer_cache::{
    hydrate_contact_book, hydrate_saved_contact_usernames, TelegramPeerCache,
};
use crate::state::SkillEnv;

const MAX_STARTUP_DIALOGS: usize = 2_000;
const MAX_RESUME_MESSAGES_PER_CHAT: usize = 200;
const DELIVERY_SOURCE_RESUME: &str = "resume";

/// Hydrates dialog state, suppression state, and missed messages before live streaming.
///
/// `live_since_ms` is the moment this subscriber session started monitoring
/// (Unix millis). Chats without a cursor baseline (fresh login, brand-new
/// chats) must not replay their history into the triage pipeline, but
/// messages dated at or after this boundary are live traffic — typically
/// arrived while hydration was still running — and are backfilled instead of
/// being silently marked seen.
///
/// Returns the chats still missing avatars. Avatar downloads are deliberately
/// NOT performed here: they only feed contact-picker UI and used to dominate
/// hydration wall-clock (~2 minutes of serial fetches on a fresh login),
/// delaying live message delivery. The caller fetches them in the background
/// via `hydrate_chat_avatars_deferred` once the update loop is running.
pub(crate) async fn hydrate_dialog_state(
    env: &SkillEnv,
    client: &Client,
    cursor: &mut DeliveryCursor,
    notification_mutes: &mut NotificationMuteCache,
    live_since_ms: i64,
) -> anyhow::Result<Vec<Chat>> {
    let was_initialized = cursor.is_initialized();
    let original_peer_cache = TelegramPeerCache::load(env).unwrap_or_default();
    let mut peer_cache = original_peer_cache.clone();
    if let Err(error) = hydrate_contact_book(client, &mut peer_cache).await {
        warn!(
            error = %error,
            "failed to hydrate Telegram contact book into peer cache"
        );
    }
    let saved_contact_usernames_resolved =
        match hydrate_saved_contact_usernames(env, client, &mut peer_cache).await {
            Ok(count) => count,
            Err(error) => {
                warn!(
                    error = %error,
                    "failed to hydrate Telegram saved contact usernames into peer cache"
                );
                0
            }
        };
    let mut dialogs_seen = 0usize;
    let mut suppressed_seen = 0usize;
    let mut pending_avatar_chats: Vec<Chat> = Vec::new();
    let mut chats_backfilled = 0usize;
    let mut messages_emitted = 0usize;
    let mut iter = client.iter_dialogs();
    while dialogs_seen < MAX_STARTUP_DIALOGS {
        let dialog = match iter.next().await {
            Ok(Some(dialog)) => dialog,
            Ok(None) => break,
            Err(error) => {
                warn!(
                    error = %error,
                    dialogs_seen,
                    "iter_dialogs failed during Telegram startup hydration; saving partial state"
                );
                break;
            }
        };
        dialogs_seen += 1;
        let last_message_at_ms = dialog
            .last_message
            .as_ref()
            .map(|message| message.date().timestamp_millis());
        peer_cache.observe_chat_with_last_message_at_ms(
            dialog.chat(),
            "dialog",
            last_message_at_ms,
        );
        if !peer_cache.has_avatar(dialog.chat()) {
            pending_avatar_chats.push(dialog.chat().clone());
        }
        let suppressed = notification_mutes.observe_dialog(&dialog);
        if suppressed {
            suppressed_seen += 1;
        }
        let Some(last_message) = dialog.last_message.as_ref() else {
            continue;
        };

        let disposition = startup_dialog_disposition(
            cursor.has_chat(last_message),
            cursor.is_new(last_message),
            last_message.date().timestamp_millis(),
            was_initialized,
            live_since_ms,
        );
        let backfill_since_ms = match disposition {
            StartupDialogDisposition::MarkSeen => {
                cursor.record_seen(last_message);
                continue;
            }
            StartupDialogDisposition::Skip => continue,
            StartupDialogDisposition::Backfill { since_ms } => since_ms,
        };

        match collect_resume_messages(client, &dialog, cursor, backfill_since_ms).await {
            Ok(mut messages) => {
                if !messages.is_empty() {
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
                if backfill_since_ms.is_some() {
                    // Boundary-limited backfill leaves the chat's older history
                    // unseen; pin the baseline at the latest message so it is
                    // never replayed by a later hydration.
                    cursor.record_seen(last_message);
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
    peer_cache.save_if_changed(env, &original_peer_cache)?;
    cursor.save(env)?;
    info!(
        dialogs_seen,
        suppressed_seen,
        saved_contact_usernames_resolved,
        avatars_pending = pending_avatar_chats.len(),
        chats_backfilled,
        messages_emitted,
        initialized = was_initialized,
        "hydrated Telegram startup dialog state"
    );
    Ok(pending_avatar_chats)
}

/// How a dialog's latest message is handled during startup hydration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartupDialogDisposition {
    /// Chat history predates this subscriber session; mark seen, never emit.
    MarkSeen,
    /// Cursor already covers the latest message; nothing to do.
    Skip,
    /// Backfill unseen messages. `since_ms` bounds how far back to emit for
    /// chats that have no cursor baseline.
    Backfill { since_ms: Option<i64> },
}

fn startup_dialog_disposition(
    has_chat_cursor: bool,
    last_message_is_new: bool,
    last_message_at_ms: i64,
    was_initialized: bool,
    live_since_ms: i64,
) -> StartupDialogDisposition {
    if !was_initialized || !has_chat_cursor {
        // No cursor baseline for this chat. Its history must not replay into
        // the monitor, but anything dated after this subscriber session
        // started is live traffic (it arrived while hydration was running)
        // and must still be delivered instead of being silently marked seen.
        if last_message_at_ms >= live_since_ms {
            return StartupDialogDisposition::Backfill {
                since_ms: Some(live_since_ms),
            };
        }
        return StartupDialogDisposition::MarkSeen;
    }
    if !last_message_is_new {
        return StartupDialogDisposition::Skip;
    }
    StartupDialogDisposition::Backfill { since_ms: None }
}

async fn collect_resume_messages(
    client: &Client,
    dialog: &Dialog,
    cursor: &DeliveryCursor,
    since_ms: Option<i64>,
) -> anyhow::Result<Vec<Message>> {
    collect_limited_resume_messages(client, dialog, cursor, MAX_RESUME_MESSAGES_PER_CHAT, since_ms)
        .await
}

async fn collect_limited_resume_messages(
    client: &Client,
    dialog: &Dialog,
    cursor: &DeliveryCursor,
    limit: usize,
    since_ms: Option<i64>,
) -> anyhow::Result<Vec<Message>> {
    collect_limited_messages_for_chat(client, dialog.chat(), cursor, limit, since_ms).await
}

async fn collect_limited_messages_for_chat(
    client: &Client,
    chat: &Chat,
    cursor: &DeliveryCursor,
    limit: usize,
    since_ms: Option<i64>,
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
        if since_ms.is_some_and(|since| message.date().timestamp_millis() < since) {
            break;
        }
        pending.push(message);
    }
    Ok(pending)
}

#[cfg(test)]
mod tests {
    use super::{startup_dialog_disposition, StartupDialogDisposition};

    const LIVE_SINCE_MS: i64 = 1_000_000;

    #[test]
    fn uninitialized_cursor_backfills_messages_arriving_after_session_start() {
        // Regression: a real message arriving during (possibly minutes-long)
        // startup hydration after a fresh login must not be swallowed.
        assert_eq!(
            startup_dialog_disposition(false, true, LIVE_SINCE_MS + 5_000, false, LIVE_SINCE_MS),
            StartupDialogDisposition::Backfill {
                since_ms: Some(LIVE_SINCE_MS)
            },
        );
    }

    #[test]
    fn uninitialized_cursor_marks_pre_session_history_seen() {
        assert_eq!(
            startup_dialog_disposition(false, true, LIVE_SINCE_MS - 5_000, false, LIVE_SINCE_MS),
            StartupDialogDisposition::MarkSeen,
        );
    }

    #[test]
    fn untracked_chat_backfills_messages_arriving_after_session_start() {
        // A brand-new chat (no cursor entry) whose first message lands while
        // this session is live must be delivered.
        assert_eq!(
            startup_dialog_disposition(false, true, LIVE_SINCE_MS + 5_000, true, LIVE_SINCE_MS),
            StartupDialogDisposition::Backfill {
                since_ms: Some(LIVE_SINCE_MS)
            },
        );
    }

    #[test]
    fn untracked_chat_marks_old_history_seen() {
        assert_eq!(
            startup_dialog_disposition(false, true, LIVE_SINCE_MS - 5_000, true, LIVE_SINCE_MS),
            StartupDialogDisposition::MarkSeen,
        );
    }

    #[test]
    fn tracked_chat_with_seen_message_skips() {
        assert_eq!(
            startup_dialog_disposition(true, false, LIVE_SINCE_MS - 5_000, true, LIVE_SINCE_MS),
            StartupDialogDisposition::Skip,
        );
    }

    #[test]
    fn tracked_chat_with_new_message_backfills_from_cursor() {
        // Known chats keep the original unbounded-resume semantics: everything
        // newer than the cursor is replayed, even if older than this session.
        assert_eq!(
            startup_dialog_disposition(true, true, LIVE_SINCE_MS - 5_000, true, LIVE_SINCE_MS),
            StartupDialogDisposition::Backfill { since_ms: None },
        );
    }
}

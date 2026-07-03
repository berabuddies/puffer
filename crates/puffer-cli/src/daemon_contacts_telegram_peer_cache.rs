//! Telegram peer-cache readers for contact ranking.
//!
//! These helpers are pure file reads: they never dial Telegram. Contact-book
//! and recent-dialog hydration is owned by the subscriber (see
//! `SubscriberCommand::TelegramHydrateContacts`), which writes the
//! `contact_book` metadata and the `recent-dialog-cache.json` marker itself.

use super::{
    merge_candidate_last_message_at_ms, merge_candidate_public_username, merge_telegram_name,
    normalize_contact_id, public_username_from_contact_id,
    read_telegram_primary_peer_metadata_from_account, Candidate,
};
use grammers_session::Session;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

const RECENT_DIALOG_CACHE_FILE: &str = "recent-dialog-cache.json";
const RECENT_DIALOG_TARGET_MIN: usize = 5;
const CONTACT_PICKER_DIALOG_TARGET_MAX: usize = 120;

/// Read-only mirror of the peer-cache `contact_book` metadata written by the
/// subscriber. A missing file or object reports the default `pending` state so
/// callers treat legacy caches as not-yet-hydrated.
#[derive(Debug, Clone)]
pub(crate) struct ContactBookView {
    /// Raw `contact_book.state` string (`pending`/`hydrating`/`ready`/`failed`).
    pub(crate) state: String,
    /// `contact_book.hydrated_at_ms`, when the subscriber recorded one.
    pub(crate) updated_at_ms: Option<i64>,
    /// `contact_book.last_error`, when the last hydration attempt failed.
    pub(crate) error: Option<String>,
}

impl ContactBookView {
    fn pending() -> Self {
        Self {
            state: "pending".to_string(),
            updated_at_ms: None,
            error: None,
        }
    }
}

/// Reads the `contact_book` object from an account's peer-cache.json without
/// dialing Telegram.
pub(crate) fn account_contact_book_view(account_dir: &Path) -> ContactBookView {
    // The subscriber owns `contact-book.json` as the single readiness source
    // (kept out of peer-cache.json so peer-cache writers cannot clobber it).
    let path = account_dir.join("contact-book.json");
    let Ok(raw) = std::fs::read_to_string(path) else {
        return ContactBookView::pending();
    };
    let Ok(book) = serde_json::from_str::<Value>(&raw) else {
        return ContactBookView::pending();
    };
    let state = book
        .get("state")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("pending")
        .to_string();
    let updated_at_ms = book.get("hydrated_at_ms").and_then(Value::as_i64);
    let error = book
        .get("last_error")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    ContactBookView {
        state,
        updated_at_ms,
        error,
    }
}

pub(super) fn collect_telegram_peer_cache_candidates(
    account_dir: &Path,
    by_id: &mut HashMap<String, Candidate>,
) {
    let self_contact_id = telegram_session_user_contact_id(account_dir);
    for (id, metadata) in read_telegram_primary_peer_metadata_from_account(account_dir) {
        if self_contact_id.as_deref() == Some(id.as_str()) {
            continue;
        }
        let entry = by_id.entry(id.clone()).or_insert_with(|| Candidate {
            id: id.clone(),
            name: metadata.name.clone(),
            public_username: metadata
                .public_username
                .clone()
                .or_else(|| public_username_from_contact_id(&id)),
            avatar: metadata.avatar.clone(),
            score: 0.01,
            last_message_at_ms: metadata.last_message_at_ms,
            context: Vec::new(),
        });
        entry.score = entry.score.max(0.01);
        merge_candidate_last_message_at_ms(
            &mut entry.last_message_at_ms,
            metadata.last_message_at_ms,
        );
        merge_telegram_name(&mut entry.name, &metadata.name);
        merge_candidate_public_username(
            &mut entry.public_username,
            metadata.public_username.as_deref(),
        );
        if entry.avatar.is_none() {
            entry.avatar = metadata.avatar;
        }
    }
}

fn telegram_session_user_contact_id(account_dir: &Path) -> Option<String> {
    let user = Session::load_file(account_dir.join("telegram.session"))
        .ok()?
        .get_user()?;
    normalize_contact_id(&format!("telegram-user-id@{}", user.id))
}

pub(super) fn telegram_contact_picker_dialog_cache_claims_target_satisfied(
    account_dir: &Path,
    limit: usize,
) -> bool {
    recent_dialog_cache_claims_target_satisfied(account_dir, contact_picker_dialog_target(limit))
}

fn contact_picker_dialog_target(limit: usize) -> usize {
    limit
        .max(RECENT_DIALOG_TARGET_MIN)
        .min(CONTACT_PICKER_DIALOG_TARGET_MAX)
}

fn recent_dialog_cache_claims_target_satisfied(account_dir: &Path, target: usize) -> bool {
    let path = account_dir.join(RECENT_DIALOG_CACHE_FILE);
    let Ok(raw) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(marker) = serde_json::from_str::<Value>(&raw) else {
        return false;
    };
    if marker.get("ready").and_then(Value::as_bool) != Some(true) {
        return false;
    }
    let Some(marker_target) = marker.get("target").and_then(Value::as_u64) else {
        return false;
    };
    let Some(direct_users_seen) = marker.get("direct_users_seen").and_then(Value::as_u64) else {
        return false;
    };
    let dialogs_exhausted = marker
        .get("dialogs_exhausted")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    marker_target as usize >= target && (direct_users_seen as usize >= target || dialogs_exhausted)
}

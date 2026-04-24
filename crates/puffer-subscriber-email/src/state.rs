//! High-water-mark persistence for the email subscriber.
//!
//! We track the last IMAP UID we have already emitted an event for, per
//! mailbox. Stored as a JSON object at `<state_dir>/seen.json`. Today we only
//! track `INBOX`, but the map shape leaves room for more mailboxes later.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use serde::{Deserialize, Serialize};

/// Name of the primary mailbox we poll.
pub const INBOX: &str = "INBOX";

/// Per-mailbox IMAP UID high-water mark, persisted to disk between runs.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SeenState {
    /// Map of mailbox name -> last UID for which we have emitted an event.
    /// A missing entry means "no baseline yet; next poll should seed one
    /// without emitting the backlog".
    #[serde(default)]
    pub mailboxes: BTreeMap<String, u32>,
}

impl SeenState {
    /// Returns the last-seen UID for the given mailbox, or `None` if we
    /// have never recorded one.
    pub fn last_uid(&self, mailbox: &str) -> Option<u32> {
        self.mailboxes.get(mailbox).copied()
    }

    /// Records a new last-seen UID for the given mailbox.
    pub fn set_last_uid(&mut self, mailbox: &str, uid: u32) {
        self.mailboxes.insert(mailbox.to_string(), uid);
    }
}

/// Returns the on-disk path of the high-water-mark file.
pub fn seen_path(state_dir: &Path) -> PathBuf {
    state_dir.join("seen.json")
}

/// Loads the high-water-mark map. Returns an empty [`SeenState`] if the file
/// does not exist yet.
pub async fn load(state_dir: &Path) -> anyhow::Result<SeenState> {
    let path = seen_path(state_dir);
    match tokio::fs::read(&path).await {
        Ok(bytes) => {
            let state: SeenState = serde_json::from_slice(&bytes)
                .with_context(|| format!("parse seen state {}", path.display()))?;
            Ok(state)
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(SeenState::default()),
        Err(err) => Err(anyhow::anyhow!(err))
            .with_context(|| format!("read seen state at {}", path.display())),
    }
}

/// Persists `state` to `<state_dir>/seen.json` atomically (tempfile + rename).
pub async fn save(state_dir: &Path, state: &SeenState) -> anyhow::Result<()> {
    tokio::fs::create_dir_all(state_dir)
        .await
        .with_context(|| format!("create state dir {}", state_dir.display()))?;
    let target = seen_path(state_dir);
    let tmp = target.with_extension("json.tmp");
    let rendered = serde_json::to_vec_pretty(state).context("serialize seen state")?;
    tokio::fs::write(&tmp, &rendered)
        .await
        .with_context(|| format!("write tempfile {}", tmp.display()))?;
    tokio::fs::rename(&tmp, &target)
        .await
        .with_context(|| format!("rename {} -> {}", tmp.display(), target.display()))?;
    Ok(())
}

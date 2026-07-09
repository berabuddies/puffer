use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

pub const DUPLICATE_RISK_ACK_REQUIRED: &str = "duplicate_risk_ack_required";
pub const OUTBOUND_ACTION_EXPIRED: &str = "outbound_action_expired";
pub const TERMINAL_OUTBOUND_ACTION: &str = "terminal_outbound_action";
pub const OUTBOUND_ACTION_VERSION_MISMATCH: &str = "outbound_action_version_mismatch";
pub const OUTBOUND_ACTION_ATTEMPT_MISMATCH: &str = "outbound_action_attempt_mismatch";

const DEFAULT_TTL_MS: u64 = 24 * 60 * 60 * 1000;

/// On-disk schema version this build understands. A file stamped with a higher
/// version was written by a newer binary and is refused rather than silently
/// mutated (§10: fail loudly, no silent fallbacks).
const OUTBOUND_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Default, Serialize, Deserialize)]
struct OutboundActionFile {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    actions: Vec<OutboundAction>,
}

/// Serializes every mutating store operation on a given file path within this
/// process. Each caller constructs a fresh [`OutboundStore`], so an
/// instance-local mutex would never mediate between concurrent operations
/// (create_draft vs begin_send, or a cancel racing an execute's finish_send).
/// The registry instead keys one lock per canonical file path; combined with a
/// re-read-from-disk under the lock and an atomic rename write, every operation
/// is a serialized read-modify-write.
///
/// Single-process assumption: the daemon is the sole writer of this file. A
/// second OS process would not observe this in-memory lock; cross-process
/// safety would need an advisory OS file lock, which the current architecture
/// (one daemon owns `~/.puffer`) does not require.
static STORE_LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();

fn store_lock(path: &Path) -> Arc<Mutex<()>> {
    let key = canonical_lock_key(path);
    let registry = STORE_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut registry = registry.lock().unwrap();
    registry
        .entry(key)
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

/// Best-effort canonical key for a store path. The file itself may not exist
/// yet, so we canonicalize the (existing) parent directory and re-join the file
/// name; if even that fails we fall back to the path as given. All in-process
/// callers derive the path the same way, so this reliably coalesces handles.
fn canonical_lock_key(path: &Path) -> PathBuf {
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => match std::fs::canonicalize(parent) {
            Ok(canonical_parent) => canonical_parent.join(name),
            Err(_) => path.to_path_buf(),
        },
        _ => path.to_path_buf(),
    }
}

pub struct OutboundStore {
    path: PathBuf,
    lock: Arc<Mutex<()>>,
}

pub struct NewOutboundDraft {
    pub connector_slug: String,
    pub connection_slug: String,
    pub action: String,
    pub input: Value,
    pub recipient_stable_id: String,
    pub recipient_source: RecipientSource,
    pub message: String,
    pub origin: OutboundOrigin,
    pub ttl_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecipientSource {
    Stamped,
    Model,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OutboundOrigin {
    pub session_id: String,
    pub turn_id: Option<String>,
    pub task_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OutboundAction {
    pub id: String,
    pub version: u64,
    pub connector_slug: String,
    pub connection_slug: String,
    pub action: String,
    pub input: Value,
    pub recipient_stable_id: String,
    pub recipient_source: RecipientSource,
    pub message: String,
    /// Canonical content fingerprint (recipient + message + input). Reserved for
    /// future outbound de-duplication (spec §4); not read back today.
    pub content_hash: String,
    pub origin: OutboundOrigin,
    pub status: String,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
    pub approved_message: Option<String>,
    pub approved_by: Option<String>,
    pub approved_at_ms: Option<u64>,
    pub client_request_id: Option<String>,
    pub send_attempt_id: Option<String>,
    pub receipt: Option<Value>,
    pub error: Option<String>,
    pub events: Vec<Value>,
}

impl OutboundStore {
    pub fn load(path: PathBuf) -> Result<OutboundStore> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        // Validate the on-disk schema version eagerly at load so an unknown
        // future file is rejected before any mutation touches it.
        if path.exists() {
            read_file(&path)?;
        }
        let lock = store_lock(&path);
        Ok(OutboundStore { path, lock })
    }

    pub fn create_draft(&self, draft: NewOutboundDraft) -> Result<OutboundAction> {
        let _guard = self.lock.lock().unwrap();
        let mut actions = read_actions(&self.path)?;
        let now = now_ms();
        let action = OutboundAction {
            id: format!("oa-{}", uuid::Uuid::new_v4()),
            version: 1,
            connector_slug: draft.connector_slug,
            connection_slug: draft.connection_slug,
            action: draft.action,
            recipient_stable_id: draft.recipient_stable_id.clone(),
            recipient_source: draft.recipient_source,
            content_hash: draft_content_hash(
                &draft.recipient_stable_id,
                &draft.message,
                &draft.input,
            ),
            input: draft.input,
            message: draft.message,
            origin: draft.origin,
            status: "draft_ready".to_string(),
            created_at_ms: now,
            expires_at_ms: now.saturating_add(draft.ttl_ms.unwrap_or(DEFAULT_TTL_MS)),
            approved_message: None,
            approved_by: None,
            approved_at_ms: None,
            client_request_id: None,
            send_attempt_id: None,
            receipt: None,
            error: None,
            events: vec![lifecycle_event("draft_created", json!({}))],
        };
        actions.push(action.clone());
        write_actions(&self.path, &actions)?;
        Ok(action)
    }

    pub fn get(&self, action_id: &str) -> Result<Option<OutboundAction>> {
        // Reads re-read from disk (atomic rename guarantees a complete file) so
        // a caller never observes a stale in-memory snapshot.
        Ok(read_actions(&self.path)?
            .into_iter()
            .find(|action| action.id == action_id))
    }

    pub fn list(&self) -> Result<Vec<OutboundAction>> {
        read_actions(&self.path)
    }

    pub fn cancel(
        &self,
        action_id: &str,
        version: u64,
        reason: Option<&str>,
    ) -> Result<OutboundAction> {
        let _guard = self.lock.lock().unwrap();
        let mut actions = read_actions(&self.path)?;
        let action = find_action_mut(&mut actions, action_id)?;
        ensure_version(action, version)?;
        match action.status.as_str() {
            // `uncertain` must remain cancellable: it is the recovery state after
            // a stale send, not a terminal one (spec §10 / #561).
            "draft_ready" | "failed" | "uncertain" => {}
            "cancelled" | "expired" | "sent" | "sending" => {
                bail!(TERMINAL_OUTBOUND_ACTION)
            }
            other => bail!("outbound action state `{other}` cannot be cancelled"),
        }
        action.status = "cancelled".to_string();
        action.version += 1;
        action
            .events
            .push(lifecycle_event("cancelled", json!({ "reason": reason })));
        let updated = action.clone();
        write_actions(&self.path, &actions)?;
        Ok(updated)
    }

    pub fn begin_send(
        &self,
        action_id: &str,
        version: u64,
        approved_message: &str,
        client_request_id: &str,
        duplicate_risk_ack: bool,
    ) -> Result<OutboundAction> {
        let _guard = self.lock.lock().unwrap();
        let mut actions = read_actions(&self.path)?;
        let action = find_action_mut(&mut actions, action_id)?;

        // Idempotent replay is keyed on `client_request_id` ALONE (not version):
        // a retry of an already-sent action returns the sent record regardless of
        // the version the caller still holds.
        if action.status == "sent" && action.client_request_id.as_deref() == Some(client_request_id)
        {
            return Ok(action.clone());
        }

        ensure_version(action, version)?;
        match action.status.as_str() {
            "sent" | "cancelled" | "expired" => bail!(TERMINAL_OUTBOUND_ACTION),
            "sending" => {
                action.status = "uncertain".to_string();
                action.version += 1;
                action.events.push(lifecycle_event(
                    "send_uncertain",
                    json!({ "client_request_id": client_request_id }),
                ));
                write_actions(&self.path, &actions)?;
                bail!(DUPLICATE_RISK_ACK_REQUIRED);
            }
            "uncertain" => {
                // A duplicate-risk acknowledgement is required to retry from
                // `uncertain`; without it this remains a hard stop, but it is no
                // longer a dead end (spec §10 "duplicate_risk_ack before retry").
                if !duplicate_risk_ack {
                    bail!(DUPLICATE_RISK_ACK_REQUIRED);
                }
            }
            "draft_ready" | "failed" => {
                if now_ms() >= action.expires_at_ms {
                    action.status = "expired".to_string();
                    action.version += 1;
                    action.events.push(lifecycle_event("expired", json!({})));
                    write_actions(&self.path, &actions)?;
                    bail!(OUTBOUND_ACTION_EXPIRED);
                }
            }
            other => bail!("outbound action state `{other}` cannot be sent"),
        }

        let now = now_ms();
        let attempt_id = uuid::Uuid::new_v4().to_string();
        action.status = "sending".to_string();
        action.version += 1;
        action.approved_message = Some(approved_message.to_string());
        action.approved_by = Some("human".to_string());
        action.approved_at_ms = Some(now);
        action.client_request_id = Some(client_request_id.to_string());
        action.send_attempt_id = Some(attempt_id.clone());
        action.error = None;
        action.events.push(lifecycle_event(
            "send_started",
            json!({
                "client_request_id": client_request_id,
                "send_attempt_id": attempt_id,
                "duplicate_risk_ack": duplicate_risk_ack,
            }),
        ));
        let updated = action.clone();
        write_actions(&self.path, &actions)?;
        Ok(updated)
    }

    pub fn finish_send(
        &self,
        action_id: &str,
        send_attempt_id: &str,
        receipt: Value,
    ) -> Result<OutboundAction> {
        let _guard = self.lock.lock().unwrap();
        let mut actions = read_actions(&self.path)?;
        let action = find_action_mut(&mut actions, action_id)?;
        if action.status != "sending" {
            bail!(
                "outbound action state `{}` cannot finish send",
                action.status
            );
        }
        ensure_attempt(action, send_attempt_id)?;
        action.status = "sent".to_string();
        action.version += 1;
        action.receipt = Some(receipt.clone());
        action.error = None;
        action
            .events
            .push(lifecycle_event("sent", json!({ "receipt": receipt })));
        let updated = action.clone();
        write_actions(&self.path, &actions)?;
        Ok(updated)
    }

    pub fn fail_send(
        &self,
        action_id: &str,
        send_attempt_id: &str,
        error: &str,
    ) -> Result<OutboundAction> {
        let _guard = self.lock.lock().unwrap();
        let mut actions = read_actions(&self.path)?;
        let action = find_action_mut(&mut actions, action_id)?;
        if action.status != "sending" {
            bail!("outbound action state `{}` cannot fail send", action.status);
        }
        ensure_attempt(action, send_attempt_id)?;
        action.status = "failed".to_string();
        action.version += 1;
        action.error = Some(error.to_string());
        action
            .events
            .push(lifecycle_event("send_failed", json!({ "error": error })));
        let updated = action.clone();
        write_actions(&self.path, &actions)?;
        Ok(updated)
    }
}

fn find_action_mut<'a>(
    actions: &'a mut [OutboundAction],
    action_id: &str,
) -> Result<&'a mut OutboundAction> {
    actions
        .iter_mut()
        .find(|action| action.id == action_id)
        .ok_or_else(|| anyhow!("outbound action `{action_id}` not found"))
}

fn ensure_version(action: &OutboundAction, version: u64) -> Result<()> {
    if action.version != version {
        bail!(OUTBOUND_ACTION_VERSION_MISMATCH);
    }
    Ok(())
}

fn ensure_attempt(action: &OutboundAction, send_attempt_id: &str) -> Result<()> {
    // The completing/​failing caller must present the attempt id begin_send
    // minted; a stale attempt (e.g. a superseded retry) must not mutate state.
    if action.send_attempt_id.as_deref() != Some(send_attempt_id) {
        bail!(OUTBOUND_ACTION_ATTEMPT_MISMATCH);
    }
    Ok(())
}

fn read_file(path: &Path) -> Result<OutboundActionFile> {
    if !path.exists() {
        return Ok(OutboundActionFile::default());
    }
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(OutboundActionFile::default());
    }
    let file: OutboundActionFile = serde_json::from_str(&raw)
        .with_context(|| format!("invalid outbound action store {}", path.display()))?;
    if file.version > OUTBOUND_SCHEMA_VERSION {
        bail!(
            "outbound action store {} has unsupported schema version {} (this build understands {})",
            path.display(),
            file.version,
            OUTBOUND_SCHEMA_VERSION
        );
    }
    Ok(file)
}

fn read_actions(path: &Path) -> Result<Vec<OutboundAction>> {
    Ok(read_file(path)?.actions)
}

fn write_actions(path: &Path, actions: &[OutboundAction]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let tmp = path.with_extension("tmp");
    let file = OutboundActionFile {
        version: OUTBOUND_SCHEMA_VERSION,
        actions: actions.to_vec(),
    };
    std::fs::write(&tmp, serde_json::to_vec_pretty(&file)?)
        .with_context(|| format!("failed to write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| {
        format!(
            "failed to replace {} with {}",
            path.display(),
            tmp.display()
        )
    })?;
    Ok(())
}

fn lifecycle_event(event: &str, details: Value) -> Value {
    json!({
        "event": event,
        "at_ms": now_ms(),
        "details": details,
    })
}

fn draft_content_hash(recipient_stable_id: &str, message: &str, input: &Value) -> String {
    // Reserved for future outbound de-duplication (spec §4). Hashes the full
    // outbound intent — recipient, approved-message text, and the raw action
    // input — so the fingerprint changes whenever any of them change.
    let canonical = json!({
        "recipient_stable_id": recipient_stable_id,
        "message": message,
        "input": input,
    });
    let bytes = serde_json::to_vec(&canonical).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("sha256:{:x}", hasher.finalize())
}

/// Milliseconds since the Unix epoch. The single wall-clock helper for the
/// outbound pipeline; the daemon workflow layer re-uses it via
/// `puffer_subscriptions::outbound_store::now_ms`.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn store() -> (tempfile::TempDir, OutboundStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = OutboundStore::load(dir.path().join("outbound_actions.json")).unwrap();
        (dir, store)
    }

    fn draft() -> NewOutboundDraft {
        NewOutboundDraft {
            connector_slug: "telegram-login".into(),
            connection_slug: "telegram-login".into(),
            action: "send_message".into(),
            input: json!({"chat_id": "42", "message": "hi"}),
            recipient_stable_id: "telegram:42".into(),
            recipient_source: RecipientSource::Model,
            message: "hi".into(),
            origin: OutboundOrigin {
                session_id: "s1".into(),
                turn_id: Some("t1".into()),
                task_id: None,
            },
            ttl_ms: None,
        }
    }

    /// Current on-disk version of an action, for callers that must present the
    /// live version to the optimistic-concurrency check after a transition.
    fn version_of(store: &OutboundStore, id: &str) -> u64 {
        store.get(id).unwrap().unwrap().version
    }

    #[test]
    fn happy_path_draft_approve_send() {
        let (_d, store) = store();
        let action = store.create_draft(draft()).unwrap();
        assert_eq!(action.status, "draft_ready");
        assert_eq!(action.version, 1);
        let sending = store
            .begin_send(&action.id, 1, "hi edited", "req-1", false)
            .unwrap();
        assert_eq!(sending.status, "sending");
        assert_eq!(sending.version, 2);
        assert_eq!(sending.approved_message.as_deref(), Some("hi edited"));
        let attempt = sending.send_attempt_id.clone().unwrap();
        let sent = store
            .finish_send(&action.id, &attempt, json!({"message_id": 7}))
            .unwrap();
        assert_eq!(sent.status, "sent");
        assert_eq!(sent.version, 3);
    }

    #[test]
    fn cancel_is_terminal() {
        let (_d, store) = store();
        let action = store.create_draft(draft()).unwrap();
        let cancelled = store.cancel(&action.id, 1, Some("user")).unwrap();
        assert_eq!(cancelled.status, "cancelled");
        assert_eq!(cancelled.version, 2);
        let err = store
            .begin_send(&action.id, 2, "hi", "req-1", false)
            .unwrap_err();
        assert!(err.to_string().contains("terminal_outbound_action"));
        let err = store.cancel(&action.id, 2, None).unwrap_err();
        assert!(err.to_string().contains("terminal_outbound_action"));
    }

    #[test]
    fn expired_draft_cannot_send() {
        let (_d, store) = store();
        let mut d = draft();
        d.ttl_ms = Some(0); // expires immediately
        let action = store.create_draft(d).unwrap();
        let err = store
            .begin_send(&action.id, 1, "hi", "req-1", false)
            .unwrap_err();
        assert!(err.to_string().contains("outbound_action_expired"));
        assert_eq!(store.get(&action.id).unwrap().unwrap().status, "expired");
    }

    #[test]
    fn stale_sending_becomes_uncertain_and_blocks_retry() {
        let (_d, store) = store();
        let action = store.create_draft(draft()).unwrap();
        store
            .begin_send(&action.id, 1, "hi", "req-1", false)
            .unwrap();
        // Second begin_send after a crash: mark uncertain and reject. The live
        // version must be presented now that transitions bump the version.
        let err = store
            .begin_send(
                &action.id,
                version_of(&store, &action.id),
                "hi",
                "req-2",
                false,
            )
            .unwrap_err();
        assert!(err.to_string().contains("duplicate_risk_ack_required"));
        assert_eq!(store.get(&action.id).unwrap().unwrap().status, "uncertain");
        let err = store
            .begin_send(
                &action.id,
                version_of(&store, &action.id),
                "hi",
                "req-3",
                false,
            )
            .unwrap_err();
        assert!(err.to_string().contains("duplicate_risk_ack_required"));
    }

    #[test]
    fn uncertain_with_ack_retries_successfully() {
        let (_d, store) = store();
        let action = store.create_draft(draft()).unwrap();
        store
            .begin_send(&action.id, 1, "hi", "req-1", false)
            .unwrap();
        store
            .begin_send(
                &action.id,
                version_of(&store, &action.id),
                "hi",
                "req-2",
                false,
            )
            .unwrap_err();
        assert_eq!(store.get(&action.id).unwrap().unwrap().status, "uncertain");
        // Acknowledging the duplicate risk permits a fresh send attempt.
        let sending = store
            .begin_send(
                &action.id,
                version_of(&store, &action.id),
                "hi",
                "req-3",
                true,
            )
            .unwrap();
        assert_eq!(sending.status, "sending");
    }

    #[test]
    fn uncertain_can_be_cancelled() {
        let (_d, store) = store();
        let action = store.create_draft(draft()).unwrap();
        store
            .begin_send(&action.id, 1, "hi", "req-1", false)
            .unwrap();
        store
            .begin_send(
                &action.id,
                version_of(&store, &action.id),
                "hi",
                "req-2",
                false,
            )
            .unwrap_err();
        assert_eq!(store.get(&action.id).unwrap().unwrap().status, "uncertain");
        let cancelled = store
            .cancel(&action.id, version_of(&store, &action.id), Some("user"))
            .unwrap();
        assert_eq!(cancelled.status, "cancelled");
    }

    #[test]
    fn failed_send_can_retry() {
        let (_d, store) = store();
        let action = store.create_draft(draft()).unwrap();
        let sending = store
            .begin_send(&action.id, 1, "hi", "req-1", false)
            .unwrap();
        store
            .fail_send(
                &action.id,
                sending.send_attempt_id.as_deref().unwrap(),
                "network down",
            )
            .unwrap();
        let retried = store
            .begin_send(
                &action.id,
                version_of(&store, &action.id),
                "hi",
                "req-2",
                false,
            )
            .unwrap();
        assert_eq!(retried.status, "sending");
    }

    #[test]
    fn finish_send_rejects_stale_attempt() {
        let (_d, store) = store();
        let action = store.create_draft(draft()).unwrap();
        store
            .begin_send(&action.id, 1, "hi", "req-1", false)
            .unwrap();
        let err = store
            .finish_send(&action.id, "not-the-attempt", json!({}))
            .unwrap_err();
        assert!(err.to_string().contains("outbound_action_attempt_mismatch"));
    }

    #[test]
    fn version_mismatch_rejected() {
        let (_d, store) = store();
        let action = store.create_draft(draft()).unwrap();
        assert!(store
            .begin_send(&action.id, 99, "hi", "req-1", false)
            .is_err());
        assert!(store.cancel(&action.id, 99, None).is_err());
    }

    #[test]
    fn already_sent_is_idempotent_for_same_request_id() {
        let (_d, store) = store();
        let action = store.create_draft(draft()).unwrap();
        let sending = store
            .begin_send(&action.id, 1, "hi", "req-1", false)
            .unwrap();
        store
            .finish_send(
                &action.id,
                sending.send_attempt_id.as_deref().unwrap(),
                json!({"message_id": 7}),
            )
            .unwrap();
        // Replay with the same client_request_id -> returns the sent record even
        // though the version param (1) is now stale.
        let replay = store
            .begin_send(&action.id, 1, "hi", "req-1", false)
            .unwrap();
        assert_eq!(replay.status, "sent");
    }

    #[test]
    fn events_record_lifecycle() {
        let (_d, store) = store();
        let action = store.create_draft(draft()).unwrap();
        store.cancel(&action.id, 1, Some("user")).unwrap();
        let events: Vec<String> = store
            .get(&action.id)
            .unwrap()
            .unwrap()
            .events
            .iter()
            .map(|e| e["event"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(events, vec!["draft_created", "cancelled"]);
    }

    #[test]
    fn concurrent_handles_do_not_clobber_each_other() {
        // Two independent store handles on the same path (the real daemon
        // constructs a fresh store per RPC). Interleaving a mutation on handle-1
        // with a create on handle-2 must not lose either record.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("outbound_actions.json");
        let handle1 = OutboundStore::load(path.clone()).unwrap();
        let handle2 = OutboundStore::load(path.clone()).unwrap();

        let a = handle1.create_draft(draft()).unwrap();
        let b = handle2.create_draft(draft()).unwrap();
        // handle-1's snapshot predates B; the re-read-under-lock must preserve B.
        handle1.cancel(&a.id, a.version, Some("user")).unwrap();

        let reopened = OutboundStore::load(path).unwrap();
        assert_eq!(
            reopened.get(&a.id).unwrap().unwrap().status,
            "cancelled",
            "A survived and was cancelled"
        );
        assert_eq!(
            reopened.get(&b.id).unwrap().unwrap().status,
            "draft_ready",
            "B was not clobbered by handle-1's write"
        );
    }

    #[test]
    fn unknown_future_schema_version_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("outbound_actions.json");
        std::fs::write(&path, json!({ "version": 999, "actions": [] }).to_string()).unwrap();
        let err = match OutboundStore::load(path) {
            Ok(_) => panic!("future schema version must be refused"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("unsupported schema version"));
    }
}

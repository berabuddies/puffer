use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub const DUPLICATE_RISK_ACK_REQUIRED: &str = "duplicate_risk_ack_required";
pub const OUTBOUND_ACTION_EXPIRED: &str = "outbound_action_expired";
pub const TERMINAL_OUTBOUND_ACTION: &str = "terminal_outbound_action";

const DEFAULT_TTL_MS: u64 = 24 * 60 * 60 * 1000;

#[derive(Debug, Default, Serialize, Deserialize)]
struct OutboundActionFile {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    actions: Vec<OutboundAction>,
}

pub struct OutboundStore {
    path: PathBuf,
    actions: Mutex<Vec<OutboundAction>>,
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
        let file = if path.exists() {
            let raw = std::fs::read_to_string(&path)
                .with_context(|| format!("failed to read {}", path.display()))?;
            if raw.trim().is_empty() {
                OutboundActionFile::default()
            } else {
                serde_json::from_str(&raw)
                    .with_context(|| format!("invalid outbound action store {}", path.display()))?
            }
        } else {
            OutboundActionFile::default()
        };
        Ok(OutboundStore {
            path,
            actions: Mutex::new(file.actions),
        })
    }

    pub fn create_draft(&self, draft: NewOutboundDraft) -> Result<OutboundAction> {
        let now = now_ms();
        let action = OutboundAction {
            id: format!("oa-{}", uuid::Uuid::new_v4()),
            version: 1,
            connector_slug: draft.connector_slug,
            connection_slug: draft.connection_slug,
            action: draft.action,
            input: draft.input,
            recipient_stable_id: draft.recipient_stable_id.clone(),
            recipient_source: draft.recipient_source,
            message: draft.message.clone(),
            content_hash: draft_content_hash(&draft.recipient_stable_id, &draft.message),
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
        let mut actions = self.actions.lock().unwrap();
        actions.push(action.clone());
        write_actions(&self.path, &actions)?;
        Ok(action)
    }

    pub fn get(&self, action_id: &str) -> Result<Option<OutboundAction>> {
        Ok(self
            .actions
            .lock()
            .unwrap()
            .iter()
            .find(|action| action.id == action_id)
            .cloned())
    }

    pub fn cancel(
        &self,
        action_id: &str,
        version: u64,
        reason: Option<&str>,
    ) -> Result<OutboundAction> {
        let mut actions = self.actions.lock().unwrap();
        let action = find_action_mut(&mut actions, action_id)?;
        ensure_version(action, version)?;
        match action.status.as_str() {
            "draft_ready" | "failed" => {}
            "cancelled" | "expired" | "sent" | "sending" | "uncertain" => {
                bail!(TERMINAL_OUTBOUND_ACTION)
            }
            other => bail!("outbound action state `{other}` cannot be cancelled"),
        }
        action.status = "cancelled".to_string();
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
    ) -> Result<OutboundAction> {
        let mut actions = self.actions.lock().unwrap();
        let action = find_action_mut(&mut actions, action_id)?;
        ensure_version(action, version)?;
        match action.status.as_str() {
            "sent" => {
                if action.client_request_id.as_deref() == Some(client_request_id) {
                    return Ok(action.clone());
                }
                bail!(TERMINAL_OUTBOUND_ACTION);
            }
            "cancelled" | "expired" => bail!(TERMINAL_OUTBOUND_ACTION),
            "sending" => {
                action.status = "uncertain".to_string();
                action.events.push(lifecycle_event(
                    "send_uncertain",
                    json!({ "client_request_id": client_request_id }),
                ));
                write_actions(&self.path, &actions)?;
                bail!(DUPLICATE_RISK_ACK_REQUIRED);
            }
            "uncertain" => bail!(DUPLICATE_RISK_ACK_REQUIRED),
            "draft_ready" | "failed" => {
                if now_ms() >= action.expires_at_ms {
                    action.status = "expired".to_string();
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
            }),
        ));
        let updated = action.clone();
        write_actions(&self.path, &actions)?;
        Ok(updated)
    }

    pub fn finish_send(&self, action_id: &str, receipt: Value) -> Result<OutboundAction> {
        let mut actions = self.actions.lock().unwrap();
        let action = find_action_mut(&mut actions, action_id)?;
        if action.status != "sending" {
            bail!(
                "outbound action state `{}` cannot finish send",
                action.status
            );
        }
        action.status = "sent".to_string();
        action.receipt = Some(receipt.clone());
        action.error = None;
        action
            .events
            .push(lifecycle_event("sent", json!({ "receipt": receipt })));
        let updated = action.clone();
        write_actions(&self.path, &actions)?;
        Ok(updated)
    }

    pub fn fail_send(&self, action_id: &str, error: &str) -> Result<OutboundAction> {
        let mut actions = self.actions.lock().unwrap();
        let action = find_action_mut(&mut actions, action_id)?;
        if action.status != "sending" {
            bail!("outbound action state `{}` cannot fail send", action.status);
        }
        action.status = "failed".to_string();
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
        bail!("outbound_action_version_mismatch");
    }
    Ok(())
}

fn write_actions(path: &Path, actions: &[OutboundAction]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let tmp = path.with_extension("tmp");
    let file = OutboundActionFile {
        version: 1,
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

fn draft_content_hash(recipient_stable_id: &str, text: &str) -> String {
    let canonical = json!({
        "recipient_stable_id": recipient_stable_id,
        "text": text,
        "media": [],
    });
    let bytes = serde_json::to_vec(&canonical).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("sha256:{:x}", hasher.finalize())
}

fn now_ms() -> u64 {
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

    #[test]
    fn happy_path_draft_approve_send() {
        let (_d, store) = store();
        let action = store.create_draft(draft()).unwrap();
        assert_eq!(action.status, "draft_ready");
        assert_eq!(action.version, 1);
        let sending = store
            .begin_send(&action.id, 1, "hi edited", "req-1")
            .unwrap();
        assert_eq!(sending.status, "sending");
        assert_eq!(sending.approved_message.as_deref(), Some("hi edited"));
        let sent = store
            .finish_send(&action.id, json!({"message_id": 7}))
            .unwrap();
        assert_eq!(sent.status, "sent");
    }

    #[test]
    fn cancel_is_terminal() {
        let (_d, store) = store();
        let action = store.create_draft(draft()).unwrap();
        let cancelled = store.cancel(&action.id, 1, Some("user")).unwrap();
        assert_eq!(cancelled.status, "cancelled");
        let err = store.begin_send(&action.id, 1, "hi", "req-1").unwrap_err();
        assert!(err.to_string().contains("terminal_outbound_action"));
        let err = store.cancel(&action.id, 1, None).unwrap_err();
        assert!(err.to_string().contains("terminal_outbound_action"));
    }

    #[test]
    fn expired_draft_cannot_send() {
        let (_d, store) = store();
        let mut d = draft();
        d.ttl_ms = Some(0); // expires immediately
        let action = store.create_draft(d).unwrap();
        let err = store.begin_send(&action.id, 1, "hi", "req-1").unwrap_err();
        assert!(err.to_string().contains("outbound_action_expired"));
        assert_eq!(store.get(&action.id).unwrap().unwrap().status, "expired");
    }

    #[test]
    fn stale_sending_becomes_uncertain_and_blocks_retry() {
        let (_d, store) = store();
        let action = store.create_draft(draft()).unwrap();
        store.begin_send(&action.id, 1, "hi", "req-1").unwrap();
        // Second begin_send after a crash: mark uncertain and reject.
        let err = store.begin_send(&action.id, 1, "hi", "req-2").unwrap_err();
        assert!(err.to_string().contains("duplicate_risk_ack_required"));
        assert_eq!(store.get(&action.id).unwrap().unwrap().status, "uncertain");
        let err = store.begin_send(&action.id, 1, "hi", "req-3").unwrap_err();
        assert!(err.to_string().contains("duplicate_risk_ack_required"));
    }

    #[test]
    fn failed_send_can_retry() {
        let (_d, store) = store();
        let action = store.create_draft(draft()).unwrap();
        store.begin_send(&action.id, 1, "hi", "req-1").unwrap();
        store.fail_send(&action.id, "network down").unwrap();
        let retried = store.begin_send(&action.id, 1, "hi", "req-2").unwrap();
        assert_eq!(retried.status, "sending");
    }

    #[test]
    fn version_mismatch_rejected() {
        let (_d, store) = store();
        let action = store.create_draft(draft()).unwrap();
        assert!(store.begin_send(&action.id, 99, "hi", "req-1").is_err());
        assert!(store.cancel(&action.id, 99, None).is_err());
    }

    #[test]
    fn already_sent_is_idempotent_for_same_request_id() {
        let (_d, store) = store();
        let action = store.create_draft(draft()).unwrap();
        store.begin_send(&action.id, 1, "hi", "req-1").unwrap();
        store
            .finish_send(&action.id, json!({"message_id": 7}))
            .unwrap();
        // Replay with the same client_request_id -> returns the sent record, no error.
        let replay = store.begin_send(&action.id, 1, "hi", "req-1").unwrap();
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
}

use anyhow::{anyhow, Context, Result};
use puffer_config::ConfigPaths;
use puffer_subscriptions::outbound_store::now_ms;
use puffer_subscriptions::{
    append_gate_audit, installed_connector_action_executor, AuditEntry, OutboundAction,
    OutboundStore, AUDIT_DECISION_APPROVED_SEND, AUDIT_DECISION_CANCELLED, AUDIT_DECISION_EXPIRED,
    OUTBOUND_ACTION_EXPIRED,
};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use time::OffsetDateTime;

use crate::subscriptions::send_authorization_for_send_message_input_with_source;

use super::monitor_task_ignore::{monitor_tasks_path, task_id_matches};

#[derive(Debug, Deserialize)]
struct OutboundActionExecuteParams {
    #[serde(alias = "actionId")]
    action_id: String,
    version: u64,
    #[serde(alias = "approvedMessage")]
    approved_message: String,
    #[serde(alias = "clientRequestId")]
    client_request_id: String,
    /// Set by the client when re-approving an `uncertain` action, acknowledging
    /// the duplicate-send risk so the store permits `uncertain → sending`.
    #[serde(default, alias = "duplicateRiskAck")]
    duplicate_risk_ack: bool,
}

#[derive(Debug, Deserialize)]
struct OutboundActionCancelParams {
    #[serde(alias = "actionId")]
    action_id: String,
    version: u64,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OutboundActionStatusParams {
    #[serde(alias = "actionId")]
    action_id: String,
}

pub(crate) trait OutboundActionExecutor: Send + Sync {
    fn execute_connector_action(
        &self,
        connector_slug: &str,
        action: &str,
        input: Value,
        trigger: Value,
    ) -> Result<Value>;
}

struct InstalledOutboundActionExecutor;

impl OutboundActionExecutor for InstalledOutboundActionExecutor {
    fn execute_connector_action(
        &self,
        connector_slug: &str,
        action: &str,
        input: Value,
        trigger: Value,
    ) -> Result<Value> {
        let executor = installed_connector_action_executor()
            .context("connector action executor is not installed")?;
        let summary = executor.run_connector_action(connector_slug, action, input, trigger)?;
        Ok(json!({
            "success": true,
            "summary": summary,
            "connector_slug": connector_slug,
            "action": action,
        }))
    }
}

static ACTION_LOCKS: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();

pub(crate) fn handle_outbound_action_execute(paths: &ConfigPaths, params: &Value) -> Result<Value> {
    handle_outbound_action_execute_with_executor(paths, params, &InstalledOutboundActionExecutor)
}

pub(crate) fn handle_outbound_action_cancel(paths: &ConfigPaths, params: &Value) -> Result<Value> {
    let params: OutboundActionCancelParams =
        serde_json::from_value(params.clone()).context("invalid outbound action cancel params")?;
    let action_id = non_empty(&params.action_id)
        .context("missing action_id")?
        .to_string();
    let reason = params.reason.as_deref().and_then(non_empty);
    // Cancel must take the SAME per-action lock execute uses, or a concurrent
    // execute's finish_send can clobber the cancel (user told "cancelled" while
    // the message is sent — #561).
    let lock = outbound_action_lock(&outbound_actions_path(paths), &action_id);
    let _guard = lock.lock().unwrap();
    let store = outbound_store(paths)?;
    let action = store.cancel(&action_id, params.version, reason)?;
    append_action_audit(paths, &action, AUDIT_DECISION_CANCELLED);
    if let Some(task_id) = action.origin.task_id.as_deref() {
        // Best-effort: the action is already terminally cancelled on disk; a
        // task-reference cleanup failure must not turn a successful cancel into
        // an RPC error.
        if let Err(error) = clear_task_outbound_action(paths, task_id, &action.id) {
            eprintln!("outbound action cancel task write-back failed for `{task_id}`: {error:#}");
        }
    }
    Ok(json!({
        "actionId": action.id,
        "status": "cancelled",
    }))
}

pub(crate) fn handle_outbound_action_status(paths: &ConfigPaths, params: &Value) -> Result<Value> {
    let params: OutboundActionStatusParams =
        serde_json::from_value(params.clone()).context("invalid outbound action status params")?;
    let action_id = non_empty(&params.action_id)
        .context("missing action_id")?
        .to_string();
    let action = outbound_store(paths)?
        .get(&action_id)?
        .with_context(|| format!("outbound action `{action_id}` not found"))?;
    Ok(status_response(&action))
}

fn handle_outbound_action_execute_with_executor(
    paths: &ConfigPaths,
    params: &Value,
    executor: &dyn OutboundActionExecutor,
) -> Result<Value> {
    let params: OutboundActionExecuteParams =
        serde_json::from_value(params.clone()).context("invalid outbound action execute params")?;
    let action_id = non_empty(&params.action_id)
        .context("missing action_id")?
        .to_string();
    let approved_message = non_empty(&params.approved_message)
        .context("missing approved_message")?
        .to_string();
    let client_request_id = non_empty(&params.client_request_id)
        .context("missing client_request_id")?
        .to_string();

    let lock = outbound_action_lock(&outbound_actions_path(paths), &action_id);
    let _guard = lock.lock().unwrap();
    let store = outbound_store(paths)?;
    let action = match store.begin_send(
        &action_id,
        params.version,
        &approved_message,
        &client_request_id,
        params.duplicate_risk_ack,
    ) {
        Ok(action) => action,
        Err(error) => {
            // TTL expiry is a gate decision (spec §8): record it before
            // propagating so the audit log stays consistent.
            if error.to_string().contains(OUTBOUND_ACTION_EXPIRED) {
                if let Ok(Some(expired)) = store.get(&action_id) {
                    append_action_audit(paths, &expired, AUDIT_DECISION_EXPIRED);
                }
            }
            return Err(error);
        }
    };
    if action.status == "sent" {
        return Ok(json!({
            "actionId": action.id,
            "status": "sent",
            "receipt": action.receipt.unwrap_or(Value::Null),
        }));
    }
    append_action_audit(paths, &action, AUDIT_DECISION_APPROVED_SEND);
    let send_attempt_id = action
        .send_attempt_id
        .clone()
        .context("outbound action missing send attempt id after begin_send")?;

    let input = input_with_approved_message(&action, &approved_message)?;
    let trigger = execute_trigger(&action, &input, &approved_message, &client_request_id)?;
    let result =
        executor.execute_connector_action(&action.connector_slug, &action.action, input, trigger);
    match result {
        Ok(receipt) => {
            let sent = store.finish_send(&action.id, &send_attempt_id, receipt.clone())?;
            if let Some(task_id) = sent.origin.task_id.as_deref() {
                // Best-effort: the message IS sent and `sent` is committed on
                // disk; a task write-back failure must not surface as an RPC
                // error (which would tell the client the send failed).
                if let Err(error) = complete_task_with_receipt(paths, task_id, &sent, &receipt) {
                    eprintln!("outbound action task write-back failed for `{task_id}`: {error:#}");
                }
            }
            Ok(json!({
                "actionId": sent.id,
                "status": "sent",
                "receipt": receipt,
            }))
        }
        Err(error) => {
            let message = format!("{error:#}");
            store.fail_send(&action.id, &send_attempt_id, &message)?;
            Err(anyhow!(message))
        }
    }
}

fn outbound_store(paths: &ConfigPaths) -> Result<OutboundStore> {
    OutboundStore::load(outbound_actions_path(paths))
}

fn outbound_actions_path(paths: &ConfigPaths) -> PathBuf {
    paths.user_config_dir.join("outbound_actions.json")
}

fn outbound_audit_path(paths: &ConfigPaths) -> PathBuf {
    paths.user_config_dir.join("outbound_audit.ndjson")
}

fn outbound_action_lock(path: &Path, action_id: &str) -> Arc<Mutex<()>> {
    let key = format!("{}::{action_id}", path.display());
    let locks = ACTION_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut locks = locks.lock().unwrap();
    locks
        .entry(key)
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

fn input_with_approved_message(action: &OutboundAction, approved_message: &str) -> Result<Value> {
    let mut input = action.input.clone();
    let object = input
        .as_object_mut()
        .context("outbound action input must be an object")?;
    let message_key = ["message", "body", "text", "caption"]
        .into_iter()
        .find(|key| object.contains_key(*key))
        .unwrap_or("message");
    object.insert(
        message_key.to_string(),
        Value::String(approved_message.to_string()),
    );
    object.insert(
        "connection_slug".to_string(),
        Value::String(action.connection_slug.clone()),
    );
    object.insert(
        "connector_slug".to_string(),
        Value::String(action.connector_slug.clone()),
    );
    Ok(input)
}

fn execute_trigger(
    action: &OutboundAction,
    input: &Value,
    approved_message: &str,
    client_request_id: &str,
) -> Result<Value> {
    let mut trigger = json!({
        "type": "outbound_action_execute",
        "envelope_id": client_request_id,
        "connection_id": action.connection_slug,
        "receivedAt": OffsetDateTime::now_utc().to_string(),
        "topic": action.connection_slug,
        "kind": "connector_action",
        "dedup_key": client_request_id,
        "text": "",
        "payload": input,
        "outbound_action_id": action.id,
    });
    if action.action == "send_message" {
        trigger["send_authorization"] =
            json!(send_authorization_for_send_message_input_with_source(
                "outbound-action",
                &action.id,
                action.version,
                &action.action,
                input,
                approved_message,
                client_request_id,
            )?);
    }
    Ok(trigger)
}

fn append_action_audit(paths: &ConfigPaths, action: &OutboundAction, decision: &str) {
    append_gate_audit(
        &outbound_audit_path(paths),
        &AuditEntry {
            origin: "human".to_string(),
            connector: action.connector_slug.clone(),
            action: action.action.clone(),
            decision: decision.to_string(),
            action_id: Some(action.id.clone()),
            rule_id: None,
        },
    );
}

fn status_response(action: &OutboundAction) -> Value {
    json!({
        "actionId": action.id,
        "version": action.version,
        "status": action.status,
        "error": action.error,
        "receipt": action.receipt,
        "updatedAtMs": action_updated_at_ms(action),
    })
}

fn action_updated_at_ms(action: &OutboundAction) -> u64 {
    action
        .events
        .iter()
        .rev()
        .find_map(|event| event.get("at_ms").and_then(Value::as_u64))
        .unwrap_or(action.created_at_ms)
}

fn complete_task_with_receipt(
    paths: &ConfigPaths,
    task_id: &str,
    action: &OutboundAction,
    receipt: &Value,
) -> Result<()> {
    update_monitor_task(paths, task_id, |task| {
        let metadata = task_metadata(task)?;
        let receipts = metadata
            .entry("action_receipts".to_string())
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .context("monitor task action_receipts must be an array")?;
        receipts.push(json!({
            "outbound_action_id": action.id,
            "connector_slug": action.connector_slug,
            "connection_slug": action.connection_slug,
            "action": action.action,
            "receipt": receipt,
            "sent_at_ms": action_updated_at_ms(action),
        }));
        task.insert("status".to_string(), Value::String("completed".to_string()));
        task.insert(
            "completed_via".to_string(),
            Value::String("human_approved_outbound_action".to_string()),
        );
        task.insert("updated_at_ms".to_string(), Value::from(now_ms()));
        Ok(())
    })
}

fn clear_task_outbound_action(paths: &ConfigPaths, task_id: &str, action_id: &str) -> Result<()> {
    update_monitor_task(paths, task_id, |task| {
        let metadata = task_metadata(task)?;
        if metadata.get("outbound_action_id").and_then(Value::as_str) == Some(action_id) {
            metadata.remove("outbound_action_id");
        }
        task.insert("updated_at_ms".to_string(), Value::from(now_ms()));
        Ok(())
    })
}

fn update_monitor_task(
    paths: &ConfigPaths,
    task_id: &str,
    update: impl FnOnce(&mut Map<String, Value>) -> Result<()>,
) -> Result<()> {
    let path = monitor_tasks_path(paths);
    let raw =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut store: Value = serde_json::from_str(&raw)
        .with_context(|| format!("invalid monitor task store {}", path.display()))?;
    let task = store
        .get_mut("tasks")
        .and_then(Value::as_array_mut)
        .context("monitor task store missing tasks array")?
        .iter_mut()
        .find(|task| task_id_matches(task, task_id))
        .with_context(|| format!("monitor task `{task_id}` not found"))?;
    let task = task
        .as_object_mut()
        .context("monitor task entry must be an object")?;
    update(task)?;
    fs::write(&path, serde_json::to_string_pretty(&store)?)
        .with_context(|| format!("failed to write {}", path.display()))
}

fn task_metadata(task: &mut Map<String, Value>) -> Result<&mut Map<String, Value>> {
    if !matches!(task.get("metadata"), Some(Value::Object(_))) {
        task.insert("metadata".to_string(), Value::Object(Map::new()));
    }
    task.get_mut("metadata")
        .and_then(Value::as_object_mut)
        .context("monitor task metadata must be an object")
}

fn non_empty(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_subscriptions::{NewOutboundDraft, OutboundOrigin, OutboundStore, RecipientSource};
    use serde_json::{json, Value};
    use std::fs;
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct RecordingExecutor {
        calls: Arc<Mutex<Vec<(String, String, Value, Value)>>>,
        fail: bool,
    }

    impl OutboundActionExecutor for RecordingExecutor {
        fn execute_connector_action(
            &self,
            connector_slug: &str,
            action: &str,
            input: Value,
            trigger: Value,
        ) -> anyhow::Result<Value> {
            self.calls.lock().unwrap().push((
                connector_slug.to_string(),
                action.to_string(),
                input,
                trigger,
            ));
            if self.fail {
                anyhow::bail!("transport disconnected");
            }
            Ok(json!({
                "message_id": "telegram-message-1",
                "summary": "sent"
            }))
        }
    }

    fn test_paths(root: &Path) -> ConfigPaths {
        ConfigPaths {
            workspace_root: root.join("workspace"),
            workspace_config_dir: root.join("workspace/.puffer"),
            user_config_dir: root.join("home/.puffer"),
            builtin_resources_dir: root.join("resources"),
        }
    }

    fn outbound_store(paths: &ConfigPaths) -> OutboundStore {
        OutboundStore::load(paths.user_config_dir.join("outbound_actions.json")).unwrap()
    }

    fn monitor_tasks_path(paths: &ConfigPaths) -> std::path::PathBuf {
        paths
            .workspace_config_dir
            .join("runtime/claude_workflow/monitor_tasks.json")
    }

    fn write_monitor_task(paths: &ConfigPaths, task_id: &str, action_id: &str) {
        let path = monitor_tasks_path(paths);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            path,
            serde_json::to_string_pretty(&json!({
                "tasks": [{
                    "task_id": task_id,
                    "subject": "Reply to Telegram",
                    "description": "Draft and send a reply",
                    "active_form": "Replying",
                    "status": "pending",
                    "owner": null,
                    "blocks": [],
                    "blocked_by": [],
                    "metadata": {
                        "_monitor": true,
                        "outbound_action_id": action_id
                    },
                    "output": null
                }]
            }))
            .unwrap(),
        )
        .unwrap();
    }

    fn read_monitor_store(paths: &ConfigPaths) -> Value {
        serde_json::from_str(&fs::read_to_string(monitor_tasks_path(paths)).unwrap()).unwrap()
    }

    fn create_action(paths: &ConfigPaths, task_id: Option<&str>, ttl_ms: Option<u64>) -> String {
        let store = outbound_store(paths);
        let action = store
            .create_draft(NewOutboundDraft {
                connector_slug: "telegram-login".into(),
                connection_slug: "telegram-user".into(),
                action: "send_message".into(),
                input: json!({
                    "connection_slug": "telegram-user",
                    "connector_slug": "telegram-login",
                    "chat_id": "42",
                    "message": "draft body"
                }),
                recipient_stable_id: "telegram:42".into(),
                recipient_source: RecipientSource::Stamped,
                message: "draft body".into(),
                origin: OutboundOrigin {
                    session_id: "session-1".into(),
                    turn_id: Some("turn-1".into()),
                    task_id: task_id.map(ToOwned::to_owned),
                },
                ttl_ms,
            })
            .unwrap();
        action.id
    }

    #[test]
    fn execute_happy_path_sends_and_completes_task() {
        let temp = tempfile::tempdir().unwrap();
        let paths = test_paths(temp.path());
        let action_id = create_action(&paths, Some("monitor-1"), None);
        write_monitor_task(&paths, "monitor-1", &action_id);
        let executor = RecordingExecutor::default();

        let result = handle_outbound_action_execute_with_executor(
            &paths,
            &json!({
                "action_id": action_id,
                "version": 1,
                "approved_message": "approved body",
                "client_request_id": "req-1"
            }),
            &executor,
        )
        .unwrap();

        assert_eq!(result["status"], "sent");
        let calls = executor.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "telegram-login");
        assert_eq!(calls[0].1, "send_message");
        assert_eq!(calls[0].2["message"], "approved body");

        let task_store = read_monitor_store(&paths);
        let task = &task_store["tasks"][0];
        assert_eq!(task["status"], "completed");
        assert_eq!(
            task["metadata"]["action_receipts"][0]["outbound_action_id"],
            action_id
        );
    }

    #[test]
    fn execute_cancelled_action_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let paths = test_paths(temp.path());
        let action_id = create_action(&paths, None, None);
        handle_outbound_action_cancel(
            &paths,
            &json!({"action_id": action_id, "version": 1, "reason": "user"}),
        )
        .unwrap();
        let executor = RecordingExecutor::default();

        // Present the live version (cancel bumped it) so the terminal-state guard
        // is what rejects, not the optimistic-version guard.
        let live_version = outbound_store(&paths)
            .get(&action_id)
            .unwrap()
            .unwrap()
            .version;
        let error = handle_outbound_action_execute_with_executor(
            &paths,
            &json!({
                "action_id": action_id,
                "version": live_version,
                "approved_message": "approved body",
                "client_request_id": "req-1"
            }),
            &executor,
        )
        .unwrap_err();

        assert!(error.to_string().contains("terminal_outbound_action"));
        assert!(executor.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn cancel_then_recancel_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let paths = test_paths(temp.path());
        let action_id = create_action(&paths, None, None);

        handle_outbound_action_cancel(
            &paths,
            &json!({"action_id": action_id, "version": 1, "reason": "user"}),
        )
        .unwrap();
        // Live version after the first cancel, so the terminal guard is exercised
        // rather than the version guard.
        let live_version = outbound_store(&paths)
            .get(&action_id)
            .unwrap()
            .unwrap()
            .version;
        let error = handle_outbound_action_cancel(
            &paths,
            &json!({"action_id": action_id, "version": live_version, "reason": "again"}),
        )
        .unwrap_err();

        assert!(error.to_string().contains("terminal_outbound_action"));
    }

    #[test]
    fn execute_stale_sending_marks_uncertain() {
        let temp = tempfile::tempdir().unwrap();
        let paths = test_paths(temp.path());
        let action_id = create_action(&paths, None, None);
        outbound_store(&paths)
            .begin_send(&action_id, 1, "approved body", "req-old", false)
            .unwrap();
        let executor = RecordingExecutor::default();

        // A transition bumped the version to 2; the recovering client presents
        // the live version it read back from status.
        let live_version = outbound_store(&paths)
            .get(&action_id)
            .unwrap()
            .unwrap()
            .version;
        let error = handle_outbound_action_execute_with_executor(
            &paths,
            &json!({
                "action_id": action_id,
                "version": live_version,
                "approved_message": "approved body",
                "client_request_id": "req-1"
            }),
            &executor,
        )
        .unwrap_err();

        assert!(error.to_string().contains("duplicate_risk_ack_required"));
        assert!(executor.calls.lock().unwrap().is_empty());
        assert_eq!(
            outbound_store(&paths)
                .get(&action_id)
                .unwrap()
                .unwrap()
                .status,
            "uncertain"
        );
    }

    #[test]
    fn execute_expired_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let paths = test_paths(temp.path());
        let action_id = create_action(&paths, None, Some(0));
        let executor = RecordingExecutor::default();

        let error = handle_outbound_action_execute_with_executor(
            &paths,
            &json!({
                "action_id": action_id,
                "version": 1,
                "approved_message": "approved body",
                "client_request_id": "req-1"
            }),
            &executor,
        )
        .unwrap_err();

        assert!(error.to_string().contains("outbound_action_expired"));
        assert!(executor.calls.lock().unwrap().is_empty());

        // An `expired` decision must be recorded in the audit log (spec §8).
        let audit = fs::read_to_string(paths.user_config_dir.join("outbound_audit.ndjson"))
            .expect("expired execute must write an audit entry");
        let last: Value = serde_json::from_str(audit.lines().last().unwrap()).unwrap();
        assert_eq!(last["decision"], "expired");
        assert_eq!(last["action_id"], action_id);
    }

    #[test]
    fn execute_version_mismatch_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let paths = test_paths(temp.path());
        let action_id = create_action(&paths, None, None);
        let executor = RecordingExecutor::default();

        let error = handle_outbound_action_execute_with_executor(
            &paths,
            &json!({
                "action_id": action_id,
                "version": 99,
                "approved_message": "approved body",
                "client_request_id": "req-1"
            }),
            &executor,
        )
        .unwrap_err();

        assert!(error
            .to_string()
            .contains("outbound_action_version_mismatch"));
        assert!(executor.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn executor_failure_marks_failed_and_allows_retry() {
        let temp = tempfile::tempdir().unwrap();
        let paths = test_paths(temp.path());
        let action_id = create_action(&paths, None, None);
        let failing = RecordingExecutor {
            fail: true,
            ..RecordingExecutor::default()
        };

        let error = handle_outbound_action_execute_with_executor(
            &paths,
            &json!({
                "action_id": action_id,
                "version": 1,
                "approved_message": "approved body",
                "client_request_id": "req-1"
            }),
            &failing,
        )
        .unwrap_err();
        assert!(error.to_string().contains("transport disconnected"));
        assert_eq!(
            outbound_store(&paths)
                .get(&action_id)
                .unwrap()
                .unwrap()
                .status,
            "failed"
        );

        // The failed attempt bumped the version; the retry presents the live one.
        let live_version = outbound_store(&paths)
            .get(&action_id)
            .unwrap()
            .unwrap()
            .version;
        let executor = RecordingExecutor::default();
        let result = handle_outbound_action_execute_with_executor(
            &paths,
            &json!({
                "action_id": action_id,
                "version": live_version,
                "approved_message": "approved body",
                "client_request_id": "req-2"
            }),
            &executor,
        )
        .unwrap();

        assert_eq!(result["status"], "sent");
        assert_eq!(
            outbound_store(&paths)
                .get(&action_id)
                .unwrap()
                .unwrap()
                .status,
            "sent"
        );
    }
}

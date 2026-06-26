use anyhow::{anyhow, bail, Context, Result};
use puffer_config::ConfigPaths;
use puffer_subscriptions::installed_connector_action_executor;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::subscriptions::send_authorization_for_send_message_input_with_source;

#[derive(Debug, Deserialize)]
struct ConnectorActionExecuteParams {
    #[serde(alias = "draftId")]
    draft_id: String,
    version: u64,
    #[serde(alias = "approvedMessage")]
    approved_message: String,
    #[serde(alias = "clientRequestId")]
    client_request_id: String,
}

trait ConnectorActionDraftExecutor: Send + Sync {
    fn execute_connector_action(
        &self,
        connector_slug: &str,
        action: &str,
        input: Value,
        trigger: Value,
    ) -> Result<Value>;
}

struct InstalledConnectorActionDraftExecutor;

impl ConnectorActionDraftExecutor for InstalledConnectorActionDraftExecutor {
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

static DRAFT_LOCKS: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();

pub(crate) fn handle_connector_action_execute(
    paths: &ConfigPaths,
    params: &Value,
) -> Result<Value> {
    handle_connector_action_execute_with_executor(
        paths,
        params,
        &InstalledConnectorActionDraftExecutor,
    )
}

fn handle_connector_action_execute_with_executor(
    paths: &ConfigPaths,
    params: &Value,
    executor: &dyn ConnectorActionDraftExecutor,
) -> Result<Value> {
    let params: ConnectorActionExecuteParams = serde_json::from_value(params.clone())
        .context("invalid connector action execute params")?;
    let draft_id = non_empty(&params.draft_id)
        .context("missing draft_id")?
        .to_string();
    let approved_message = non_empty(&params.approved_message)
        .context("missing approved_message")?
        .to_string();
    let client_request_id = non_empty(&params.client_request_id)
        .context("missing client_request_id")?
        .to_string();

    let path = outbound_action_drafts_path(paths);
    let draft_lock = connector_draft_lock(&path, &draft_id);
    let _guard = draft_lock.lock().unwrap();
    let mut store = read_store(&path)?;
    let draft = find_draft_mut(&mut store, &draft_id)?;
    validate_draft_identity(draft, &draft_id, params.version)?;
    validate_draft_provenance(draft)?;
    match draft.get("status").and_then(Value::as_str) {
        Some("sent") => return Ok(json!({"status": "already_sent", "draftId": draft_id})),
        Some("sending") | Some("send_uncertain") => bail!("duplicate_risk_ack_required"),
        Some("draft_ready" | "send_failed") => {}
        Some(other) => bail!("draft state `{other}` cannot be sent"),
        None => bail!("draft missing status"),
    }

    let connector_slug = string_field(draft, &["connector_slug", "connectorSlug"])
        .context("draft missing connector_slug")?
        .to_string();
    let connection_slug = string_field(draft, &["connection_slug", "connectionSlug"])
        .context("draft missing connection_slug")?
        .to_string();
    let action = string_field(draft, &["action"])
        .context("draft missing action")?
        .to_string();
    if action != "send_message" {
        bail!("connector action draft execution only supports send_message");
    }
    let original_message = string_field(draft, &["message"]).context("draft missing message")?;
    if original_message != approved_message {
        bail!("approved message does not match draft content");
    }
    let recipient_stable_id = string_field(draft, &["recipient_stable_id", "recipientStableId"])
        .context("draft missing recipient_stable_id")?
        .to_string();
    let mut input = draft
        .get("input")
        .cloned()
        .context("draft missing action input")?;
    validate_input_recipient(&input, &recipient_stable_id)?;
    if let Some(object) = input.as_object_mut() {
        object.insert(
            "message".to_string(),
            Value::String(approved_message.clone()),
        );
        object.insert(
            "connection_slug".to_string(),
            Value::String(connection_slug.clone()),
        );
        object.insert(
            "connector_slug".to_string(),
            Value::String(connector_slug.clone()),
        );
    } else {
        bail!("draft action input must be an object");
    }
    let authorization = send_authorization_for_send_message_input_with_source(
        "connector-action-draft",
        &draft_id,
        params.version,
        &action,
        &input,
        &approved_message,
        &client_request_id,
    )?;
    let attempt_id = Uuid::new_v4().to_string();
    let now = now_ms();
    draft.insert("status".to_string(), Value::String("sending".to_string()));
    draft.insert(
        "approved_message".to_string(),
        Value::String(approved_message.clone()),
    );
    draft.insert(
        "approved_by".to_string(),
        Value::String("human".to_string()),
    );
    draft.insert("approved_at".to_string(), Value::from(now));
    draft.insert(
        "client_request_id".to_string(),
        Value::String(client_request_id.clone()),
    );
    draft.insert(
        "send_attempt_id".to_string(),
        Value::String(attempt_id.clone()),
    );
    draft.insert(
        "content_hash".to_string(),
        Value::String(authorization.content_hash.clone()),
    );
    draft.insert("error".to_string(), Value::Null);
    write_store(&path, &store)?;

    let trigger = json!({
        "type": "connector_action_execute",
        "envelope_id": client_request_id,
        "connection_id": connection_slug,
        "receivedAt": OffsetDateTime::now_utc().to_string(),
        "topic": connection_slug,
        "kind": "connector_action",
        "dedup_key": client_request_id,
        "text": "",
        "payload": input,
        "send_authorization": authorization,
    });
    let result = executor.execute_connector_action(&connector_slug, &action, input, trigger);
    let mut store = read_store(&path)?;
    let draft = find_draft_mut(&mut store, &draft_id)?;
    match result {
        Ok(receipt) => {
            draft.insert("status".to_string(), Value::String("sent".to_string()));
            draft.insert("receipt".to_string(), receipt.clone());
            draft.insert("error".to_string(), Value::Null);
            draft.insert("updated_at_ms".to_string(), Value::from(now_ms()));
            write_store(&path, &store)?;
            Ok(json!({
                "status": "sent",
                "draftId": draft_id,
                "receipt": receipt,
            }))
        }
        Err(error) => {
            draft.insert(
                "status".to_string(),
                Value::String("send_uncertain".to_string()),
            );
            draft.insert("error".to_string(), Value::String(format!("{error:#}")));
            draft.insert("updated_at_ms".to_string(), Value::from(now_ms()));
            write_store(&path, &store)?;
            Err(anyhow!("connector_action_send_uncertain: {error:#}"))
        }
    }
}

fn connector_draft_lock(path: &Path, draft_id: &str) -> Arc<Mutex<()>> {
    let key = format!("{}::{draft_id}", path.display());
    let locks = DRAFT_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut locks = locks.lock().unwrap();
    locks
        .entry(key)
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

fn outbound_action_drafts_path(paths: &ConfigPaths) -> PathBuf {
    paths.user_config_dir.join("outbound_action_drafts.json")
}

fn read_store(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(json!({"drafts": []}));
    }
    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("invalid draft store {}", path.display()))
}

fn write_store(path: &Path, store: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(path, serde_json::to_string_pretty(store)?)
        .with_context(|| format!("failed to write {}", path.display()))
}

fn find_draft_mut<'a>(store: &'a mut Value, draft_id: &str) -> Result<&'a mut Map<String, Value>> {
    store
        .get_mut("drafts")
        .and_then(Value::as_array_mut)
        .context("draft store missing drafts array")?
        .iter_mut()
        .find(|draft| draft.get("id").and_then(Value::as_str) == Some(draft_id))
        .and_then(Value::as_object_mut)
        .ok_or_else(|| anyhow!("connector action draft `{draft_id}` not found"))
}

fn validate_draft_identity(draft: &Map<String, Value>, draft_id: &str, version: u64) -> Result<()> {
    if draft.get("id").and_then(Value::as_str) != Some(draft_id) {
        bail!("draft_id mismatch");
    }
    if draft.get("version").and_then(Value::as_u64) != Some(version) {
        bail!("draft version mismatch");
    }
    Ok(())
}

fn validate_draft_provenance(draft: &Map<String, Value>) -> Result<()> {
    if draft.get("created_by").and_then(Value::as_str) != Some("ConnectorActionDraft") {
        bail!("connector action draft was not created by ConnectorActionDraft");
    }
    Ok(())
}

fn validate_input_recipient(input: &Value, recipient_stable_id: &str) -> Result<()> {
    let target = first_value(
        input,
        &[
            "to",
            "target",
            "channel",
            "chat_id",
            "open_id",
            "user",
            "receive_id",
        ],
        true,
    )
    .context("draft input missing recipient")?;
    if target != recipient_stable_id {
        bail!("draft recipient no longer matches approved recipient");
    }
    Ok(())
}

fn string_field<'a>(object: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| object.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn first_value(input: &Value, keys: &[&str], accept_numbers: bool) -> Option<String> {
    keys.iter()
        .filter_map(|key| input.get(*key))
        .find_map(|value| match value {
            Value::String(value) => non_empty(value).map(ToString::to_string),
            Value::Number(value) if accept_numbers => Some(value.to_string()),
            _ => None,
        })
}

fn non_empty(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn now_ms() -> u64 {
    OffsetDateTime::now_utc().unix_timestamp_nanos() as u64 / 1_000_000
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct RecordingExecutor {
        calls: Arc<Mutex<Vec<(String, String, Value, Value)>>>,
    }

    impl ConnectorActionDraftExecutor for RecordingExecutor {
        fn execute_connector_action(
            &self,
            connector_slug: &str,
            action: &str,
            input: Value,
            trigger: Value,
        ) -> Result<Value> {
            self.calls.lock().unwrap().push((
                connector_slug.to_string(),
                action.to_string(),
                input,
                trigger,
            ));
            Ok(json!({"ok": true}))
        }
    }

    fn write_draft_store(paths: &ConfigPaths, store: Value) {
        let path = outbound_action_drafts_path(paths);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, serde_json::to_string_pretty(&store).unwrap()).unwrap();
    }

    fn test_paths(root: &Path) -> ConfigPaths {
        ConfigPaths {
            workspace_root: root.join("workspace"),
            workspace_config_dir: root.join("workspace/.puffer"),
            user_config_dir: root.join("home/.puffer"),
            builtin_resources_dir: root.join("resources"),
        }
    }

    fn draft_store() -> Value {
        json!({
            "drafts": [{
                "id": "draft-1",
                "created_by": "ConnectorActionDraft",
                "status": "draft_ready",
                "version": 1,
                "connector_slug": "telegram-login",
                "connection_slug": "telegram-user",
                "action": "send_message",
                "input": {
                    "chat_id": 123456789,
                    "message": "draft body"
                },
                "recipient_stable_id": "123456789",
                "message": "draft body",
                "content_hash": "sha256:old"
            }]
        })
    }

    #[test]
    fn connector_action_execute_sends_with_bound_authorization_and_consumes_draft() {
        let temp = tempfile::tempdir().unwrap();
        let paths = test_paths(temp.path());
        write_draft_store(&paths, draft_store());
        let executor = RecordingExecutor::default();

        let result = handle_connector_action_execute_with_executor(
            &paths,
            &json!({
                "draftId": "draft-1",
                "version": 1,
                "approvedMessage": "draft body",
                "clientRequestId": "request-1"
            }),
            &executor,
        )
        .unwrap();

        assert_eq!(result["status"], "sent");
        let calls = executor.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "telegram-login");
        assert_eq!(calls[0].1, "send_message");
        assert_eq!(calls[0].2["message"], "draft body");
        let auth = &calls[0].3["send_authorization"];
        assert_eq!(auth["draft_id"], "draft-1");
        assert_eq!(auth["version"], 1);
        assert_eq!(auth["recipient_stable_id"], "123456789");
        assert_eq!(auth["action"], "send_message");

        let store = read_store(&outbound_action_drafts_path(&paths)).unwrap();
        assert_eq!(store["drafts"][0]["status"], "sent");
    }

    #[test]
    fn connector_action_execute_does_not_send_twice_after_success() {
        let temp = tempfile::tempdir().unwrap();
        let paths = test_paths(temp.path());
        let mut store = draft_store();
        store["drafts"][0]["status"] = json!("sent");
        write_draft_store(&paths, store);
        let executor = RecordingExecutor::default();

        let result = handle_connector_action_execute_with_executor(
            &paths,
            &json!({
                "draftId": "draft-1",
                "version": 1,
                "approvedMessage": "draft body",
                "clientRequestId": "request-2"
            }),
            &executor,
        )
        .unwrap();

        assert_eq!(result["status"], "already_sent");
        assert!(executor.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn connector_action_execute_rejects_changed_content_before_sending() {
        let temp = tempfile::tempdir().unwrap();
        let paths = test_paths(temp.path());
        write_draft_store(&paths, draft_store());
        let executor = RecordingExecutor::default();

        let err = handle_connector_action_execute_with_executor(
            &paths,
            &json!({
                "draftId": "draft-1",
                "version": 1,
                "approvedMessage": "changed body",
                "clientRequestId": "request-changed"
            }),
            &executor,
        )
        .unwrap_err();

        assert!(err.to_string().contains("approved message"));
        assert!(executor.calls.lock().unwrap().is_empty());
        let store = read_store(&outbound_action_drafts_path(&paths)).unwrap();
        assert_eq!(store["drafts"][0]["status"], "draft_ready");
    }
}

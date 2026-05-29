use crate::AppState;
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SecretValueInput {
    action: String,
    value: String,
    #[serde(default)]
    label: Option<String>,
}

/// Prepares a session-scoped secret handle for verified Lambda Skill bridges.
pub fn execute_secret_value(state: &mut AppState, _cwd: &Path, input: Value) -> Result<String> {
    let parsed: SecretValueInput =
        serde_json::from_value(input).context("invalid SecretValue input")?;
    if parsed.action != "prepare" {
        bail!("unsupported SecretValue action `{}`", parsed.action);
    }
    if parsed.value.is_empty() {
        bail!("SecretValue prepare requires a non-empty value");
    }
    let secret_id = Uuid::new_v4().to_string();
    state
        .secret_values
        .lock()
        .map_err(|_| anyhow::anyhow!("secret store lock poisoned"))?
        .insert(secret_id.clone(), parsed.value);
    let mut output = json!({
        "secretId": secret_id,
        "sec": "secret",
    });
    if let Some(label) = parsed.label.filter(|label| !label.trim().is_empty()) {
        output["label"] = json!(label);
    }
    Ok(serde_json::to_string_pretty(&output)?)
}

pub(super) fn resolve_secret_handle(state: &AppState, value: &Value) -> Result<String> {
    let Some(object) = value.as_object() else {
        bail!("secret value must be a SecretValue handle object");
    };
    if object.get("sec").and_then(Value::as_str) != Some("secret") {
        bail!("secret value handle must carry sec=secret");
    }
    let Some(secret_id) = object.get("secretId").and_then(Value::as_str) else {
        bail!("secret value handle missing secretId");
    };
    state
        .secret_values
        .lock()
        .map_err(|_| anyhow::anyhow!("secret store lock poisoned"))?
        .get(secret_id)
        .cloned()
        .with_context(|| format!("unknown SecretValue handle `{secret_id}`"))
}

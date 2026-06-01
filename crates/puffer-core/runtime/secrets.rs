use crate::AppState;
use anyhow::{bail, Result};
use serde_json::{Map, Value};
use uuid::Uuid;

const MASK_PREFIX: &str = "PUFFER_SECRET_";

/// Registers a raw secret value and returns its model-visible placeholder.
pub(crate) fn register_masked_secret(state: &AppState, value: String) -> Result<String> {
    let token = format!("{MASK_PREFIX}{}", Uuid::new_v4().simple());
    state
        .masked_secrets
        .lock()
        .map_err(|_| anyhow::anyhow!("masked secret store lock poisoned"))?
        .insert(token.clone(), value);
    Ok(token)
}

/// Replaces known secret placeholders inside a JSON tool input tree.
pub(crate) fn expand_secret_placeholders(state: &AppState, value: &Value) -> Result<Value> {
    match value {
        Value::String(text) => Ok(Value::String(expand_string(state, text)?)),
        Value::Array(items) => Ok(Value::Array(
            items
                .iter()
                .map(|item| expand_secret_placeholders(state, item))
                .collect::<Result<Vec<_>>>()?,
        )),
        Value::Object(map) => {
            let mut out = Map::new();
            for (key, item) in map {
                out.insert(key.clone(), expand_secret_placeholders(state, item)?);
            }
            Ok(Value::Object(out))
        }
        _ => Ok(value.clone()),
    }
}

/// Redacts raw secret values from text using their registered placeholders.
pub(crate) fn redact_known_secrets(state: &AppState, text: &str) -> String {
    let Ok(secrets) = state.masked_secrets.lock() else {
        return text.to_string();
    };
    let mut pairs = secrets
        .iter()
        .filter(|(_, secret)| !secret.is_empty())
        .map(|(token, secret)| (token.clone(), secret.clone()))
        .collect::<Vec<_>>();
    pairs.sort_by(|a, b| b.1.len().cmp(&a.1.len()));
    let mut redacted = text.to_string();
    for (token, secret) in pairs {
        redacted = redacted.replace(&secret, &token);
    }
    redacted
}

/// Redacts raw secret values from every string inside a JSON value.
pub(crate) fn redact_json_value(state: &AppState, value: &Value) -> Value {
    match value {
        Value::String(text) => Value::String(redact_known_secrets(state, text)),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| redact_json_value(state, item))
                .collect(),
        ),
        Value::Object(map) => {
            let mut out = Map::new();
            for (key, item) in map {
                out.insert(key.clone(), redact_json_value(state, item));
            }
            Value::Object(out)
        }
        _ => value.clone(),
    }
}

/// Redacts raw secret values from a tool execution result in place.
pub(crate) fn redact_tool_result(state: &AppState, result: &mut puffer_tools::ToolExecutionResult) {
    result.output.stdout = redact_known_secrets(state, &result.output.stdout);
    result.output.stderr = redact_known_secrets(state, &result.output.stderr);
    result.output.metadata = redact_json_value(state, &result.output.metadata);
}

fn expand_string(state: &AppState, text: &str) -> Result<String> {
    if !text.contains(MASK_PREFIX) {
        return Ok(text.to_string());
    }
    let secrets = state
        .masked_secrets
        .lock()
        .map_err(|_| anyhow::anyhow!("masked secret store lock poisoned"))?;
    let mut expanded = text.to_string();
    for (token, secret) in secrets.iter() {
        expanded = expanded.replace(token, secret);
    }
    if expanded.contains(MASK_PREFIX) {
        bail!("tool input references an unknown Puffer secret placeholder");
    }
    Ok(expanded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppState;
    use puffer_config::PufferConfig;
    use puffer_session_store::SessionMetadata;
    use serde_json::json;
    use std::path::PathBuf;
    use uuid::Uuid;

    fn temp_state() -> AppState {
        AppState::new(
            PufferConfig::default(),
            PathBuf::from("."),
            SessionMetadata {
                id: Uuid::new_v4(),
                display_name: None,
                generated_title: None,
                cwd: PathBuf::from("."),
                created_at_ms: 0,
                updated_at_ms: 0,
                parent_session_id: None,
                slug: None,
                tags: Vec::new(),
                note: None,
            },
        )
    }

    #[test]
    fn expands_masked_tokens_inside_tool_inputs() {
        let state = temp_state();
        let token = register_masked_secret(&state, "raw-secret".to_string()).unwrap();
        let expanded = expand_secret_placeholders(
            &state,
            &json!({"command": format!("echo {token}"), "args": [token]}),
        )
        .unwrap();
        assert_eq!(expanded["command"], "echo raw-secret");
        assert_eq!(expanded["args"][0], "raw-secret");
    }

    #[test]
    fn redacts_registered_secret_values_from_outputs() {
        let state = temp_state();
        let token = register_masked_secret(&state, "raw-secret".to_string()).unwrap();
        assert_eq!(
            redact_known_secrets(&state, "value=raw-secret"),
            format!("value={token}")
        );
        assert_eq!(
            redact_json_value(&state, &json!({"nested": ["raw-secret"]})),
            json!({"nested": [token]})
        );
    }
}

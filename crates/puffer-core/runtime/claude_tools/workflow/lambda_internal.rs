use crate::AppState;
use anyhow::Result;
use serde_json::{json, Value};
use std::path::Path;

/// Executes the `LambdaInternal` verified-skill internal step tool.
pub fn execute_lambda_internal(state: &mut AppState, cwd: &Path, input: Value) -> Result<String> {
    let _ = state;
    let _ = cwd;
    if let Some(result) = input.get("result") {
        return Ok(match result {
            Value::String(text) => text.clone(),
            other => serde_json::to_string(other)?,
        });
    }
    Ok(serde_json::to_string(&json!({
        "step": input.get("step").cloned().unwrap_or(Value::Null),
        "args": input.get("args").cloned().unwrap_or_else(|| json!({})),
    }))?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_config::PufferConfig;
    use puffer_session_store::SessionMetadata;
    use serde_json::json;
    use uuid::Uuid;

    fn temp_state() -> AppState {
        let tempdir = tempfile::tempdir().unwrap();
        let cwd = tempdir.path().to_path_buf();
        std::mem::forget(tempdir);
        let session = SessionMetadata {
            id: Uuid::new_v4(),
            display_name: None,
            generated_title: None,
            cwd: cwd.clone(),
            created_at_ms: 0,
            updated_at_ms: 0,
            parent_session_id: None,
            slug: None,
            tags: Vec::new(),
            note: None,
        };
        AppState::new(PufferConfig::default(), cwd, session)
    }

    #[test]
    fn lambda_internal_returns_declared_result() {
        let mut state = temp_state();
        let cwd = state.cwd.clone();
        let output = execute_lambda_internal(
            &mut state,
            &cwd,
            json!({
                "step": "support-ticket-triage.parse_context",
                "args": {"ticket": "refund request"},
                "result": "parsed ticket"
            }),
        )
        .unwrap();
        assert_eq!(output, "parsed ticket");
    }

    #[test]
    fn lambda_internal_echoes_step_without_result() {
        let mut state = temp_state();
        let cwd = state.cwd.clone();
        let output = execute_lambda_internal(
            &mut state,
            &cwd,
            json!({
                "step": "support-ticket-triage.parse_context",
                "args": {"ticket": "refund request"}
            }),
        )
        .unwrap();
        let parsed: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed["step"], json!("support-ticket-triage.parse_context"));
        assert_eq!(parsed["args"]["ticket"], json!("refund request"));
    }
}

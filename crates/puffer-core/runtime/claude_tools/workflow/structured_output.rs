use crate::runtime::structured_output_support::{
    validate_structured_output_payload, StructuredOutputConfig,
};
use crate::AppState;
use anyhow::Result;
use serde_json::json;
use serde_json::Value;
use std::path::Path;

/// Executes the Claude-compatible `StructuredOutput` workflow tool.
pub fn execute_structured_output(
    state: &mut AppState,
    cwd: &Path,
    input: Value,
    structured_output: Option<&StructuredOutputConfig>,
) -> Result<String> {
    let _ = state;
    let _ = cwd;
    if let Some(structured_output) = structured_output {
        validate_structured_output_payload(structured_output, &input)?;
    }
    Ok(serde_json::to_string_pretty(&json!({
        "data": "Structured output provided successfully",
        "structured_output": input
    }))?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::structured_output_support::StructuredOutputConfig;
    use crate::AppState;
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
    fn structured_output_falls_back_without_request_schema() {
        let mut state = temp_state();
        let cwd = state.cwd.clone();
        let output =
            execute_structured_output(&mut state, &cwd, json!({"value": 1}), None).unwrap();
        let parsed: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed["structured_output"]["value"], json!(1));
    }

    #[test]
    fn structured_output_validates_request_schema() {
        let mut state = temp_state();
        let cwd = state.cwd.clone();
        let config = StructuredOutputConfig::new(
            "answer",
            json!({
                "type": "object",
                "properties": {
                    "value": { "type": "string" }
                },
                "required": ["value"],
                "additionalProperties": false
            }),
        );
        let error = execute_structured_output(&mut state, &cwd, json!({"value": 1}), Some(&config))
            .unwrap_err()
            .to_string();
        assert!(error.contains("Output does not match required schema"));
    }
}

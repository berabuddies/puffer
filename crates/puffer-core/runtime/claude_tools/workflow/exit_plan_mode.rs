use crate::plan_mode::exit_plan_mode;
use crate::plans::{plan_file_path, read_plan_text};
use crate::AppState;
use anyhow::bail;
use anyhow::Result;
use serde_json::Value;
use std::path::Path;

/// Executes the Claude-compatible `ExitPlanMode` tool scaffold.
pub fn execute_exit_plan_mode(state: &mut AppState, cwd: &Path, input: Value) -> Result<String> {
    let _ = cwd;
    if !input.is_null() && !input.is_object() {
        bail!("invalid ExitPlanMode input");
    }
    if !state.plan_mode {
        bail!("ExitPlanMode can only be used while plan mode is active");
    }
    let plan_path = plan_file_path(state)?;
    let plan = read_plan_text(state)?;
    exit_plan_mode(state);
    Ok(serde_json::to_string_pretty(&serde_json::json!({
        "isAgent": false,
        "filePath": plan_path.display().to_string(),
        "plan": plan,
    }))?)
}

#[cfg(test)]
mod tests {
    use super::execute_exit_plan_mode;
    use crate::plans::{persist_plan_output, plan_file_path};
    use crate::AppState;
    use puffer_config::{ensure_workspace_dirs, ConfigPaths, PufferConfig};
    use puffer_session_store::SessionStore;
    use serde_json::{json, Value};
    use tempfile::tempdir;

    fn state() -> AppState {
        let tempdir = tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        ensure_workspace_dirs(&paths).unwrap();
        let session_store = SessionStore::from_paths(&paths).unwrap();
        let session = session_store
            .create_session(tempdir.path().to_path_buf())
            .unwrap();
        AppState::new(PufferConfig::default(), tempdir.keep(), session)
    }

    #[test]
    fn exit_plan_mode_does_not_create_a_default_plan_file() {
        let mut state = state();
        state.plan_mode = true;
        let cwd = state.cwd.clone();
        let expected_path = plan_file_path(&state).unwrap().display().to_string();

        let output = execute_exit_plan_mode(&mut state, &cwd, json!({})).unwrap();
        let payload: Value = serde_json::from_str(&output).unwrap();

        assert_eq!(payload.get("isAgent").and_then(Value::as_bool), Some(false));
        assert_eq!(
            payload.get("filePath").and_then(Value::as_str),
            Some(expected_path.as_str())
        );
        assert!(payload.get("plan").is_some_and(Value::is_null));
        assert!(!state.plan_mode);
        assert!(!plan_file_path(&state).unwrap().exists());
    }

    #[test]
    fn exit_plan_mode_returns_existing_plan_contents() {
        let mut state = state();
        state.plan_mode = true;
        let cwd = state.cwd.clone();
        persist_plan_output(&state, "# Current Plan\n\n1. Verify tooling.\n").unwrap();

        let output = execute_exit_plan_mode(&mut state, &cwd, json!({})).unwrap();
        let payload: Value = serde_json::from_str(&output).unwrap();

        assert_eq!(
            payload.get("plan").and_then(Value::as_str),
            Some("# Current Plan\n\n1. Verify tooling.\n")
        );
        assert!(!state.plan_mode);
    }
}

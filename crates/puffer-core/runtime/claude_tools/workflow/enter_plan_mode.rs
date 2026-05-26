use crate::plan_mode::enter_plan_mode;
use crate::AppState;
use anyhow::bail;
use anyhow::Result;
use serde_json::json;
use serde_json::Value;
use std::path::Path;

/// Executes the Claude-compatible `EnterPlanMode` tool scaffold.
pub fn execute_enter_plan_mode(state: &mut AppState, cwd: &Path, input: Value) -> Result<String> {
    let _ = cwd;
    if !input.is_null() && !input.is_object() {
        bail!("invalid EnterPlanMode input");
    }
    if state.plan_mode {
        return Ok(serde_json::to_string_pretty(&json!({
            "message": "Already in plan mode. Continue exploring the codebase and refining your implementation plan."
        }))?);
    }
    enter_plan_mode(state)?;
    Ok(serde_json::to_string_pretty(&json!({
        "message": "Entered plan mode. You should now focus on exploring the codebase and designing an implementation approach."
    }))?)
}

#[cfg(test)]
mod tests {
    use super::execute_enter_plan_mode;
    use crate::plans::plan_file_path;
    use crate::AppState;
    use puffer_config::{ensure_workspace_dirs, ConfigPaths, PufferConfig};
    use puffer_session_store::SessionStore;
    use serde_json::Value;
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
    fn enter_plan_mode_does_not_materialize_a_default_plan_file() {
        let mut state = state();
        let cwd = state.cwd.clone();

        let output =
            execute_enter_plan_mode(&mut state, &cwd, Value::Object(Default::default())).unwrap();
        let payload: Value = serde_json::from_str(&output).unwrap();

        assert_eq!(
            payload.get("message").and_then(Value::as_str),
            Some(
                "Entered plan mode. You should now focus on exploring the codebase and designing an implementation approach."
            )
        );
        assert!(state.plan_mode);
        assert!(!plan_file_path(&state).unwrap().exists());
    }
}

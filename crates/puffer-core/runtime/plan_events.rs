use crate::plans::{plan_file_path, read_plan_text};
use crate::AppState;
use serde_json::Value;
use std::path::{Path, PathBuf};

use super::{ToolInvocation, TurnStreamEvent};

/// Builds plan-mode stream events produced by successful tool invocations.
pub(super) fn stream_events_for_tool_invocations(
    state: &AppState,
    invocations: &[ToolInvocation],
) -> Vec<TurnStreamEvent> {
    invocations
        .iter()
        .filter_map(|invocation| stream_event_for_tool_invocation(state, invocation))
        .collect()
}

fn stream_event_for_tool_invocation(
    state: &AppState,
    invocation: &ToolInvocation,
) -> Option<TurnStreamEvent> {
    if !invocation.success {
        return None;
    }
    if invocation.tool_id == "ExitPlanMode" {
        return plan_completed_event(state, invocation);
    }
    if !state.plan_mode || !matches!(invocation.tool_id.as_str(), "Write" | "Edit") {
        return None;
    }
    if !invocation_targets_active_plan(state, &invocation.input) {
        return None;
    }
    let path = plan_file_path(state).ok()?;
    Some(TurnStreamEvent::PlanUpdated {
        file_path: path.display().to_string(),
        content: read_plan_text(state).ok().flatten(),
    })
}

fn plan_completed_event(state: &AppState, invocation: &ToolInvocation) -> Option<TurnStreamEvent> {
    let output = serde_json::from_str::<Value>(&invocation.output).ok();
    let file_path = output
        .as_ref()
        .and_then(|output| {
            output
                .get("filePath")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| {
            plan_file_path(state)
                .ok()
                .map(|path| path.display().to_string())
        })?;
    let content = output
        .as_ref()
        .and_then(|output| {
            output
                .get("plan")
                .and_then(|value| (!value.is_null()).then_some(value))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| read_plan_text(state).ok().flatten());

    Some(TurnStreamEvent::PlanCompleted { file_path, content })
}

fn invocation_targets_active_plan(state: &AppState, input: &str) -> bool {
    let Some(input_path) = input_file_path(input) else {
        return false;
    };
    let Ok(plan_path) = plan_file_path(state) else {
        return false;
    };
    paths_equal(&state.cwd, &input_path, &plan_path)
}

fn input_file_path(input: &str) -> Option<PathBuf> {
    let value: Value = serde_json::from_str(input).ok()?;
    value
        .get("filePath")
        .or_else(|| value.get("file_path"))
        .or_else(|| value.get("path"))
        .and_then(Value::as_str)
        .map(PathBuf::from)
}

fn paths_equal(cwd: &Path, left: &Path, right: &Path) -> bool {
    let left = absolute_path(cwd, left);
    let right = absolute_path(cwd, right);
    if left == right {
        return true;
    }
    let canonical_left = std::fs::canonicalize(&left).unwrap_or(left);
    let canonical_right = std::fs::canonicalize(&right).unwrap_or(right);
    canonical_left == canonical_right
}

fn absolute_path(cwd: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::stream_events_for_tool_invocations;
    use crate::plans::{persist_plan_output, plan_file_path};
    use crate::runtime::{ToolInvocation, TurnStreamEvent};
    use crate::AppState;
    use puffer_config::{ensure_workspace_dirs, ConfigPaths, PufferConfig};
    use puffer_session_store::SessionStore;
    use serde_json::{json, Value};
    use tempfile::{tempdir, TempDir};

    fn plan_state() -> (TempDir, AppState) {
        let tempdir = tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        ensure_workspace_dirs(&paths).unwrap();
        let session_store = SessionStore::from_paths(&paths).unwrap();
        let session = session_store
            .create_session(tempdir.path().to_path_buf())
            .unwrap();
        let mut state = AppState::new(
            PufferConfig::default(),
            tempdir.path().to_path_buf(),
            session,
        );
        state.plan_mode = true;
        (tempdir, state)
    }

    fn invocation(
        tool_id: &str,
        input: serde_json::Value,
        output: serde_json::Value,
    ) -> ToolInvocation {
        ToolInvocation {
            call_id: "call-1".to_string(),
            tool_id: tool_id.to_string(),
            input: input.to_string(),
            output: output.to_string(),
            success: true,
            metadata: Value::Null,
            terminate: false,
        }
    }

    #[test]
    fn write_to_active_plan_emits_plan_updated() {
        let (_tempdir, state) = plan_state();
        let plan_text = "# Current Plan\n\n1. Wire daemon plan mode.\n";
        let plan_path = persist_plan_output(&state, plan_text).unwrap();
        let events = stream_events_for_tool_invocations(
            &state,
            &[invocation(
                "Write",
                json!({
                    "file_path": plan_path.display().to_string(),
                    "content": plan_text,
                }),
                json!({"ok": true}),
            )],
        );

        assert_eq!(
            events,
            vec![TurnStreamEvent::PlanUpdated {
                file_path: plan_path.display().to_string(),
                content: Some(plan_text.to_string()),
            }]
        );
    }

    #[test]
    fn write_to_non_plan_file_does_not_emit_plan_updated() {
        let (_tempdir, state) = plan_state();
        let plan_text = "# Current Plan\n\n1. Wire daemon plan mode.\n";
        persist_plan_output(&state, plan_text).unwrap();
        let events = stream_events_for_tool_invocations(
            &state,
            &[invocation(
                "Write",
                json!({
                    "file_path": state.cwd.join("notes.md").display().to_string(),
                    "content": plan_text,
                }),
                json!({"ok": true}),
            )],
        );

        assert!(events.is_empty());
    }

    #[test]
    fn exit_plan_mode_emits_plan_completed_from_tool_output() {
        let (_tempdir, state) = plan_state();
        let plan_text = "# Current Plan\n\n1. Confirm plan mode support.\n";
        persist_plan_output(&state, plan_text).unwrap();
        let plan_path = plan_file_path(&state).unwrap();
        let events = stream_events_for_tool_invocations(
            &state,
            &[invocation(
                "ExitPlanMode",
                json!({}),
                json!({
                    "isAgent": false,
                    "filePath": plan_path.display().to_string(),
                    "plan": plan_text,
                }),
            )],
        );

        assert_eq!(
            events,
            vec![TurnStreamEvent::PlanCompleted {
                file_path: plan_path.display().to_string(),
                content: Some(plan_text.to_string()),
            }]
        );
    }
}

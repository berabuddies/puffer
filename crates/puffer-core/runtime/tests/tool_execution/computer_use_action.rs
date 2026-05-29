use super::*;
use puffer_runner_api::{
    ChunkSink, DirEntry, McpPrompt, McpPromptContent, McpResourceContent, McpResourceRecord,
    McpResult, McpServerInfo, McpTool, RunnerCapabilities, RunnerError, RunnerPing, ToolRequest,
    ToolResult,
};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug, Default)]
struct RecordingRunner {
    calls: Mutex<Vec<(String, String, Value)>>,
    stdout: Mutex<String>,
}

impl RecordingRunner {
    fn with_stdout(stdout: &str) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            stdout: Mutex::new(stdout.to_string()),
        }
    }

    fn calls(&self) -> Vec<(String, String, Value)> {
        self.calls.lock().unwrap().clone()
    }
}

impl puffer_runner_api::ToolRunner for RecordingRunner {
    fn ping(&self) -> Result<RunnerPing, RunnerError> {
        Ok(RunnerPing {
            version: "test".to_string(),
            uptime: Duration::ZERO,
        })
    }

    fn capabilities(&self) -> RunnerCapabilities {
        RunnerCapabilities::default()
    }

    fn execute_tool(
        &self,
        _: ToolRequest,
        _: &mut dyn ChunkSink,
    ) -> Result<ToolResult, RunnerError> {
        Err(RunnerError::Unsupported("not used".to_string()))
    }

    fn read_file(&self, _: &Path) -> Result<Vec<u8>, RunnerError> {
        Err(RunnerError::Unsupported("not used".to_string()))
    }

    fn list_dir(&self, _: &Path) -> Result<Vec<DirEntry>, RunnerError> {
        Err(RunnerError::Unsupported("not used".to_string()))
    }

    fn glob(&self, _: &Path, _: &str) -> Result<Vec<PathBuf>, RunnerError> {
        Err(RunnerError::Unsupported("not used".to_string()))
    }

    fn list_mcp_servers(&self) -> Result<Vec<McpServerInfo>, RunnerError> {
        Ok(Vec::new())
    }

    fn list_mcp_tools(&self, _: &str) -> Result<Vec<McpTool>, RunnerError> {
        Ok(Vec::new())
    }

    fn call_mcp_tool(
        &self,
        server: &str,
        tool: &str,
        args: Value,
        _: &mut dyn ChunkSink,
    ) -> Result<McpResult, RunnerError> {
        self.calls
            .lock()
            .unwrap()
            .push((server.to_string(), tool.to_string(), args.clone()));
        Ok(McpResult {
            server: server.to_string(),
            tool: tool.to_string(),
            success: true,
            stdout: self.stdout.lock().unwrap().clone(),
            stderr: String::new(),
            metadata: json!({ "args": args }),
        })
    }

    fn list_mcp_resources(&self, _: Option<&str>) -> Result<Vec<McpResourceRecord>, RunnerError> {
        Err(RunnerError::Unsupported("not used".to_string()))
    }

    fn read_mcp_resource(&self, _: &str, _: &str) -> Result<McpResourceContent, RunnerError> {
        Err(RunnerError::Unsupported("not used".to_string()))
    }

    fn list_mcp_prompts(&self, _: &str) -> Result<Vec<McpPrompt>, RunnerError> {
        Err(RunnerError::Unsupported("not used".to_string()))
    }

    fn get_mcp_prompt(&self, _: &str, _: &str, _: Value) -> Result<McpPromptContent, RunnerError> {
        Err(RunnerError::Unsupported("not used".to_string()))
    }
}

#[test]
fn computer_use_capture_sets_active_app_for_key_combo() {
    let runner = Arc::new(RecordingRunner::default());
    let mut state = temp_state().with_tool_runner(runner.clone());
    let cwd = state.cwd.clone();

    crate::runtime::claude_tools::workflow::computer_use_action::execute_computer_use_action(
        &mut state,
        &cwd,
        json!({
            "action": "capture",
            "mode": "ax",
            "app": "Safari"
        }),
    )
    .unwrap();
    crate::runtime::claude_tools::workflow::computer_use_action::execute_computer_use_action(
        &mut state,
        &cwd,
        json!({
            "action": "keyCombo",
            "keys": "Return"
        }),
    )
    .unwrap();

    let calls = runner.calls();
    assert_eq!(calls[0].1, "get_app_state");
    assert_eq!(calls[1].1, "press_key");
    assert_eq!(calls[1].2["app"], "Safari");
    assert_eq!(calls[1].2["key"], "Return");
}

#[test]
fn computer_use_click_coord_maps_coordinates() {
    let runner = Arc::new(RecordingRunner::default());
    let mut state = temp_state().with_tool_runner(runner.clone());
    let cwd = state.cwd.clone();

    crate::runtime::claude_tools::workflow::computer_use_action::execute_computer_use_action(
        &mut state,
        &cwd,
        json!({
            "action": "clickCoord",
            "app": "Finder",
            "x": 10,
            "y": 20
        }),
    )
    .unwrap();

    let calls = runner.calls();
    assert_eq!(calls[0].0, "computer_use");
    assert_eq!(calls[0].1, "click");
    assert_eq!(calls[0].2["app"], "Finder");
    assert_eq!(calls[0].2["x"], 10);
    assert_eq!(calls[0].2["y"], 20);
}

#[test]
fn computer_use_focus_app_binds_active_app_without_raising() {
    let runner = Arc::new(RecordingRunner::default());
    let mut state = temp_state().with_tool_runner(runner.clone());
    let cwd = state.cwd.clone();

    crate::runtime::claude_tools::workflow::computer_use_action::execute_computer_use_action(
        &mut state,
        &cwd,
        json!({
            "action": "focusApp",
            "app": "Safari",
            "raiseWindow": false
        }),
    )
    .unwrap();
    crate::runtime::claude_tools::workflow::computer_use_action::execute_computer_use_action(
        &mut state,
        &cwd,
        json!({
            "action": "typeText",
            "text": "hello"
        }),
    )
    .unwrap();

    let calls = runner.calls();
    assert_eq!(calls[0].0, "computer_use");
    assert_eq!(calls[0].1, "list_apps");
    assert_eq!(calls[0].2, json!({}));
    assert_eq!(calls[1].1, "type_text");
    assert_eq!(calls[1].2["app"], "Safari");
    assert_eq!(calls[1].2["text"], "hello");
}

#[test]
fn computer_use_focus_app_can_raise_window() {
    let runner = Arc::new(RecordingRunner::default());
    let mut state = temp_state().with_tool_runner(runner.clone());
    let cwd = state.cwd.clone();

    crate::runtime::claude_tools::workflow::computer_use_action::execute_computer_use_action(
        &mut state,
        &cwd,
        json!({
            "action": "focusApp",
            "app": "Finder",
            "raiseWindow": true
        }),
    )
    .unwrap();

    let calls = runner.calls();
    assert_eq!(calls[0].0, "computer_use");
    assert_eq!(calls[0].1, "get_app_state");
    assert_eq!(calls[0].2["app"], "Finder");
}

#[test]
fn computer_use_drag_resolves_element_centers_from_capture() {
    let runner = Arc::new(RecordingRunner::with_stdout(
        "#3  AXButton 'Source' @ (10, 20, 30, 40) [Finder]\n\
         #17 AXButton 'Target' @ (100, 200, 20, 10) [Finder]\n",
    ));
    let mut state = temp_state().with_tool_runner(runner.clone());
    let cwd = state.cwd.clone();

    crate::runtime::claude_tools::workflow::computer_use_action::execute_computer_use_action(
        &mut state,
        &cwd,
        json!({
            "action": "capture",
            "mode": "ax",
            "app": "Finder"
        }),
    )
    .unwrap();
    crate::runtime::claude_tools::workflow::computer_use_action::execute_computer_use_action(
        &mut state,
        &cwd,
        json!({
            "action": "drag",
            "fromElement": 3,
            "toElement": 17
        }),
    )
    .unwrap();

    let calls = runner.calls();
    assert_eq!(calls[1].0, "computer_use");
    assert_eq!(calls[1].1, "drag");
    assert_eq!(calls[1].2["app"], "Finder");
    assert_eq!(calls[1].2["from_x"], 25);
    assert_eq!(calls[1].2["from_y"], 40);
    assert_eq!(calls[1].2["to_x"], 110);
    assert_eq!(calls[1].2["to_y"], 205);
}

#[test]
fn computer_use_drag_requires_captured_element_bounds() {
    let runner = Arc::new(RecordingRunner::with_stdout(
        "#3  AXButton 'Source' @ (10, 20, 30, 40) [Finder]\n",
    ));
    let mut state = temp_state().with_tool_runner(runner);
    let cwd = state.cwd.clone();

    crate::runtime::claude_tools::workflow::computer_use_action::execute_computer_use_action(
        &mut state,
        &cwd,
        json!({
            "action": "capture",
            "mode": "ax",
            "app": "Finder"
        }),
    )
    .unwrap();
    let error =
        crate::runtime::claude_tools::workflow::computer_use_action::execute_computer_use_action(
            &mut state,
            &cwd,
            json!({
                "action": "drag",
                "fromElement": 3,
                "toElement": 99
            }),
        )
        .expect_err("drag must fail closed without both captured elements");

    assert!(format!("{error:#}").contains("requires captured bounds for element 99"));
}

#[test]
fn computer_use_app_less_action_requires_active_app() {
    let mut state = temp_state().with_tool_runner(Arc::new(RecordingRunner::default()));
    let cwd = state.cwd.clone();

    let error =
        crate::runtime::claude_tools::workflow::computer_use_action::execute_computer_use_action(
            &mut state,
            &cwd,
            json!({
                "action": "typeText",
                "text": "hello"
            }),
        )
        .expect_err("app-less actions must fail without a current capture");

    assert!(format!("{error:#}").contains("requires capture or an app-targeted action"));
}

#[test]
fn computer_use_capture_rejects_unknown_mode() {
    let mut state = temp_state().with_tool_runner(Arc::new(RecordingRunner::default()));
    let cwd = state.cwd.clone();

    let error =
        crate::runtime::claude_tools::workflow::computer_use_action::execute_computer_use_action(
            &mut state,
            &cwd,
            json!({
                "action": "capture",
                "mode": "pixels",
                "app": "Safari"
            }),
        )
        .expect_err("unsupported capture modes must fail closed");

    assert!(format!("{error:#}").contains("capture mode `pixels` is unsupported"));
}

use super::store::{load_store, save_store, workflow_root};
use crate::AppState;
use anyhow::{anyhow, bail, Context, Result};
use puffer_runner_api::{ChunkSink, McpResult, NullChunkSink, RunnerError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const COMPUTER_USE_SERVER: &str = "computer_use";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ComputerUseInput {
    action: String,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    app: Option<String>,
    #[serde(default)]
    element: Option<i64>,
    #[serde(default)]
    x: Option<i64>,
    #[serde(default)]
    y: Option<i64>,
    #[serde(default)]
    direction: Option<String>,
    #[serde(default)]
    amount: Option<i64>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    keys: Option<String>,
    #[serde(default)]
    modifiers: Vec<String>,
    #[serde(default, rename = "captureAfter", alias = "capture_after")]
    capture_after: bool,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ComputerUseStore {
    active_app: Option<String>,
}

/// Executes one typed macOS computer-use operation through Puffer's MCP runner.
pub fn execute_computer_use_action(
    state: &mut AppState,
    cwd: &Path,
    input: Value,
) -> Result<String> {
    let parsed: ComputerUseInput =
        serde_json::from_value(input).context("invalid ComputerUseAction input")?;
    let output = match parsed.action.as_str() {
        "capture" => capture(state, cwd, parsed)?,
        "clickElement" => click_element(state, cwd, parsed)?,
        "clickCoord" => click_coord(state, cwd, parsed)?,
        "doubleClick" => element_click(state, cwd, parsed, 2, None)?,
        "rightClick" => element_click(state, cwd, parsed, 1, Some("right"))?,
        "scroll" => scroll(state, cwd, parsed)?,
        "typeText" => type_text(state, cwd, parsed)?,
        "keyCombo" => key_combo(state, cwd, parsed)?,
        other => bail!("unsupported ComputerUseAction action `{other}`"),
    };
    Ok(serde_json::to_string_pretty(&output)?)
}

fn capture(state: &mut AppState, cwd: &Path, input: ComputerUseInput) -> Result<Value> {
    let mode = required_capture_mode(input.mode.as_deref())?;
    let app = required_string(input.app, "app")?;
    save_active_app(cwd, &app)?;
    let mut output = format_result(call_mcp(state, "get_app_state", json!({ "app": app }))?);
    output["mode"] = json!(mode);
    Ok(output)
}

fn click_element(state: &mut AppState, cwd: &Path, input: ComputerUseInput) -> Result<Value> {
    if !input.modifiers.is_empty() {
        bail!("ComputerUseAction clickElement does not support modifiers");
    }
    let app = required_string(input.app, "app")?;
    save_active_app(cwd, &app)?;
    let mut output = format_result(call_mcp(
        state,
        "click",
        json!({
            "app": app,
            "element_index": required_element(input.element)?,
        }),
    )?);
    if input.capture_after {
        let capture = call_mcp(state, "get_app_state", json!({ "app": active_app(cwd)? }))?;
        output["captureAfter"] = format_result(capture);
    }
    Ok(output)
}

fn click_coord(state: &mut AppState, cwd: &Path, input: ComputerUseInput) -> Result<Value> {
    let app = required_string(input.app, "app")?;
    save_active_app(cwd, &app)?;
    let result = call_mcp(
        state,
        "click",
        json!({
            "app": app,
            "x": required_i64(input.x, "x")?,
            "y": required_i64(input.y, "y")?,
        }),
    )?;
    Ok(format_result(result))
}

fn element_click(
    state: &mut AppState,
    cwd: &Path,
    input: ComputerUseInput,
    click_count: i64,
    mouse_button: Option<&str>,
) -> Result<Value> {
    let mut args = json!({
        "app": active_app(cwd)?,
        "element_index": required_element(input.element)?,
        "click_count": click_count,
    });
    if let Some(button) = mouse_button {
        args["mouse_button"] = json!(button);
    }
    let result = call_mcp(state, "click", args)?;
    Ok(format_result(result))
}

fn scroll(state: &mut AppState, cwd: &Path, input: ComputerUseInput) -> Result<Value> {
    let result = call_mcp(
        state,
        "scroll",
        json!({
            "app": active_app(cwd)?,
            "direction": required_string(input.direction, "direction")?,
            "element_index": required_element(input.element)?,
            "pages": required_i64(input.amount, "amount")?,
        }),
    )?;
    Ok(format_result(result))
}

fn type_text(state: &mut AppState, cwd: &Path, input: ComputerUseInput) -> Result<Value> {
    let result = call_mcp(
        state,
        "type_text",
        json!({
            "app": active_app(cwd)?,
            "text": required_string(input.text, "text")?,
        }),
    )?;
    Ok(format_result(result))
}

fn key_combo(state: &mut AppState, cwd: &Path, input: ComputerUseInput) -> Result<Value> {
    let result = call_mcp(
        state,
        "press_key",
        json!({
            "app": active_app(cwd)?,
            "key": required_string(input.keys, "keys")?,
        }),
    )?;
    Ok(format_result(result))
}

fn call_mcp(state: &AppState, tool: &str, args: Value) -> Result<McpResult> {
    let mut sink = NullChunkSink;
    state
        .tool_runner
        .call_mcp_tool(
            COMPUTER_USE_SERVER,
            tool,
            args,
            &mut sink as &mut dyn ChunkSink,
        )
        .map_err(map_runner_error)
}

fn map_runner_error(error: RunnerError) -> anyhow::Error {
    match error {
        RunnerError::NotFound(message) => anyhow!("computer_use MCP target not found: {message}"),
        RunnerError::PermissionDenied(message) => anyhow!("computer_use MCP denied: {message}"),
        RunnerError::Unsupported(message) => anyhow!("computer_use MCP unsupported: {message}"),
        RunnerError::InvalidArgument(message) => {
            anyhow!("computer_use MCP invalid argument: {message}")
        }
        RunnerError::Mcp(message) => anyhow!("computer_use MCP call failed: {message}"),
        RunnerError::Transport(message) => anyhow!("computer_use MCP transport error: {message}"),
        RunnerError::OAuthRequired {
            server_id,
            authorization_url,
        } => {
            let suffix = authorization_url
                .map(|url| format!("; authorize at {url}"))
                .unwrap_or_default();
            anyhow!("computer_use MCP requires OAuth for server `{server_id}`{suffix}")
        }
        RunnerError::Execution(message) => anyhow!("computer_use MCP execution failed: {message}"),
        RunnerError::Other(message) => anyhow!("computer_use MCP error: {message}"),
    }
}

fn format_result(result: McpResult) -> Value {
    json!({
        "server": result.server,
        "tool": result.tool,
        "success": result.success,
        "stdout": result.stdout,
        "stderr": result.stderr,
        "metadata": result.metadata,
    })
}

fn active_app(cwd: &Path) -> Result<String> {
    load_store::<ComputerUseStore>(&store_path(cwd)?)?
        .active_app
        .context(
            "ComputerUseAction requires capture or an app-targeted action before app-less actions",
        )
}

fn save_active_app(cwd: &Path, app: &str) -> Result<()> {
    save_store(
        &store_path(cwd)?,
        &ComputerUseStore {
            active_app: Some(app.to_string()),
        },
    )
}

fn store_path(cwd: &Path) -> Result<PathBuf> {
    Ok(workflow_root(cwd)?.join("computer_use.json"))
}

fn required_string(value: Option<String>, field: &str) -> Result<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .with_context(|| format!("ComputerUseAction `{field}` is required"))
}

fn required_i64(value: Option<i64>, field: &str) -> Result<i64> {
    value.with_context(|| format!("ComputerUseAction `{field}` is required"))
}

fn required_element(value: Option<i64>) -> Result<String> {
    Ok(required_i64(value, "element")?.to_string())
}

fn required_capture_mode(value: Option<&str>) -> Result<&'static str> {
    match value.map(str::trim).filter(|value| !value.is_empty()) {
        None | Some("som") => Ok("som"),
        Some("vision") => Ok("vision"),
        Some("ax") => Ok("ax"),
        Some(other) => bail!("ComputerUseAction capture mode `{other}` is unsupported"),
    }
}

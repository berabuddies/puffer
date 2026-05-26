use super::store::{load_store, save_store, workflow_root};
use crate::AppState;
use anyhow::{anyhow, bail, Context, Result};
use puffer_runner_api::{ChunkSink, McpResult, NullChunkSink, RunnerError};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

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
    #[serde(default, rename = "fromElement", alias = "from_element")]
    from_element: Option<i64>,
    #[serde(default, rename = "toElement", alias = "to_element")]
    to_element: Option<i64>,
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
    #[serde(default, rename = "raiseWindow", alias = "raise_window")]
    raise_window: Option<bool>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ComputerUseStore {
    active_app: Option<String>,
    #[serde(default)]
    elements: BTreeMap<i64, ElementBounds>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ElementBounds {
    x: i64,
    y: i64,
    width: i64,
    height: i64,
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
        "focusApp" => focus_app(state, cwd, parsed)?,
        "clickElement" => click_element(state, cwd, parsed)?,
        "clickCoord" => click_coord(state, cwd, parsed)?,
        "doubleClick" => element_click(state, cwd, parsed, 2, None)?,
        "rightClick" => element_click(state, cwd, parsed, 1, Some("right"))?,
        "drag" => drag(state, cwd, parsed)?,
        "scroll" => scroll(state, cwd, parsed)?,
        "typeText" => type_text(state, cwd, parsed)?,
        "keyCombo" => key_combo(state, cwd, parsed)?,
        other => bail!("unsupported ComputerUseAction action `{other}`"),
    };
    Ok(serde_json::to_string_pretty(&output)?)
}

fn focus_app(state: &mut AppState, cwd: &Path, input: ComputerUseInput) -> Result<Value> {
    let app = required_string(input.app, "app")?;
    save_active_app(cwd, &app)?;
    let raise_window = input.raise_window.unwrap_or(false);
    let tool = if raise_window {
        "get_app_state"
    } else {
        "list_apps"
    };
    let args = if raise_window {
        json!({ "app": app })
    } else {
        json!({})
    };
    let result = call_mcp(state, tool, args)?;
    if raise_window {
        save_capture_state(cwd, &app, &result)?;
    }
    let mut output = format_result(result);
    output["app"] = json!(app);
    output["raiseWindow"] = json!(raise_window);
    Ok(output)
}

fn capture(state: &mut AppState, cwd: &Path, input: ComputerUseInput) -> Result<Value> {
    let mode = required_capture_mode(input.mode.as_deref())?;
    let app = required_string(input.app, "app")?;
    save_active_app(cwd, &app)?;
    let result = call_mcp(state, "get_app_state", json!({ "app": app }))?;
    save_capture_state(cwd, &app, &result)?;
    let mut output = format_result(result);
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
        save_capture_state(cwd, &app, &capture)?;
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

fn drag(state: &mut AppState, cwd: &Path, input: ComputerUseInput) -> Result<Value> {
    let app = active_app(cwd)?;
    let from = required_element_center(cwd, input.from_element, "fromElement")?;
    let to = required_element_center(cwd, input.to_element, "toElement")?;
    let result = call_mcp(
        state,
        "drag",
        json!({
            "app": app,
            "from_x": from.0,
            "from_y": from.1,
            "to_x": to.0,
            "to_y": to.1,
        }),
    )?;
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
    let prior = load_store::<ComputerUseStore>(&store_path(cwd)?).unwrap_or_default();
    let elements = if prior.active_app.as_deref() == Some(app) {
        prior.elements
    } else {
        BTreeMap::new()
    };
    save_store(
        &store_path(cwd)?,
        &ComputerUseStore {
            active_app: Some(app.to_string()),
            elements,
        },
    )
}

fn save_capture_state(cwd: &Path, app: &str, result: &McpResult) -> Result<()> {
    save_store(
        &store_path(cwd)?,
        &ComputerUseStore {
            active_app: Some(app.to_string()),
            elements: extract_element_bounds(result),
        },
    )
}

fn required_element_center(cwd: &Path, element: Option<i64>, field: &str) -> Result<(i64, i64)> {
    let element = required_i64(element, field)?;
    let store = load_store::<ComputerUseStore>(&store_path(cwd)?)?;
    let bounds = store.elements.get(&element).with_context(|| {
        format!("ComputerUseAction drag requires captured bounds for element {element}")
    })?;
    Ok((bounds.x + bounds.width / 2, bounds.y + bounds.height / 2))
}

fn extract_element_bounds(result: &McpResult) -> BTreeMap<i64, ElementBounds> {
    let mut text = String::new();
    text.push_str(&result.stdout);
    collect_metadata_text(&result.metadata, &mut text);
    parse_element_bounds(&text)
}

fn collect_metadata_text(value: &Value, out: &mut String) {
    match value {
        Value::String(text) if text.len() <= 200_000 => {
            out.push('\n');
            out.push_str(text);
        }
        Value::Array(items) => {
            for item in items {
                collect_metadata_text(item, out);
            }
        }
        Value::Object(map) => {
            for value in map.values() {
                collect_metadata_text(value, out);
            }
        }
        _ => {}
    }
}

fn parse_element_bounds(text: &str) -> BTreeMap<i64, ElementBounds> {
    static ELEMENT_RE: OnceLock<Regex> = OnceLock::new();
    let re = ELEMENT_RE.get_or_init(|| {
        Regex::new(
            r"#\s*(\d+)\b[^\n]*@\s*\(\s*(-?\d+)\s*,\s*(-?\d+)\s*,\s*(-?\d+)\s*,\s*(-?\d+)\s*\)",
        )
        .expect("valid element bounds regex")
    });
    let mut elements = BTreeMap::new();
    for cap in re.captures_iter(text) {
        let Some(index) = cap.get(1).and_then(|m| m.as_str().parse::<i64>().ok()) else {
            continue;
        };
        let Some(x) = cap.get(2).and_then(|m| m.as_str().parse::<i64>().ok()) else {
            continue;
        };
        let Some(y) = cap.get(3).and_then(|m| m.as_str().parse::<i64>().ok()) else {
            continue;
        };
        let Some(width) = cap.get(4).and_then(|m| m.as_str().parse::<i64>().ok()) else {
            continue;
        };
        let Some(height) = cap.get(5).and_then(|m| m.as_str().parse::<i64>().ok()) else {
            continue;
        };
        if width <= 0 || height <= 0 {
            continue;
        }
        elements.insert(
            index,
            ElementBounds {
                x,
                y,
                width,
                height,
            },
        );
    }
    elements
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

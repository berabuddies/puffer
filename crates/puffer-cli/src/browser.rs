//! CLI command execution for `puffer browser`.

use crate::browser_args::{
    BrowserArgs, BrowserCommand, BrowserKeyboardCommand, BrowserTabCommand, BrowserTargetArgs,
};
use crate::browser_output::{
    normalize_agent_result, normalize_screenshot_result, print_execution_result,
    redact_internal_fields, BrowserExecution, BrowserPrintKind, BrowserScreenshotOutput,
    BrowserScreenshotWire,
};
#[cfg(test)]
use crate::browser_output::{
    ok_message, render_snapshot_body, BrowserSnapshotOutput, BrowserSnapshotRef,
};
use crate::daemon::Handshake;
use crate::daemon_browser::{default_cli_session_id, ensure_daemon, send_daemon_request};
use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
#[cfg(test)]
use indexmap::IndexMap;
use puffer_config::ConfigPaths;
use puffer_tools::internal_permissions::{
    request_internal_tool_permission_from_env, require_internal_tool_permission_from_env,
    InternalToolPermissionRequest,
};
use serde::Serialize;
use serde_json::{Map, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Runs one `puffer browser` command end to end.
pub(crate) fn run_browser_command(
    cwd: &Path,
    paths: &ConfigPaths,
    args: BrowserArgs,
) -> Result<()> {
    run_browser_command_inner(cwd, paths, args, false)
}

/// Runs one `puffer internal-tool browser` command end to end.
pub(crate) fn run_internal_browser_command(
    cwd: &Path,
    paths: &ConfigPaths,
    args: BrowserArgs,
) -> Result<()> {
    run_browser_command_inner(cwd, paths, args, true)
}

fn run_browser_command_inner(
    cwd: &Path,
    paths: &ConfigPaths,
    args: BrowserArgs,
    internal_permission_required: bool,
) -> Result<()> {
    let session_id = resolve_session_id(paths, args.session_id.as_deref())?;
    let handshake = ensure_daemon(paths)?;
    let execution = execute_browser_command(
        cwd,
        paths,
        &handshake,
        &session_id,
        args.command,
        internal_permission_required,
    )?;

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&BrowserJsonOutput {
                action: execution.action.to_string(),
                session_id,
                workspace_root: handshake.workspace_root,
                result: redact_internal_fields(execution.result),
            })?
        );
        return Ok(());
    }

    print_execution_result(&session_id, &execution)
}

fn execute_browser_command(
    cwd: &Path,
    paths: &ConfigPaths,
    handshake: &Handshake,
    session_id: &str,
    command: BrowserCommand,
    internal_permission_required: bool,
) -> Result<BrowserExecution> {
    match command {
        BrowserCommand::List => execute_agent_action(
            handshake,
            "list",
            base_payload("list", session_id),
            BrowserPrintKind::TabsState,
            internal_permission_required,
        ),
        BrowserCommand::Open {
            url,
            tab_id,
            label,
            width,
            height,
        } => {
            let mut payload = base_payload("open", session_id);
            insert_optional_string(&mut payload, "tabId", tab_id.as_deref());
            insert_optional_string(&mut payload, "url", url.as_deref());
            insert_optional_string(&mut payload, "label", label.as_deref());
            insert_optional_u32(&mut payload, "width", width);
            insert_optional_u32(&mut payload, "height", height);
            execute_agent_action(
                handshake,
                "open",
                payload,
                BrowserPrintKind::TabInfo,
                internal_permission_required,
            )
        }
        BrowserCommand::Navigate { url, target } => {
            let mut payload = base_payload("navigate", session_id);
            apply_target_args(&mut payload, &target);
            insert_optional_string(&mut payload, "url", Some(url.as_str()));
            execute_agent_action(
                handshake,
                "navigate",
                payload,
                BrowserPrintKind::TabsState,
                internal_permission_required,
            )
        }
        BrowserCommand::Back { target } => execute_targeted_ok_action(
            handshake,
            session_id,
            "back",
            target,
            internal_permission_required,
        ),
        BrowserCommand::Forward { target } => execute_targeted_ok_action(
            handshake,
            session_id,
            "forward",
            target,
            internal_permission_required,
        ),
        BrowserCommand::Reload { target } => execute_targeted_ok_action(
            handshake,
            session_id,
            "reload",
            target,
            internal_permission_required,
        ),
        BrowserCommand::Close { group, target } => {
            if group {
                execute_agent_action(
                    handshake,
                    "quit",
                    base_payload("quit", session_id),
                    BrowserPrintKind::TabsState,
                    internal_permission_required,
                )
            } else {
                let mut payload = base_payload("close", session_id);
                apply_target_args(&mut payload, &target);
                execute_agent_action(
                    handshake,
                    "close",
                    payload,
                    BrowserPrintKind::TabsState,
                    internal_permission_required,
                )
            }
        }
        BrowserCommand::Quit => execute_agent_action(
            handshake,
            "quit",
            base_payload("quit", session_id),
            BrowserPrintKind::TabsState,
            internal_permission_required,
        ),
        BrowserCommand::Tab { command } => {
            execute_tab_command(handshake, session_id, command, internal_permission_required)
        }
        BrowserCommand::Snapshot { target } => {
            let mut payload = base_payload("snapshot", session_id);
            apply_target_args(&mut payload, &target);
            execute_agent_action(
                handshake,
                "snapshot",
                payload,
                BrowserPrintKind::Snapshot,
                internal_permission_required,
            )
        }
        BrowserCommand::Screenshot {
            path,
            annotate,
            screenshot_dir,
            screenshot_format,
            screenshot_quality,
            target,
        } => execute_screenshot_action(
            cwd,
            paths,
            handshake,
            session_id,
            path,
            annotate,
            screenshot_dir,
            screenshot_format,
            screenshot_quality,
            target,
            internal_permission_required,
        ),
        BrowserCommand::Click { ref_id, target } => {
            let mut payload = base_payload("click", session_id);
            apply_target_args(&mut payload, &target);
            insert_optional_string(&mut payload, "ref", Some(ref_id.as_str()));
            execute_agent_action(
                handshake,
                "click",
                payload,
                BrowserPrintKind::Ok,
                internal_permission_required,
            )
        }
        BrowserCommand::Dblclick { ref_id, target } => {
            let mut payload = base_payload("dblclick", session_id);
            apply_target_args(&mut payload, &target);
            insert_optional_string(&mut payload, "ref", Some(ref_id.as_str()));
            execute_agent_action(
                handshake,
                "dblclick",
                payload,
                BrowserPrintKind::Ok,
                internal_permission_required,
            )
        }
        BrowserCommand::Hover { ref_id, target } => {
            let mut payload = base_payload("hover", session_id);
            apply_target_args(&mut payload, &target);
            insert_optional_string(&mut payload, "ref", Some(ref_id.as_str()));
            execute_agent_action(
                handshake,
                "hover",
                payload,
                BrowserPrintKind::Ok,
                internal_permission_required,
            )
        }
        BrowserCommand::Focus { ref_id, target } => {
            let mut payload = base_payload("focus_ref", session_id);
            apply_target_args(&mut payload, &target);
            insert_optional_string(&mut payload, "ref", Some(ref_id.as_str()));
            execute_agent_action(
                handshake,
                "focus_ref",
                payload,
                BrowserPrintKind::Ok,
                internal_permission_required,
            )
        }
        BrowserCommand::Fill {
            ref_id,
            text,
            target,
        } => {
            let mut payload = base_payload("fill", session_id);
            apply_target_args(&mut payload, &target);
            insert_optional_string(&mut payload, "ref", Some(ref_id.as_str()));
            insert_optional_string(&mut payload, "text", Some(text.as_str()));
            execute_agent_action(
                handshake,
                "fill",
                payload,
                BrowserPrintKind::Ok,
                internal_permission_required,
            )
        }
        BrowserCommand::Select {
            ref_id,
            value,
            target,
        } => {
            let mut payload = base_payload("select", session_id);
            apply_target_args(&mut payload, &target);
            insert_optional_string(&mut payload, "ref", Some(ref_id.as_str()));
            insert_optional_string(&mut payload, "value", Some(value.as_str()));
            execute_agent_action(
                handshake,
                "select",
                payload,
                BrowserPrintKind::Ok,
                internal_permission_required,
            )
        }
        BrowserCommand::Upload {
            ref_id,
            files,
            target,
        } => {
            let mut payload = base_payload("upload", session_id);
            apply_target_args(&mut payload, &target);
            insert_optional_string(&mut payload, "ref", Some(ref_id.as_str()));
            insert_string_array(
                &mut payload,
                "files",
                canonicalize_upload_files(cwd, &files)?,
            );
            execute_agent_action(
                handshake,
                "upload",
                payload,
                BrowserPrintKind::Ok,
                internal_permission_required,
            )
        }
        BrowserCommand::Check { ref_id, target } => {
            let mut payload = base_payload("check", session_id);
            apply_target_args(&mut payload, &target);
            insert_optional_string(&mut payload, "ref", Some(ref_id.as_str()));
            execute_agent_action(
                handshake,
                "check",
                payload,
                BrowserPrintKind::Ok,
                internal_permission_required,
            )
        }
        BrowserCommand::Uncheck { ref_id, target } => {
            let mut payload = base_payload("uncheck", session_id);
            apply_target_args(&mut payload, &target);
            insert_optional_string(&mut payload, "ref", Some(ref_id.as_str()));
            execute_agent_action(
                handshake,
                "uncheck",
                payload,
                BrowserPrintKind::Ok,
                internal_permission_required,
            )
        }
        BrowserCommand::Type {
            text,
            ref_id,
            target,
        } => {
            let mut payload = base_payload("type", session_id);
            apply_target_args(&mut payload, &target);
            insert_optional_string(&mut payload, "ref", ref_id.as_deref());
            insert_optional_string(&mut payload, "text", Some(text.as_str()));
            execute_agent_action(
                handshake,
                "type",
                payload,
                BrowserPrintKind::Ok,
                internal_permission_required,
            )
        }
        BrowserCommand::Press { key, target } => {
            let mut payload = base_payload("press", session_id);
            apply_target_args(&mut payload, &target);
            insert_optional_string(&mut payload, "key", Some(key.as_str()));
            execute_agent_action(
                handshake,
                "press",
                payload,
                BrowserPrintKind::Ok,
                internal_permission_required,
            )
        }
        BrowserCommand::Keydown { key, target } => {
            let mut payload = base_payload("keydown", session_id);
            apply_target_args(&mut payload, &target);
            insert_optional_string(&mut payload, "key", Some(key.as_str()));
            execute_agent_action(
                handshake,
                "keydown",
                payload,
                BrowserPrintKind::Ok,
                internal_permission_required,
            )
        }
        BrowserCommand::Keyup { key, target } => {
            let mut payload = base_payload("keyup", session_id);
            apply_target_args(&mut payload, &target);
            insert_optional_string(&mut payload, "key", Some(key.as_str()));
            execute_agent_action(
                handshake,
                "keyup",
                payload,
                BrowserPrintKind::Ok,
                internal_permission_required,
            )
        }
        BrowserCommand::Keyboard { command } => {
            execute_keyboard_command(handshake, session_id, command, internal_permission_required)
        }
        BrowserCommand::Scroll {
            direction,
            px,
            target,
        } => {
            let mut payload = base_payload("scroll", session_id);
            apply_target_args(&mut payload, &target);
            insert_optional_string(&mut payload, "direction", Some(direction.as_str()));
            insert_optional_u32(&mut payload, "px", px);
            execute_agent_action(
                handshake,
                "scroll",
                payload,
                BrowserPrintKind::Ok,
                internal_permission_required,
            )
        }
        BrowserCommand::ScrollIntoView { ref_id, target } => {
            let mut payload = base_payload("scrollIntoView", session_id);
            apply_target_args(&mut payload, &target);
            insert_optional_string(&mut payload, "ref", Some(ref_id.as_str()));
            execute_agent_action(
                handshake,
                "scrollIntoView",
                payload,
                BrowserPrintKind::Ok,
                internal_permission_required,
            )
        }
        BrowserCommand::Eval { script, target } => {
            let mut payload = base_payload("eval", session_id);
            apply_target_args(&mut payload, &target);
            insert_optional_string(&mut payload, "script", Some(script.as_str()));
            execute_agent_action(
                handshake,
                "eval",
                payload,
                BrowserPrintKind::Value,
                internal_permission_required,
            )
        }
    }
}

fn execute_tab_command(
    handshake: &Handshake,
    session_id: &str,
    command: BrowserTabCommand,
    internal_permission_required: bool,
) -> Result<BrowserExecution> {
    match command {
        BrowserTabCommand::List => execute_agent_action(
            handshake,
            "list",
            base_payload("list", session_id),
            BrowserPrintKind::TabsState,
            internal_permission_required,
        ),
        BrowserTabCommand::New {
            url,
            tab_id,
            label,
            width,
            height,
        } => {
            let mut payload = base_payload("new", session_id);
            insert_optional_string(&mut payload, "tabId", tab_id.as_deref());
            insert_optional_string(&mut payload, "url", url.as_deref());
            insert_optional_string(&mut payload, "label", label.as_deref());
            insert_optional_u32(&mut payload, "width", width);
            insert_optional_u32(&mut payload, "height", height);
            execute_agent_action(
                handshake,
                "new",
                payload,
                BrowserPrintKind::TabInfo,
                internal_permission_required,
            )
        }
        BrowserTabCommand::Close { tab_id } => {
            let mut payload = base_payload("close", session_id);
            insert_optional_string(&mut payload, "tabId", tab_id.as_deref());
            execute_agent_action(
                handshake,
                "close",
                payload,
                BrowserPrintKind::TabsState,
                internal_permission_required,
            )
        }
        BrowserTabCommand::Focus { tab_id } => {
            let mut payload = base_payload("focus", session_id);
            insert_optional_string(&mut payload, "tabId", Some(tab_id.as_str()));
            execute_agent_action(
                handshake,
                "focus",
                payload,
                BrowserPrintKind::TabInfo,
                internal_permission_required,
            )
        }
    }
}

fn execute_keyboard_command(
    handshake: &Handshake,
    session_id: &str,
    command: BrowserKeyboardCommand,
    internal_permission_required: bool,
) -> Result<BrowserExecution> {
    match command {
        BrowserKeyboardCommand::Type { text, target } => {
            let mut payload = base_payload("type", session_id);
            apply_target_args(&mut payload, &target);
            insert_optional_string(&mut payload, "text", Some(text.as_str()));
            execute_agent_action(
                handshake,
                "type",
                payload,
                BrowserPrintKind::Ok,
                internal_permission_required,
            )
        }
        BrowserKeyboardCommand::InsertText { text, target } => {
            let mut payload = base_payload("insertText", session_id);
            apply_target_args(&mut payload, &target);
            insert_optional_string(&mut payload, "text", Some(text.as_str()));
            execute_agent_action(
                handshake,
                "insertText",
                payload,
                BrowserPrintKind::Ok,
                internal_permission_required,
            )
        }
    }
}

fn execute_targeted_ok_action(
    handshake: &Handshake,
    session_id: &str,
    action: &'static str,
    target: BrowserTargetArgs,
    internal_permission_required: bool,
) -> Result<BrowserExecution> {
    let mut payload = base_payload(action, session_id);
    apply_target_args(&mut payload, &target);
    execute_agent_action(
        handshake,
        action,
        payload,
        BrowserPrintKind::Ok,
        internal_permission_required,
    )
}

fn execute_screenshot_action(
    cwd: &Path,
    paths: &ConfigPaths,
    handshake: &Handshake,
    session_id: &str,
    path: Option<PathBuf>,
    annotate: bool,
    screenshot_dir: Option<PathBuf>,
    screenshot_format: Option<String>,
    screenshot_quality: Option<u8>,
    target: BrowserTargetArgs,
    internal_permission_required: bool,
) -> Result<BrowserExecution> {
    let mut payload = base_payload("screenshot", session_id);
    apply_target_args(&mut payload, &target);
    payload.insert("annotate".to_string(), Value::Bool(annotate));
    insert_optional_string(
        &mut payload,
        "screenshotFormat",
        screenshot_format.as_deref(),
    );
    insert_optional_u32(
        &mut payload,
        "screenshotQuality",
        screenshot_quality.map(u32::from),
    );
    ensure_internal_browser_permission(&payload, internal_permission_required)?;
    let result = send_daemon_request(handshake, "browser_agent", Value::Object(payload))?;
    let screenshot = persist_screenshot_result(cwd, paths, result, path, screenshot_dir)?;
    Ok(BrowserExecution {
        action: "screenshot",
        result: serde_json::to_value(screenshot)?,
        print_kind: BrowserPrintKind::Screenshot,
    })
}

fn persist_screenshot_result(
    cwd: &Path,
    paths: &ConfigPaths,
    result: Value,
    path: Option<PathBuf>,
    screenshot_dir: Option<PathBuf>,
) -> Result<BrowserScreenshotOutput> {
    let screenshot: BrowserScreenshotWire =
        serde_json::from_value(result).context("decode browser screenshot")?;
    let output_path = resolve_screenshot_path(
        cwd,
        paths,
        path,
        screenshot_dir,
        &screenshot.tab_id,
        &screenshot.format,
    )?;
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create screenshot directory {}", parent.display()))?;
    }
    let bytes = BASE64_STANDARD
        .decode(screenshot.data.as_bytes())
        .context("decode browser screenshot data")?;
    fs::write(&output_path, bytes)
        .with_context(|| format!("write screenshot {}", output_path.display()))?;
    Ok(normalize_screenshot_result(screenshot, output_path))
}

fn resolve_screenshot_path(
    cwd: &Path,
    paths: &ConfigPaths,
    path: Option<PathBuf>,
    screenshot_dir: Option<PathBuf>,
    tab_id: &str,
    format: &str,
) -> Result<PathBuf> {
    if let Some(path) = path {
        return Ok(resolve_cli_path(cwd, path));
    }
    let dir = screenshot_dir
        .map(|dir| resolve_cli_path(cwd, dir))
        .unwrap_or_else(|| paths.workspace_config_dir.join("screenshots"));
    let extension = screenshot_extension(format)?;
    Ok(auto_screenshot_path(&dir, tab_id, extension))
}

fn resolve_cli_path(cwd: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}

fn screenshot_extension(format: &str) -> Result<&'static str> {
    match format {
        "png" => Ok("png"),
        "jpeg" => Ok("jpeg"),
        other => bail!("unsupported screenshot format `{other}`"),
    }
}

fn auto_screenshot_path(dir: &Path, tab_id: &str, extension: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let base = format!("screenshot-{tab_id}-{stamp}");
    let mut candidate = dir.join(format!("{base}.{extension}"));
    let mut suffix = 1u32;
    while candidate.exists() {
        candidate = dir.join(format!("{base}-{suffix}.{extension}"));
        suffix += 1;
    }
    candidate
}

fn execute_agent_action(
    handshake: &Handshake,
    action: &'static str,
    payload: Map<String, Value>,
    print_kind: BrowserPrintKind,
    internal_permission_required: bool,
) -> Result<BrowserExecution> {
    ensure_internal_browser_permission(&payload, internal_permission_required)?;
    let result = send_daemon_request(handshake, "browser_agent", Value::Object(payload))?;
    let result = normalize_agent_result(print_kind, result)?;
    Ok(BrowserExecution {
        action,
        result,
        print_kind,
    })
}

fn ensure_internal_browser_permission(
    payload: &Map<String, Value>,
    internal_permission_required: bool,
) -> Result<()> {
    let request = InternalToolPermissionRequest {
        tool_id: "browser".to_string(),
        input: Value::Object(payload.clone()),
    };
    let response = if internal_permission_required {
        Some(require_internal_tool_permission_from_env(request)?)
    } else {
        request_internal_tool_permission_from_env(request)?
    };
    let Some(response) = response else {
        return Ok(());
    };
    if response.is_allowed() {
        return Ok(());
    }
    bail!(
        "browser action denied: {}",
        response
            .reason
            .unwrap_or_else(|| "permission denied".to_string())
    )
}

fn resolve_session_id(paths: &ConfigPaths, explicit: Option<&str>) -> Result<String> {
    explicit
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .map(Ok)
        .unwrap_or_else(|| default_cli_session_id(paths))
}

fn base_payload(action: &str, session_id: &str) -> Map<String, Value> {
    let mut payload = Map::new();
    payload.insert("action".to_string(), Value::String(action.to_string()));
    payload.insert(
        "sessionId".to_string(),
        Value::String(session_id.to_string()),
    );
    payload
}

fn apply_target_args(payload: &mut Map<String, Value>, target: &BrowserTargetArgs) {
    insert_optional_string(payload, "tabId", target.tab_id.as_deref());
    insert_optional_u32(payload, "width", target.width);
    insert_optional_u32(payload, "height", target.height);
}

fn insert_optional_string(payload: &mut Map<String, Value>, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        payload.insert(key.to_string(), Value::String(value.to_string()));
    }
}

fn insert_optional_u32(payload: &mut Map<String, Value>, key: &str, value: Option<u32>) {
    if let Some(value) = value {
        payload.insert(key.to_string(), Value::from(value));
    }
}

fn insert_string_array(payload: &mut Map<String, Value>, key: &str, values: Vec<String>) {
    payload.insert(
        key.to_string(),
        Value::Array(values.into_iter().map(Value::String).collect()),
    );
}

fn canonicalize_upload_files(cwd: &Path, files: &[PathBuf]) -> Result<Vec<String>> {
    files
        .iter()
        .map(|path| canonicalize_upload_file(cwd, path))
        .collect()
}

fn canonicalize_upload_file(cwd: &Path, path: &Path) -> Result<String> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    let absolute = candidate
        .canonicalize()
        .with_context(|| format!("browser upload file not found: {}", path.display()))?;
    if !absolute.is_file() {
        bail!("browser upload path is not a file: {}", absolute.display());
    }
    Ok(absolute.to_string_lossy().into_owned())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BrowserJsonOutput {
    action: String,
    session_id: String,
    workspace_root: String,
    result: Value,
}

#[cfg(test)]
#[path = "browser_tests.rs"]
mod tests;

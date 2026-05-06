//! CLI command execution for `puffer browser`.

use anyhow::{Context, Result};
use indexmap::IndexMap;
use puffer_config::ConfigPaths;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::Path;

use crate::cli_args::{
    BrowserArgs, BrowserCommand, BrowserKeyboardCommand, BrowserTabCommand, BrowserTargetArgs,
};
use crate::daemon::Handshake;
use crate::daemon_browser::{
    default_cli_session_id, ensure_daemon, send_daemon_request, BrowserTabInfo, BrowserTabsState,
};

/// Runs one `puffer browser` command end to end.
pub(crate) fn run_browser_command(
    _cwd: &Path,
    paths: &ConfigPaths,
    args: BrowserArgs,
) -> Result<()> {
    let session_id = resolve_session_id(paths, args.session_id.as_deref())?;
    let handshake = ensure_daemon(paths)?;
    let execution = execute_browser_command(&handshake, &session_id, args.command)?;

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
    handshake: &Handshake,
    session_id: &str,
    command: BrowserCommand,
) -> Result<BrowserExecution> {
    match command {
        BrowserCommand::List => execute_agent_action(
            handshake,
            "list",
            base_payload("list", session_id),
            BrowserPrintKind::TabsState,
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
            execute_agent_action(handshake, "open", payload, BrowserPrintKind::TabInfo)
        }
        BrowserCommand::Navigate { url, target } => {
            let mut payload = base_payload("navigate", session_id);
            apply_target_args(&mut payload, &target);
            insert_optional_string(&mut payload, "url", Some(url.as_str()));
            execute_agent_action(handshake, "navigate", payload, BrowserPrintKind::TabsState)
        }
        BrowserCommand::Back { target } => {
            execute_targeted_ok_action(handshake, session_id, "back", target)
        }
        BrowserCommand::Forward { target } => {
            execute_targeted_ok_action(handshake, session_id, "forward", target)
        }
        BrowserCommand::Reload { target } => {
            execute_targeted_ok_action(handshake, session_id, "reload", target)
        }
        BrowserCommand::Close { group, target } => {
            if group {
                execute_agent_action(
                    handshake,
                    "quit",
                    base_payload("quit", session_id),
                    BrowserPrintKind::TabsState,
                )
            } else {
                let mut payload = base_payload("close", session_id);
                apply_target_args(&mut payload, &target);
                execute_agent_action(handshake, "close", payload, BrowserPrintKind::TabsState)
            }
        }
        BrowserCommand::Quit => execute_agent_action(
            handshake,
            "quit",
            base_payload("quit", session_id),
            BrowserPrintKind::TabsState,
        ),
        BrowserCommand::Tab { command } => execute_tab_command(handshake, session_id, command),
        BrowserCommand::Snapshot { target } => {
            let mut payload = base_payload("snapshot", session_id);
            apply_target_args(&mut payload, &target);
            execute_agent_action(handshake, "snapshot", payload, BrowserPrintKind::Snapshot)
        }
        BrowserCommand::Click { ref_id, target } => {
            let mut payload = base_payload("click", session_id);
            apply_target_args(&mut payload, &target);
            insert_optional_string(&mut payload, "ref", Some(ref_id.as_str()));
            execute_agent_action(handshake, "click", payload, BrowserPrintKind::Ok)
        }
        BrowserCommand::Dblclick { ref_id, target } => {
            let mut payload = base_payload("dblclick", session_id);
            apply_target_args(&mut payload, &target);
            insert_optional_string(&mut payload, "ref", Some(ref_id.as_str()));
            execute_agent_action(handshake, "dblclick", payload, BrowserPrintKind::Ok)
        }
        BrowserCommand::Hover { ref_id, target } => {
            let mut payload = base_payload("hover", session_id);
            apply_target_args(&mut payload, &target);
            insert_optional_string(&mut payload, "ref", Some(ref_id.as_str()));
            execute_agent_action(handshake, "hover", payload, BrowserPrintKind::Ok)
        }
        BrowserCommand::Focus { ref_id, target } => {
            let mut payload = base_payload("focus_ref", session_id);
            apply_target_args(&mut payload, &target);
            insert_optional_string(&mut payload, "ref", Some(ref_id.as_str()));
            execute_agent_action(handshake, "focus_ref", payload, BrowserPrintKind::Ok)
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
            execute_agent_action(handshake, "fill", payload, BrowserPrintKind::Ok)
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
            execute_agent_action(handshake, "select", payload, BrowserPrintKind::Ok)
        }
        BrowserCommand::Check { ref_id, target } => {
            let mut payload = base_payload("check", session_id);
            apply_target_args(&mut payload, &target);
            insert_optional_string(&mut payload, "ref", Some(ref_id.as_str()));
            execute_agent_action(handshake, "check", payload, BrowserPrintKind::Ok)
        }
        BrowserCommand::Uncheck { ref_id, target } => {
            let mut payload = base_payload("uncheck", session_id);
            apply_target_args(&mut payload, &target);
            insert_optional_string(&mut payload, "ref", Some(ref_id.as_str()));
            execute_agent_action(handshake, "uncheck", payload, BrowserPrintKind::Ok)
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
            execute_agent_action(handshake, "type", payload, BrowserPrintKind::Ok)
        }
        BrowserCommand::Press { key, target } => {
            let mut payload = base_payload("press", session_id);
            apply_target_args(&mut payload, &target);
            insert_optional_string(&mut payload, "key", Some(key.as_str()));
            execute_agent_action(handshake, "press", payload, BrowserPrintKind::Ok)
        }
        BrowserCommand::Keydown { key, target } => {
            let mut payload = base_payload("keydown", session_id);
            apply_target_args(&mut payload, &target);
            insert_optional_string(&mut payload, "key", Some(key.as_str()));
            execute_agent_action(handshake, "keydown", payload, BrowserPrintKind::Ok)
        }
        BrowserCommand::Keyup { key, target } => {
            let mut payload = base_payload("keyup", session_id);
            apply_target_args(&mut payload, &target);
            insert_optional_string(&mut payload, "key", Some(key.as_str()));
            execute_agent_action(handshake, "keyup", payload, BrowserPrintKind::Ok)
        }
        BrowserCommand::Keyboard { command } => {
            execute_keyboard_command(handshake, session_id, command)
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
            execute_agent_action(handshake, "scroll", payload, BrowserPrintKind::Ok)
        }
        BrowserCommand::ScrollIntoView { ref_id, target } => {
            let mut payload = base_payload("scrollIntoView", session_id);
            apply_target_args(&mut payload, &target);
            insert_optional_string(&mut payload, "ref", Some(ref_id.as_str()));
            execute_agent_action(handshake, "scrollIntoView", payload, BrowserPrintKind::Ok)
        }
        BrowserCommand::Eval { script, target } => {
            let mut payload = base_payload("eval", session_id);
            apply_target_args(&mut payload, &target);
            insert_optional_string(&mut payload, "script", Some(script.as_str()));
            execute_agent_action(handshake, "eval", payload, BrowserPrintKind::Value)
        }
    }
}

fn execute_tab_command(
    handshake: &Handshake,
    session_id: &str,
    command: BrowserTabCommand,
) -> Result<BrowserExecution> {
    match command {
        BrowserTabCommand::List => execute_agent_action(
            handshake,
            "list",
            base_payload("list", session_id),
            BrowserPrintKind::TabsState,
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
            execute_agent_action(handshake, "new", payload, BrowserPrintKind::TabInfo)
        }
        BrowserTabCommand::Close { tab_id } => {
            let mut payload = base_payload("close", session_id);
            insert_optional_string(&mut payload, "tabId", tab_id.as_deref());
            execute_agent_action(handshake, "close", payload, BrowserPrintKind::TabsState)
        }
        BrowserTabCommand::Focus { tab_id } => {
            let mut payload = base_payload("focus", session_id);
            insert_optional_string(&mut payload, "tabId", Some(tab_id.as_str()));
            execute_agent_action(handshake, "focus", payload, BrowserPrintKind::TabInfo)
        }
    }
}

fn execute_keyboard_command(
    handshake: &Handshake,
    session_id: &str,
    command: BrowserKeyboardCommand,
) -> Result<BrowserExecution> {
    match command {
        BrowserKeyboardCommand::Type { text, target } => {
            let mut payload = base_payload("type", session_id);
            apply_target_args(&mut payload, &target);
            insert_optional_string(&mut payload, "text", Some(text.as_str()));
            execute_agent_action(handshake, "type", payload, BrowserPrintKind::Ok)
        }
        BrowserKeyboardCommand::InsertText { text, target } => {
            let mut payload = base_payload("insertText", session_id);
            apply_target_args(&mut payload, &target);
            insert_optional_string(&mut payload, "text", Some(text.as_str()));
            execute_agent_action(handshake, "insertText", payload, BrowserPrintKind::Ok)
        }
    }
}

fn execute_targeted_ok_action(
    handshake: &Handshake,
    session_id: &str,
    action: &'static str,
    target: BrowserTargetArgs,
) -> Result<BrowserExecution> {
    let mut payload = base_payload(action, session_id);
    apply_target_args(&mut payload, &target);
    execute_agent_action(handshake, action, payload, BrowserPrintKind::Ok)
}

fn execute_agent_action(
    handshake: &Handshake,
    action: &'static str,
    payload: Map<String, Value>,
    print_kind: BrowserPrintKind,
) -> Result<BrowserExecution> {
    let result = send_daemon_request(handshake, "browser_agent", Value::Object(payload))?;
    let result = normalize_agent_result(print_kind, result)?;
    Ok(BrowserExecution {
        action,
        result,
        print_kind,
    })
}

fn print_execution_result(session_id: &str, execution: &BrowserExecution) -> Result<()> {
    match execution.print_kind {
        BrowserPrintKind::TabsState => {
            let tabs: BrowserTabsState =
                serde_json::from_value(execution.result.clone()).context("decode browser tabs")?;
            print_tabs_state(session_id, &tabs);
        }
        BrowserPrintKind::TabInfo => {
            let tab: BrowserTabInfo =
                serde_json::from_value(execution.result.clone()).context("decode browser tab")?;
            print_tab_event(session_id, execution.action, &tab);
        }
        BrowserPrintKind::Snapshot => print_snapshot_result(session_id, &execution.result)?,
        BrowserPrintKind::Value => {
            if let Some(value) = execution.result.get("value") {
                println!("{}", serde_json::to_string_pretty(value)?);
            } else {
                println!("{}", serde_json::to_string_pretty(&execution.result)?);
            }
        }
        BrowserPrintKind::Ok => println!("{}", ok_message(execution.action)),
    }
    Ok(())
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

fn print_tabs_state(session_id: &str, tabs: &BrowserTabsState) {
    println!("session: {session_id}");
    let active = tabs.active_tab_id.as_deref().unwrap_or("<none>");
    println!("active: {active}");
    if tabs.tabs.is_empty() {
        println!("tabs: none");
        return;
    }
    for tab in &tabs.tabs {
        println!();
        print_tab_summary(tab);
    }
}

fn print_tab_event(session_id: &str, action: &str, tab: &BrowserTabInfo) {
    println!("session: {session_id}");
    let label = match action {
        "focus" => "focused",
        _ => "opened",
    };
    println!("{label}: {}", tab.tab_id);
    print_tab_summary(tab);
}

fn print_tab_summary(tab: &BrowserTabInfo) {
    let status = if tab.active { "active" } else { "idle" };
    let connectivity = if tab.connected {
        "connected"
    } else {
        "disconnected"
    };
    println!("tab: {} ({status}, {connectivity})", tab.tab_id);
    println!("label: {}", tab.label);
    println!("title: {}", printable_text(&tab.title));
    println!("url: {}", printable_text(&tab.url));
}

fn print_snapshot_result(session_id: &str, result: &Value) -> Result<()> {
    let snapshot: BrowserSnapshotOutput =
        serde_json::from_value(result.clone()).context("decode browser snapshot")?;
    println!("session: {session_id}");
    print!("{}", render_snapshot_body(&snapshot));
    Ok(())
}

fn render_snapshot_body(snapshot: &BrowserSnapshotOutput) -> String {
    let mut body = String::new();
    body.push_str(&format!("title: {}\n", printable_text(&snapshot.title)));
    body.push_str(&format!("origin: {}\n", printable_text(&snapshot.origin)));
    body.push('\n');
    body.push_str("snapshot:\n");
    body.push_str(printable_text(&snapshot.snapshot));

    if !snapshot.refs.is_empty() {
        body.push('\n');
        body.push('\n');
        body.push_str("refs:\n");
        for (ref_id, entry) in &snapshot.refs {
            body.push_str("  ");
            body.push_str(ref_id);
            body.push(' ');
            body.push_str(printable_text(&entry.role));
            body.push(' ');
            body.push_str(printable_text(&entry.tag));
            if !entry.name.trim().is_empty() {
                body.push(' ');
                body.push('"');
                body.push_str(&entry.name);
                body.push('"');
            }
            if let Some(href) = entry
                .href
                .as_deref()
                .filter(|value| !value.trim().is_empty())
            {
                body.push(' ');
                body.push('<');
                body.push_str(href);
                body.push('>');
            }
            body.push('\n');
        }
    }

    if let Some(instruction) =
        (!snapshot.instruction.trim().is_empty()).then_some(snapshot.instruction.as_str())
    {
        body.push('\n');
        body.push_str(instruction);
        body.push('\n');
    } else {
        body.push('\n');
    }

    body
}
fn ok_message(action: &str) -> &str {
    match action {
        "back" => "moved back",
        "forward" => "moved forward",
        "reload" => "reloaded",
        "click" => "clicked",
        "dblclick" => "double-clicked",
        "hover" => "hovered",
        "focus_ref" => "focused element",
        "fill" => "filled",
        "select" => "selected option",
        "check" => "checked",
        "uncheck" => "unchecked",
        "type" => "typed",
        "insertText" => "inserted text",
        "press" => "pressed key",
        "keydown" => "held key down",
        "keyup" => "released key",
        "scroll" => "scrolled",
        "scrollIntoView" => "scrolled into view",
        other => other,
    }
}

fn printable_text(value: &str) -> &str {
    if value.trim().is_empty() {
        "<empty>"
    } else {
        value
    }
}

fn normalize_agent_result(print_kind: BrowserPrintKind, result: Value) -> Result<Value> {
    match print_kind {
        BrowserPrintKind::Snapshot => Ok(serde_json::to_value(normalize_snapshot_result(result)?)?),
        _ => Ok(result),
    }
}

fn normalize_snapshot_result(result: Value) -> Result<BrowserSnapshotOutput> {
    let snapshot: BrowserSnapshotWire =
        serde_json::from_value(result).context("decode browser snapshot")?;
    let refs = snapshot
        .elements
        .into_iter()
        .map(|element| {
            (
                element.ref_id,
                BrowserSnapshotRef {
                    role: element.role,
                    name: element.name,
                    tag: element.tag,
                    href: element.href,
                },
            )
        })
        .collect();
    Ok(BrowserSnapshotOutput {
        origin: snapshot.url,
        title: snapshot.title,
        snapshot: snapshot.text,
        refs,
        instruction: snapshot.instruction,
    })
}

fn redact_internal_fields(value: Value) -> Value {
    match value {
        Value::Array(values) => {
            Value::Array(values.into_iter().map(redact_internal_fields).collect())
        }
        Value::Object(mut map) => {
            map.remove("backendSessionId");
            map.remove("backend_session_id");
            Value::Object(
                map.into_iter()
                    .map(|(key, value)| (key, redact_internal_fields(value)))
                    .collect(),
            )
        }
        other => other,
    }
}

struct BrowserExecution {
    action: &'static str,
    result: Value,
    print_kind: BrowserPrintKind,
}

#[derive(Clone, Copy)]
enum BrowserPrintKind {
    TabsState,
    TabInfo,
    Snapshot,
    Value,
    Ok,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrowserSnapshotWire {
    url: String,
    title: String,
    text: String,
    elements: Vec<BrowserSnapshotWireElement>,
    #[serde(default)]
    instruction: String,
}

#[derive(Debug, Deserialize)]
struct BrowserSnapshotWireElement {
    #[serde(rename = "ref")]
    ref_id: String,
    role: String,
    name: String,
    tag: String,
    #[serde(default)]
    href: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BrowserSnapshotOutput {
    origin: String,
    title: String,
    snapshot: String,
    refs: IndexMap<String, BrowserSnapshotRef>,
    #[serde(default)]
    instruction: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct BrowserSnapshotRef {
    role: String,
    name: String,
    tag: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    href: Option<String>,
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
mod tests {
    use super::*;

    #[test]
    fn ok_message_is_human_readable_for_common_actions() {
        assert_eq!(ok_message("reload"), "reloaded");
        assert_eq!(ok_message("select"), "selected option");
        assert_eq!(ok_message("press"), "pressed key");
        assert_eq!(ok_message("scrollIntoView"), "scrolled into view");
    }

    #[test]
    fn payload_helpers_skip_missing_values() {
        let mut payload = base_payload("snapshot", "session-123");
        apply_target_args(
            &mut payload,
            &BrowserTargetArgs {
                tab_id: None,
                width: Some(1200),
                height: None,
            },
        );
        assert_eq!(
            payload.get("sessionId").and_then(Value::as_str),
            Some("session-123")
        );
        assert_eq!(payload.get("width").and_then(Value::as_u64), Some(1200));
        assert!(payload.get("tabId").is_none());
        assert!(payload.get("height").is_none());
    }

    #[test]
    fn snapshot_results_are_normalized_into_snapshot_and_refs() {
        let result = normalize_agent_result(
            BrowserPrintKind::Snapshot,
            serde_json::json!({
                "url": "https://example.com/form",
                "title": "Example Form",
                "text": "Name\nSave",
                "elements": [
                    {
                        "ref": "@e1",
                        "role": "textbox",
                        "name": "Your name",
                        "tag": "textarea",
                        "href": null,
                        "x": 120.0,
                        "y": 220.0
                    },
                    {
                        "ref": "@e2",
                        "role": "button",
                        "name": "Save",
                        "tag": "button",
                        "href": null,
                        "x": 120.0,
                        "y": 260.0
                    }
                ],
                "instruction": "Refs are fresh for this snapshot."
            }),
        )
        .expect("normalize snapshot");

        assert_eq!(
            result.get("origin").and_then(Value::as_str),
            Some("https://example.com/form")
        );
        assert_eq!(
            result.get("snapshot").and_then(Value::as_str),
            Some("Name\nSave")
        );
        assert!(result.get("elements").is_none());
        assert!(
            result.pointer("/refs/@e1/x").is_none(),
            "normalized refs should not expose raw coordinates"
        );
        assert_eq!(
            result.pointer("/refs/@e2/role").and_then(Value::as_str),
            Some("button")
        );
    }

    #[test]
    fn render_snapshot_body_uses_origin_snapshot_and_refs_sections() {
        let snapshot = BrowserSnapshotOutput {
            origin: "https://example.com/form".to_string(),
            title: "Example Form".to_string(),
            snapshot: "Name\nSave".to_string(),
            refs: IndexMap::from([
                (
                    "@e1".to_string(),
                    BrowserSnapshotRef {
                        role: "textbox".to_string(),
                        name: "Your name".to_string(),
                        tag: "textarea".to_string(),
                        href: None,
                    },
                ),
                (
                    "@e2".to_string(),
                    BrowserSnapshotRef {
                        role: "button".to_string(),
                        name: "Save".to_string(),
                        tag: "button".to_string(),
                        href: Some("https://example.com/save".to_string()),
                    },
                ),
            ]),
            instruction: "Refs are fresh for this snapshot.".to_string(),
        };

        let rendered = render_snapshot_body(&snapshot);
        assert!(rendered.contains("origin: https://example.com/form"));
        assert!(rendered.contains("snapshot:\nName\nSave"));
        assert!(rendered.contains("refs:\n  @e1 textbox textarea \"Your name\""));
        assert!(rendered.contains("@e2 button button \"Save\" <https://example.com/save>"));
    }

    #[test]
    fn redact_internal_fields_removes_backend_session_ids() {
        let value = redact_internal_fields(serde_json::json!({
            "tabId": "t1",
            "backendSessionId": "root:browser:t1",
            "tabs": [
                {
                    "tabId": "t2",
                    "backendSessionId": "root:browser:t2"
                }
            ]
        }));

        assert!(value.get("backendSessionId").is_none());
        assert!(value.pointer("/tabs/0/backendSessionId").is_none());
    }
}

use crate::daemon_browser::{BrowserTabInfo, BrowserTabsState};
use anyhow::{Context, Result};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

/// Prints one normalized browser command result for the terminal UI.
pub(crate) fn print_execution_result(session_id: &str, execution: &BrowserExecution) -> Result<()> {
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
        BrowserPrintKind::Screenshot => print_screenshot_result(session_id, &execution.result)?,
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

/// Normalizes daemon output for browser command printing and JSON output.
pub(crate) fn normalize_agent_result(print_kind: BrowserPrintKind, result: Value) -> Result<Value> {
    match print_kind {
        BrowserPrintKind::Snapshot => Ok(serde_json::to_value(normalize_snapshot_result(result)?)?),
        _ => Ok(result),
    }
}

/// Converts screenshot wire output into the CLI-facing shape.
pub(crate) fn normalize_screenshot_result(
    screenshot: BrowserScreenshotWire,
    output_path: PathBuf,
) -> BrowserScreenshotOutput {
    BrowserScreenshotOutput {
        tab_id: screenshot.tab_id,
        path: output_path.display().to_string(),
        format: screenshot.format,
        origin: screenshot.url,
        title: screenshot.title,
        width: screenshot.width,
        height: screenshot.height,
        annotated: screenshot.annotated,
        refs: normalize_snapshot_refs(screenshot.elements),
        instruction: screenshot.instruction,
    }
}

/// Removes daemon-only fields before emitting JSON to users.
pub(crate) fn redact_internal_fields(value: Value) -> Value {
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

pub(crate) struct BrowserExecution {
    pub(crate) action: &'static str,
    pub(crate) result: Value,
    pub(crate) print_kind: BrowserPrintKind,
}

#[derive(Clone, Copy)]
pub(crate) enum BrowserPrintKind {
    TabsState,
    TabInfo,
    Snapshot,
    Screenshot,
    Value,
    Ok,
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

fn print_screenshot_result(session_id: &str, result: &Value) -> Result<()> {
    let screenshot: BrowserScreenshotOutput =
        serde_json::from_value(result.clone()).context("decode browser screenshot")?;
    println!("session: {session_id}");
    print!("{}", render_screenshot_body(&screenshot));
    Ok(())
}

pub(crate) fn render_snapshot_body(snapshot: &BrowserSnapshotOutput) -> String {
    let mut body = String::new();
    body.push_str(&format!("title: {}\n", printable_text(&snapshot.title)));
    body.push_str(&format!("origin: {}\n", printable_text(&snapshot.origin)));
    body.push('\n');
    body.push_str("snapshot:\n");
    body.push_str(printable_text(&snapshot.snapshot));

    append_refs_section(&mut body, &snapshot.refs);
    append_instruction(&mut body, &snapshot.instruction);
    body
}

fn render_screenshot_body(screenshot: &BrowserScreenshotOutput) -> String {
    let mut body = String::new();
    body.push_str(&format!("tab: {}\n", screenshot.tab_id));
    body.push_str(&format!("saved: {}\n", screenshot.path));
    body.push_str(&format!(
        "format: {} ({}x{})\n",
        screenshot.format, screenshot.width, screenshot.height
    ));
    body.push_str(&format!("title: {}\n", printable_text(&screenshot.title)));
    body.push_str(&format!("origin: {}\n", printable_text(&screenshot.origin)));
    if screenshot.annotated {
        body.push_str("annotated: true\n");
    }
    append_refs_section(&mut body, &screenshot.refs);
    append_instruction(&mut body, &screenshot.instruction);
    body
}

fn append_refs_section(body: &mut String, refs: &IndexMap<String, BrowserSnapshotRef>) {
    if refs.is_empty() {
        return;
    }
    body.push('\n');
    body.push_str("refs:\n");
    for (ref_id, entry) in refs {
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

fn append_instruction(body: &mut String, instruction: &str) {
    if let Some(instruction) = (!instruction.trim().is_empty()).then_some(instruction) {
        body.push('\n');
        body.push_str(instruction);
        body.push('\n');
    } else {
        body.push('\n');
    }
}

pub(crate) fn ok_message(action: &str) -> &str {
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
        "upload" => "uploaded files",
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

fn normalize_snapshot_result(result: Value) -> Result<BrowserSnapshotOutput> {
    let snapshot: BrowserSnapshotWire =
        serde_json::from_value(result).context("decode browser snapshot")?;
    let refs = normalize_snapshot_refs(snapshot.elements);
    Ok(BrowserSnapshotOutput {
        origin: snapshot.url,
        title: snapshot.title,
        snapshot: snapshot.text,
        refs,
        instruction: snapshot.instruction,
    })
}

fn normalize_snapshot_refs(
    elements: Vec<BrowserSnapshotWireElement>,
) -> IndexMap<String, BrowserSnapshotRef> {
    elements
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
        .collect()
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
pub(crate) struct BrowserSnapshotOutput {
    pub(crate) origin: String,
    pub(crate) title: String,
    pub(crate) snapshot: String,
    pub(crate) refs: IndexMap<String, BrowserSnapshotRef>,
    #[serde(default)]
    pub(crate) instruction: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct BrowserSnapshotRef {
    pub(crate) role: String,
    pub(crate) name: String,
    pub(crate) tag: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) href: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BrowserScreenshotWire {
    pub(crate) tab_id: String,
    pub(crate) format: String,
    pub(crate) data: String,
    pub(crate) url: String,
    pub(crate) title: String,
    pub(crate) width: u32,
    pub(crate) height: u32,
    #[serde(default)]
    pub(crate) annotated: bool,
    #[serde(default)]
    elements: Vec<BrowserSnapshotWireElement>,
    #[serde(default)]
    pub(crate) instruction: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BrowserScreenshotOutput {
    pub(crate) tab_id: String,
    pub(crate) path: String,
    pub(crate) format: String,
    pub(crate) origin: String,
    pub(crate) title: String,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) annotated: bool,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub(crate) refs: IndexMap<String, BrowserSnapshotRef>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) instruction: String,
}

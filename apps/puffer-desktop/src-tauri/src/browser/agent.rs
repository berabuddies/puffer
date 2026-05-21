//! Agent-facing browser actions layered over the managed Chrome sessions.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::thread;
use std::time::Duration;

use crate::events::EventEmitter;

use super::params::{optional_u32, required_string};
use super::tabs::{backend_session_id, BrowserTabInfo, BrowserTabsState};
use super::{
    BrowserHistoryDirection, BrowserInputEvent, BrowserRegistry, DEFAULT_URL, INITIAL_HEIGHT,
    INITIAL_WIDTH,
};

/// Element reference captured from the last agent browser snapshot.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BrowserElementRef {
    #[serde(rename = "ref")]
    pub(crate) ref_id: String,
    pub(crate) role: String,
    pub(crate) name: String,
    pub(crate) tag: String,
    #[serde(default)]
    pub(crate) href: Option<String>,
    pub(crate) x: f64,
    pub(crate) y: f64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BrowserSnapshot {
    url: String,
    title: String,
    text: String,
    elements: Vec<BrowserElementRef>,
}

/// Handles `browser_agent`, the agent-oriented browser action endpoint.
pub(crate) fn handle_browser_agent(
    browsers: &BrowserRegistry,
    events: EventEmitter,
    params: &Value,
) -> Result<Value> {
    let action = required_string(params, "action")?;
    let root_session_id = required_string(params, "sessionId")?;
    let width = optional_u32(params, "width").unwrap_or(INITIAL_WIDTH);
    let height = optional_u32(params, "height").unwrap_or(INITIAL_HEIGHT);
    match action.as_str() {
        "list" => Ok(serde_json::to_value(browsers.list_tabs(&root_session_id))?),
        "open" => {
            let tab_id = resolve_open_target_tab_id(browsers, &root_session_id, params);
            arm_agent_recording(browsers, &root_session_id, &tab_id);
            let tab = open_agent_tab(
                browsers,
                events.clone(),
                &root_session_id,
                params,
                width,
                height,
                Some(tab_id),
            )?;
            browsers.arm_agent_recording(&tab.backend_session_id);
            publish_tabs(browsers, &events, &root_session_id);
            Ok(serde_json::to_value(tab)?)
        }
        "focus" => {
            let tab_id = required_string(params, "tabId")?;
            let tab = browsers.focus_tab(&root_session_id, &tab_id)?;
            publish_tabs(browsers, &events, &root_session_id);
            Ok(serde_json::to_value(tab)?)
        }
        "close" => {
            let tab_id = required_string(params, "tabId")?;
            let tabs = browsers.close_tab(&root_session_id, &tab_id)?;
            publish_tabs(browsers, &events, &root_session_id);
            Ok(serde_json::to_value(tabs)?)
        }
        "navigate" => {
            let url = required_string(params, "url")?;
            let (tab_id, backend_id) = ensure_target_tab(
                browsers,
                events.clone(),
                &root_session_id,
                params,
                width,
                height,
            )?;
            browsers.arm_agent_recording(&backend_id);
            browsers.navigate(&backend_id, url)?;
            browsers.focus_tab(&root_session_id, &tab_id)?;
            publish_tabs(browsers, &events, &root_session_id);
            Ok(serde_json::to_value(browsers.list_tabs(&root_session_id))?)
        }
        "reload" => {
            let (_, backend_id) = ensure_target_tab(
                browsers,
                events.clone(),
                &root_session_id,
                params,
                width,
                height,
            )?;
            browsers.arm_agent_recording(&backend_id);
            browsers.reload(&backend_id)?;
            Ok(json!({ "ok": true }))
        }
        "back" | "forward" => {
            let (_, backend_id) = ensure_target_tab(
                browsers,
                events.clone(),
                &root_session_id,
                params,
                width,
                height,
            )?;
            browsers.arm_agent_recording(&backend_id);
            let direction = if action == "back" {
                BrowserHistoryDirection::Back
            } else {
                BrowserHistoryDirection::Forward
            };
            browsers.history(&backend_id, direction)?;
            Ok(json!({ "ok": true }))
        }
        "snapshot" => {
            let (_, backend_id) = ensure_target_tab(
                browsers,
                events.clone(),
                &root_session_id,
                params,
                width,
                height,
            )?;
            browsers.arm_agent_recording(&backend_id);
            browsers.agent_snapshot(&backend_id)
        }
        "click" => {
            let (_, backend_id) = ensure_target_tab(
                browsers,
                events.clone(),
                &root_session_id,
                params,
                width,
                height,
            )?;
            browsers.arm_agent_recording(&backend_id);
            let target = required_string(params, "ref")?;
            browsers.agent_click(&backend_id, &target)?;
            Ok(json!({ "ok": true }))
        }
        "type" => {
            let (_, backend_id) = ensure_target_tab(
                browsers,
                events.clone(),
                &root_session_id,
                params,
                width,
                height,
            )?;
            browsers.arm_agent_recording(&backend_id);
            if let Some(target) = optional_string(params, "ref") {
                browsers.agent_click(&backend_id, &target)?;
                thread::sleep(Duration::from_millis(40));
            }
            let text = required_string(params, "text")?;
            browsers.input(&backend_id, BrowserInputEvent::Text { text })?;
            Ok(json!({ "ok": true }))
        }
        "fill" => {
            let (_, backend_id) = ensure_target_tab(
                browsers,
                events.clone(),
                &root_session_id,
                params,
                width,
                height,
            )?;
            browsers.arm_agent_recording(&backend_id);
            let target = required_string(params, "ref")?;
            let text = required_string(params, "text")?;
            browsers.agent_fill(&backend_id, &target, &text)?;
            Ok(json!({ "ok": true }))
        }
        "press" => {
            let (_, backend_id) = ensure_target_tab(
                browsers,
                events.clone(),
                &root_session_id,
                params,
                width,
                height,
            )?;
            browsers.arm_agent_recording(&backend_id);
            let key = required_string(params, "key")?;
            browsers.agent_press(&backend_id, &key)?;
            Ok(json!({ "ok": true }))
        }
        "evaluate" => {
            let (_, backend_id) = ensure_target_tab(
                browsers,
                events.clone(),
                &root_session_id,
                params,
                width,
                height,
            )?;
            browsers.arm_agent_recording(&backend_id);
            let script = required_string(params, "script")?;
            let value = browsers.get(&backend_id)?.evaluate(script)?.value;
            Ok(json!({ "value": value }))
        }
        other => bail!("unsupported browser agent action `{other}`"),
    }
}

fn arm_agent_recording(browsers: &BrowserRegistry, root_session_id: &str, tab_id: &str) {
    browsers.arm_agent_recording(&backend_session_id(root_session_id, tab_id));
}

fn open_agent_tab(
    browsers: &BrowserRegistry,
    events: EventEmitter,
    root_session_id: &str,
    params: &Value,
    width: u32,
    height: u32,
    resolved_tab_id: Option<String>,
) -> Result<BrowserTabInfo> {
    if let Some(tab_id) = resolved_tab_id.or_else(|| optional_string(params, "tabId")) {
        return browsers.open_tab(
            events.clone(),
            root_session_id.to_string(),
            Some(tab_id),
            optional_string(params, "label"),
            optional_string(params, "url"),
            width,
            height,
            params
                .get("activate")
                .and_then(Value::as_bool)
                .unwrap_or(true),
        );
    }
    if let Some(tab) = active_or_first(&browsers.list_tabs(root_session_id)) {
        return browsers.open_tab(
            events.clone(),
            root_session_id.to_string(),
            Some(tab.tab_id),
            optional_string(params, "label"),
            optional_string(params, "url"),
            width,
            height,
            params
                .get("activate")
                .and_then(Value::as_bool)
                .unwrap_or(true),
        );
    }
    browsers.open_tab(
        events.clone(),
        root_session_id.to_string(),
        None,
        optional_string(params, "label"),
        optional_string(params, "url"),
        width,
        height,
        true,
    )
}

fn resolve_open_target_tab_id(
    browsers: &BrowserRegistry,
    root_session_id: &str,
    params: &Value,
) -> String {
    if let Some(tab_id) = optional_string(params, "tabId") {
        return tab_id;
    }
    if let Some(tab) = active_or_first(&browsers.list_tabs(root_session_id)) {
        return tab.tab_id;
    }
    browsers
        .tabs
        .lock()
        .unwrap()
        .next_tab_id(root_session_id)
}

impl BrowserRegistry {
    /// Captures an agent-readable DOM snapshot and fresh element refs.
    pub(crate) fn agent_snapshot(&self, backend_session_id: &str) -> Result<Value> {
        let snapshot_value = self
            .get(backend_session_id)?
            .evaluate(snapshot_expression().to_string())?
            .value;
        let snapshot: BrowserSnapshot =
            serde_json::from_value(snapshot_value).context("decode browser snapshot")?;
        self.agent_refs
            .lock()
            .unwrap()
            .insert(backend_session_id.to_string(), snapshot.elements.clone());
        Ok(json!({
            "url": snapshot.url,
            "title": snapshot.title,
            "text": snapshot.text,
            "elements": snapshot.elements,
            "instruction": "Refs are fresh for this snapshot. Re-snapshot after navigation or dynamic page changes."
        }))
    }

    /// Clicks an element ref from the last agent snapshot.
    pub(crate) fn agent_click(&self, backend_session_id: &str, ref_id: &str) -> Result<()> {
        let target = self.lookup_ref(backend_session_id, ref_id)?;
        let session = self.get(backend_session_id)?;
        session.input(BrowserInputEvent::Mouse {
            event_type: "mousePressed".to_string(),
            x: target.x,
            y: target.y,
            button: "left".to_string(),
            buttons: Some(1),
            click_count: 1,
        })?;
        thread::sleep(Duration::from_millis(30));
        session.input(BrowserInputEvent::Mouse {
            event_type: "mouseReleased".to_string(),
            x: target.x,
            y: target.y,
            button: "left".to_string(),
            buttons: Some(0),
            click_count: 1,
        })
    }

    /// Fills an input-like element ref from the last agent snapshot.
    pub(crate) fn agent_fill(
        &self,
        backend_session_id: &str,
        ref_id: &str,
        text: &str,
    ) -> Result<()> {
        let target = self.lookup_ref(backend_session_id, ref_id)?;
        let expression = fill_expression(target.x, target.y, text)?;
        self.get(backend_session_id)?.evaluate(expression)?;
        Ok(())
    }

    /// Presses one keyboard key in the target browser tab.
    pub(crate) fn agent_press(&self, backend_session_id: &str, key: &str) -> Result<()> {
        let code = key_code(key);
        let session = self.get(backend_session_id)?;
        session.input(BrowserInputEvent::Key {
            event_type: "rawKeyDown".to_string(),
            key: key.to_string(),
            code: code.clone(),
            text: None,
            modifiers: 0,
        })?;
        session.input(BrowserInputEvent::Key {
            event_type: "keyUp".to_string(),
            key: key.to_string(),
            code,
            text: None,
            modifiers: 0,
        })
    }

    fn lookup_ref(&self, backend_session_id: &str, ref_id: &str) -> Result<BrowserElementRef> {
        self.agent_refs
            .lock()
            .unwrap()
            .get(backend_session_id)
            .and_then(|refs| refs.iter().find(|item| item.ref_id == ref_id).cloned())
            .with_context(|| format!("no browser ref `{ref_id}`; run snapshot again"))
    }
}

fn ensure_target_tab(
    browsers: &BrowserRegistry,
    events: EventEmitter,
    root_session_id: &str,
    params: &Value,
    width: u32,
    height: u32,
) -> Result<(String, String)> {
    if let Some(tab_id) = optional_string(params, "tabId") {
        let backend_id = backend_session_id(root_session_id, &tab_id);
        ensure_backend_session(
            browsers,
            events.clone(),
            root_session_id,
            &tab_id,
            &backend_id,
            width,
            height,
        )?;
        return Ok((tab_id, backend_id));
    }
    let tabs = browsers.list_tabs(root_session_id);
    if let Some(tab) = active_or_first(&tabs) {
        ensure_backend_session(
            browsers,
            events.clone(),
            root_session_id,
            &tab.tab_id,
            &tab.backend_session_id,
            width,
            height,
        )?;
        return Ok((tab.tab_id, tab.backend_session_id));
    }
    let tab = browsers.open_tab(
        events.clone(),
        root_session_id.to_string(),
        None,
        None,
        Some(DEFAULT_URL.to_string()),
        width,
        height,
        true,
    )?;
    publish_tabs(browsers, &events, root_session_id);
    Ok((tab.tab_id, tab.backend_session_id))
}

fn ensure_backend_session(
    browsers: &BrowserRegistry,
    events: EventEmitter,
    root_session_id: &str,
    tab_id: &str,
    backend_id: &str,
    width: u32,
    height: u32,
) -> Result<()> {
    if browsers.resize(backend_id, width, height).is_ok() {
        return Ok(());
    }
    let browser_state = browsers.open(
        events.clone(),
        backend_id.to_string(),
        Some(DEFAULT_URL.to_string()),
        width,
        height,
    )?;
    browsers.tabs.lock().unwrap().open_tab(
        root_session_id,
        Some(tab_id.to_string()),
        None,
        backend_id.to_string(),
        browser_state,
        false,
    );
    publish_tabs(browsers, &events, root_session_id);
    Ok(())
}

fn active_or_first(tabs: &BrowserTabsState) -> Option<BrowserTabInfo> {
    tabs.tabs
        .iter()
        .find(|tab| tab.active)
        .or_else(|| tabs.tabs.first())
        .cloned()
}

fn publish_tabs(browsers: &BrowserRegistry, events: &EventEmitter, root_session_id: &str) {
    events.emit(
        format!("browser:{root_session_id}:tabs"),
        serde_json::to_value(browsers.list_tabs(root_session_id))
            .unwrap_or_else(|_| json!({ "tabs": [] })),
    );
}

fn optional_string(params: &Value, key: &str) -> Option<String> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn snapshot_expression() -> &'static str {
    r#"(() => {
  const isVisible = (el) => {
    const style = getComputedStyle(el);
    if (style.visibility === 'hidden' || style.display === 'none') return false;
    const rect = el.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0 && rect.bottom >= 0 && rect.right >= 0 &&
      rect.top <= innerHeight && rect.left <= innerWidth;
  };
  const nameFor = (el) => {
    const aria = el.getAttribute('aria-label') || el.getAttribute('alt') || el.getAttribute('title');
    if (aria) return aria.trim();
    if (el.labels && el.labels.length) return Array.from(el.labels).map((label) => label.innerText).join(' ').trim();
    if (el.placeholder) return el.placeholder.trim();
    if (el.value && el.tagName !== 'OPTION') return String(el.value).trim();
    return (el.innerText || el.textContent || '').replace(/\s+/g, ' ').trim();
  };
  const roleFor = (el) => {
    const explicit = el.getAttribute('role');
    if (explicit) return explicit;
    const tag = el.tagName.toLowerCase();
    if (tag === 'a') return 'link';
    if (tag === 'button') return 'button';
    if (tag === 'input') return el.type || 'textbox';
    if (tag === 'textarea') return 'textbox';
    if (tag === 'select') return 'combobox';
    return tag;
  };
  const selector = 'a,button,input,textarea,select,summary,[role],[contenteditable="true"],[tabindex],label';
  const elements = Array.from(document.querySelectorAll(selector)).filter(isVisible).slice(0, 120).map((el, index) => {
    const rect = el.getBoundingClientRect();
    return {
      ref: `@e${index + 1}`,
      role: roleFor(el),
      name: nameFor(el).slice(0, 160),
      tag: el.tagName.toLowerCase(),
      href: el.href || null,
      x: rect.left + rect.width / 2,
      y: rect.top + rect.height / 2
    };
  });
  return {
    url: location.href,
    title: document.title,
    text: (document.body ? document.body.innerText : '').replace(/\n{3,}/g, '\n\n').slice(0, 6000),
    elements
  };
})()"#
}

fn fill_expression(x: f64, y: f64, text: &str) -> Result<String> {
    let text = serde_json::to_string(text)?;
    Ok(format!(
        r#"(() => {{
  const el = document.elementFromPoint({x}, {y});
  if (!el) throw new Error('No element at target ref');
  const target = el.closest('input, textarea, [contenteditable="true"]') || el;
  target.focus();
  if ('value' in target) {{
    target.value = {text};
    target.dispatchEvent(new InputEvent('input', {{ bubbles: true, inputType: 'insertText', data: {text} }}));
    target.dispatchEvent(new Event('change', {{ bubbles: true }}));
  }} else if (target.isContentEditable) {{
    target.textContent = {text};
    target.dispatchEvent(new InputEvent('input', {{ bubbles: true, inputType: 'insertText', data: {text} }}));
  }} else {{
    throw new Error('Target is not editable');
  }}
  return true;
}})()"#
    ))
}

fn key_code(key: &str) -> String {
    match key {
        "Enter" => "Enter",
        "Escape" => "Escape",
        "Tab" => "Tab",
        "Backspace" => "Backspace",
        "Delete" => "Delete",
        "ArrowUp" => "ArrowUp",
        "ArrowDown" => "ArrowDown",
        "ArrowLeft" => "ArrowLeft",
        "ArrowRight" => "ArrowRight",
        value if value.len() == 1 && value.chars().all(|c| c.is_ascii_alphabetic()) => {
            return format!("Key{}", value.to_ascii_uppercase());
        }
        value if value.len() == 1 && value.chars().all(|c| c.is_ascii_digit()) => {
            return format!("Digit{value}");
        }
        _ => key,
    }
    .to_string()
}

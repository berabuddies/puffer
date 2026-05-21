//! Agent-facing browser actions layered over the managed Chrome sessions.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::daemon::{DaemonState, ServerEnvelope};

use super::params::{optional_u32, required_string, required_string_array};
use super::screenshot::{parse_agent_screenshot_options, BrowserElementRef};
use super::tabs::{backend_session_id, BrowserTabInfo, BrowserTabsState};
use super::upload::upload_input_handle_expression;
use super::{
    BrowserHistoryDirection, BrowserInputEvent, BrowserRegistry, DEFAULT_URL, INITIAL_HEIGHT,
    INITIAL_WIDTH,
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrowserCheckableState {
    kind: String,
    checked: bool,
}

/// Handles `browser_agent`, the agent-oriented browser action endpoint.
pub(crate) fn handle_browser_agent(state: &Arc<DaemonState>, params: &Value) -> Result<Value> {
    let action = required_string(params, "action")?;
    let root_session_id = required_string(params, "sessionId")?;
    let width = optional_u32(params, "width").unwrap_or(INITIAL_WIDTH);
    let height = optional_u32(params, "height").unwrap_or(INITIAL_HEIGHT);
    match action.as_str() {
        "list" => Ok(serde_json::to_value(
            state.browsers.list_tabs(&root_session_id),
        )?),
        "open" => {
            let tab_id =
                resolve_open_target_tab_id(&state.browsers, &root_session_id, params, true);
            arm_agent_recording(state, &root_session_id, &tab_id);
            let tab = open_agent_tab(
                state,
                &root_session_id,
                params,
                width,
                height,
                true,
                Some(tab_id),
            )?;
            state.browsers.arm_agent_recording(&tab.backend_session_id);
            publish_tabs(state, &root_session_id);
            Ok(serde_json::to_value(tab)?)
        }
        "new" => {
            let tab_id =
                resolve_open_target_tab_id(&state.browsers, &root_session_id, params, false);
            arm_agent_recording(state, &root_session_id, &tab_id);
            let tab = open_agent_tab(
                state,
                &root_session_id,
                params,
                width,
                height,
                false,
                Some(tab_id),
            )?;
            state.browsers.arm_agent_recording(&tab.backend_session_id);
            publish_tabs(state, &root_session_id);
            Ok(serde_json::to_value(tab)?)
        }
        "focus" => {
            let tab_id = required_string(params, "tabId")?;
            let tab = state.browsers.focus_tab(&root_session_id, &tab_id)?;
            publish_tabs(state, &root_session_id);
            Ok(serde_json::to_value(tab)?)
        }
        "close" => {
            let tab_id = optional_string(params, "tabId")
                .or_else(|| {
                    active_or_first(&state.browsers.list_tabs(&root_session_id))
                        .map(|tab| tab.tab_id)
                })
                .with_context(|| format!("no browser tabs for session `{root_session_id}`"))?;
            let tabs = state.browsers.close_tab(&root_session_id, &tab_id)?;
            publish_tabs(state, &root_session_id);
            Ok(serde_json::to_value(tabs)?)
        }
        "quit" | "exit" => {
            state.browsers.close_root(&root_session_id)?;
            let tabs = state.browsers.list_tabs(&root_session_id);
            publish_tabs(state, &root_session_id);
            Ok(serde_json::to_value(tabs)?)
        }
        "navigate" => {
            let url = required_string(params, "url")?;
            let (tab_id, backend_id) =
                ensure_target_tab(state, &root_session_id, params, width, height)?;
            state.browsers.arm_agent_recording(&backend_id);
            state.browsers.navigate(&backend_id, url)?;
            state.browsers.focus_tab(&root_session_id, &tab_id)?;
            publish_tabs(state, &root_session_id);
            Ok(serde_json::to_value(
                state.browsers.list_tabs(&root_session_id),
            )?)
        }
        "reload" => {
            let (_, backend_id) =
                ensure_target_tab(state, &root_session_id, params, width, height)?;
            state.browsers.arm_agent_recording(&backend_id);
            state.browsers.reload(&backend_id)?;
            Ok(json!({ "ok": true }))
        }
        "back" | "forward" => {
            let (_, backend_id) =
                ensure_target_tab(state, &root_session_id, params, width, height)?;
            state.browsers.arm_agent_recording(&backend_id);
            let direction = if action == "back" {
                BrowserHistoryDirection::Back
            } else {
                BrowserHistoryDirection::Forward
            };
            state.browsers.history(&backend_id, direction)?;
            Ok(json!({ "ok": true }))
        }
        "snapshot" => {
            let (_, backend_id) =
                ensure_target_tab(state, &root_session_id, params, width, height)?;
            state.browsers.arm_agent_recording(&backend_id);
            state.browsers.agent_snapshot(&backend_id)
        }
        "screenshot" => {
            let (tab_id, backend_id) =
                ensure_target_tab(state, &root_session_id, params, width, height)?;
            state.browsers.arm_agent_recording(&backend_id);
            let options = parse_agent_screenshot_options(params)?;
            state
                .browsers
                .agent_screenshot(&backend_id, &tab_id, options)
        }
        "click" => {
            let (_, backend_id) =
                ensure_target_tab(state, &root_session_id, params, width, height)?;
            state.browsers.arm_agent_recording(&backend_id);
            let target = required_string(params, "ref")?;
            state.browsers.agent_click(&backend_id, &target)?;
            Ok(json!({ "ok": true }))
        }
        "dblclick" => {
            let (_, backend_id) =
                ensure_target_tab(state, &root_session_id, params, width, height)?;
            state.browsers.arm_agent_recording(&backend_id);
            let target = required_string(params, "ref")?;
            state.browsers.agent_double_click(&backend_id, &target)?;
            Ok(json!({ "ok": true }))
        }
        "hover" => {
            let (_, backend_id) =
                ensure_target_tab(state, &root_session_id, params, width, height)?;
            state.browsers.arm_agent_recording(&backend_id);
            let target = required_string(params, "ref")?;
            state.browsers.agent_hover(&backend_id, &target)?;
            Ok(json!({ "ok": true }))
        }
        "focus_ref" => {
            let (_, backend_id) =
                ensure_target_tab(state, &root_session_id, params, width, height)?;
            state.browsers.arm_agent_recording(&backend_id);
            let target = required_string(params, "ref")?;
            state.browsers.agent_focus(&backend_id, &target)?;
            Ok(json!({ "ok": true }))
        }
        "type" => {
            let (_, backend_id) =
                ensure_target_tab(state, &root_session_id, params, width, height)?;
            state.browsers.arm_agent_recording(&backend_id);
            if let Some(target) = optional_string(params, "ref") {
                state.browsers.agent_click(&backend_id, &target)?;
                thread::sleep(Duration::from_millis(40));
            }
            let text = required_string(params, "text")?;
            state
                .browsers
                .input(&backend_id, BrowserInputEvent::Text { text })?;
            Ok(json!({ "ok": true }))
        }
        "insertText" => {
            let (_, backend_id) =
                ensure_target_tab(state, &root_session_id, params, width, height)?;
            state.browsers.arm_agent_recording(&backend_id);
            let text = required_string(params, "text")?;
            state
                .browsers
                .input(&backend_id, BrowserInputEvent::Text { text })?;
            Ok(json!({ "ok": true }))
        }
        "fill" => {
            let (_, backend_id) =
                ensure_target_tab(state, &root_session_id, params, width, height)?;
            state.browsers.arm_agent_recording(&backend_id);
            let target = required_string(params, "ref")?;
            let text = required_string(params, "text")?;
            state.browsers.agent_fill(&backend_id, &target, &text)?;
            Ok(json!({ "ok": true }))
        }
        "select" => {
            let (_, backend_id) =
                ensure_target_tab(state, &root_session_id, params, width, height)?;
            state.browsers.arm_agent_recording(&backend_id);
            let target = required_string(params, "ref")?;
            let value = required_string(params, "value")?;
            state.browsers.agent_select(&backend_id, &target, &value)?;
            Ok(json!({ "ok": true }))
        }
        "upload" => {
            let (_, backend_id) =
                ensure_target_tab(state, &root_session_id, params, width, height)?;
            state.browsers.arm_agent_recording(&backend_id);
            let target = required_string(params, "ref")?;
            let files = required_string_array(params, "files")?;
            state.browsers.agent_upload(&backend_id, &target, files)?;
            Ok(json!({ "ok": true }))
        }
        "check" => {
            let (_, backend_id) =
                ensure_target_tab(state, &root_session_id, params, width, height)?;
            state.browsers.arm_agent_recording(&backend_id);
            let target = required_string(params, "ref")?;
            state.browsers.agent_check(&backend_id, &target)?;
            Ok(json!({ "ok": true }))
        }
        "uncheck" => {
            let (_, backend_id) =
                ensure_target_tab(state, &root_session_id, params, width, height)?;
            state.browsers.arm_agent_recording(&backend_id);
            let target = required_string(params, "ref")?;
            state.browsers.agent_uncheck(&backend_id, &target)?;
            Ok(json!({ "ok": true }))
        }
        "press" => {
            let (_, backend_id) =
                ensure_target_tab(state, &root_session_id, params, width, height)?;
            state.browsers.arm_agent_recording(&backend_id);
            let key = required_string(params, "key")?;
            state.browsers.agent_press(&backend_id, &key)?;
            Ok(json!({ "ok": true }))
        }
        "keydown" => {
            let (_, backend_id) =
                ensure_target_tab(state, &root_session_id, params, width, height)?;
            state.browsers.arm_agent_recording(&backend_id);
            let key = required_string(params, "key")?;
            state.browsers.agent_key_down(&backend_id, &key)?;
            Ok(json!({ "ok": true }))
        }
        "keyup" => {
            let (_, backend_id) =
                ensure_target_tab(state, &root_session_id, params, width, height)?;
            state.browsers.arm_agent_recording(&backend_id);
            let key = required_string(params, "key")?;
            state.browsers.agent_key_up(&backend_id, &key)?;
            Ok(json!({ "ok": true }))
        }
        "scroll" => {
            let (_, backend_id) =
                ensure_target_tab(state, &root_session_id, params, width, height)?;
            state.browsers.arm_agent_recording(&backend_id);
            let direction = required_string(params, "direction")?;
            let px = optional_u32(params, "px").unwrap_or(600);
            state.browsers.agent_scroll(&backend_id, &direction, px)?;
            Ok(json!({ "ok": true }))
        }
        "scrollIntoView" => {
            let (_, backend_id) =
                ensure_target_tab(state, &root_session_id, params, width, height)?;
            state.browsers.arm_agent_recording(&backend_id);
            let target = required_string(params, "ref")?;
            state
                .browsers
                .agent_scroll_into_view(&backend_id, &target)?;
            Ok(json!({ "ok": true }))
        }
        "evaluate" | "eval" => {
            let (_, backend_id) =
                ensure_target_tab(state, &root_session_id, params, width, height)?;
            state.browsers.arm_agent_recording(&backend_id);
            let script = required_string(params, "script")?;
            let value = state.browsers.get(&backend_id)?.evaluate(script)?.value;
            Ok(json!({ "value": value }))
        }
        other => bail!("unsupported browser agent action `{other}`"),
    }
}

fn arm_agent_recording(state: &Arc<DaemonState>, root_session_id: &str, tab_id: &str) {
    state
        .browsers
        .arm_agent_recording(&backend_session_id(root_session_id, tab_id));
}

fn open_agent_tab(
    state: &Arc<DaemonState>,
    root_session_id: &str,
    params: &Value,
    width: u32,
    height: u32,
    reuse_existing: bool,
    resolved_tab_id: Option<String>,
) -> Result<BrowserTabInfo> {
    if let Some(tab_id) = resolved_tab_id.or_else(|| optional_string(params, "tabId")) {
        return state.browsers.open_tab(
            state.event_sender(),
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
    if reuse_existing {
        if let Some(tab) = active_or_first(&state.browsers.list_tabs(root_session_id)) {
            return state.browsers.open_tab(
                state.event_sender(),
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
    }
    state.browsers.open_tab(
        state.event_sender(),
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
    reuse_existing: bool,
) -> String {
    if let Some(tab_id) = optional_string(params, "tabId") {
        return tab_id;
    }
    if reuse_existing {
        if let Some(tab) = active_or_first(&browsers.list_tabs(root_session_id)) {
            return tab.tab_id;
        }
    }
    browsers.tabs.lock().unwrap().next_tab_id(root_session_id)
}

impl BrowserRegistry {
    /// Clicks an element ref from the last agent snapshot.
    pub(crate) fn agent_click(&self, backend_session_id: &str, ref_id: &str) -> Result<()> {
        let target = self.lookup_ref(backend_session_id, ref_id)?;
        let session = self.get(backend_session_id)?;
        move_mouse(&session, target.x, target.y)?;
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

    /// Double-clicks an element ref from the last agent snapshot.
    pub(crate) fn agent_double_click(&self, backend_session_id: &str, ref_id: &str) -> Result<()> {
        let target = self.lookup_ref(backend_session_id, ref_id)?;
        let session = self.get(backend_session_id)?;
        move_mouse(&session, target.x, target.y)?;
        dispatch_click(&session, target.x, target.y, 1)?;
        thread::sleep(Duration::from_millis(30));
        dispatch_click(&session, target.x, target.y, 2)
    }

    /// Moves the pointer over an element ref from the last agent snapshot.
    pub(crate) fn agent_hover(&self, backend_session_id: &str, ref_id: &str) -> Result<()> {
        let target = self.lookup_ref(backend_session_id, ref_id)?;
        move_mouse(&self.get(backend_session_id)?, target.x, target.y)
    }

    /// Focuses an element ref from the last agent snapshot.
    pub(crate) fn agent_focus(&self, backend_session_id: &str, ref_id: &str) -> Result<()> {
        let target = self.lookup_ref(backend_session_id, ref_id)?;
        let expression = focus_expression(target.x, target.y);
        self.get(backend_session_id)?.evaluate(expression)?;
        Ok(())
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

    /// Selects one option in a native `<select>` ref from the last agent snapshot.
    pub(crate) fn agent_select(
        &self,
        backend_session_id: &str,
        ref_id: &str,
        value: &str,
    ) -> Result<()> {
        let target = self.lookup_ref(backend_session_id, ref_id)?;
        let expression = select_expression(target.x, target.y, value)?;
        self.get(backend_session_id)?.evaluate(expression)?;
        Ok(())
    }

    /// Uploads one or more files into a native file input ref from the last agent snapshot.
    pub(crate) fn agent_upload(
        &self,
        backend_session_id: &str,
        ref_id: &str,
        files: Vec<String>,
    ) -> Result<()> {
        let target = self.lookup_ref(backend_session_id, ref_id)?;
        let expression = upload_input_handle_expression(target.x, target.y);
        self.get(backend_session_id)?.upload(expression, files)
    }

    /// Checks one checkbox-like ref from the last agent snapshot.
    pub(crate) fn agent_check(&self, backend_session_id: &str, ref_id: &str) -> Result<()> {
        self.set_checkable_state(backend_session_id, ref_id, true)
    }

    /// Unchecks one checkbox-like ref from the last agent snapshot.
    pub(crate) fn agent_uncheck(&self, backend_session_id: &str, ref_id: &str) -> Result<()> {
        self.set_checkable_state(backend_session_id, ref_id, false)
    }

    /// Presses one keyboard key in the target browser tab.
    pub(crate) fn agent_press(&self, backend_session_id: &str, key: &str) -> Result<()> {
        self.agent_key_down(backend_session_id, key)?;
        self.agent_key_up(backend_session_id, key)
    }

    /// Holds one keyboard key down in the target browser tab.
    pub(crate) fn agent_key_down(&self, backend_session_id: &str, key: &str) -> Result<()> {
        let code = key_code(key);
        self.get(backend_session_id)?.input(BrowserInputEvent::Key {
            event_type: "rawKeyDown".to_string(),
            key: key.to_string(),
            code,
            text: key_text(key),
            modifiers: 0,
        })
    }

    /// Releases one keyboard key in the target browser tab.
    pub(crate) fn agent_key_up(&self, backend_session_id: &str, key: &str) -> Result<()> {
        let code = key_code(key);
        self.get(backend_session_id)?.input(BrowserInputEvent::Key {
            event_type: "keyUp".to_string(),
            key: key.to_string(),
            code,
            text: None,
            modifiers: 0,
        })
    }

    /// Scrolls the target tab by a fixed amount in one direction.
    pub(crate) fn agent_scroll(
        &self,
        backend_session_id: &str,
        direction: &str,
        px: u32,
    ) -> Result<()> {
        let (delta_x, delta_y) = scroll_delta(direction, px)?;
        let session = self.get(backend_session_id)?;
        let state = session.state();
        session.input(BrowserInputEvent::Wheel {
            x: f64::from(state.width.max(1)) / 2.0,
            y: f64::from(state.height.max(1)) / 2.0,
            delta_x,
            delta_y,
        })
    }

    /// Scrolls an element ref into view from the last agent snapshot.
    pub(crate) fn agent_scroll_into_view(
        &self,
        backend_session_id: &str,
        ref_id: &str,
    ) -> Result<()> {
        let target = self.lookup_ref(backend_session_id, ref_id)?;
        let expression = scroll_into_view_expression(target.x, target.y);
        self.get(backend_session_id)?.evaluate(expression)?;
        Ok(())
    }

    fn lookup_ref(&self, backend_session_id: &str, ref_id: &str) -> Result<BrowserElementRef> {
        self.agent_refs
            .lock()
            .unwrap()
            .get(backend_session_id)
            .and_then(|refs| refs.iter().find(|item| item.ref_id == ref_id).cloned())
            .with_context(|| format!("no browser ref `{ref_id}`; run snapshot again"))
    }

    fn set_checkable_state(
        &self,
        backend_session_id: &str,
        ref_id: &str,
        checked: bool,
    ) -> Result<()> {
        let target = self.lookup_ref(backend_session_id, ref_id)?;
        let session = self.get(backend_session_id)?;
        let current = checkable_state_at_point(&session, target.x, target.y)?;
        if current.checked == checked {
            return Ok(());
        }
        if !checked && current.kind == "radio" {
            bail!("radio buttons cannot be unchecked directly with the current browser ref model");
        }
        move_mouse(&session, target.x, target.y)?;
        dispatch_click(&session, target.x, target.y, 1)?;
        thread::sleep(Duration::from_millis(40));
        let updated = checkable_state_at_point(&session, target.x, target.y)?;
        if updated.checked != checked {
            let status = if checked { "checked" } else { "unchecked" };
            bail!("target did not become {status}");
        }
        Ok(())
    }
}

fn ensure_target_tab(
    state: &Arc<DaemonState>,
    root_session_id: &str,
    params: &Value,
    width: u32,
    height: u32,
) -> Result<(String, String)> {
    let tabs = state.browsers.list_tabs(root_session_id);
    if let Some(tab_id) = optional_string(params, "tabId") {
        let backend_id = backend_session_id(root_session_id, &tab_id);
        let restore_url = tabs
            .tabs
            .iter()
            .find(|tab| tab.tab_id == tab_id)
            .map(|tab| tab.url.clone())
            .unwrap_or_else(|| DEFAULT_URL.to_string());
        ensure_backend_session(
            state,
            root_session_id,
            &tab_id,
            &backend_id,
            restore_url,
            width,
            height,
        )?;
        return Ok((tab_id, backend_id));
    }
    if let Some(tab) = active_or_first(&tabs) {
        ensure_backend_session(
            state,
            root_session_id,
            &tab.tab_id,
            &tab.backend_session_id,
            tab.url.clone(),
            width,
            height,
        )?;
        return Ok((tab.tab_id, tab.backend_session_id));
    }
    let tab = state.browsers.open_tab(
        state.event_sender(),
        root_session_id.to_string(),
        None,
        None,
        Some(DEFAULT_URL.to_string()),
        width,
        height,
        true,
    )?;
    publish_tabs(state, root_session_id);
    Ok((tab.tab_id, tab.backend_session_id))
}

fn ensure_backend_session(
    state: &Arc<DaemonState>,
    root_session_id: &str,
    tab_id: &str,
    backend_id: &str,
    restore_url: String,
    width: u32,
    height: u32,
) -> Result<()> {
    if state.browsers.resize(backend_id, width, height).is_ok() {
        return Ok(());
    }
    let browser_state = state.browsers.open(
        state.event_sender(),
        backend_id.to_string(),
        Some(restore_url),
        width,
        height,
    )?;
    state.browsers.tabs.lock().unwrap().open_tab(
        root_session_id,
        Some(tab_id.to_string()),
        None,
        backend_id.to_string(),
        browser_state,
        false,
    );
    publish_tabs(state, root_session_id);
    Ok(())
}

fn active_or_first(tabs: &BrowserTabsState) -> Option<BrowserTabInfo> {
    tabs.tabs
        .iter()
        .find(|tab| tab.active)
        .or_else(|| tabs.tabs.first())
        .cloned()
}

fn publish_tabs(state: &Arc<DaemonState>, root_session_id: &str) {
    state.publish_event(ServerEnvelope::Event {
        event: format!("browser:{root_session_id}:tabs"),
        payload: serde_json::to_value(state.browsers.list_tabs(root_session_id))
            .unwrap_or_else(|_| json!({ "tabs": [] })),
    });
}

fn optional_string(params: &Value, key: &str) -> Option<String> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

/// Builds the JavaScript used to fill one editable control at a viewport point.
pub(super) fn fill_expression(x: f64, y: f64, text: &str) -> Result<String> {
    let text = serde_json::to_string(text)?;
    Ok(format!(
        r#"(() => {{
  const el = document.elementFromPoint({x}, {y});
  if (!el) throw new Error('No element at target ref');
  const editableSelector = 'input, textarea, [contenteditable="true"]';
  const resolveEditable = (node) => {{
    if (!node) return null;
    const direct = node.closest(editableSelector);
    if (direct) return direct;
    const label = node.closest('label');
    if (label) {{
      if (label.control) return label.control;
      const nested = label.querySelector(editableSelector);
      if (nested) return nested;
    }}
    return node.querySelector?.(editableSelector) || null;
  }};
  const target = resolveEditable(el);
  if (!target) throw new Error('Target is not editable');
  target.focus();
  if ('value' in target) {{
    const prototype = target instanceof HTMLTextAreaElement
      ? HTMLTextAreaElement.prototype
      : HTMLInputElement.prototype;
    const descriptor = Object.getOwnPropertyDescriptor(prototype, 'value');
    if (descriptor && descriptor.set) {{
      descriptor.set.call(target, {text});
    }} else {{
      target.value = {text};
    }}
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

/// Builds the JavaScript used to focus one element at a viewport point.
pub(super) fn focus_expression(x: f64, y: f64) -> String {
    format!(
        r#"(() => {{
  const el = document.elementFromPoint({x}, {y});
  if (!el) throw new Error('No element at target ref');
  const target = el.closest('input, textarea, select, button, a, [tabindex], [contenteditable="true"], [role="button"], [role="link"], [role="checkbox"], [role="radio"], [role="switch"], [role="combobox"], [role="textbox"]') || el;
  if (typeof target.focus !== 'function') throw new Error('Target is not focusable');
  target.focus({{ preventScroll: false }});
  return true;
}})()"#
    )
}

/// Builds the JavaScript used to scroll one target element into view.
pub(super) fn scroll_into_view_expression(x: f64, y: f64) -> String {
    format!(
        r#"(() => {{
  const el = document.elementFromPoint({x}, {y});
  if (!el) throw new Error('No element at target ref');
  const target = el.closest('a, button, input, textarea, select, label, [contenteditable="true"], [role="button"], [role="link"], [role="checkbox"], [role="radio"], [role="switch"], [role="combobox"], [role="textbox"], [role="option"]') || el;
  target.scrollIntoView({{ block: 'center', inline: 'center', behavior: 'instant' }});
  return true;
}})()"#
    )
}

/// Builds the JavaScript used to select one native `<select>` option at a point.
pub(super) fn select_expression(x: f64, y: f64, value: &str) -> Result<String> {
    let value = serde_json::to_string(value)?;
    Ok(format!(
        r#"(() => {{
  const el = document.elementFromPoint({x}, {y});
  if (!el) throw new Error('No element at target ref');
  const normalize = (value) => String(value ?? '').trim();
  const requested = normalize({value});
  const resolveSelect = (node) => {{
    if (!node) return null;
    const direct = node.closest('select');
    if (direct) return direct;
    const label = node.closest('label');
    if (label) {{
      if (label.control instanceof HTMLSelectElement) return label.control;
      const nested = label.querySelector('select');
      if (nested) return nested;
    }}
    return node.querySelector?.('select') || null;
  }};
  const target = resolveSelect(el);
  if (!target) throw new Error('Target is not a native select control');
  const options = Array.from(target.options || []);
  const match = options.find((option) => {{
    const optionValue = normalize(option.value);
    const optionLabel = normalize(option.label || option.textContent || option.value);
    return optionValue === requested || optionLabel === requested;
  }});
  if (!match) {{
    const available = options
      .slice(0, 12)
      .map((option) => normalize(option.label || option.textContent || option.value))
      .filter(Boolean)
      .join(', ');
    throw new Error(
      available
        ? `No option matched "${{requested}}". Match exact option value or label text. Available: ${{available}}`
        : `No option matched "${{requested}}". Match exact option value or label text.`
    );
  }}
  for (const option of options) option.selected = option === match;
  target.value = match.value;
  target.dispatchEvent(new Event('input', {{ bubbles: true }}));
  target.dispatchEvent(new Event('change', {{ bubbles: true }}));
  return {{ value: match.value, label: normalize(match.label || match.textContent || match.value) }};
}})()"#
    ))
}

/// Builds the JavaScript used to inspect one checkbox-like control at a point.
pub(super) fn checkable_state_expression(x: f64, y: f64) -> String {
    format!(
        r#"(() => {{
  const el = document.elementFromPoint({x}, {y});
  if (!el) throw new Error('No element at target ref');
  const selector = 'input[type="checkbox"], input[type="radio"], [role="checkbox"], [role="radio"]';
  const resolveCheckable = (node) => {{
    if (!node) return null;
    if (node instanceof HTMLInputElement && (node.type === 'checkbox' || node.type === 'radio')) {{
      return node;
    }}
    const direct = node.closest(selector);
    if (direct) return direct;
    const label = node.closest('label');
    if (label) {{
      if (label.control instanceof HTMLInputElement &&
          (label.control.type === 'checkbox' || label.control.type === 'radio')) {{
        return label.control;
      }}
      const nested = label.querySelector(selector);
      if (nested) return nested;
    }}
    return node.querySelector?.(selector) || null;
  }};
  const target = resolveCheckable(el);
  if (!target) throw new Error('Target is not a checkbox or radio control');
  if (target instanceof HTMLInputElement) {{
    return {{
      kind: target.type === 'radio' ? 'radio' : 'checkbox',
      checked: !!target.checked
    }};
  }}
  return {{
    kind: target.getAttribute('role') === 'radio' ? 'radio' : 'checkbox',
    checked: target.getAttribute('aria-checked') === 'true'
  }};
}})()"#
    )
}

fn checkable_state_at_point(
    session: &super::BrowserSession,
    x: f64,
    y: f64,
) -> Result<BrowserCheckableState> {
    let value = session.evaluate(checkable_state_expression(x, y))?.value;
    serde_json::from_value(value).context("decode browser checkable state")
}

fn move_mouse(session: &super::BrowserSession, x: f64, y: f64) -> Result<()> {
    session.input(BrowserInputEvent::Mouse {
        event_type: "mouseMoved".to_string(),
        x,
        y,
        button: "none".to_string(),
        buttons: Some(0),
        click_count: 0,
    })
}

fn dispatch_click(session: &super::BrowserSession, x: f64, y: f64, click_count: u32) -> Result<()> {
    session.input(BrowserInputEvent::Mouse {
        event_type: "mousePressed".to_string(),
        x,
        y,
        button: "left".to_string(),
        buttons: Some(1),
        click_count,
    })?;
    thread::sleep(Duration::from_millis(30));
    session.input(BrowserInputEvent::Mouse {
        event_type: "mouseReleased".to_string(),
        x,
        y,
        button: "left".to_string(),
        buttons: Some(0),
        click_count,
    })
}

/// Returns the text payload for one synthesized key event when applicable.
pub(super) fn key_text(key: &str) -> Option<String> {
    (key.len() == 1).then(|| key.to_string())
}

/// Converts one named scroll direction into wheel deltas.
pub(super) fn scroll_delta(direction: &str, px: u32) -> Result<(f64, f64)> {
    match direction {
        "up" => Ok((0.0, -f64::from(px))),
        "down" => Ok((0.0, f64::from(px))),
        "left" => Ok((-f64::from(px), 0.0)),
        "right" => Ok((f64::from(px), 0.0)),
        other => bail!("unsupported scroll direction `{other}`; use up, down, left, or right"),
    }
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

#[cfg(test)]
mod tests {
    use super::super::BrowserState;
    use super::*;

    fn test_browser_state(url: &str) -> BrowserState {
        BrowserState {
            url: url.to_string(),
            title: String::new(),
            loading: false,
            width: INITIAL_WIDTH,
            height: INITIAL_HEIGHT,
        }
    }

    #[test]
    fn open_target_resolution_reserves_first_new_tab() {
        let profile = tempfile::tempdir().unwrap();
        let browsers = BrowserRegistry::new(profile.path().to_path_buf(), false);
        let params = json!({});

        let tab_id = resolve_open_target_tab_id(&browsers, "root", &params, true);
        assert_eq!(tab_id, "t1");
        let backend_id = backend_session_id("root", &tab_id);
        let tab = browsers.tabs.lock().unwrap().open_tab(
            "root",
            Some(tab_id.clone()),
            None,
            backend_id.clone(),
            test_browser_state("about:blank"),
            true,
        );

        assert_eq!(tab.backend_session_id, backend_id);
        assert_eq!(browsers.tabs.lock().unwrap().next_tab_id("root"), "t2");
    }

    #[test]
    fn open_target_resolution_reuses_active_tab() {
        let profile = tempfile::tempdir().unwrap();
        let browsers = BrowserRegistry::new(profile.path().to_path_buf(), false);
        browsers.tabs.lock().unwrap().open_tab(
            "root",
            Some("existing".to_string()),
            None,
            backend_session_id("root", "existing"),
            test_browser_state("https://example.com"),
            true,
        );

        let tab_id = resolve_open_target_tab_id(&browsers, "root", &json!({}), true);

        assert_eq!(tab_id, "existing");
    }
}

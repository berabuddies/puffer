//! Agent-facing browser actions layered over the managed Chrome sessions.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::daemon::{DaemonState, ServerEnvelope};

use super::dom_inspect::dom_inspect_expression;
use super::params::{optional_u32, required_string, required_string_array};
use super::ref_resolution::{
    fill_expression, focus_expression, hosted_fill_focus_check_expression,
    hosted_fill_point_expression, in_frame_prepare_fill_fn, in_frame_readback_fn,
    in_frame_select_fn, in_frame_set_checked_fn, scroll_into_view_expression, select_expression,
    set_checkable_state_expression, target_point_expression, upload_input_handle_expression,
};
use super::screenshot::{parse_agent_screenshot_options, BrowserElementRef};
use super::session::BrowserSession;
use super::tabs::{backend_session_id, BrowserTabInfo, BrowserTabsState};
use super::{
    browser_debug, BrowserHistoryDirection, BrowserInputEvent, BrowserRegistry, DEFAULT_URL,
    INITIAL_HEIGHT, INITIAL_WIDTH,
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
    browser_debug(
        "agent.request",
        format!(
            "action={} root_session_id={} tab_id={:?} width={} height={}",
            action,
            root_session_id,
            optional_string(params, "tabId"),
            width,
            height
        ),
    );
    match action.as_str() {
        "list" => {
            // Pull in any tab the user opened directly in the native browser so
            // the agent sees what the user is actually looking at (#649).
            state.browsers.sync_native_tabs(
                &state.event_sender(),
                &root_session_id,
                width,
                height,
            );
            Ok(serde_json::to_value(
                state.browsers.list_tabs(&root_session_id),
            )?)
        }
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
            browser_debug(
                "agent.close.resolved",
                format!("root_session_id={} tab_id={}", root_session_id, tab_id),
            );
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
        "domInspect" => {
            let (_, backend_id) =
                ensure_target_tab(state, &root_session_id, params, width, height)?;
            state.browsers.arm_agent_recording(&backend_id);
            let query = required_string(params, "query")?;
            Ok(state
                .browsers
                .get(&backend_id)?
                .evaluate(dom_inspect_expression(&query)?)?
                .value)
        }
        "consoleLogs" | "console" => {
            let (_, backend_id) =
                ensure_target_tab(state, &root_session_id, params, width, height)?;
            let clear = params
                .get("clear")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            state.browsers.console_logs(&backend_id, clear)
        }
        "waitNetworkIdle" => {
            let (_, backend_id) =
                ensure_target_tab(state, &root_session_id, params, width, height)?;
            state.browsers.wait_for_network_idle(
                &backend_id,
                network_idle_duration(params),
                navigation_timeout(params),
            )?;
            Ok(json!({ "ok": true }))
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
        "openScreenshot" => {
            let url = required_string(params, "url")?;
            let (tab_id, backend_id) =
                ensure_target_tab(state, &root_session_id, params, width, height)?;
            state.browsers.arm_agent_recording(&backend_id);
            state.browsers.navigate(&backend_id, url)?;
            state.browsers.focus_tab(&root_session_id, &tab_id)?;
            publish_tabs(state, &root_session_id);
            state
                .browsers
                .wait_for_load(&backend_id, navigation_timeout(params))?;
            let options = parse_agent_screenshot_options(params)?;
            state
                .browsers
                .agent_screenshot(&backend_id, &tab_id, options)
        }
        "openConsoleLogs" => {
            let url = required_string(params, "url")?;
            let (_, backend_id) =
                ensure_target_tab(state, &root_session_id, params, width, height)?;
            let _ = state.browsers.console_logs(&backend_id, true);
            state.browsers.arm_agent_recording(&backend_id);
            state.browsers.navigate(&backend_id, url)?;
            state
                .browsers
                .wait_for_load(&backend_id, navigation_timeout(params))?;
            let clear = params
                .get("clear")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            state.browsers.console_logs(&backend_id, clear)
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
            state.browsers.agent_fill(&backend_id, &target, &text)
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
    let background = background_requested(params);
    let activate = params
        .get("activate")
        .and_then(Value::as_bool)
        .unwrap_or(!background);
    if let Some(tab_id) = resolved_tab_id.or_else(|| optional_string(params, "tabId")) {
        return state.browsers.open_tab(
            state.event_sender(),
            root_session_id.to_string(),
            Some(tab_id),
            optional_string(params, "label"),
            optional_string(params, "url"),
            width,
            height,
            activate,
            background,
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
                activate,
                background,
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
        activate,
        background,
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
        let session = self.get(backend_session_id)?;
        let (x, y) = self.agent_target_point(backend_session_id, ref_id)?;
        dispatch_agent_mouse_click(&session, x, y, 1)?;
        Ok(())
    }

    /// Double-clicks an element ref from the last agent snapshot.
    pub(crate) fn agent_double_click(&self, backend_session_id: &str, ref_id: &str) -> Result<()> {
        let session = self.get(backend_session_id)?;
        let (x, y) = self.agent_target_point(backend_session_id, ref_id)?;
        dispatch_agent_mouse_click(&session, x, y, 1)?;
        dispatch_agent_mouse_click(&session, x, y, 2)?;
        Ok(())
    }

    /// Moves the pointer over an element ref from the last agent snapshot.
    pub(crate) fn agent_hover(&self, backend_session_id: &str, ref_id: &str) -> Result<()> {
        let session = self.get(backend_session_id)?;
        let (x, y) = self.agent_target_point(backend_session_id, ref_id)?;
        session.input(BrowserInputEvent::Mouse {
            event_type: "mouseMoved".to_string(),
            x,
            y,
            button: "none".to_string(),
            buttons: Some(0),
            click_count: 0,
        })?;
        Ok(())
    }

    /// Focuses an element ref from the last agent snapshot.
    pub(crate) fn agent_focus(&self, backend_session_id: &str, ref_id: &str) -> Result<()> {
        let target = self.lookup_ref(backend_session_id, ref_id)?;
        let session = self.get(backend_session_id)?;
        if let Some(backend_node_id) = in_frame_backend_node_id(&target) {
            session.cdp_call("DOM.focus", json!({ "backendNodeId": backend_node_id }))?;
            return Ok(());
        }
        session.evaluate(focus_expression(&target)?)?;
        Ok(())
    }

    /// Fills an input-like element ref from the last agent snapshot.
    ///
    /// Editable controls in the top document are filled directly. When the
    /// ref resolves to a hosted payment field — a cross-origin iframe (e.g.
    /// Shopify/Stripe PCI card fields) whose real `<input>` the top document
    /// cannot reach — the fill switches to trusted input: focus the frame
    /// with a real mouse click, select existing content with a triple
    /// click, and commit the text through `Input.insertText`, which the
    /// browser routes to the focused frame.
    pub(crate) fn agent_fill(
        &self,
        backend_session_id: &str,
        ref_id: &str,
        text: &str,
    ) -> Result<Value> {
        let target = self.lookup_ref(backend_session_id, ref_id)?;
        let session = self.get(backend_session_id)?;
        if target.in_frame {
            return in_frame_fill(&session, &target, text);
        }
        let outcome = session.evaluate(fill_expression(&target, text)?)?.value;
        let hosted = outcome
            .get("hostedFrameFill")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !hosted {
            return Ok(json!({ "ok": true }));
        }
        let x = outcome
            .get("x")
            .and_then(Value::as_f64)
            .context("hosted frame fill point missing x")?;
        let y = outcome
            .get("y")
            .and_then(Value::as_f64)
            .context("hosted frame fill point missing y")?;
        hosted_frame_fill(&session, x, y, text)
    }

    /// Selects one option in a native `<select>` ref from the last agent snapshot.
    pub(crate) fn agent_select(
        &self,
        backend_session_id: &str,
        ref_id: &str,
        value: &str,
    ) -> Result<()> {
        let target = self.lookup_ref(backend_session_id, ref_id)?;
        let session = self.get(backend_session_id)?;
        if target.in_frame {
            return in_frame_select(&session, &target, value);
        }
        session.evaluate(select_expression(&target, value)?)?;
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
        let session = self.get(backend_session_id)?;
        let expression = upload_input_handle_expression(&target)?;
        session.upload(expression, files)
    }

    /// Checks one checkbox-like ref from the last agent snapshot.
    pub(crate) fn agent_check(&self, backend_session_id: &str, ref_id: &str) -> Result<()> {
        self.set_checkable(backend_session_id, ref_id, true)
    }

    /// Unchecks one checkbox-like ref from the last agent snapshot.
    pub(crate) fn agent_uncheck(&self, backend_session_id: &str, ref_id: &str) -> Result<()> {
        self.set_checkable(backend_session_id, ref_id, false)
    }

    fn set_checkable(&self, backend_session_id: &str, ref_id: &str, checked: bool) -> Result<()> {
        let target = self.lookup_ref(backend_session_id, ref_id)?;
        let session = self.get(backend_session_id)?;
        if let Some(backend_node_id) = in_frame_backend_node_id(&target) {
            let object_id = resolve_in_frame_object_id(&session, backend_node_id)?;
            session.cdp_call(
                "Runtime.callFunctionOn",
                json!({
                    "objectId": object_id,
                    "functionDeclaration": in_frame_set_checked_fn(checked),
                    "returnByValue": true,
                }),
            )?;
            return Ok(());
        }
        session.evaluate(set_checkable_state_expression(&target, checked)?)?;
        Ok(())
    }

    /// Presses one keyboard key in the target browser tab.
    pub(crate) fn agent_press(&self, backend_session_id: &str, key: &str) -> Result<()> {
        self.agent_key_down(backend_session_id, key)?;
        self.agent_key_up(backend_session_id, key)
    }

    /// Holds one keyboard key down in the target browser tab.
    pub(crate) fn agent_key_down(&self, backend_session_id: &str, key: &str) -> Result<()> {
        let combo = parse_key_combo(key);
        let code = key_code(&combo.key);
        self.get(backend_session_id)?.input(BrowserInputEvent::Key {
            event_type: "rawKeyDown".to_string(),
            key: combo.key.clone(),
            code,
            text: key_text(&combo.key).filter(|_| combo.modifiers == 0),
            modifiers: combo.modifiers,
            commands: combo.commands,
        })
    }

    /// Releases one keyboard key in the target browser tab.
    pub(crate) fn agent_key_up(&self, backend_session_id: &str, key: &str) -> Result<()> {
        let combo = parse_key_combo(key);
        let code = key_code(&combo.key);
        self.get(backend_session_id)?.input(BrowserInputEvent::Key {
            event_type: "keyUp".to_string(),
            key: combo.key,
            code,
            text: None,
            modifiers: combo.modifiers,
            commands: Vec::new(),
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
        // An in-frame ref already carries a captured top-viewport quad center, so
        // it was rendered and in view at snapshot time; the top-document scroll
        // helper can't reach it. Treat scroll-into-view as a no-op.
        if target.in_frame {
            return Ok(());
        }
        self.get(backend_session_id)?
            .evaluate(scroll_into_view_expression(&target)?)?;
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

    fn agent_target_point(&self, backend_session_id: &str, ref_id: &str) -> Result<(f64, f64)> {
        let target = self.lookup_ref(backend_session_id, ref_id)?;
        if target.in_frame {
            // The top document can't re-resolve a cross-origin node, so trust the
            // field's quad center captured at snapshot time. A coordinate click
            // there is routed into the OOPIF by browser-side hit testing.
            if target.x.is_finite() && target.y.is_finite() {
                return Ok((target.x, target.y));
            }
            bail!("in-frame ref `{ref_id}` has no finite viewport point; re-snapshot");
        }
        let evaluated = self
            .get(backend_session_id)?
            .evaluate(target_point_expression(&target)?)?;
        let x = evaluated
            .value
            .get("x")
            .and_then(Value::as_f64)
            .context("browser ref target point missing x")?;
        let y = evaluated
            .value
            .get("y")
            .and_then(Value::as_f64)
            .context("browser ref target point missing y")?;
        if !x.is_finite() || !y.is_finite() {
            bail!("browser ref target point is not finite");
        }
        Ok((x, y))
    }
}

/// Fills one field that lives inside a cross-origin payment iframe (#656),
/// addressed by CDP node identity because the top document can't see into the
/// OOPIF. Focuses the field via `DOM.focus`, verifies focus actually landed
/// (the #580 guard) and clears it, then types one real keystroke per character
/// (`Input.dispatchKeyEvent`) — a guarded widget like Amazon's APX reverts a
/// bulk `Input.insertText` value, so per-character typing is required. Finally
/// reads the value back to confirm it persisted, which — unlike a top-document
/// hosted fill — is possible here.
fn in_frame_fill(session: &BrowserSession, target: &BrowserElementRef, text: &str) -> Result<Value> {
    let backend_node_id = target
        .backend_node_id
        .context("in-frame ref is missing a backend node id; re-snapshot")?;
    session
        .cdp_call("DOM.focus", json!({ "backendNodeId": backend_node_id }))
        .context("focus the in-frame field")?;
    let object_id = resolve_in_frame_object_id(session, backend_node_id)?;
    let prepared = session.cdp_call(
        "Runtime.callFunctionOn",
        json!({
            "objectId": object_id,
            "functionDeclaration": in_frame_prepare_fill_fn(),
            "returnByValue": true,
        }),
    )?;
    let focused = prepared
        .pointer("/result/value/focused")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !focused {
        bail!(
            "fill failed: focus did not land on the in-frame field (it may be covered or still \
             loading). No text was sent."
        );
    }
    // Type one real keystroke per character instead of a single Input.insertText.
    // A guarded card widget (e.g. Amazon's APX) builds its submitted value from
    // genuine keydown events and reverts a bulk programmatic/insertText value, so
    // insertText leaves the field empty at submit time even though it briefly
    // shows in the DOM (#656 follow-up).
    for ch in text.chars() {
        type_in_frame_char(session, ch)?;
    }
    // Let the widget's re-render settle, then read the value back. Unlike a
    // top-document hosted fill, an in-frame field's value CAN be read — so a
    // reverted/guarded fill is caught honestly instead of being reported as a
    // false success (which previously caused retries until Amazon rate-limited).
    thread::sleep(Duration::from_millis(180));
    let readback = session.cdp_call(
        "Runtime.callFunctionOn",
        json!({
            "objectId": object_id,
            "functionDeclaration": in_frame_readback_fn(),
            "returnByValue": true,
        }),
    )?;
    let value = readback
        .pointer("/result/value/value")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !text.is_empty() && value.is_empty() {
        bail!(
            "fill failed: the in-frame field reverted to empty after typing — the payment widget \
             rejected the input and did not keep it. No value stuck."
        );
    }
    Ok(json!({
        "ok": true,
        "mode": "inFrame",
        "note": "Typed into a field inside a cross-origin payment iframe and confirmed the value \
                 persisted."
    }))
}

/// Types one character into the focused in-frame field as a real keystroke
/// (`keyDown` carrying the text, then `keyUp`). `nativeVirtualKeyCode` is never
/// set (see input.rs / #636), so this stays on the renderer-only path.
fn type_in_frame_char(session: &BrowserSession, ch: char) -> Result<()> {
    let key = ch.to_string();
    let code = dom_code_for_char(ch);
    session.input(BrowserInputEvent::Key {
        event_type: "keyDown".to_string(),
        key: key.clone(),
        code: code.clone(),
        text: Some(key.clone()),
        modifiers: 0,
        commands: Vec::new(),
    })?;
    session.input(BrowserInputEvent::Key {
        event_type: "keyUp".to_string(),
        key,
        code,
        text: None,
        modifiers: 0,
        commands: Vec::new(),
    })
}

/// Best-effort DOM `code` (e.g. `Digit4`, `KeyA`) for a character so guarded
/// fields that inspect `event.code`/`keyCode` see a plausible key. Empty for
/// other characters; the carried `text` still inserts them.
fn dom_code_for_char(ch: char) -> String {
    if ch.is_ascii_digit() {
        format!("Digit{ch}")
    } else if ch.is_ascii_alphabetic() {
        format!("Key{}", ch.to_ascii_uppercase())
    } else {
        String::new()
    }
}

/// Selects one option on a native `<select>` inside a cross-origin payment
/// iframe (the Amazon expiry month/year). A native popup can't be driven by
/// coordinate clicks, so the value is set inside the frame via `callFunctionOn`,
/// firing `input`/`change`. Fails loudly when no option matches (#580: never
/// claim success silently).
fn in_frame_select(session: &BrowserSession, target: &BrowserElementRef, value: &str) -> Result<()> {
    let backend_node_id = target
        .backend_node_id
        .context("in-frame ref is missing a backend node id; re-snapshot")?;
    let object_id = resolve_in_frame_object_id(session, backend_node_id)?;
    let outcome = session.cdp_call(
        "Runtime.callFunctionOn",
        json!({
            "objectId": object_id,
            "functionDeclaration": in_frame_select_fn(value)?,
            "returnByValue": true,
        }),
    )?;
    let matched = outcome
        .pointer("/result/value/matched")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !matched {
        let available = outcome
            .pointer("/result/value/available")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if available.is_empty() {
            bail!("select failed: no option matched \"{value}\" in the in-frame dropdown");
        }
        bail!("select failed: no option matched \"{value}\". Available: {available}");
    }
    let selected = outcome
        .pointer("/result/value/value")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    // Confirm the selection persisted: a guarded/controlled dropdown can revert a
    // programmatic value after re-render, which would otherwise submit as the
    // default (#656 follow-up). Read it back after a beat and fail honestly.
    thread::sleep(Duration::from_millis(180));
    let readback = session.cdp_call(
        "Runtime.callFunctionOn",
        json!({
            "objectId": object_id,
            "functionDeclaration": in_frame_readback_fn(),
            "returnByValue": true,
        }),
    )?;
    let now = readback
        .pointer("/result/value/value")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if now != selected {
        bail!(
            "select failed: the in-frame dropdown reverted after selecting \"{value}\" (the widget \
             did not keep the selection)."
        );
    }
    Ok(())
}

/// Returns the CDP backend node id when a ref points inside a cross-origin
/// payment iframe, so ref actions can route through node-identity CDP calls
/// instead of the top-document `findTarget` path that can't reach it.
fn in_frame_backend_node_id(target: &BrowserElementRef) -> Option<i64> {
    if target.in_frame {
        target.backend_node_id
    } else {
        None
    }
}

/// Resolves a backend node inside a cross-origin frame to a JS object id in that
/// frame's own context, so `Runtime.callFunctionOn` runs inside the OOPIF.
fn resolve_in_frame_object_id(session: &BrowserSession, backend_node_id: i64) -> Result<String> {
    let resolved = session.cdp_call("DOM.resolveNode", json!({ "backendNodeId": backend_node_id }))?;
    resolved
        .pointer("/object/objectId")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .context("in-frame node could not be resolved to a script object")
}

/// Completes a fill that targets a hosted (cross-origin) field iframe.
///
/// The value cannot be read back from the top document, so this path is
/// strict about the one thing it can verify: focus must actually land inside
/// the target frame before any text is sent, otherwise the keystrokes would
/// leak into whatever element holds focus (#580-class silent misfill).
fn hosted_frame_fill(session: &BrowserSession, x: f64, y: f64, text: &str) -> Result<Value> {
    // Right after the probe's scrollIntoView the browser-side hit test can
    // lag the new layout by a frame, routing a correctly-placed click to the
    // parent document instead of the iframe. Give the compositor a beat,
    // then retry the whole click with refreshed coordinates if focus does
    // not arrive.
    let mut focused = false;
    let mut point = (x, y);
    for attempt in 0..4 {
        if attempt > 0 {
            let refreshed = session
                .evaluate(hosted_fill_point_expression().to_string())?
                .value;
            if let (Some(x), Some(y)) = (
                refreshed.get("x").and_then(Value::as_f64),
                refreshed.get("y").and_then(Value::as_f64),
            ) {
                point = (x, y);
            }
        }
        thread::sleep(Duration::from_millis(80));
        dispatch_agent_mouse_click(session, point.0, point.1, 1)?;
        for _ in 0..6 {
            thread::sleep(Duration::from_millis(50));
            let check = session
                .evaluate(hosted_fill_focus_check_expression().to_string())?
                .value;
            if check
                .get("focused")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                focused = true;
                break;
            }
        }
        if focused {
            break;
        }
    }
    if !focused {
        let (x, y) = point;
        bail!(
            "fill failed: clicking the hosted field iframe at ({x:.0}, {y:.0}) did not move focus \
             into the frame; the field may be covered by an overlay or still loading. \
             No text was sent."
        );
    }
    // Clear any prior content by selecting it with a triple click — the
    // standard select-all gesture for single-line fields. Mouse events are
    // routed by browser-side hit testing into the frame, and `insertText`
    // then replaces the live selection in the focused frame. No synthetic
    // keyboard events: key dispatch can stall the CT Chromium input pipeline
    // (observed as a key-event storm wedging CDP), and platform select-all
    // shortcuts never reach cross-origin frames anyway.
    dispatch_agent_mouse_click(session, point.0, point.1, 2)?;
    dispatch_agent_mouse_click(session, point.0, point.1, 3)?;
    session.input(BrowserInputEvent::Text {
        text: text.to_string(),
    })?;
    Ok(json!({
        "ok": true,
        "mode": "hostedFrame",
        "note": "Typed into a hosted (cross-origin) field iframe after verifying focus. \
                 The frame's value cannot be read back from the top document; confirm via \
                 the page's own field validation (e.g. error labels) in the next snapshot."
    }))
}

fn dispatch_agent_mouse_click(
    session: &BrowserSession,
    x: f64,
    y: f64,
    click_count: u32,
) -> Result<()> {
    session.input(BrowserInputEvent::Mouse {
        event_type: "mouseMoved".to_string(),
        x,
        y,
        button: "none".to_string(),
        buttons: Some(0),
        click_count: 0,
    })?;
    session.input(BrowserInputEvent::Mouse {
        event_type: "mousePressed".to_string(),
        x,
        y,
        button: "left".to_string(),
        buttons: Some(1),
        click_count,
    })?;
    session.input(BrowserInputEvent::Mouse {
        event_type: "mouseReleased".to_string(),
        x,
        y,
        button: "left".to_string(),
        buttons: Some(0),
        click_count,
    })
}

fn ensure_target_tab(
    state: &Arc<DaemonState>,
    root_session_id: &str,
    params: &Value,
    width: u32,
    height: u32,
) -> Result<(String, String)> {
    let background = background_requested(params);
    let tabs = state.browsers.list_tabs(root_session_id);
    if let Some(tab_id) = optional_string(params, "tabId").or_else(|| tab_id_from_page(params)) {
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
            background,
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
            background,
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
        !background,
        background,
    )?;
    publish_tabs(state, root_session_id);
    Ok((tab.tab_id, tab.backend_session_id))
}

fn tab_id_from_page(params: &Value) -> Option<String> {
    let page = params.get("page")?;
    if let Some(tab_id) = page
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Some(tab_id.to_string());
    }
    page.get("tabId")
        .or_else(|| page.get("tab_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn ensure_backend_session(
    state: &Arc<DaemonState>,
    root_session_id: &str,
    tab_id: &str,
    backend_id: &str,
    restore_url: String,
    width: u32,
    height: u32,
    background: bool,
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
        !background,
    )?;
    let native_cef_session_id = state
        .browsers
        .tabs
        .lock()
        .unwrap()
        .tab(root_session_id, tab_id)
        .and_then(|tab| tab.native_cef_session_id);
    state.browsers.tabs.lock().unwrap().open_tab(
        root_session_id,
        Some(tab_id.to_string()),
        None,
        backend_id.to_string(),
        native_cef_session_id,
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

fn background_requested(params: &Value) -> bool {
    params
        .get("background")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn navigation_timeout(params: &Value) -> Duration {
    Duration::from_millis(u64::from(
        optional_u32(params, "timeoutMs")
            .unwrap_or(30_000)
            .clamp(1, 120_000),
    ))
}

fn network_idle_duration(params: &Value) -> Duration {
    Duration::from_millis(u64::from(
        optional_u32(params, "idleMs")
            .unwrap_or(500)
            .clamp(0, 30_000),
    ))
}

/// Returns the text payload for one synthesized key event when applicable.
pub(super) fn key_text(key: &str) -> Option<String> {
    (key.len() == 1).then(|| key.to_string())
}

/// A parsed `Modifier+Key` combo for press/keydown/keyup actions.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct KeyCombo {
    pub(super) key: String,
    pub(super) modifiers: u32,
    pub(super) commands: Vec<String>,
}

/// Parses agent key strings like `Meta+A`, `Ctrl+Shift+Z`, or plain `Enter`
/// into a CDP key plus modifier mask. Editing shortcuts that depend on
/// platform-level translation (macOS `Cmd+A`) are mapped to explicit CDP
/// editing `commands` so they work in any focused frame, including
/// cross-origin payment iframes.
pub(super) fn parse_key_combo(raw: &str) -> KeyCombo {
    let mut modifiers = 0u32;
    let mut key = raw.to_string();
    if raw.len() > 1 && raw.contains('+') {
        let parts: Vec<&str> = raw.split('+').collect();
        let mut parsed_modifiers = 0u32;
        let mut consumed = 0usize;
        while consumed < parts.len() - 1 {
            match modifier_mask(parts[consumed]) {
                Some(mask) => {
                    parsed_modifiers |= mask;
                    consumed += 1;
                }
                None => break,
            }
        }
        if consumed > 0 {
            // The unconsumed tail is the key; `Meta++` leaves ["", ""],
            // which joins back into the literal `+` key.
            let tail = parts[consumed..].join("+");
            modifiers = parsed_modifiers;
            key = if tail.is_empty() {
                "+".to_string()
            } else {
                tail
            };
        }
    }
    let commands = editing_commands(&key, modifiers);
    KeyCombo {
        key,
        modifiers,
        commands,
    }
}

fn modifier_mask(name: &str) -> Option<u32> {
    match name.to_ascii_lowercase().as_str() {
        "alt" | "option" | "opt" => Some(1),
        "control" | "ctrl" => Some(2),
        "meta" | "cmd" | "command" | "super" | "win" => Some(4),
        "shift" => Some(8),
        _ => None,
    }
}

/// Maps primary-modifier editing shortcuts to CDP editing commands. CDP key
/// events are injected below the platform shortcut layer, so on macOS
/// `Cmd+A` never reaches the renderer as select-all unless the command is
/// attached explicitly.
fn editing_commands(key: &str, modifiers: u32) -> Vec<String> {
    let primary = modifiers & (2 | 4) != 0;
    let extra = modifiers & !(2 | 4);
    if !primary || extra != 0 {
        return Vec::new();
    }
    match key.to_ascii_lowercase().as_str() {
        "a" => vec!["selectAll".to_string()],
        _ => Vec::new(),
    }
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
        let browsers = BrowserRegistry::new(
            profile.path().to_path_buf(),
            false,
            crate::daemon_browser::BrowserLaunchSettings::default(),
        );
        let params = json!({});

        let tab_id = resolve_open_target_tab_id(&browsers, "root", &params, true);
        assert_eq!(tab_id, "t1");
        let backend_id = backend_session_id("root", &tab_id);
        let tab = browsers.tabs.lock().unwrap().open_tab(
            "root",
            Some(tab_id.clone()),
            None,
            backend_id.clone(),
            None,
            test_browser_state("about:blank"),
            true,
        );

        assert_eq!(tab.backend_session_id, backend_id);
        assert_eq!(browsers.tabs.lock().unwrap().next_tab_id("root"), "t2");
    }

    #[test]
    fn open_target_resolution_reuses_active_tab() {
        let profile = tempfile::tempdir().unwrap();
        let browsers = BrowserRegistry::new(
            profile.path().to_path_buf(),
            false,
            crate::daemon_browser::BrowserLaunchSettings::default(),
        );
        browsers.tabs.lock().unwrap().open_tab(
            "root",
            Some("existing".to_string()),
            None,
            backend_session_id("root", "existing"),
            None,
            test_browser_state("https://example.com"),
            true,
        );

        let tab_id = resolve_open_target_tab_id(&browsers, "root", &json!({}), true);

        assert_eq!(tab_id, "existing");
    }

    #[test]
    fn page_handle_can_resolve_target_tab_id() {
        assert_eq!(
            tab_id_from_page(&json!({ "page": { "tabId": "t3" } })).as_deref(),
            Some("t3")
        );
        assert_eq!(
            tab_id_from_page(&json!({ "page": "t4" })).as_deref(),
            Some("t4")
        );
    }
}

//! Session-scoped browser tab metadata for the desktop and agent APIs.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use url::Url;

use super::BrowserState;

/// Public metadata for one managed browser tab.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BrowserTabInfo {
    pub(crate) tab_id: String,
    pub(crate) label: String,
    pub(crate) url: String,
    pub(crate) title: String,
    pub(crate) loading: bool,
    pub(crate) connected: bool,
    pub(crate) active: bool,
    pub(crate) backend_session_id: String,
    pub(crate) native_cef_session_id: Option<String>,
    pub(crate) created_at_ms: u64,
    pub(crate) updated_at_ms: u64,
}

/// Public tab list state for one Puffer agent session.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BrowserTabsState {
    pub(crate) active_tab_id: Option<String>,
    pub(crate) tabs: Vec<BrowserTabInfo>,
}

/// Snapshot of the current active tab for permission-context hydration.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BrowserCurrentTabContext {
    pub(crate) status: BrowserCurrentTabStatus,
    pub(crate) tab_id: Option<String>,
    pub(crate) url: Option<String>,
    pub(crate) origin: Option<String>,
    pub(crate) host: Option<String>,
    pub(crate) port: Option<u16>,
    pub(crate) title: Option<String>,
    /// Whether the backing CDP session is still alive. A registry entry can
    /// outlive its browser (status stays `Available` from the recorded URL),
    /// so URL presence alone must not be read as a usable tab.
    #[serde(default)]
    pub(crate) connected: bool,
}

/// Describes how much active-tab URL context the daemon could resolve.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BrowserCurrentTabStatus {
    NoActiveTab,
    EmptyUrl,
    AboutBlank,
    Available,
}

#[derive(Default)]
pub(crate) struct BrowserTabRegistry {
    sessions: HashMap<String, BrowserTabSet>,
}

#[derive(Default)]
struct BrowserTabSet {
    active_tab_id: Option<String>,
    next_index: u64,
    tabs: Vec<BrowserTabInfo>,
}

impl BrowserTabRegistry {
    /// Returns the next stable tab id for an agent session without inserting it.
    pub(crate) fn next_tab_id(&mut self, root_session_id: &str) -> String {
        self.sessions
            .entry(root_session_id.to_string())
            .or_default()
            .next_tab_id()
    }

    pub(crate) fn list(&self, root_session_id: &str) -> BrowserTabsState {
        self.sessions
            .get(root_session_id)
            .map(BrowserTabSet::state)
            .unwrap_or_default()
    }

    /// Returns one recorded tab by id for the given root browser session.
    pub(crate) fn tab(&self, root_session_id: &str, tab_id: &str) -> Option<BrowserTabInfo> {
        self.sessions
            .get(root_session_id)?
            .tabs
            .iter()
            .find(|tab| tab.tab_id == tab_id)
            .cloned()
    }

    /// Returns the recorded active tab for the given root browser session.
    pub(crate) fn active_tab(&self, root_session_id: &str) -> Option<BrowserTabInfo> {
        let set = self.sessions.get(root_session_id)?;
        let active_tab_id = set.active_tab_id.as_deref()?;
        set.tabs
            .iter()
            .find(|tab| tab.tab_id == active_tab_id)
            .cloned()
    }

    pub(crate) fn open_tab(
        &mut self,
        root_session_id: &str,
        requested_tab_id: Option<String>,
        label: Option<String>,
        backend_session_id: String,
        native_cef_session_id: Option<String>,
        state: BrowserState,
        activate: bool,
    ) -> BrowserTabInfo {
        let set = self
            .sessions
            .entry(root_session_id.to_string())
            .or_default();
        let tab_id = requested_tab_id.unwrap_or_else(|| set.next_tab_id());
        let now = now_ms();
        let label = label.filter(|value| !value.trim().is_empty());
        let active = activate || set.active_tab_id.is_none();
        if active {
            set.active_tab_id = Some(tab_id.clone());
        }
        if let Some(existing) = set.tabs.iter_mut().find(|tab| tab.tab_id == tab_id) {
            if let Some(label) = label {
                existing.label = label;
            }
            existing.url = state.url;
            existing.title = state.title;
            existing.loading = state.loading;
            existing.connected = true;
            existing.active = active;
            existing.backend_session_id = backend_session_id;
            existing.native_cef_session_id = native_cef_session_id;
            existing.updated_at_ms = now;
        } else {
            set.tabs.push(BrowserTabInfo {
                tab_id: tab_id.clone(),
                label: label.unwrap_or_else(|| default_label(&tab_id)),
                url: state.url,
                title: state.title,
                loading: state.loading,
                connected: true,
                active,
                backend_session_id,
                native_cef_session_id,
                created_at_ms: now,
                updated_at_ms: now,
            });
        }
        set.refresh_active_flags();
        set.tabs
            .iter()
            .find(|tab| tab.tab_id == tab_id)
            .cloned()
            .unwrap_or_else(|| BrowserTabInfo::blank(root_session_id, &tab_id))
    }

    pub(crate) fn focus_tab(
        &mut self,
        root_session_id: &str,
        tab_id: &str,
    ) -> Option<BrowserTabInfo> {
        let set = self.sessions.get_mut(root_session_id)?;
        set.active_tab_id = Some(tab_id.to_string());
        set.refresh_active_flags();
        set.tabs.iter().find(|tab| tab.tab_id == tab_id).cloned()
    }

    pub(crate) fn close_tab(
        &mut self,
        root_session_id: &str,
        tab_id: &str,
    ) -> Option<BrowserTabInfo> {
        let set = self.sessions.get_mut(root_session_id)?;
        let index = set.tabs.iter().position(|tab| tab.tab_id == tab_id)?;
        let closed = set.tabs.remove(index);
        if set.active_tab_id.as_deref() == Some(tab_id) {
            set.active_tab_id = set
                .tabs
                .get(index.saturating_sub(1))
                .or_else(|| set.tabs.first())
                .map(|tab| tab.tab_id.clone());
        }
        set.refresh_active_flags();
        Some(closed)
    }

    pub(crate) fn record_opened_backend(
        &mut self,
        root_session_id: &str,
        tab_id: &str,
        backend_session_id: String,
        native_cef_session_id: Option<String>,
        state: BrowserState,
    ) {
        let set = self
            .sessions
            .entry(root_session_id.to_string())
            .or_default();
        let now = now_ms();
        if set.active_tab_id.is_none() {
            set.active_tab_id = Some(tab_id.to_string());
        }
        if let Some(existing) = set.tabs.iter_mut().find(|tab| tab.tab_id == tab_id) {
            existing.url = state.url;
            existing.title = state.title;
            existing.loading = state.loading;
            existing.connected = true;
            existing.backend_session_id = backend_session_id;
            existing.native_cef_session_id = native_cef_session_id;
            existing.updated_at_ms = now;
        } else {
            set.tabs.push(BrowserTabInfo {
                tab_id: tab_id.to_string(),
                label: default_label(tab_id),
                url: state.url,
                title: state.title,
                loading: state.loading,
                connected: true,
                active: false,
                backend_session_id,
                native_cef_session_id,
                created_at_ms: now,
                updated_at_ms: now,
            });
        }
        set.refresh_active_flags();
    }

    pub(crate) fn remove_backend(&mut self, root_session_id: &str, tab_id: &str) {
        let Some(set) = self.sessions.get_mut(root_session_id) else {
            return;
        };
        if let Some(tab) = set.tabs.iter_mut().find(|tab| tab.tab_id == tab_id) {
            tab.connected = false;
            tab.loading = false;
            tab.updated_at_ms = now_ms();
        }
    }

    /// Removes all tab metadata for one root browser session.
    pub(crate) fn remove_root(&mut self, root_session_id: &str) {
        self.sessions.remove(root_session_id);
    }
}

impl BrowserTabSet {
    fn state(&self) -> BrowserTabsState {
        BrowserTabsState {
            active_tab_id: self.active_tab_id.clone(),
            tabs: self.tabs.clone(),
        }
    }

    fn next_tab_id(&mut self) -> String {
        self.next_index = self.next_index.max(1);
        loop {
            let candidate = format!("t{}", self.next_index);
            self.next_index += 1;
            if !self.tabs.iter().any(|tab| tab.tab_id == candidate) {
                return candidate;
            }
        }
    }

    fn refresh_active_flags(&mut self) {
        for tab in &mut self.tabs {
            tab.active = self.active_tab_id.as_deref() == Some(tab.tab_id.as_str());
        }
    }
}

impl BrowserTabInfo {
    fn blank(root_session_id: &str, tab_id: &str) -> Self {
        let now = now_ms();
        Self {
            tab_id: tab_id.to_string(),
            label: default_label(tab_id),
            url: "about:blank".to_string(),
            title: String::new(),
            loading: false,
            connected: false,
            active: false,
            backend_session_id: backend_session_id(root_session_id, tab_id),
            native_cef_session_id: None,
            created_at_ms: now,
            updated_at_ms: now,
        }
    }
}

impl BrowserCurrentTabContext {
    /// Builds a no-active-tab snapshot for browser permission lookups.
    pub(crate) fn no_active_tab() -> Self {
        Self {
            status: BrowserCurrentTabStatus::NoActiveTab,
            tab_id: None,
            url: None,
            origin: None,
            host: None,
            port: None,
            title: None,
            connected: false,
        }
    }

    /// Builds an active-tab snapshot from one managed browser tab record.
    pub(crate) fn from_tab(tab: &BrowserTabInfo) -> Self {
        let url = Some(tab.url.clone());
        let title = Some(tab.title.clone());
        if tab.url.trim().is_empty() {
            return Self {
                status: BrowserCurrentTabStatus::EmptyUrl,
                tab_id: Some(tab.tab_id.clone()),
                url,
                origin: None,
                host: None,
                port: None,
                title,
                connected: tab.connected,
            };
        }
        if tab.url.eq_ignore_ascii_case("about:blank") {
            return Self {
                status: BrowserCurrentTabStatus::AboutBlank,
                tab_id: Some(tab.tab_id.clone()),
                url,
                origin: None,
                host: None,
                port: None,
                title,
                connected: tab.connected,
            };
        }
        let (origin, host, port) = parse_tab_url_fields(&tab.url);
        Self {
            status: BrowserCurrentTabStatus::Available,
            tab_id: Some(tab.tab_id.clone()),
            url,
            origin,
            host,
            port,
            title,
            connected: tab.connected,
        }
    }

    /// Whether the registry currently tracks an active tab at all.
    pub(crate) fn has_active_tab(&self) -> bool {
        !matches!(self.status, BrowserCurrentTabStatus::NoActiveTab)
    }

    /// One-line agent-facing summary of the live tab state, injected into the
    /// per-turn system reminder (issue #560). The no-tab and disconnected
    /// variants explicitly call earlier transcript output stale — that is the
    /// signal the model is missing after a daemon restart.
    pub(crate) fn agent_status_line(&self) -> String {
        if !self.has_active_tab() {
            return "No browser tab is currently open in this session. Any browser state \
                    reported earlier in the conversation is stale — that browser is gone. \
                    Open the page again before relying on it."
                .to_string();
        }
        let url = self.url.as_deref().unwrap_or("unknown URL");
        if !self.connected {
            return format!(
                "The active browser tab ({url}) is disconnected — the underlying browser \
                 is gone, so earlier browser output is stale. Open the page again before \
                 relying on it."
            );
        }
        match self.status {
            BrowserCurrentTabStatus::EmptyUrl | BrowserCurrentTabStatus::AboutBlank => {
                "A browser tab is open but no page is loaded yet.".to_string()
            }
            _ => match self.title.as_deref().filter(|title| !title.trim().is_empty()) {
                Some(title) => format!("Active browser tab: {url} ({title})"),
                None => format!("Active browser tab: {url}"),
            },
        }
    }
}

pub(crate) fn backend_session_id(root_session_id: &str, tab_id: &str) -> String {
    format!("{root_session_id}:browser:{tab_id}")
}

pub(crate) fn parse_backend_session_id(session_id: &str) -> Option<(&str, &str)> {
    let (root, tab_id) = session_id.split_once(":browser:")?;
    (!root.is_empty() && !tab_id.is_empty()).then_some((root, tab_id))
}

fn parse_tab_url_fields(raw_url: &str) -> (Option<String>, Option<String>, Option<u16>) {
    let Ok(parsed) = Url::parse(raw_url) else {
        return (None, None, None);
    };
    let origin =
        matches!(parsed.scheme(), "http" | "https").then(|| parsed.origin().ascii_serialization());
    let host = parsed.host_str().map(|value| value.to_ascii_lowercase());
    let port = parsed.port_or_known_default();
    (origin, host, port)
}

fn default_label(tab_id: &str) -> String {
    if let Some(index) = tab_id.strip_prefix('t') {
        format!("Tab {index}")
    } else {
        "New tab".to_string()
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(url: &str, title: &str, connected: bool) -> BrowserTabInfo {
        BrowserTabInfo {
            tab_id: "t1".to_string(),
            label: "Tab 1".to_string(),
            url: url.to_string(),
            title: title.to_string(),
            loading: false,
            connected,
            active: true,
            backend_session_id: "sess:browser:t1".to_string(),
            native_cef_session_id: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        }
    }

    #[test]
    fn no_active_tab_status_line_marks_earlier_browser_state_stale() {
        let context = BrowserCurrentTabContext::no_active_tab();
        let line = context.agent_status_line();
        assert!(line.contains("No browser tab"), "line: {line}");
        assert!(line.contains("stale"), "line: {line}");
    }

    #[test]
    fn available_tab_status_line_reports_url_and_title() {
        let context =
            BrowserCurrentTabContext::from_tab(&tab("https://example.com/checkout", "Checkout", true));
        assert!(context.connected);
        let line = context.agent_status_line();
        assert!(line.contains("https://example.com/checkout"), "line: {line}");
        assert!(line.contains("Checkout"), "line: {line}");
        assert!(!line.contains("stale"), "line: {line}");
    }

    #[test]
    fn disconnected_tab_status_line_marks_browser_gone() {
        let context =
            BrowserCurrentTabContext::from_tab(&tab("https://example.com", "Example", false));
        assert!(!context.connected);
        let line = context.agent_status_line();
        assert!(line.contains("disconnected"), "line: {line}");
        assert!(line.contains("stale"), "line: {line}");
    }

    #[test]
    fn blank_tab_status_line_reports_no_page_loaded() {
        let context = BrowserCurrentTabContext::from_tab(&tab("about:blank", "", true));
        let line = context.agent_status_line();
        assert!(line.contains("no page"), "line: {line}");
    }
}

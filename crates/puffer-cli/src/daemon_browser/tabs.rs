//! Session-scoped browser tab metadata for the desktop and agent APIs.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

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

    pub(crate) fn open_tab(
        &mut self,
        root_session_id: &str,
        requested_tab_id: Option<String>,
        label: Option<String>,
        backend_session_id: String,
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
            created_at_ms: now,
            updated_at_ms: now,
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

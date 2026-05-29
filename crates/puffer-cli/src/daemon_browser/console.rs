//! Console log buffering for agent-readable browser diagnostics.

use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_CONSOLE_ENTRIES_PER_SESSION: usize = 500;

/// Stores recent console events per browser backend session.
#[derive(Default)]
pub(crate) struct BrowserConsoleRegistry {
    entries: HashMap<String, VecDeque<Value>>,
}

impl BrowserConsoleRegistry {
    /// Records one DevTools console payload for a backend browser session.
    pub(crate) fn record(&mut self, session_id: &str, payload: &Value) {
        if payload.get("kind").and_then(Value::as_str) != Some("console") {
            return;
        }
        let mut entry = payload.clone();
        if let Some(object) = entry.as_object_mut() {
            object.insert("recordedAtMs".to_string(), json!(now_ms()));
        }
        let values = self.entries.entry(session_id.to_string()).or_default();
        values.push_back(entry);
        while values.len() > MAX_CONSOLE_ENTRIES_PER_SESSION {
            values.pop_front();
        }
    }

    /// Reads buffered console entries for one backend browser session.
    pub(crate) fn read(&mut self, session_id: &str, clear: bool) -> Vec<Value> {
        if clear {
            return self
                .entries
                .remove(session_id)
                .map(VecDeque::into_iter)
                .map(Iterator::collect)
                .unwrap_or_default();
        }
        self.entries
            .get(session_id)
            .map(|values| values.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Removes all buffered console entries for one backend browser session.
    pub(crate) fn remove(&mut self, session_id: &str) {
        self.entries.remove(session_id);
    }

    /// Removes all buffered console entries below one root browser session.
    pub(crate) fn remove_root(&mut self, root_session_id: &str) {
        self.entries
            .retain(|session_id, _| super::session_root_id(session_id) != root_session_id);
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

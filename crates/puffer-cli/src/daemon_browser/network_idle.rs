//! CDP network request tracking for BrowserAction network-idle waits.

use serde_json::Value;
use std::collections::HashSet;
use std::time::{Duration, Instant};

/// Tracks active CDP network requests for one browser page worker.
#[derive(Debug)]
pub(super) struct BrowserNetworkState {
    active_request_ids: HashSet<String>,
    last_activity: Instant,
}

impl Default for BrowserNetworkState {
    fn default() -> Self {
        Self {
            active_request_ids: HashSet::new(),
            last_activity: Instant::now(),
        }
    }
}

impl BrowserNetworkState {
    /// Applies one CDP network event to the active request set.
    pub(super) fn update_from_cdp(&mut self, method: &str, value: &Value) {
        let Some(request_id) = value
            .pointer("/params/requestId")
            .and_then(Value::as_str)
            .map(ToString::to_string)
        else {
            return;
        };
        match method {
            "Network.requestWillBeSent" => {
                self.active_request_ids.insert(request_id);
                self.last_activity = Instant::now();
            }
            "Network.loadingFinished" | "Network.loadingFailed" => {
                self.active_request_ids.remove(&request_id);
                self.last_activity = Instant::now();
            }
            _ => {}
        }
    }

    /// Returns true when no tracked request has been active for `idle`.
    pub(super) fn is_idle_for(&self, idle: Duration) -> bool {
        self.active_request_ids.is_empty() && self.last_activity.elapsed() >= idle
    }

    /// Returns the number of currently tracked active requests.
    pub(super) fn active_count(&self) -> usize {
        self.active_request_ids.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tracks_active_request_ids() {
        let mut network = BrowserNetworkState::default();

        network.update_from_cdp(
            "Network.requestWillBeSent",
            &json!({ "params": { "requestId": "r1" } }),
        );
        assert_eq!(network.active_count(), 1);

        network.update_from_cdp(
            "Network.loadingFinished",
            &json!({ "params": { "requestId": "r1" } }),
        );
        assert_eq!(network.active_count(), 0);
    }

    #[test]
    fn active_requests_are_not_idle() {
        let mut network = BrowserNetworkState::default();
        network.update_from_cdp(
            "Network.requestWillBeSent",
            &json!({ "params": { "requestId": "r1" } }),
        );

        assert!(!network.is_idle_for(Duration::from_millis(0)));
    }
}

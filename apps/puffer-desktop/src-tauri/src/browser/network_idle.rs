//! Network-idle tracking for agent browser actions.

use serde_json::Value;
use std::collections::HashSet;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

/// Tracks in-flight Chrome DevTools Protocol network requests.
#[derive(Default)]
pub(super) struct NetworkIdleTracker {
    active_requests: HashSet<String>,
    last_activity: Option<Instant>,
}

impl NetworkIdleTracker {
    /// Records one Chrome DevTools Protocol network event.
    pub(super) fn record_event(&mut self, method: &str, value: &Value) {
        let Some(request_id) = value
            .pointer("/params/requestId")
            .and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty())
        else {
            return;
        };
        match method {
            "Network.requestWillBeSent" => {
                self.active_requests.insert(request_id.to_string());
                self.last_activity = Some(Instant::now());
            }
            "Network.responseReceived" => {
                self.last_activity = Some(Instant::now());
            }
            "Network.loadingFinished" | "Network.loadingFailed" => {
                self.active_requests.remove(request_id);
                self.last_activity = Some(Instant::now());
            }
            _ => {}
        }
    }

    fn is_idle_for(&self, created_at: Instant, idle_for: Duration) -> bool {
        if !self.active_requests.is_empty() {
            return false;
        }
        let anchor = self
            .last_activity
            .filter(|activity| *activity > created_at)
            .unwrap_or(created_at);
        anchor.elapsed() >= idle_for
    }
}

/// One pending request waiting for network-idle state.
pub(super) struct NetworkIdleWaiter {
    created_at: Instant,
    idle_for: Duration,
    deadline: Instant,
    reply: Sender<std::result::Result<(), String>>,
}

impl NetworkIdleWaiter {
    /// Creates a waiter that completes after `idle_for` or fails at `deadline`.
    pub(super) fn new(
        idle_for: Duration,
        timeout: Duration,
        reply: Sender<std::result::Result<(), String>>,
    ) -> Self {
        let created_at = Instant::now();
        Self {
            created_at,
            idle_for,
            deadline: created_at + timeout,
            reply,
        }
    }
}

/// Completes or expires pending network-idle waiters.
pub(super) fn drain_network_idle_waiters(
    waiters: &mut Vec<NetworkIdleWaiter>,
    tracker: &NetworkIdleTracker,
) {
    let now = Instant::now();
    let mut pending = Vec::new();
    for waiter in waiters.drain(..) {
        if tracker.is_idle_for(waiter.created_at, waiter.idle_for) {
            let _ = waiter.reply.send(Ok(()));
        } else if now >= waiter.deadline {
            let _ = waiter
                .reply
                .send(Err("timed out waiting for browser network idle".to_string()));
        } else {
            pending.push(waiter);
        }
    }
    *waiters = pending;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn idle_waiter_completes_when_no_requests_are_active() {
        let tracker = NetworkIdleTracker::default();
        let (reply, rx) = mpsc::channel();
        let mut waiters = vec![NetworkIdleWaiter::new(
            Duration::ZERO,
            Duration::from_secs(1),
            reply,
        )];

        drain_network_idle_waiters(&mut waiters, &tracker);

        assert!(waiters.is_empty());
        assert_eq!(rx.try_recv().unwrap(), Ok(()));
    }

    #[test]
    fn active_request_blocks_until_terminal_event() {
        let mut tracker = NetworkIdleTracker::default();
        tracker.record_event(
            "Network.requestWillBeSent",
            &serde_json::json!({ "params": { "requestId": "r1" } }),
        );
        let (reply, rx) = mpsc::channel();
        let mut waiters = vec![NetworkIdleWaiter::new(
            Duration::ZERO,
            Duration::from_secs(1),
            reply,
        )];

        drain_network_idle_waiters(&mut waiters, &tracker);

        assert_eq!(waiters.len(), 1);
        assert!(matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)));

        tracker.record_event(
            "Network.loadingFinished",
            &serde_json::json!({ "params": { "requestId": "r1" } }),
        );
        drain_network_idle_waiters(&mut waiters, &tracker);

        assert!(waiters.is_empty());
        assert_eq!(rx.try_recv().unwrap(), Ok(()));
    }

    #[test]
    fn active_request_times_out() {
        let mut tracker = NetworkIdleTracker::default();
        tracker.record_event(
            "Network.requestWillBeSent",
            &serde_json::json!({ "params": { "requestId": "r1" } }),
        );
        let (reply, rx) = mpsc::channel();
        let mut waiters = vec![NetworkIdleWaiter::new(
            Duration::ZERO,
            Duration::ZERO,
            reply,
        )];

        drain_network_idle_waiters(&mut waiters, &tracker);

        assert!(waiters.is_empty());
        assert!(rx.try_recv().unwrap().is_err());
    }
}

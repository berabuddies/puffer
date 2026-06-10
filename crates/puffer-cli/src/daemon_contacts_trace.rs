//! Contact inference trace event helpers for the desktop daemon.

use crate::daemon::{DaemonState, ServerEnvelope};
use serde_json::{json, Value};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

/// Emits per-run contact inference events to the desktop daemon event bus.
pub(crate) struct ContactInferTrace<'a> {
    state: &'a DaemonState,
    channel: Option<String>,
}

impl<'a> ContactInferTrace<'a> {
    /// Creates a trace emitter for `trace_id`, or a no-op emitter when absent.
    pub(crate) fn new(state: &'a DaemonState, trace_id: Option<&str>) -> Self {
        let channel = trace_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| format!("contacts:infer:{value}:event"));
        Self { state, channel }
    }

    /// Emits a conversation-style message event.
    pub(crate) fn message(&self, role: &str, title: &str, body: impl Into<String>) {
        self.emit(json!({
            "type": "message",
            "id": Uuid::new_v4().to_string(),
            "role": role,
            "title": title,
            "body": body.into(),
            "createdAtMs": now_ms(),
        }));
    }

    /// Returns a stable tool event id for a single logical trace operation.
    pub(crate) fn tool_id(&self, tool_name: &str) -> String {
        format!("{tool_name}-{}", Uuid::new_v4())
    }

    /// Emits or updates a conversation-style tool event.
    pub(crate) fn tool_event(
        &self,
        id: &str,
        tool_name: &str,
        status: &str,
        summary: &str,
        input: Value,
        output: Value,
    ) {
        self.emit(json!({
            "type": "tool",
            "id": id,
            "toolName": tool_name,
            "status": status,
            "title": format!("Tool call: {tool_name}"),
            "summary": summary,
            "input": input,
            "output": output,
            "createdAtMs": now_ms(),
        }));
    }

    fn emit(&self, payload: Value) {
        let Some(channel) = &self.channel else {
            return;
        };
        self.state.publish_event(ServerEnvelope::Event {
            event: channel.clone(),
            payload,
        });
    }
}

fn now_ms() -> i128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i128)
        .unwrap_or_default()
}

//! JSON snapshot helpers for workflow daemon responses.

use puffer_subscriptions::ConnectionRecord;
use serde_json::{json, Value};

/// Returns the Desktop-facing JSON for one connector connection.
pub(super) fn connection_snapshot_json(
    connection: ConnectionRecord,
    can_trigger_workflow: bool,
    monitor_rule_schema: Option<Value>,
) -> Value {
    let monitor_command = can_trigger_workflow.then(|| format!("/monitor {}", connection.slug));
    let connect_command = format!("/connect {} {}", connection.connector_slug, connection.slug);
    let health = connection
        .health
        .as_ref()
        .and_then(|health| serde_json::to_value(health).ok())
        .unwrap_or(Value::Null);
    json!({
        "slug": connection.slug,
        "connector_slug": connection.connector_slug,
        "description": connection.description,
        "state": connection.state,
        "has_consumer": connection.has_consumer,
        "auth_failure_notified": connection.auth_failure_notified,
        "health": health,
        "can_trigger_workflow": can_trigger_workflow,
        "monitor_rule_schema": monitor_rule_schema,
        "connect_command": connect_command,
        "monitor_command": monitor_command,
    })
}

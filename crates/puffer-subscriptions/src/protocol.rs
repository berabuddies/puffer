use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Host-to-connector stdin command for subscription streams.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ConnectorSubscribeCommand {
    /// Start streaming from a connection at an optional cursor.
    Subscribe {
        /// Authorized connection slug.
        connection: String,
        /// Last acknowledged cursor, if any.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cursor: Option<String>,
    },
    /// Acknowledge that the host has durably handled one event.
    Ack {
        /// Cursor returned by the connector.
        cursor: String,
        /// Connector event id being acknowledged.
        event_id: String,
    },
}

/// Connector-to-host stdout frame for JSONL subscription streams.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConnectorSubscribeFrame {
    /// One event produced by the connector.
    Event {
        /// Stable connector event id.
        id: String,
        /// Cursor that should be acked after durable handling.
        cursor: String,
        /// Connector-defined event payload.
        payload: Value,
    },
    /// Cursor checkpoint that does not carry an event.
    Checkpoint {
        /// Cursor that may be persisted by the host.
        cursor: String,
    },
    /// Health status frame.
    Health {
        /// Status string such as `ok`, `degraded`, or `error`.
        status: String,
        /// Optional detail safe for logs.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
}

/// Host-to-connector action request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConnectorActionRequest {
    /// Authorized connection slug.
    pub connection: String,
    /// Connector action slug, for example `send_message`.
    pub action: String,
    /// Action input JSON.
    #[serde(default)]
    pub input: Value,
    /// Optional idempotency key supplied by the workflow runtime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
}

/// Connector action response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConnectorActionResponse {
    /// Whether the connector accepted or completed the action.
    pub success: bool,
    /// Human-readable summary.
    pub summary: String,
    /// Connector-specific output JSON.
    #[serde(default)]
    pub output: Value,
    /// Whether retrying the same request may succeed.
    #[serde(default)]
    pub retryable: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn subscribe_frames_are_jsonl_friendly() {
        let frame = ConnectorSubscribeFrame::Event {
            id: "e1".into(),
            cursor: "c1".into(),
            payload: json!({"message":"gm"}),
        };
        let encoded = serde_json::to_string(&frame).unwrap();

        assert!(!encoded.contains('\n'));
        assert_eq!(
            serde_json::from_str::<ConnectorSubscribeFrame>(&encoded).unwrap(),
            frame
        );
    }

    #[test]
    fn subscribe_command_shape_matches_spec() {
        let command = ConnectorSubscribeCommand::Subscribe {
            connection: "my-telegram".into(),
            cursor: Some("42".into()),
        };

        assert_eq!(
            serde_json::to_value(command).unwrap(),
            json!({"op":"subscribe","connection":"my-telegram","cursor":"42"})
        );
    }
}

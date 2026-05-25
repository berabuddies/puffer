use serde_json::Value;

/// Returns the standard connector action result schema.
pub(super) fn action_output_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "completed": {"type": "boolean"},
            "summary": {"type": "string"}
        },
        "required": ["completed", "summary"]
    })
}

/// Returns the Slack message action input schema.
pub(super) fn slack_message_action_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": slack_common_properties(),
        "additionalProperties": true
    })
}

fn slack_common_properties() -> Value {
    serde_json::json!({
        "to": {"type": "string"},
        "target": {"type": "string"},
        "channel": {"type": "string"},
        "user": {"type": "string"},
        "message": {"type": "string"},
        "text": {"type": "string"},
        "caption": {"type": "string"},
        "thread_ts": {"type": "string"},
        "reply_to": {"oneOf": [{"type": "string"}, {"type": "object"}]},
        "reply_to_message_id": {"type": "string"},
        "ts": {"type": "string"},
        "timestamp": {"type": "string"},
        "message_ts": {"type": "string"},
        "message_id": {"type": "string"},
        "emoji": {"type": "string"},
        "reaction": {"type": "string"},
        "remove": {"type": "boolean"},
        "path": {"type": "string"},
        "file": {"oneOf": [{"type": "string"}, {"type": "object"}]},
        "media": {"oneOf": [{"type": "string"}, {"type": "object"}, {"type": "array"}]},
        "files": {"type": "array"}
    })
}

/// Returns the Lark message action input schema.
pub(super) fn lark_message_action_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": lark_common_properties(),
        "additionalProperties": true
    })
}

fn lark_common_properties() -> Value {
    serde_json::json!({
        "to": {"type": "string"},
        "target": {"type": "string"},
        "receive_id": {"type": "string"},
        "receive_id_type": {"type": "string"},
        "chat_id": {"type": "string"},
        "chat": {"type": "string"},
        "channel": {"type": "string"},
        "open_id": {"type": "string"},
        "user_id": {"type": "string"},
        "user": {"type": "string"},
        "message": {"type": "string"},
        "text": {"type": "string"},
        "caption": {"type": "string"},
        "content": {"oneOf": [{"type": "string"}, {"type": "object"}]},
        "msg_type": {"type": "string"},
        "message_type": {"type": "string"},
        "message_id": {"type": "string"},
        "id": {"type": "string"},
        "reply_to": {"oneOf": [{"type": "string"}, {"type": "object"}]},
        "reply_to_message_id": {"type": "string"},
        "reply_in_thread": {"type": "boolean"},
        "emoji_type": {"type": "string"},
        "emoji": {"type": "string"},
        "reaction": {"type": "string"},
        "reaction_id": {"type": "string"},
        "remove": {"type": "boolean"},
        "path": {"type": "string"},
        "image": {"oneOf": [{"type": "string"}, {"type": "object"}]},
        "file": {"oneOf": [{"type": "string"}, {"type": "object"}]},
        "media": {"oneOf": [{"type": "string"}, {"type": "object"}, {"type": "array"}]},
        "files": {"type": "array"},
        "idempotency_key": {"type": "string"},
        "uuid": {"type": "string"}
    })
}

/// Returns the Telegram peer action input schema.
pub(super) fn telegram_peer_action_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": telegram_common_properties(),
        "additionalProperties": true
    })
}

/// Returns the Telegram message action input schema.
pub(super) fn telegram_message_action_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": telegram_common_properties(),
        "additionalProperties": true
    })
}

/// Returns the Telegram group action input schema.
pub(super) fn telegram_group_action_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": telegram_common_properties(),
        "additionalProperties": true
    })
}

/// Returns the Telegram membership action input schema.
pub(super) fn telegram_membership_action_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": telegram_common_properties(),
        "additionalProperties": true
    })
}

/// Returns the Telegram profile action input schema.
pub(super) fn telegram_profile_action_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": telegram_common_properties(),
        "additionalProperties": true
    })
}

/// Returns the Telegram media action input schema.
pub(super) fn telegram_media_action_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": telegram_common_properties(),
        "additionalProperties": true
    })
}

/// Returns the Telegram poll vote action input schema.
pub(super) fn telegram_poll_vote_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": telegram_common_properties(),
        "required": ["to"],
        "additionalProperties": true
    })
}

fn telegram_common_properties() -> Value {
    serde_json::json!({
        "to": {"type": "string"},
        "target": {"type": "string"},
        "channel": {"type": "string"},
        "chat": {"type": "string"},
        "peer": {"type": "string"},
        "from": {"type": "string"},
        "source": {"type": "string"},
        "message": {"type": "string"},
        "text": {"type": "string"},
        "caption": {"type": "string"},
        "message_id": {"oneOf": [{"type": "integer"}, {"type": "string"}]},
        "id": {"oneOf": [{"type": "integer"}, {"type": "string"}]},
        "message_ids": {"type": "array"},
        "reply_to": {"oneOf": [{"type": "integer"}, {"type": "string"}, {"type": "object"}]},
        "emoji": {"type": "string"},
        "reaction": {"type": "string"},
        "user": {"type": "string"},
        "users": {"type": "array"},
        "title": {"type": "string"},
        "name": {"type": "string"},
        "username": {"type": "string"},
        "handle": {"type": "string"},
        "path": {"type": "string"},
        "file": {"type": "string"},
        "media": {"oneOf": [{"type": "string"}, {"type": "object"}, {"type": "array"}]},
        "option": {"oneOf": [{"type": "integer"}, {"type": "string"}, {"type": "object"}]},
        "answer": {"oneOf": [{"type": "integer"}, {"type": "string"}, {"type": "object"}]},
        "answer_index": {"type": "integer"},
        "option_hex": {"type": "string"},
        "options": {"type": "array"}
    })
}

/// Returns the common message event output schema.
pub(super) fn message_output_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "message": {"type": "string"},
            "from": {"type": "string"},
            "assets": {"type": "array"},
            "thread": {"type": "string"},
            "reply_to": {"type": "object"},
            "reply_count": {"type": "integer"},
            "media": {"type": ["string", "null"]},
            "poll": {"type": ["object", "null"]}
        },
        "required": ["message"]
    })
}

/// Returns the Asana webhook event output schema.
pub(super) fn asana_event_output_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "kind": {"type": "string", "const": "asana_event"},
            "action": {"type": "string"},
            "resource_type": {"type": "string"},
            "resource_gid": {"type": "string"},
            "parent_gid": {"type": "string"},
            "actor": {"type": "string"},
            "message": {"type": "string"}
        },
        "required": ["action", "message"],
        "additionalProperties": true
    })
}

/// Returns the GitHub webhook event output schema.
pub(super) fn github_event_output_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "kind": {"type": "string", "const": "github_event"},
            "event": {"type": "string"},
            "action": {"type": "string"},
            "repository": {"type": "string"},
            "sender": {"type": "string"},
            "message": {"type": "string"},
            "url": {"type": "string"}
        },
        "required": ["event", "repository", "message"],
        "additionalProperties": true
    })
}

/// Returns the GitLab webhook event output schema.
pub(super) fn gitlab_event_output_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "kind": {"type": "string", "const": "gitlab_event"},
            "event": {"type": "string"},
            "object_kind": {"type": "string"},
            "action": {"type": "string"},
            "project": {"type": "string"},
            "sender": {"type": "string"},
            "message": {"type": "string"},
            "url": {"type": "string"}
        },
        "required": ["event", "project", "message"],
        "additionalProperties": true
    })
}

/// Returns the Jira webhook event output schema.
pub(super) fn jira_event_output_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "kind": {"type": "string", "const": "jira_event"},
            "event": {"type": "string"},
            "issue_event_type_name": {"type": "string"},
            "issue_key": {"type": "string"},
            "project": {"type": "string"},
            "actor": {"type": "string"},
            "message": {"type": "string"},
            "url": {"type": "string"}
        },
        "required": ["event", "message"],
        "additionalProperties": true
    })
}

/// Returns the Linear webhook event output schema.
pub(super) fn linear_event_output_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "kind": {"type": "string", "const": "linear_event"},
            "type": {"type": "string"},
            "action": {"type": "string"},
            "actor": {"type": "string"},
            "message": {"type": "string"},
            "url": {"type": "string"}
        },
        "required": ["type", "action", "message"],
        "additionalProperties": true
    })
}

/// Returns the Stripe webhook event output schema.
pub(super) fn stripe_event_output_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "kind": {"type": "string", "const": "stripe_event"},
            "type": {"type": "string"},
            "object_type": {"type": "string"},
            "object_id": {"type": "string"},
            "account": {"type": "string"},
            "message": {"type": "string"},
            "url": {"type": "string"}
        },
        "required": ["type", "message"],
        "additionalProperties": true
    })
}

/// Returns the Trello webhook event output schema.
pub(super) fn trello_event_output_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "kind": {"type": "string", "const": "trello_event"},
            "action_type": {"type": "string"},
            "board": {"type": "string"},
            "card": {"type": "string"},
            "list": {"type": "string"},
            "actor": {"type": "string"},
            "message": {"type": "string"}
        },
        "required": ["action_type", "message"],
        "additionalProperties": true
    })
}

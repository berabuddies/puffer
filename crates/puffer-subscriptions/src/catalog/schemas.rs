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

/// Returns the Email and Gmail-browser action input schema.
pub(super) fn email_action_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "connection_slug": {"type": "string"},
            "account_slug": {"type": "string"},
            "connection": {"type": "string"},
            "account": {"type": "string"},
            "mailbox": {
                "description": "IMAP mailbox or Gmail category/label target; defaults to INBOX.",
                "type": "string"
            },
            "category": {
                "description": "Gmail category or mailbox shortcut such as inbox, primary, promotions, social, updates, forums, sent, drafts, spam, or trash.",
                "type": "string"
            },
            "label": {"type": "string"},
            "query": {"type": "string"},
            "keywords": {
                "oneOf": [
                    {"type": "string"},
                    {"type": "array", "items": {"type": "string"}}
                ]
            },
            "from": {"type": "string"},
            "to": {
                "oneOf": [
                    {"type": "string"},
                    {"type": "array", "items": {"type": "string"}}
                ]
            },
            "cc": {
                "oneOf": [
                    {"type": "string"},
                    {"type": "array", "items": {"type": "string"}}
                ]
            },
            "bcc": {
                "oneOf": [
                    {"type": "string"},
                    {"type": "array", "items": {"type": "string"}}
                ]
            },
            "subject": {"type": "string"},
            "body": {"type": "string"},
            "text": {"type": "string"},
            "message": {"type": "string"},
            "uid": {
                "oneOf": [
                    {"type": "integer"},
                    {"type": "string"},
                    {"type": "array"}
                ]
            },
            "uids": {"type": "array"},
            "message_id": {"type": "string"},
            "thread_id": {"type": "string"},
            "gmail_thread_id": {"type": "string"},
            "id": {
                "oneOf": [
                    {"type": "integer"},
                    {"type": "string"}
                ]
            },
            "unread": {"type": "boolean"},
            "limit": {"type": "integer", "minimum": 1, "maximum": 100},
            "scan_limit": {"type": "integer", "minimum": 1, "maximum": 1000},
            "expunge": {"type": "boolean"},
            "in_reply_to": {"type": "string"},
            "references": {
                "oneOf": [
                    {"type": "string"},
                    {"type": "array", "items": {"type": "string"}}
                ]
            }
        },
        "additionalProperties": true
    })
}

/// Returns the Google Calendar browser action input schema.
pub(super) fn calendar_action_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "connection_slug": {"type": "string"},
            "account_slug": {"type": "string"},
            "connection": {"type": "string"},
            "account": {"type": "string"},
            "email": {"type": "string"},
            "event_id": {"type": "string"},
            "calendar_event_id": {"type": "string"},
            "id": {"type": "string"},
            "title": {"type": "string"},
            "summary": {"type": "string"},
            "event_title": {"type": "string"},
            "url": {"type": "string"},
            "event_url": {"type": "string"}
        },
        "additionalProperties": true
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

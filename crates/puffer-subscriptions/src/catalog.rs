use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Stable slug for a connector template, for example `telegram-login`.
pub type ConnectorSlug = String;

/// One action exposed by a connector binary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConnectorActionDefinition {
    /// Stable action slug, for example `send_message`.
    pub slug: String,
    /// Human-readable description shown in list and permission surfaces.
    #[serde(default)]
    pub description: String,
    /// JSON Schema accepted by the connector action.
    #[serde(default)]
    pub input_schema: Value,
    /// JSON Schema returned by the connector action.
    #[serde(default)]
    pub output_schema: Value,
    /// Permission contract the host enforces before calling this action.
    pub permission: ConnectorPermissionDefinition,
}

/// Permission metadata supplied by a connector template and enforced by Puffer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConnectorPermissionDefinition {
    /// Stable permission category used for grants and audit entries.
    pub category: String,
    /// Short summary template shown to the user.
    pub summary: String,
    /// Whether this action produces an external side effect.
    #[serde(default)]
    pub external_side_effect: bool,
}

/// Subscriber manifest metadata used by connectors whose event stream is
/// provided by a reusable subscriber binary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConnectorSubscriberTemplate {
    /// Subscriber manifest slug to load when a connection has no dedicated
    /// manifest directory of its own.
    pub manifest_slug: String,
    /// Optional user config subdirectory used for per-connection subscriber
    /// state when instantiating a shared manifest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_root: Option<String>,
    /// Optional display name prefix used for instantiated subscribers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

/// Describes one connector implementation independently of any user auth state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConnectorTemplate {
    /// Stable connector slug, for example `telegram-login`.
    pub slug: ConnectorSlug,
    /// Human-readable description.
    pub description: String,
    /// Skill name that teaches agents how to authenticate or operate it.
    pub skill: String,
    /// Human-readable internal binary or tool entrypoint.
    pub binary: String,
    /// Typed argv used for connector protocol operations such as `auth-ok`
    /// and `act`. `cmd[0]` is the program and the rest are fixed args.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command: Vec<String>,
    /// Whether a connection must be authenticated before first use.
    #[serde(default)]
    pub requires_auth: bool,
    /// Whether the connector can produce subscription events.
    #[serde(default)]
    pub can_subscribe: bool,
    /// Whether the connector can act as an agent proxy.
    #[serde(default)]
    pub can_proxy_agent: bool,
    /// Optional reusable subscriber manifest for per-connection event streams.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscriber: Option<ConnectorSubscriberTemplate>,
    /// JSON shape emitted by `subscribe` events.
    #[serde(default)]
    pub output_schema: Value,
    /// Actions supported by `act`.
    #[serde(default)]
    pub actions: BTreeMap<String, ConnectorActionDefinition>,
}

impl ConnectorTemplate {
    /// Returns the typed command used for connector protocol subprocesses.
    pub fn command_argv(&self) -> Option<&[String]> {
        (!self.command.is_empty()).then_some(self.command.as_slice())
    }
}

/// Returns the built-in connector templates required by the workflow spec.
pub fn builtin_connector_templates() -> Vec<ConnectorTemplate> {
    vec![
        telegram_login_template(),
        telegram_bot_template(),
        lark_app_template(),
        lark_login_template(),
        slack_app_template(),
        slack_login_template(),
        slack_bot_template(),
        email_template(),
    ]
}

/// Looks up a built-in connector template by slug.
pub fn builtin_connector_template(slug: &str) -> Option<ConnectorTemplate> {
    builtin_connector_templates()
        .into_iter()
        .find(|template| template.slug == slug)
}

fn telegram_login_template() -> ConnectorTemplate {
    ConnectorTemplate {
        slug: "telegram-login".to_string(),
        description: "Telegram personal account over MTProto".to_string(),
        skill: "telegram".to_string(),
        binary: "puffer internal-tool telegram".to_string(),
        command: Vec::new(),
        requires_auth: true,
        can_subscribe: true,
        can_proxy_agent: false,
        subscriber: Some(ConnectorSubscriberTemplate {
            manifest_slug: "telegram-user".to_string(),
            state_root: Some("telegram-accounts".to_string()),
            display_name: Some("Telegram".to_string()),
        }),
        output_schema: message_output_schema(),
        actions: telegram_login_actions(),
    }
}

fn telegram_bot_template() -> ConnectorTemplate {
    ConnectorTemplate {
        slug: "telegram-bot".to_string(),
        description: "Telegram bot connector for agent proxy and bot chats".to_string(),
        skill: "telegram-bot".to_string(),
        binary: "puffer connector telegram-bot".to_string(),
        command: Vec::new(),
        requires_auth: true,
        can_subscribe: true,
        can_proxy_agent: true,
        subscriber: None,
        output_schema: message_output_schema(),
        actions: send_message_actions(),
    }
}

fn slack_bot_template() -> ConnectorTemplate {
    ConnectorTemplate {
        slug: "slack-bot".to_string(),
        description: "Legacy Slack bot connector placeholder; use slack-app or slack-login actions"
            .to_string(),
        skill: "slack".to_string(),
        binary: "puffer connector slack-bot".to_string(),
        command: Vec::new(),
        requires_auth: true,
        can_subscribe: false,
        can_proxy_agent: false,
        subscriber: None,
        output_schema: message_output_schema(),
        actions: BTreeMap::new(),
    }
}

fn slack_app_template() -> ConnectorTemplate {
    ConnectorTemplate {
        slug: "slack-app".to_string(),
        description: "Slack app connector for bot-token Web API actions".to_string(),
        skill: "slack".to_string(),
        binary: "puffer internal-tool slack".to_string(),
        command: Vec::new(),
        requires_auth: true,
        can_subscribe: false,
        can_proxy_agent: false,
        subscriber: None,
        output_schema: message_output_schema(),
        actions: slack_actions(),
    }
}

fn slack_login_template() -> ConnectorTemplate {
    ConnectorTemplate {
        slug: "slack-login".to_string(),
        description: "Slack workspace account over Web API or local app session".to_string(),
        skill: "slack".to_string(),
        binary: "puffer internal-tool slack".to_string(),
        command: Vec::new(),
        requires_auth: true,
        can_subscribe: false,
        can_proxy_agent: false,
        subscriber: None,
        output_schema: message_output_schema(),
        actions: slack_actions(),
    }
}

fn lark_app_template() -> ConnectorTemplate {
    ConnectorTemplate {
        slug: "lark-app".to_string(),
        description: "Lark custom app connector over OpenAPI".to_string(),
        skill: "lark".to_string(),
        binary: "puffer internal-tool lark".to_string(),
        command: Vec::new(),
        requires_auth: true,
        can_subscribe: false,
        can_proxy_agent: false,
        subscriber: None,
        output_schema: message_output_schema(),
        actions: lark_actions(),
    }
}

fn lark_login_template() -> ConnectorTemplate {
    ConnectorTemplate {
        slug: "lark-login".to_string(),
        description: "Lark user-token account connector over OpenAPI".to_string(),
        skill: "lark".to_string(),
        binary: "puffer internal-tool lark".to_string(),
        command: Vec::new(),
        requires_auth: true,
        can_subscribe: false,
        can_proxy_agent: false,
        subscriber: None,
        output_schema: message_output_schema(),
        actions: lark_actions(),
    }
}

fn email_template() -> ConnectorTemplate {
    ConnectorTemplate {
        slug: "email".to_string(),
        description: "Email connector over SMTP and POP3/IMAP-compatible polling".to_string(),
        skill: "email".to_string(),
        binary: "puffer internal-tool email".to_string(),
        command: Vec::new(),
        requires_auth: true,
        can_subscribe: true,
        can_proxy_agent: false,
        subscriber: None,
        output_schema: message_output_schema(),
        actions: send_message_actions(),
    }
}

fn send_message_actions() -> BTreeMap<String, ConnectorActionDefinition> {
    let send_message = ConnectorActionDefinition {
        slug: "send_message".to_string(),
        description: "Send a message through the connector".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "to": {"type": "string"},
                "target": {"type": "string"},
                "channel": {"type": "string"},
                "chat_id": {"type": "string"},
                "open_id": {"type": "string"},
                "user": {"type": "string"},
                "receive_id": {"type": "string"},
                "receive_id_type": {"type": "string"},
                "message": {"type": "string"},
                "text": {"type": "string"},
                "caption": {
                    "description": "Optional caption used as message text when sending media.",
                    "type": "string"
                },
                "media": {
                    "description": "Optional path/URL, attachment object, or array of attachments to send with captions.",
                    "oneOf": [
                        {"type": "string"},
                        {
                            "type": "object",
                            "properties": {
                                "path": {"type": "string"},
                                "file": {"type": "string"},
                                "url": {"type": "string"},
                                "caption": {"type": "string"},
                                "kind": {
                                    "type": "string",
                                    "enum": ["auto", "photo", "image", "document", "doc", "file", "audio", "voice", "video", "media"]
                                },
                                "mime_type": {"type": "string"},
                                "thumbnail": {"type": "string"}
                            },
                            "additionalProperties": true
                        },
                        {"type": "array"}
                    ]
                },
                "file": {
                    "description": "Convenience alias for a single media path or attachment object.",
                    "oneOf": [{"type": "string"}, {"type": "object"}]
                },
                "files": {
                    "description": "Convenience alias for an array of media paths or attachment objects.",
                    "type": "array"
                },
                "reply_to": {
                    "description": "Optional platform message id, or an object with message_id, to send this as a reply.",
                    "oneOf": [
                        {"type": "integer"},
                        {"type": "string"},
                        {"type": "object"}
                    ]
                },
                "reply_to_message_id": {
                    "description": "Optional platform message id to send this as a reply.",
                    "oneOf": [
                        {"type": "integer"},
                        {"type": "string"}
                    ]
                }
            },
            "anyOf": [
                {"required": ["to"]},
                {"required": ["target"]},
                {"required": ["channel"]},
                {"required": ["chat_id"]},
                {"required": ["open_id"]},
                {"required": ["user"]},
                {"required": ["receive_id"]},
                {"required": ["reply_to"]},
                {"required": ["reply_to_message_id"]}
            ],
            "additionalProperties": true
        }),
        output_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "completed": {"type": "boolean"},
                "summary": {"type": "string"}
            },
            "required": ["completed", "summary"]
        }),
        permission: ConnectorPermissionDefinition {
            category: "external_message_send".to_string(),
            summary: "Send a message through this connector".to_string(),
            external_side_effect: true,
        },
    };
    BTreeMap::from([(send_message.slug.clone(), send_message)])
}

fn telegram_login_actions() -> BTreeMap<String, ConnectorActionDefinition> {
    let mut actions = send_message_actions();
    for action in telegram_specific_actions() {
        actions.insert(action.slug.clone(), action);
    }
    actions
}

fn slack_actions() -> BTreeMap<String, ConnectorActionDefinition> {
    let mut actions = send_message_actions();
    for action in slack_specific_actions() {
        actions.insert(action.slug.clone(), action);
    }
    actions
}

fn slack_specific_actions() -> Vec<ConnectorActionDefinition> {
    vec![
        slack_action_definition(
            "react",
            "React to a Slack message",
            "external_message_interaction",
            "React to an external Slack message",
            slack_message_action_schema(),
        ),
        slack_action_definition(
            "send_reaction",
            "Alias for reacting to a Slack message",
            "external_message_interaction",
            "React to an external Slack message",
            slack_message_action_schema(),
        ),
        slack_action_definition(
            "remove_reaction",
            "Remove a reaction from a Slack message",
            "external_message_interaction",
            "Remove a reaction from an external Slack message",
            slack_message_action_schema(),
        ),
    ]
}

fn slack_action_definition(
    slug: &str,
    description: &str,
    category: &str,
    summary: &str,
    input_schema: Value,
) -> ConnectorActionDefinition {
    ConnectorActionDefinition {
        slug: slug.to_string(),
        description: description.to_string(),
        input_schema,
        output_schema: action_output_schema(),
        permission: ConnectorPermissionDefinition {
            category: category.to_string(),
            summary: summary.to_string(),
            external_side_effect: true,
        },
    }
}

fn slack_message_action_schema() -> Value {
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

fn lark_actions() -> BTreeMap<String, ConnectorActionDefinition> {
    let mut actions = send_message_actions();
    for action in lark_specific_actions() {
        actions.insert(action.slug.clone(), action);
    }
    actions
}

fn lark_specific_actions() -> Vec<ConnectorActionDefinition> {
    vec![
        lark_action_definition(
            "react",
            "React to a Lark message",
            "external_message_interaction",
            "React to an external Lark message",
            lark_message_action_schema(),
        ),
        lark_action_definition(
            "send_reaction",
            "Alias for reacting to a Lark message",
            "external_message_interaction",
            "React to an external Lark message",
            lark_message_action_schema(),
        ),
        lark_action_definition(
            "remove_reaction",
            "Remove a reaction from a Lark message by reaction_id",
            "external_message_interaction",
            "Remove a reaction from an external Lark message",
            lark_message_action_schema(),
        ),
    ]
}

fn lark_action_definition(
    slug: &str,
    description: &str,
    category: &str,
    summary: &str,
    input_schema: Value,
) -> ConnectorActionDefinition {
    ConnectorActionDefinition {
        slug: slug.to_string(),
        description: description.to_string(),
        input_schema,
        output_schema: action_output_schema(),
        permission: ConnectorPermissionDefinition {
            category: category.to_string(),
            summary: summary.to_string(),
            external_side_effect: true,
        },
    }
}

fn lark_message_action_schema() -> Value {
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

fn telegram_specific_actions() -> Vec<ConnectorActionDefinition> {
    vec![
        telegram_action_definition(
            "vote_poll",
            "Vote in a Telegram poll message",
            "external_message_interaction",
            "Vote in an external Telegram poll",
            telegram_poll_vote_schema(),
        ),
        telegram_action_definition(
            "edit_message",
            "Edit a Telegram message or caption",
            "external_message_edit",
            "Edit an external Telegram message",
            telegram_message_action_schema(),
        ),
        telegram_action_definition(
            "delete_messages",
            "Delete one or more Telegram messages",
            "external_message_delete",
            "Delete external Telegram messages",
            telegram_message_action_schema(),
        ),
        telegram_action_definition(
            "forward_messages",
            "Forward one or more Telegram messages",
            "external_message_send",
            "Forward external Telegram messages",
            telegram_message_action_schema(),
        ),
        telegram_action_definition(
            "pin_message",
            "Pin a Telegram message",
            "external_message_admin",
            "Pin an external Telegram message",
            telegram_message_action_schema(),
        ),
        telegram_action_definition(
            "unpin_message",
            "Unpin a Telegram message",
            "external_message_admin",
            "Unpin an external Telegram message",
            telegram_message_action_schema(),
        ),
        telegram_action_definition(
            "unpin_all_messages",
            "Unpin all Telegram messages in a chat",
            "external_message_admin",
            "Unpin all external Telegram messages",
            telegram_peer_action_schema(),
        ),
        telegram_action_definition(
            "react",
            "React to a Telegram message",
            "external_message_interaction",
            "React to an external Telegram message",
            telegram_message_action_schema(),
        ),
        telegram_action_definition(
            "mark_read",
            "Mark a Telegram chat as read",
            "external_message_interaction",
            "Mark an external Telegram chat as read",
            telegram_peer_action_schema(),
        ),
        telegram_action_definition(
            "clear_mentions",
            "Clear Telegram mention counters",
            "external_message_interaction",
            "Clear external Telegram mentions",
            telegram_peer_action_schema(),
        ),
        telegram_action_definition(
            "send_typing",
            "Send a Telegram chat action such as typing or uploading",
            "external_message_interaction",
            "Send an external Telegram chat action",
            telegram_peer_action_schema(),
        ),
        telegram_action_definition(
            "join_chat",
            "Join a public Telegram group or channel",
            "external_chat_membership",
            "Join an external Telegram chat",
            telegram_peer_action_schema(),
        ),
        telegram_action_definition(
            "leave_chat",
            "Leave a Telegram chat",
            "external_chat_membership",
            "Leave an external Telegram chat",
            telegram_peer_action_schema(),
        ),
        telegram_action_definition(
            "invite_users",
            "Invite users to a Telegram group or channel",
            "external_chat_membership",
            "Invite users to an external Telegram chat",
            telegram_membership_action_schema(),
        ),
        telegram_action_definition(
            "kick_participant",
            "Kick a Telegram chat participant",
            "external_chat_admin",
            "Kick an external Telegram chat participant",
            telegram_membership_action_schema(),
        ),
        telegram_action_definition(
            "ban_participant",
            "Ban a Telegram chat participant",
            "external_chat_admin",
            "Ban an external Telegram chat participant",
            telegram_membership_action_schema(),
        ),
        telegram_action_definition(
            "unban_participant",
            "Unban a Telegram chat participant",
            "external_chat_admin",
            "Unban an external Telegram chat participant",
            telegram_membership_action_schema(),
        ),
        telegram_action_definition(
            "update_profile",
            "Update the Telegram account profile fields",
            "external_account_profile",
            "Update the external Telegram account profile",
            telegram_profile_action_schema(),
        ),
        telegram_action_definition(
            "update_username",
            "Update the Telegram account username",
            "external_account_profile",
            "Update the external Telegram account username",
            telegram_profile_action_schema(),
        ),
        telegram_action_definition(
            "update_avatar",
            "Upload a new Telegram account avatar",
            "external_account_profile",
            "Update the external Telegram account avatar",
            telegram_media_action_schema(),
        ),
        telegram_action_definition(
            "update_group_title",
            "Update a Telegram group, megagroup, or channel title",
            "external_chat_admin",
            "Update an external Telegram group title",
            telegram_group_action_schema(),
        ),
        telegram_action_definition(
            "update_group_name",
            "Alias for updating a Telegram group title",
            "external_chat_admin",
            "Update an external Telegram group name",
            telegram_group_action_schema(),
        ),
        telegram_action_definition(
            "update_group_username",
            "Update a Telegram public group or channel username",
            "external_chat_admin",
            "Update an external Telegram group username",
            telegram_group_action_schema(),
        ),
        telegram_action_definition(
            "update_group_photo",
            "Update or remove a Telegram group photo",
            "external_chat_admin",
            "Update an external Telegram group photo",
            telegram_media_action_schema(),
        ),
        telegram_action_definition(
            "send_story",
            "Send a Telegram story with photo or document media",
            "external_message_send",
            "Send an external Telegram story",
            telegram_media_action_schema(),
        ),
    ]
}

fn telegram_action_definition(
    slug: &str,
    description: &str,
    category: &str,
    summary: &str,
    input_schema: Value,
) -> ConnectorActionDefinition {
    ConnectorActionDefinition {
        slug: slug.to_string(),
        description: description.to_string(),
        input_schema,
        output_schema: action_output_schema(),
        permission: ConnectorPermissionDefinition {
            category: category.to_string(),
            summary: summary.to_string(),
            external_side_effect: true,
        },
    }
}

fn action_output_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "completed": {"type": "boolean"},
            "summary": {"type": "string"}
        },
        "required": ["completed", "summary"]
    })
}

fn telegram_peer_action_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": telegram_common_properties(),
        "additionalProperties": true
    })
}

fn telegram_message_action_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": telegram_common_properties(),
        "additionalProperties": true
    })
}

fn telegram_group_action_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": telegram_common_properties(),
        "additionalProperties": true
    })
}

fn telegram_membership_action_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": telegram_common_properties(),
        "additionalProperties": true
    })
}

fn telegram_profile_action_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": telegram_common_properties(),
        "additionalProperties": true
    })
}

fn telegram_media_action_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": telegram_common_properties(),
        "additionalProperties": true
    })
}

fn telegram_poll_vote_schema() -> Value {
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

fn message_output_schema() -> Value {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_cover_required_initial_connectors() {
        let slugs = builtin_connector_templates()
            .into_iter()
            .map(|template| template.slug)
            .collect::<Vec<_>>();

        assert!(slugs.contains(&"telegram-bot".to_string()));
        assert!(slugs.contains(&"telegram-login".to_string()));
        assert!(slugs.contains(&"lark-app".to_string()));
        assert!(slugs.contains(&"lark-login".to_string()));
        assert!(slugs.contains(&"slack-app".to_string()));
        assert!(slugs.contains(&"slack-login".to_string()));
        assert!(slugs.contains(&"slack-bot".to_string()));
        assert!(slugs.contains(&"email".to_string()));
    }

    #[test]
    fn builtins_define_host_enforced_action_permissions() {
        let telegram = builtin_connector_template("telegram-login").unwrap();
        let action = telegram.actions.get("send_message").unwrap();
        let vote = telegram.actions.get("vote_poll").unwrap();
        let update_group = telegram.actions.get("update_group_title").unwrap();
        let lark = builtin_connector_template("lark-login").unwrap();
        let slack = builtin_connector_template("slack-login").unwrap();

        assert_eq!(action.permission.category, "external_message_send");
        assert!(action.permission.external_side_effect);
        assert_eq!(
            telegram.subscriber.as_ref().unwrap().manifest_slug,
            "telegram-user"
        );
        assert_eq!(vote.permission.category, "external_message_interaction");
        assert!(vote.permission.external_side_effect);
        assert_eq!(update_group.permission.category, "external_chat_admin");
        assert!(lark.actions.contains_key("send_message"));
        assert_eq!(
            lark.actions.get("react").unwrap().permission.category,
            "external_message_interaction"
        );
        assert!(!lark.can_subscribe);
        assert!(slack.actions.contains_key("send_message"));
        assert_eq!(
            slack.actions.get("react").unwrap().permission.category,
            "external_message_interaction"
        );
        assert!(
            !builtin_connector_template("slack-app")
                .unwrap()
                .can_subscribe
        );
        assert!(
            !builtin_connector_template("slack-app")
                .unwrap()
                .can_proxy_agent
        );
        assert!(
            !builtin_connector_template("slack-bot")
                .unwrap()
                .can_subscribe
        );
        assert!(
            !builtin_connector_template("slack-bot")
                .unwrap()
                .can_proxy_agent
        );
        assert!(builtin_connector_template("slack-bot")
            .unwrap()
            .actions
            .is_empty());
        assert!(!slack.actions.contains_key("vote_poll"));
        assert!(!slack.actions.contains_key("update_group_title"));
    }
}

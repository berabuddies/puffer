use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

mod schemas;

use schemas::*;

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
        discord_bot_template(),
        lark_login_template(),
        lark_bot_template(),
        matrix_bot_template(),
        slack_app_template(),
        slack_login_template(),
        slack_bot_template(),
        email_template(),
        gmail_browser_template(),
        gcal_browser_template(),
        wechat_login_template(),
    ]
}

/// Looks up a built-in connector template by slug.
pub fn builtin_connector_template(slug: &str) -> Option<ConnectorTemplate> {
    builtin_connector_templates()
        .into_iter()
        .find(|template| template.slug == slug)
}

/// Returns the deterministic default connection slug for a connector template.
pub fn suggested_connection_slug(connector_slug: &str) -> String {
    match connector_slug {
        "telegram-login" => "telegram-user".to_string(),
        "email" => "email".to_string(),
        "gmail-browser" => "gmail-browser".to_string(),
        "gcal-browser" => "gcal-browser".to_string(),
        "lark-login" => "lark-user".to_string(),
        "lark-bot" => "lark-bot".to_string(),
        "wechat-login" => "wechat-user".to_string(),
        "discord-bot" => "discord-bot".to_string(),
        "matrix-bot" => "matrix-bot".to_string(),
        "slack-app" => "slack-app".to_string(),
        "slack-login" => "slack-login".to_string(),
        "telegram-bot" => "telegram-bot".to_string(),
        "slack-bot" => "slack-bot".to_string(),
        _ => connector_slug.to_string(),
    }
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

fn discord_bot_template() -> ConnectorTemplate {
    ConnectorTemplate {
        slug: "discord-bot".to_string(),
        description: "Discord bot connector configured through puffer serve".to_string(),
        skill: "discord".to_string(),
        binary: "puffer connector discord".to_string(),
        command: Vec::new(),
        requires_auth: true,
        can_subscribe: false,
        can_proxy_agent: false,
        subscriber: None,
        output_schema: message_output_schema(),
        actions: BTreeMap::new(),
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

fn matrix_bot_template() -> ConnectorTemplate {
    ConnectorTemplate {
        slug: "matrix-bot".to_string(),
        description: "Matrix room connector configured through puffer serve".to_string(),
        skill: "matrix".to_string(),
        binary: "puffer connector matrix".to_string(),
        command: Vec::new(),
        requires_auth: true,
        can_subscribe: false,
        can_proxy_agent: false,
        subscriber: None,
        output_schema: message_output_schema(),
        actions: BTreeMap::new(),
    }
}

/// Lark personal account over lark-cli (`--as user`): monitor your messages and
/// act as you. No auto-reply.
fn lark_login_template() -> ConnectorTemplate {
    ConnectorTemplate {
        slug: "lark-login".to_string(),
        description: "Lark personal account over lark-cli (monitor + act as you)".to_string(),
        skill: "lark".to_string(),
        binary: "puffer __connector lark-user".to_string(),
        command: vec![
            "puffer".to_string(),
            "__connector".to_string(),
            "lark-user".to_string(),
        ],
        requires_auth: true,
        can_subscribe: true,
        can_proxy_agent: false,
        subscriber: None,
        output_schema: message_output_schema(),
        actions: lark_cli_actions(),
    }
}

/// Lark bot over lark-cli (`--as bot`): auto-reply to incoming messages and act
/// as the bot.
fn lark_bot_template() -> ConnectorTemplate {
    ConnectorTemplate {
        slug: "lark-bot".to_string(),
        description: "Lark bot over lark-cli (auto-reply + act as the bot)".to_string(),
        skill: "lark".to_string(),
        binary: "puffer __connector lark-bot".to_string(),
        command: vec![
            "puffer".to_string(),
            "__connector".to_string(),
            "lark-bot".to_string(),
        ],
        requires_auth: true,
        can_subscribe: true,
        can_proxy_agent: true,
        subscriber: None,
        output_schema: message_output_schema(),
        actions: lark_cli_actions(),
    }
}

/// WeChat personal account over a managed Docker desktop (KasmVNC + native
/// WeChat). Puffer drives the running client through `docker exec` (xdotool /
/// xclip / screenshot) with human-like input timing; there is no WeChat API,
/// so `monitor` reads the screen and `act` simulates a real user. The user
/// scans the QR in the embedded browser pane to log in.
fn wechat_login_template() -> ConnectorTemplate {
    ConnectorTemplate {
        slug: "wechat-login".to_string(),
        description: "WeChat personal account over a managed Docker desktop \
            (monitor + act as you, human-like input)"
            .to_string(),
        skill: "wechat".to_string(),
        binary: "puffer __connector wechat-user".to_string(),
        command: vec![
            "puffer".to_string(),
            "__connector".to_string(),
            "wechat-user".to_string(),
        ],
        requires_auth: true,
        can_subscribe: true,
        can_proxy_agent: false,
        subscriber: None,
        output_schema: message_output_schema(),
        actions: wechat_actions(),
    }
}

/// WeChat connector actions: `send_message` plus group-mention and mark-read.
/// (Money + mass/broadcast operations are intentionally NOT exposed — they are
/// hard-blocked in the bridge dispatcher.)
fn wechat_actions() -> BTreeMap<String, ConnectorActionDefinition> {
    let mut actions = send_message_actions();
    // Override the shared send_message with a WeChat-specific schema/description:
    // WeChat has no message-ids, so `reply_to`/`quote` is the quoted message TEXT
    // (a snippet), not a platform id — distinct from lark/telegram.
    if let Some(send) = actions.get_mut("send_message") {
        send.description =
            "Send a WeChat message: `text` and/or media (`image`/`file`/`media`/`files`, local \
             paths or http(s) URLs). Optional `reply_to`/`quote` = the quoted message TEXT (a \
             snippet to find on screen; WeChat has NO message ids, so a numeric id is rejected)."
                .to_string();
        send.input_schema = wechat_message_action_schema();
    }
    for action in [
        wechat_action_definition(
            "mention",
            "Send a group message that @-mentions one or more members (field `mention`: a name or array of names)",
            "external_message_send",
            "Send an @-mention message in an external WeChat group",
            true,
        ),
        wechat_action_definition(
            "react",
            "Send a 拍一拍 (pat/nudge) to a member by right-clicking their avatar and choosing \
             拍一拍 (field `on`: whom to pat; defaults to the chat's other party). WeChat has no \
             per-message emoji reactions",
            "external_message_interaction",
            "Send a 拍一拍 (pat) in an external WeChat chat",
            true,
        ),
        wechat_action_definition(
            "mark_read",
            "Open a WeChat chat (marks it read); no message is sent",
            "external_message_read",
            "Open/read an external WeChat conversation",
            false,
        ),
        wechat_action_definition(
            "logout",
            "Log out of WeChat via the client UI, KEEPING the container and local \
             data (distinct from deleting the connection, which wipes everything). \
             Re-login is a fresh QR scan",
            "external_account_profile",
            "Log out of the external WeChat account (keep data)",
            true,
        ),
        wechat_action_definition(
            "read_history",
            "Read the last N messages of a chat from the local DB (fields `chat`, `limit`); \
             read-only, requires the direct DB reader",
            "external_message_read",
            "Read external WeChat chat history",
            false,
        ),
    ] {
        actions.insert(action.slug.clone(), action);
    }
    actions
}

fn wechat_action_definition(
    slug: &str,
    description: &str,
    category: &str,
    summary: &str,
    external_side_effect: bool,
) -> ConnectorActionDefinition {
    ConnectorActionDefinition {
        slug: slug.to_string(),
        description: description.to_string(),
        input_schema: wechat_message_action_schema(),
        output_schema: wechat_action_output_schema(),
        permission: ConnectorPermissionDefinition {
            category: category.to_string(),
            summary: summary.to_string(),
            external_side_effect,
        },
    }
}

/// Input schema documenting the WeChat-specific action fields (the connector
/// does not validate input, so this is for agent discoverability). Permissive
/// (`additionalProperties: true`) since different actions use different keys.
fn wechat_message_action_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "to": { "description": "Recipient: a contact or group display name (also accepts target/contact/chat).", "type": "string" },
            "text": { "description": "Message text (alias: message). Optional when sending media-only.", "type": "string" },
            "caption": { "description": "Caption sent as a following text message with media.", "type": "string" },
            "image": { "description": "Image to send inline: a local path or http(s) URL (or an array).", "oneOf": [{"type": "string"}, {"type": "array", "items": {"type": "string"}}] },
            "file": { "description": "File to send as a document card: a local path or http(s) URL (or an array).", "oneOf": [{"type": "string"}, {"type": "array", "items": {"type": "string"}}] },
            "media": { "description": "Image/file path(s)/URL(s), or attachment object(s) {path|url, kind, caption}.", "oneOf": [{"type": "string"}, {"type": "object"}, {"type": "array", "items": {"oneOf": [{"type": "string"}, {"type": "object"}]}}] },
            "files": { "description": "Array of media paths/URLs.", "type": "array", "items": {"oneOf": [{"type": "string"}, {"type": "object"}]} },
            "reply_to": { "description": "Quote/reply target = the quoted message TEXT (a snippet to locate on screen). WeChat has NO message ids; a numeric id is rejected. (alias: quote)", "type": "string" },
            "mention": { "description": "For the `mention` action: a member name or array of names to @ in a group.", "oneOf": [{"type": "string"}, {"type": "array", "items": {"type": "string"}}] },
            "on": { "description": "For the `react` (拍一拍) action: which member to pat; defaults to the chat's other party.", "type": "string" },
            "chat": { "description": "For `read_history`: the chat (contact/group name or wxid/*@chatroom) to read.", "type": "string" },
            "limit": { "description": "For `read_history`: number of recent messages to return.", "type": "integer" }
        },
        "additionalProperties": true
    })
}

/// Output schema matching what the WeChat bridge actually returns.
fn wechat_action_output_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "success": { "type": "boolean" },
            "summary": { "type": "string" },
            "output": { "type": "object" }
        },
        "required": ["success", "summary"]
    })
}

/// `lark-cli` connector actions: `send_message` plus reaction actions.
fn lark_cli_actions() -> BTreeMap<String, ConnectorActionDefinition> {
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
            "Add a reaction to a Lark message",
            "external_message_interaction",
            "React to an external Lark message",
        ),
        lark_action_definition(
            "send_reaction",
            "Alias for adding a reaction to a Lark message",
            "external_message_interaction",
            "React to an external Lark message",
        ),
        lark_action_definition(
            "remove_reaction",
            "Remove a reaction from a Lark message by reaction_id",
            "external_message_interaction",
            "Remove a reaction from an external Lark message",
        ),
    ]
}

fn lark_action_definition(
    slug: &str,
    description: &str,
    category: &str,
    summary: &str,
) -> ConnectorActionDefinition {
    ConnectorActionDefinition {
        slug: slug.to_string(),
        description: description.to_string(),
        input_schema: lark_message_action_schema(),
        output_schema: action_output_schema(),
        permission: ConnectorPermissionDefinition {
            category: category.to_string(),
            summary: summary.to_string(),
            external_side_effect: true,
        },
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
        actions: email_actions(),
    }
}

fn gmail_browser_template() -> ConnectorTemplate {
    ConnectorTemplate {
        slug: "gmail-browser".to_string(),
        description: "Gmail web connector using the global Puffer browser profile".to_string(),
        skill: "gmail-browser".to_string(),
        binary: "puffer __subscriber gmail-browser".to_string(),
        command: Vec::new(),
        requires_auth: true,
        can_subscribe: true,
        can_proxy_agent: false,
        subscriber: Some(ConnectorSubscriberTemplate {
            manifest_slug: "gmail-browser".to_string(),
            state_root: Some("gmail-browser-accounts".to_string()),
            display_name: Some("Gmail Browser".to_string()),
        }),
        output_schema: message_output_schema(),
        actions: gmail_browser_actions(),
    }
}

fn gcal_browser_template() -> ConnectorTemplate {
    ConnectorTemplate {
        slug: "gcal-browser".to_string(),
        description: "Google Calendar web connector using the global Puffer browser profile"
            .to_string(),
        skill: "gcal-browser".to_string(),
        binary: "puffer __subscriber gcal-browser".to_string(),
        command: Vec::new(),
        requires_auth: true,
        can_subscribe: true,
        can_proxy_agent: false,
        subscriber: Some(ConnectorSubscriberTemplate {
            manifest_slug: "gcal-browser".to_string(),
            state_root: Some("gcal-browser-accounts".to_string()),
            display_name: Some("Google Calendar Browser".to_string()),
        }),
        output_schema: message_output_schema(),
        actions: gcal_browser_actions(),
    }
}

fn email_actions() -> BTreeMap<String, ConnectorActionDefinition> {
    let mut actions = send_message_actions();
    for action in email_specific_actions("email") {
        actions.insert(action.slug.clone(), action);
    }
    actions
}

fn gmail_browser_actions() -> BTreeMap<String, ConnectorActionDefinition> {
    let mut actions = BTreeMap::new();
    for action in email_specific_actions("Gmail") {
        actions.insert(action.slug.clone(), action);
    }
    let browser_action = request_user_browser_action_definition();
    actions.insert(browser_action.slug.clone(), browser_action);
    actions
}

fn gcal_browser_actions() -> BTreeMap<String, ConnectorActionDefinition> {
    let mut actions = BTreeMap::new();
    for action in [
        calendar_action_definition(
            "list_events",
            "List visible Google Calendar agenda events from the browser session",
            "external_calendar_read",
            "Read external calendar events",
            false,
        ),
        calendar_action_definition(
            "search_events",
            "Search visible Google Calendar agenda events from the browser session",
            "external_calendar_read",
            "Read external calendar events",
            false,
        ),
        calendar_action_definition(
            "get_detail",
            "Read Google Calendar event details from the browser session",
            "external_calendar_read",
            "Read external calendar event details",
            false,
        ),
        calendar_action_definition(
            "accept",
            "Accept a Google Calendar event invitation",
            "external_calendar_rsvp",
            "Accept an external calendar invitation",
            true,
        ),
        calendar_action_definition(
            "deny",
            "Decline a Google Calendar event invitation",
            "external_calendar_rsvp",
            "Decline an external calendar invitation",
            true,
        ),
    ] {
        actions.insert(action.slug.clone(), action);
    }
    let browser_action = request_user_browser_action_definition();
    actions.insert(browser_action.slug.clone(), browser_action);
    actions
}

fn calendar_action_definition(
    slug: &str,
    description: &str,
    category: &str,
    summary: &str,
    external_side_effect: bool,
) -> ConnectorActionDefinition {
    ConnectorActionDefinition {
        slug: slug.to_string(),
        description: description.to_string(),
        input_schema: calendar_action_schema(),
        output_schema: action_output_schema(),
        permission: ConnectorPermissionDefinition {
            category: category.to_string(),
            summary: summary.to_string(),
            external_side_effect,
        },
    }
}

fn request_user_browser_action_definition() -> ConnectorActionDefinition {
    ConnectorActionDefinition {
        slug: "requestuserbrowseraction".to_string(),
        description: "Ask the user to complete a browser action in the global Puffer profile"
            .to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "description": {
                    "type": "string",
                    "description": "Specific browser action the user should complete."
                },
                "url": {
                    "type": "string",
                    "description": "Optional URL or page that the user should use."
                }
            },
            "required": ["description"],
            "additionalProperties": false
        }),
        output_schema: action_output_schema(),
        permission: ConnectorPermissionDefinition {
            category: "user_browser_action".to_string(),
            summary: "Ask the user to complete a browser action".to_string(),
            external_side_effect: true,
        },
    }
}

fn email_specific_actions(service: &str) -> Vec<ConnectorActionDefinition> {
    vec![
        email_action_definition(
            "list_emails",
            &format!("List {service} emails with optional mailbox, category, keyword, sender, and unread filters"),
            "external_message_read",
            "Read external email messages",
            false,
        ),
        email_action_definition(
            "list_inbox",
            &format!("List recent {service} inbox emails"),
            "external_message_read",
            "Read external email messages",
            false,
        ),
        email_action_definition(
            "list_category",
            &format!("List {service} emails from a mailbox, category, or label"),
            "external_message_read",
            "Read external email messages",
            false,
        ),
        email_action_definition(
            "search_emails",
            &format!("Search {service} emails by keywords and headers"),
            "external_message_read",
            "Read external email messages",
            false,
        ),
        email_action_definition(
            "mark_read",
            &format!("Mark one or more {service} emails as read"),
            "external_message_interaction",
            "Mark external email messages as read",
            true,
        ),
        email_action_definition(
            "draft_reply",
            &format!("Create a draft reply in {service}"),
            "external_message_draft",
            "Draft an external email reply",
            true,
        ),
        email_action_definition(
            "draft_forward",
            &format!("Create a draft forward in {service}"),
            "external_message_draft",
            "Draft an external email forward",
            true,
        ),
        email_action_definition(
            "send_email",
            &format!("Send an email through {service}"),
            "external_message_send",
            "Send an external email",
            true,
        ),
        email_action_definition(
            "delete",
            &format!("Delete one or more {service} emails"),
            "external_message_delete",
            "Delete external email messages",
            true,
        ),
    ]
}

fn email_action_definition(
    slug: &str,
    description: &str,
    category: &str,
    summary: &str,
    external_side_effect: bool,
) -> ConnectorActionDefinition {
    ConnectorActionDefinition {
        slug: slug.to_string(),
        description: description.to_string(),
        input_schema: email_action_schema(),
        output_schema: action_output_schema(),
        permission: ConnectorPermissionDefinition {
            category: category.to_string(),
            summary: summary.to_string(),
            external_side_effect,
        },
    }
}

fn send_message_actions() -> BTreeMap<String, ConnectorActionDefinition> {
    let send_message = ConnectorActionDefinition {
        slug: "send_message".to_string(),
        description: "Send a message through the connector".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "to": {"oneOf": [{"type": "string"}, {"type": "integer"}]},
                "target": {"oneOf": [{"type": "string"}, {"type": "integer"}]},
                "channel": {"oneOf": [{"type": "string"}, {"type": "integer"}]},
                "chat_id": {"oneOf": [{"type": "string"}, {"type": "integer"}]},
                "open_id": {"oneOf": [{"type": "string"}, {"type": "integer"}]},
                "user": {"oneOf": [{"type": "string"}, {"type": "integer"}]},
                "receive_id": {"oneOf": [{"type": "string"}, {"type": "integer"}]},
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
                "attachments": {
                    "description": "Convenience alias for an array of media paths or attachment objects.",
                    "type": "array"
                },
                "path": {
                    "description": "Convenience alias for a single media path or URL.",
                    "type": "string"
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
            "allOf": [
                {
                    "anyOf": [
                        {"required": ["to"]},
                        {"required": ["target"]},
                        {"required": ["channel"]},
                        {"required": ["chat_id"]},
                        {"required": ["open_id"]},
                        {"required": ["user"]},
                        {"required": ["receive_id"]}
                    ]
                },
                {
                    "anyOf": [
                        {"required": ["message"]},
                        {"required": ["text"]},
                        {"required": ["caption"]},
                        {"required": ["media"]},
                        {"required": ["file"]},
                        {"required": ["files"]},
                        {"required": ["attachments"]},
                        {"required": ["path"]}
                    ]
                }
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

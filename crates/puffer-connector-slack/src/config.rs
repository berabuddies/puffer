use puffer_connector_core::GroupKeyPolicy;
use serde::{Deserialize, Serialize};

/// Platform-specific configuration parsed from
/// `[connectors.slack]` in the Puffer config file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SlackConfig {
    /// Bot User OAuth token (`xoxb-…`) used for Web API calls such as
    /// `chat.postMessage`.
    pub bot_token: String,

    /// App-level token (`xapp-…`) with `connections:write` used to open a
    /// Socket Mode connection. Required for the polling-friendly live
    /// driver; ignored by the handler itself.
    pub app_token: String,

    /// Slack user ids (e.g. `"U12345"`) permitted to talk to the bot. When
    /// empty or absent the bot accepts messages from everyone.
    #[serde(default)]
    pub allowed_users: Vec<String>,

    /// Optional greeting sent in response to `/start`.
    #[serde(default)]
    pub welcome_message: Option<String>,

    /// Whether to only respond in channels when the bot is explicitly
    /// @mentioned or replied to. Defaults to `true` — the noise-free
    /// choice that matches Hermes. Ignored in DMs.
    #[serde(default = "default_require_mention")]
    pub require_mention: bool,

    /// How to map channel messages to sessions. `PerUser` (default) keeps
    /// one session per user even in a shared channel; `PerChat` treats
    /// the whole channel as a single shared session.
    #[serde(default)]
    pub group_key_policy: GroupKeyPolicy,
}

fn default_require_mention() -> bool {
    true
}

impl SlackConfig {
    /// Returns `true` when `user_id` may talk to the bot.
    pub fn is_user_allowed(&self, user_id: &str) -> bool {
        self.allowed_users.is_empty() || self.allowed_users.iter().any(|u| u == user_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config(allowed: Vec<String>) -> SlackConfig {
        SlackConfig {
            bot_token: "xoxb-1".to_string(),
            app_token: "xapp-1".to_string(),
            allowed_users: allowed,
            welcome_message: None,
            require_mention: true,
            group_key_policy: GroupKeyPolicy::PerUser,
        }
    }

    #[test]
    fn empty_allowed_list_means_public_bot() {
        let config = make_config(Vec::new());
        assert!(config.is_user_allowed("U123"));
        assert!(config.is_user_allowed("UANY"));
    }

    #[test]
    fn allowed_list_filters_foreign_users() {
        let config = make_config(vec!["U1".to_string(), "U2".to_string(), "U3".to_string()]);
        assert!(config.is_user_allowed("U2"));
        assert!(!config.is_user_allowed("U99"));
    }

    #[test]
    fn config_parses_from_raw_json() {
        let raw = serde_json::json!({
            "bot_token": "xoxb-abc",
            "app_token": "xapp-abc",
            "allowed_users": ["U42", "U7"],
            "welcome_message": "hi",
            "require_mention": false,
            "group_key_policy": "per_chat"
        });
        let config: SlackConfig = serde_json::from_value(raw).unwrap();
        assert_eq!(config.bot_token, "xoxb-abc");
        assert_eq!(config.app_token, "xapp-abc");
        assert_eq!(
            config.allowed_users,
            vec!["U42".to_string(), "U7".to_string()]
        );
        assert_eq!(config.welcome_message.as_deref(), Some("hi"));
        assert!(!config.require_mention);
        assert_eq!(config.group_key_policy, GroupKeyPolicy::PerChat);
    }

    #[test]
    fn optional_fields_default_when_missing() {
        let raw = serde_json::json!({
            "bot_token": "xoxb-abc",
            "app_token": "xapp-abc"
        });
        let config: SlackConfig = serde_json::from_value(raw).unwrap();
        assert!(config.allowed_users.is_empty());
        assert!(config.welcome_message.is_none());
        assert!(config.require_mention, "require_mention defaults true");
        assert_eq!(
            config.group_key_policy,
            GroupKeyPolicy::PerUser,
            "safe default is per-user session segmentation for channels"
        );
    }
}

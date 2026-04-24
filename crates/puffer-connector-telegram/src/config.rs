use puffer_connector_core::GroupKeyPolicy;
use serde::{Deserialize, Serialize};

/// Platform-specific configuration parsed from
/// `[connectors.telegram]` in the Puffer config file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TelegramConfig {
    /// Bot token issued by @BotFather.
    pub token: String,

    /// Numeric Telegram user ids permitted to talk to the bot. When
    /// empty or absent the bot accepts messages from everyone.
    #[serde(default)]
    pub allowed_users: Vec<i64>,

    /// Optional greeting sent in response to `/start`.
    #[serde(default)]
    pub welcome_message: Option<String>,

    /// Whether to only respond in group chats when the bot is
    /// explicitly @mentioned or replied to. Defaults to `true` — the
    /// noise-free choice that matches Hermes. Ignored in DMs.
    #[serde(default = "default_require_mention")]
    pub require_mention: bool,

    /// How to map group messages to sessions. `PerUser` (default) keeps
    /// one session per user even in a shared group; `PerChat` treats
    /// the whole group as a single shared session.
    #[serde(default)]
    pub group_key_policy: GroupKeyPolicy,
}

fn default_require_mention() -> bool {
    true
}

impl TelegramConfig {
    /// Returns `true` when `user_id` may talk to the bot.
    pub fn is_user_allowed(&self, user_id: i64) -> bool {
        self.allowed_users.is_empty() || self.allowed_users.contains(&user_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config(allowed: Vec<i64>) -> TelegramConfig {
        TelegramConfig {
            token: "t".to_string(),
            allowed_users: allowed,
            welcome_message: None,
            require_mention: true,
            group_key_policy: GroupKeyPolicy::PerUser,
        }
    }

    #[test]
    fn empty_allowed_list_means_public_bot() {
        let config = make_config(Vec::new());
        assert!(config.is_user_allowed(123));
        assert!(config.is_user_allowed(-1));
    }

    #[test]
    fn allowed_list_filters_foreign_users() {
        let config = make_config(vec![1, 2, 3]);
        assert!(config.is_user_allowed(2));
        assert!(!config.is_user_allowed(99));
    }

    #[test]
    fn config_parses_from_raw_json() {
        let raw = serde_json::json!({
            "token": "abc",
            "allowed_users": [42, 7],
            "welcome_message": "hi",
            "require_mention": false,
            "group_key_policy": "per_chat"
        });
        let config: TelegramConfig = serde_json::from_value(raw).unwrap();
        assert_eq!(config.token, "abc");
        assert_eq!(config.allowed_users, vec![42, 7]);
        assert_eq!(config.welcome_message.as_deref(), Some("hi"));
        assert!(!config.require_mention);
        assert_eq!(config.group_key_policy, GroupKeyPolicy::PerChat);
    }

    #[test]
    fn optional_fields_default_when_missing() {
        let raw = serde_json::json!({ "token": "abc" });
        let config: TelegramConfig = serde_json::from_value(raw).unwrap();
        assert!(config.allowed_users.is_empty());
        assert!(config.welcome_message.is_none());
        assert!(config.require_mention, "require_mention defaults true");
        assert_eq!(
            config.group_key_policy,
            GroupKeyPolicy::PerUser,
            "safe default is per-user session segmentation for groups"
        );
    }
}

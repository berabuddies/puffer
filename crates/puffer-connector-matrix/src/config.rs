use puffer_connector_core::GroupKeyPolicy;
use serde::{Deserialize, Serialize};

/// Platform-specific configuration parsed from
/// `[connectors.matrix]` in the Puffer config file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MatrixConfig {
    /// Homeserver URL, e.g. `https://matrix.example.org`.
    pub homeserver_url: String,

    /// Matrix account username (localpart or full MXID, depending on the
    /// homeserver). Used for password login.
    pub username: String,

    /// Password for the Matrix account. OAuth/SSO logins are a TODO for a
    /// follow-up: v1 keeps the flow simple.
    pub password: String,

    /// Matrix user ids permitted to talk to the bot, e.g.
    /// `@alice:example.org`. Empty or absent means the bot accepts
    /// messages from everyone in rooms it is joined to.
    #[serde(default)]
    pub allowed_users: Vec<String>,

    /// Optional greeting sent in response to `/start`.
    #[serde(default)]
    pub welcome_message: Option<String>,

    /// Whether to only respond in group rooms when the bot is explicitly
    /// mentioned or replied to. Defaults to `true` — the noise-free
    /// choice that matches Hermes. Ignored in DMs.
    #[serde(default = "default_require_mention")]
    pub require_mention: bool,

    /// How to map group messages to sessions. `PerUser` (default) keeps
    /// one session per user even in a shared room; `PerChat` treats
    /// the whole room as a single shared session.
    #[serde(default)]
    pub group_key_policy: GroupKeyPolicy,
}

fn default_require_mention() -> bool {
    true
}

impl MatrixConfig {
    /// Returns `true` when `user_id` may talk to the bot.
    pub fn is_user_allowed(&self, user_id: &str) -> bool {
        self.allowed_users.is_empty()
            || self.allowed_users.iter().any(|allowed| allowed == user_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config(allowed: Vec<String>) -> MatrixConfig {
        MatrixConfig {
            homeserver_url: "https://matrix.example.org".to_string(),
            username: "bot".to_string(),
            password: "hunter2".to_string(),
            allowed_users: allowed,
            welcome_message: None,
            require_mention: true,
            group_key_policy: GroupKeyPolicy::PerUser,
        }
    }

    #[test]
    fn empty_allowed_list_means_public_bot() {
        let config = make_config(Vec::new());
        assert!(config.is_user_allowed("@alice:example.org"));
        assert!(config.is_user_allowed("@bob:example.org"));
    }

    #[test]
    fn allowed_list_filters_foreign_users() {
        let config = make_config(vec![
            "@alice:example.org".to_string(),
            "@bob:example.org".to_string(),
        ]);
        assert!(config.is_user_allowed("@bob:example.org"));
        assert!(!config.is_user_allowed("@mallory:example.org"));
    }

    #[test]
    fn config_parses_from_raw_json() {
        let raw = serde_json::json!({
            "homeserver_url": "https://matrix.example.org",
            "username": "bot",
            "password": "hunter2",
            "allowed_users": ["@alice:example.org", "@bob:example.org"],
            "welcome_message": "hi",
            "require_mention": false,
            "group_key_policy": "per_chat"
        });
        let config: MatrixConfig = serde_json::from_value(raw).unwrap();
        assert_eq!(config.homeserver_url, "https://matrix.example.org");
        assert_eq!(config.username, "bot");
        assert_eq!(config.password, "hunter2");
        assert_eq!(
            config.allowed_users,
            vec![
                "@alice:example.org".to_string(),
                "@bob:example.org".to_string()
            ]
        );
        assert_eq!(config.welcome_message.as_deref(), Some("hi"));
        assert!(!config.require_mention);
        assert_eq!(config.group_key_policy, GroupKeyPolicy::PerChat);
    }

    #[test]
    fn optional_fields_default_when_missing() {
        let raw = serde_json::json!({
            "homeserver_url": "https://matrix.example.org",
            "username": "bot",
            "password": "hunter2"
        });
        let config: MatrixConfig = serde_json::from_value(raw).unwrap();
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

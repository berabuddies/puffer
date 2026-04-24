use puffer_connector_core::GroupKeyPolicy;
use serde::{Deserialize, Serialize};

fn default_poll_interval_secs() -> u64 {
    60
}

/// Platform-specific configuration parsed from
/// `[connectors.email]` in the Puffer config file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmailConfig {
    /// IMAP server hostname, e.g. `"imap.gmail.com"`.
    pub imap_host: String,

    /// IMAP server port, typically 993 for IMAPS.
    pub imap_port: u16,

    /// SMTP server hostname, e.g. `"smtp.gmail.com"`.
    pub smtp_host: String,

    /// SMTP server port, typically 465 (SMTPS) or 587 (STARTTLS).
    pub smtp_port: u16,

    /// Username used for both IMAP and SMTP authentication.
    pub username: String,

    /// Password (or app password) used for both IMAP and SMTP
    /// authentication.
    pub password: String,

    /// Address placed in the `From:` header of outbound replies.
    pub from_address: String,

    /// Email addresses permitted to talk to the bot. When empty or
    /// absent the bot accepts messages from anyone. Matched
    /// case-insensitively.
    #[serde(default)]
    pub allowed_senders: Vec<String>,

    /// Optional greeting sent in response to `/start`.
    #[serde(default)]
    pub welcome_message: Option<String>,

    /// Poll interval in seconds. Defaults to 60.
    #[serde(default)]
    pub poll_interval_secs: Option<u64>,

    /// Whether to require an explicit @mention before responding.
    /// Ignored by the email driver: email is inherently DM-like so
    /// there is no concept of a group mention. Present for parity with
    /// other connectors. Defaults to `false`.
    #[serde(default)]
    pub require_mention: bool,

    /// How to map conversations to sessions. Has no practical effect
    /// for email because `is_group` is always `false` on this platform,
    /// but is kept for parity with other connectors. Defaults to
    /// [`GroupKeyPolicy::PerUser`].
    #[serde(default)]
    pub group_key_policy: GroupKeyPolicy,
}

impl EmailConfig {
    /// Returns `true` when `sender` may talk to the bot. The comparison
    /// is case-insensitive and trims surrounding whitespace.
    pub fn is_sender_allowed(&self, sender: &str) -> bool {
        if self.allowed_senders.is_empty() {
            return true;
        }
        let candidate = sender.trim().to_ascii_lowercase();
        self.allowed_senders
            .iter()
            .any(|allowed| allowed.trim().to_ascii_lowercase() == candidate)
    }

    /// Effective poll interval, falling back to the 60s default when not
    /// set.
    pub fn effective_poll_interval_secs(&self) -> u64 {
        self.poll_interval_secs
            .unwrap_or_else(default_poll_interval_secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_config() -> EmailConfig {
        EmailConfig {
            imap_host: "imap.example.com".to_string(),
            imap_port: 993,
            smtp_host: "smtp.example.com".to_string(),
            smtp_port: 465,
            username: "bot@example.com".to_string(),
            password: "secret".to_string(),
            from_address: "bot@example.com".to_string(),
            allowed_senders: Vec::new(),
            welcome_message: None,
            poll_interval_secs: None,
            require_mention: false,
            group_key_policy: GroupKeyPolicy::PerUser,
        }
    }

    #[test]
    fn empty_allowed_list_means_public_bot() {
        let config = base_config();
        assert!(config.is_sender_allowed("alice@example.com"));
        assert!(config.is_sender_allowed("BOB@EXAMPLE.COM"));
    }

    #[test]
    fn allowed_list_filters_foreign_senders_case_insensitively() {
        let mut config = base_config();
        config.allowed_senders = vec!["Alice@Example.com".to_string()];
        assert!(config.is_sender_allowed("alice@example.com"));
        assert!(config.is_sender_allowed("ALICE@EXAMPLE.COM"));
        assert!(!config.is_sender_allowed("bob@example.com"));
    }

    #[test]
    fn config_parses_from_raw_json() {
        let raw = serde_json::json!({
            "imap_host": "imap.example.com",
            "imap_port": 993,
            "smtp_host": "smtp.example.com",
            "smtp_port": 465,
            "username": "bot@example.com",
            "password": "secret",
            "from_address": "bot@example.com",
            "allowed_senders": ["alice@example.com", "bob@example.com"],
            "welcome_message": "hi",
            "poll_interval_secs": 30,
            "require_mention": true,
            "group_key_policy": "per_chat"
        });
        let config: EmailConfig = serde_json::from_value(raw).unwrap();
        assert_eq!(config.imap_host, "imap.example.com");
        assert_eq!(config.imap_port, 993);
        assert_eq!(config.smtp_port, 465);
        assert_eq!(
            config.allowed_senders,
            vec!["alice@example.com".to_string(), "bob@example.com".to_string()]
        );
        assert_eq!(config.welcome_message.as_deref(), Some("hi"));
        assert_eq!(config.effective_poll_interval_secs(), 30);
        assert!(config.require_mention);
        assert_eq!(config.group_key_policy, GroupKeyPolicy::PerChat);
    }

    #[test]
    fn optional_fields_default_when_missing() {
        let raw = serde_json::json!({
            "imap_host": "imap.example.com",
            "imap_port": 993,
            "smtp_host": "smtp.example.com",
            "smtp_port": 587,
            "username": "bot@example.com",
            "password": "secret",
            "from_address": "bot@example.com"
        });
        let config: EmailConfig = serde_json::from_value(raw).unwrap();
        assert!(config.allowed_senders.is_empty());
        assert!(config.welcome_message.is_none());
        assert!(config.poll_interval_secs.is_none());
        assert_eq!(config.effective_poll_interval_secs(), 60);
        assert!(
            !config.require_mention,
            "require_mention defaults false on email (inherently DM-like)"
        );
        assert_eq!(
            config.group_key_policy,
            GroupKeyPolicy::PerUser,
            "safe default is per-user session segmentation for parity with other connectors"
        );
    }
}

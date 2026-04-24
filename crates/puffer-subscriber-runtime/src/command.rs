//! Control-channel messages sent *into* a subscriber on its stdin.
//!
//! Subscribers are not required to read stdin, but those that do (e.g. the
//! Telegram user subscriber for login) speak the same ndjson protocol in
//! reverse: one JSON value per line, each matching [`SubscriberCommand`].
//!
//! Today the runtime ships one variant group relevant to Telegram login.
//! New variants can be added freely; subscribers must ignore variants they
//! don't understand.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::process::ChildStdin;
use tokio::sync::Mutex;

/// All control messages the runtime can send to a subscriber child. Tagged
/// with a short `kind` for JSON compatibility with the event envelope.
///
/// Subscribers respond asynchronously by emitting events on stdout (for
/// state changes like `login_awaiting_code`) rather than by writing a reply
/// on stdin.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SubscriberCommand {
    /// Begin a Telegram login flow with the given phone number. The
    /// subscriber arranges for the code to be sent by Telegram and emits
    /// a `login_awaiting_code` event on success or a `login_error` event
    /// on failure.
    ///
    /// `api_id` and `api_hash` are optional. When omitted the subscriber
    /// uses Telegram Desktop's published credentials, which means the
    /// user does not need to register an application at my.telegram.org
    /// for typical usage. Supply your own only if you've hit a
    /// `FLOOD_WAIT` from sharing the default credentials.
    TelegramLoginStart {
        /// E.164 phone number including the leading '+'.
        phone: String,
        /// Optional Telegram `api_id` from my.telegram.org.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        api_id: Option<i32>,
        /// Optional Telegram `api_hash` from my.telegram.org.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        api_hash: Option<String>,
    },
    /// Submit the 5-digit login code Telegram delivered to the user.
    TelegramLoginSubmitCode {
        /// The code string.
        code: String,
    },
    /// Submit the 2FA cloud password when Telegram requests it.
    TelegramLoginSubmitPassword {
        /// The user's 2FA password.
        password: String,
    },
    /// Configure the email subscriber. Sent by the agent after the user
    /// describes their IMAP/SMTP credentials. The subscriber persists the
    /// settings to its state directory and (re)starts polling on success;
    /// failures emit a `config_error` event.
    EmailConfigure {
        /// IMAP server hostname.
        imap_host: String,
        /// IMAP server port (defaults to 993 when 0).
        #[serde(default)]
        imap_port: u16,
        /// SMTP server hostname.
        smtp_host: String,
        /// SMTP server port (defaults to 587 when 0).
        #[serde(default)]
        smtp_port: u16,
        /// IMAP/SMTP login (typically the email address).
        username: String,
        /// IMAP/SMTP password — stored in the subscriber state dir.
        password: String,
        /// `From:` header value for outbound mail. Almost always equal to
        /// `username`.
        from_address: String,
        /// Optional list of senders the subscription router should care
        /// about. Empty means "every inbound email."
        #[serde(default)]
        allowed_senders: Vec<String>,
    },
    /// Send a text message to a peer through this subscriber's account.
    /// Subscribers that also act as message senders (e.g. the user-account
    /// Telegram driver) handle this; subscribers that don't should ignore
    /// the command and emit a `send_unsupported` event.
    SendMessage {
        /// Subscriber-defined peer reference: a `@username`, a numeric
        /// chat id as a string, or a phone number — semantics are owned
        /// by the subscriber.
        peer: String,
        /// Message body. Subscribers may truncate or split for transport
        /// limits.
        text: String,
    },
    /// Opaque pass-through. Useful for subscriber-specific controls that
    /// don't warrant a first-class variant yet.
    Custom {
        /// Subcommand name the subscriber switches on.
        op: String,
        /// Subcommand payload.
        #[serde(default)]
        args: Value,
    },
}

/// Writer handle for sending [`SubscriberCommand`] to a specific
/// subscriber. Wraps the child's stdin behind a mutex so multiple tool
/// invocations can enqueue commands concurrently without interleaving
/// bytes of different JSON lines.
#[derive(Clone)]
pub struct CommandSender {
    inner: std::sync::Arc<Mutex<Option<ChildStdin>>>,
}

impl CommandSender {
    /// Wraps a fresh `ChildStdin` in a shared sender.
    pub fn new(stdin: ChildStdin) -> Self {
        Self {
            inner: std::sync::Arc::new(Mutex::new(Some(stdin))),
        }
    }

    /// Returns a disconnected sender. `send` will always error with a
    /// "subscriber is not running" message. Handy for preserving sender
    /// shape across subscriber restarts.
    pub fn disconnected() -> Self {
        Self {
            inner: std::sync::Arc::new(Mutex::new(None)),
        }
    }

    /// Replaces the underlying stdin (e.g. after a supervisor restart).
    pub async fn replace(&self, stdin: Option<ChildStdin>) {
        let mut guard = self.inner.lock().await;
        *guard = stdin;
    }

    /// Sends one command to the subscriber. Returns an error when the
    /// subscriber is not running or has closed its stdin.
    pub async fn send(&self, command: &SubscriberCommand) -> Result<()> {
        let mut guard = self.inner.lock().await;
        let Some(stdin) = guard.as_mut() else {
            return Err(anyhow!(
                "subscriber is not running; cannot deliver control command"
            ));
        };
        crate::codec::write_line(stdin, command).await
    }
}

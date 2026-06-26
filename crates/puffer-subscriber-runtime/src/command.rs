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
use std::sync::atomic::{AtomicU64, Ordering};
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
    /// uses a hardcoded public Telegram client credential pair.
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
    /// Start a Telegram QR login flow. The subscriber exports a short-lived
    /// login token and emits `login_qr` with a `tg://login?...` URL that can
    /// be opened or scanned from an already logged-in Telegram app.
    TelegramQrLoginStart {
        /// Optional Telegram `api_id` from my.telegram.org.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        api_id: Option<i32>,
        /// Optional Telegram `api_hash` from my.telegram.org.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        api_hash: Option<String>,
    },
    /// Wait for approval of the active Telegram QR login token. On approval
    /// the subscriber persists the authorized session and emits
    /// `login_complete`; on expiration it may emit a refreshed `login_qr`.
    TelegramQrLoginWait {
        /// Optional wait timeout in seconds. Defaults to the subscriber's
        /// standard QR wait window.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_seconds: Option<u64>,
    },
    /// Import an already-authenticated Telegram Desktop `tdata` account into
    /// the subscriber's grammers session file. The subscriber should verify
    /// the imported session by reconnecting and emitting `login_complete`.
    TelegramImportTdata {
        /// Optional path to a Telegram Desktop `tdata` directory. When absent,
        /// the subscriber uses the platform default Telegram Desktop location.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        /// Optional Telegram Desktop local passcode. This is the local app
        /// passcode, not the Telegram cloud 2FA password.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        passcode: Option<String>,
        /// Optional zero-based Telegram Desktop account slot. Defaults to the
        /// main account.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        account_index: Option<usize>,
        /// Optional tdata key file name. Defaults to Telegram Desktop's
        /// standard `data` key file.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key_file: Option<String>,
    },
    /// Probe whether the Telegram session is currently authorized without
    /// touching dialogs, peers, or update state.
    TelegramAuthOk,
    /// List or search Telegram dialogs so callers can obtain stable numeric
    /// peer ids before sending messages or creating workflow filters.
    TelegramListPeers {
        /// Optional case-insensitive query matched against title, username,
        /// alternate usernames, or numeric id.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        query: Option<String>,
        /// Optional Telegram peer type filter.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        peer_kind: Option<TelegramPeerKind>,
        /// Optional maximum result count. Subscribers should apply a
        /// conservative default when omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<usize>,
    },
    /// Search message text within one Telegram peer and optionally return
    /// previous messages so callers can inspect the match safely before acting.
    TelegramSearchMessages {
        /// Peer reference: a `@username` or numeric chat id string returned by
        /// `TelegramListPeers`.
        peer: String,
        /// Text query passed to Telegram message search.
        query: String,
        /// Optional maximum result count.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<usize>,
        /// Optional number of previous messages to include before each hit.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context: Option<usize>,
        /// Return compact message and context payloads for LLM consumption.
        #[serde(default)]
        succinct: bool,
    },
    /// List recent messages within one Telegram peer without requiring a
    /// search term. Callers can page backward with `before_id`, and may
    /// filter by sender while bounding the amount of history scanned.
    TelegramListMessages {
        /// Peer reference: a `@username` or numeric chat id string returned by
        /// `TelegramListPeers`.
        peer: String,
        /// Optional maximum message count.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<usize>,
        /// Optional exclusive Telegram message id cursor. When present,
        /// subscribers return messages older than this id.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before_id: Option<i32>,
        /// Optional sender filter matched against sender id, username, handle,
        /// or display name. The aliases keep local CLI and tool JSON natural.
        #[serde(
            default,
            alias = "from",
            alias = "from_user",
            skip_serializing_if = "Option::is_none"
        )]
        sender: Option<String>,
        /// Optional maximum number of messages to scan while looking for
        /// sender-filtered matches.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scan_limit: Option<usize>,
        /// Return compact message payloads for LLM consumption.
        #[serde(default)]
        succinct: bool,
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
    /// Send a message to a peer through this subscriber's account.
    /// Subscribers that also act as message senders (e.g. the user-account
    /// Telegram driver) handle this; subscribers that don't should ignore
    /// the command and emit a `send_unsupported` event.
    SendMessage {
        /// Server-minted authorization proving this send was approved or
        /// came from a daemon-owned automation path. The sender runtime keeps
        /// this on the typed command so new send exits cannot be constructed
        /// without making an explicit authorization decision.
        authorization: SendAuthorization,
        /// Subscriber-defined peer reference: a `@username`, a numeric
        /// chat id as a string, or a phone number — semantics are owned
        /// by the subscriber.
        peer: String,
        /// Message body. Subscribers may truncate or split for transport
        /// limits.
        text: String,
        /// Optional platform message id to reply to.
        #[serde(default)]
        reply_to: Option<i32>,
        /// Optional file, photo, or document attachments. When present,
        /// `text` is used as the primary caption unless an attachment supplies
        /// its own caption.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        media: Vec<SendMediaAttachment>,
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

/// Server-owned proof that a [`SubscriberCommand::SendMessage`] was created
/// by an approved draft/monitor execution path rather than an ordinary agent
/// tool call.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct SendAuthorization {
    /// Stable authorization source. This is for audit/routing only; callers
    /// still validate the bound recipient/content fields before sending.
    pub source: String,
    /// Draft or action id that produced the authorization.
    pub draft_id: String,
    /// Approved draft/action version.
    pub version: u64,
    /// Connector action being authorized, for example `send_message`.
    pub action: String,
    /// Server-resolved recipient identity. For Telegram this is the resolved
    /// chat id string, not a display name supplied by the agent.
    pub recipient_stable_id: String,
    /// `sha256:<hex>` hash over the approved external message content.
    pub content_hash: String,
    /// Idempotency key supplied by the approving client/daemon operation.
    pub client_request_id: String,
}

/// Telegram dialog kind used when listing or searching peers.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TelegramPeerKind {
    /// A one-to-one user chat.
    User,
    /// A Telegram group or megagroup.
    Group,
    /// A broadcast channel.
    Channel,
}

/// Transport hint for an outbound media attachment.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SendMediaKind {
    /// Let the subscriber infer whether to send as a compressed photo or a
    /// regular document from the path, URL, or MIME type.
    #[default]
    Auto,
    /// Send the attachment as a Telegram photo.
    Photo,
    /// Send the attachment as a Telegram document.
    Document,
    /// Send the attachment as a forced file document.
    File,
}

/// File, URL, or document attachment for [`SubscriberCommand::SendMessage`].
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
pub struct SendMediaAttachment {
    /// Local filesystem path or HTTPS/HTTP URL to attach.
    pub path: String,
    /// Optional per-attachment caption. When absent, the message text becomes
    /// the caption for the first attachment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    /// Optional transport hint. Defaults to automatic photo/document inference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<SendMediaKind>,
    /// Optional MIME type override for document/file uploads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// Optional thumbnail path for document/file uploads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<String>,
}

/// Writer handle for sending [`SubscriberCommand`] to a specific
/// subscriber. Wraps the child's stdin behind a mutex so multiple tool
/// invocations can enqueue commands concurrently without interleaving
/// bytes of different JSON lines.
#[derive(Clone)]
pub struct CommandSender {
    inner: std::sync::Arc<Mutex<Option<ChildStdin>>>,
    generation: std::sync::Arc<AtomicU64>,
}

impl CommandSender {
    /// Wraps a fresh `ChildStdin` in a shared sender.
    pub fn new(stdin: ChildStdin) -> Self {
        Self {
            inner: std::sync::Arc::new(Mutex::new(Some(stdin))),
            generation: std::sync::Arc::new(AtomicU64::new(1)),
        }
    }

    /// Returns a disconnected sender. `send` will always error with a
    /// "subscriber is not running" message. Handy for preserving sender
    /// shape across subscriber restarts.
    pub fn disconnected() -> Self {
        Self {
            inner: std::sync::Arc::new(Mutex::new(None)),
            generation: std::sync::Arc::new(AtomicU64::new(0)),
        }
    }

    /// Replaces the underlying stdin (e.g. after a supervisor restart).
    pub async fn replace(&self, stdin: Option<ChildStdin>) {
        let mut guard = self.inner.lock().await;
        *guard = stdin;
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    /// Returns a monotonically increasing value for stdin replacement.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Returns whether a child stdin handle is currently connected.
    pub async fn is_connected(&self) -> bool {
        self.inner.lock().await.is_some()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telegram_auth_ok_serializes_as_stable_kind() {
        let value = serde_json::to_value(SubscriberCommand::TelegramAuthOk).unwrap();
        assert_eq!(value, serde_json::json!({"kind": "telegram_auth_ok"}));
    }

    #[test]
    fn telegram_list_messages_accepts_sender_aliases() {
        let command: SubscriberCommand = serde_json::from_value(serde_json::json!({
            "kind": "telegram_list_messages",
            "peer": "477843728",
            "from": "@tony",
            "scan_limit": 200,
            "succinct": true
        }))
        .unwrap();

        match command {
            SubscriberCommand::TelegramListMessages {
                sender, scan_limit, ..
            } => {
                assert_eq!(sender.as_deref(), Some("@tony"));
                assert_eq!(scan_limit, Some(200));
            }
            other => panic!("expected telegram list messages command, got {other:?}"),
        }
    }
}

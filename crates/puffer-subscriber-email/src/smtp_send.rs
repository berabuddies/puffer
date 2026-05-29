//! Outbound SMTP helpers.
//!
//! Builds a STARTTLS transport from the current [`EmailConfig`] and sends
//! one plain-text message per `SendMessage` or `send_email` action.

use anyhow::{anyhow, Context as _};
use lettre::message::{Mailbox, Message};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};

use crate::config::EmailConfig;

/// Result returned by [`send_message`]: success carries the byte length of
/// the rendered body for telemetry purposes.
pub struct SendOutcome {
    /// Length of the body sent, in UTF-8 bytes.
    pub bytes: usize,
    /// Number of envelope recipients addressed by the message.
    pub recipients: usize,
}

/// Fully addressed plain-text email request used by connector actions.
pub(crate) struct EmailSendRequest {
    /// Primary recipients.
    pub(crate) to: Vec<String>,
    /// Carbon-copy recipients.
    pub(crate) cc: Vec<String>,
    /// Blind-carbon-copy recipients.
    pub(crate) bcc: Vec<String>,
    /// Subject header value.
    pub(crate) subject: String,
    /// Plain-text body.
    pub(crate) body: String,
    /// Optional `In-Reply-To` header.
    pub(crate) in_reply_to: Option<String>,
    /// Optional `References` header values.
    pub(crate) references: Vec<String>,
}

/// Builds an `AsyncSmtpTransport` configured for the given account.
///
/// Uses `starttls_relay` (submission on port 587 by default) with basic
/// credentials. Callers must still be connected to the tokio runtime.
pub fn build_transport(config: &EmailConfig) -> anyhow::Result<AsyncSmtpTransport<Tokio1Executor>> {
    let creds = Credentials::new(config.username.clone(), config.password.clone());
    let transport = AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&config.smtp_host)
        .with_context(|| format!("invalid SMTP host {}", config.smtp_host))?
        .port(config.smtp_port)
        .credentials(creds)
        .build();
    Ok(transport)
}

/// Sends a single plain-text email from `config.from_address` to `peer`.
///
/// `peer` must be a parseable RFC5322 address; display names are accepted
/// but typically the agent just supplies a bare address. On success returns
/// a [`SendOutcome`] with the body length; on failure returns an `anyhow`
/// error suitable for surfacing in a `send_error` control event.
pub async fn send_message(
    config: &EmailConfig,
    peer: &str,
    text: &str,
) -> anyhow::Result<SendOutcome> {
    send_email(
        config,
        EmailSendRequest {
            to: vec![peer.trim().to_string()],
            cc: Vec::new(),
            bcc: Vec::new(),
            subject: String::new(),
            body: text.to_string(),
            in_reply_to: None,
            references: Vec::new(),
        },
    )
    .await
}

/// Sends a plain-text email through the configured SMTP account.
pub(crate) async fn send_email(
    config: &EmailConfig,
    request: EmailSendRequest,
) -> anyhow::Result<SendOutcome> {
    let from: Mailbox = config
        .from_address
        .parse()
        .with_context(|| format!("invalid from_address `{}`", config.from_address))?;
    let recipient_count = request.to.len() + request.cc.len() + request.bcc.len();
    if recipient_count == 0 {
        anyhow::bail!("email requires at least one recipient");
    }

    let mut builder = Message::builder().from(from).subject(request.subject);
    for recipient in &request.to {
        builder = builder.to(parse_mailbox(recipient)?);
    }
    for recipient in &request.cc {
        builder = builder.cc(parse_mailbox(recipient)?);
    }
    for recipient in &request.bcc {
        builder = builder.bcc(parse_mailbox(recipient)?);
    }
    if let Some(in_reply_to) = request.in_reply_to.as_deref() {
        builder = builder.in_reply_to(in_reply_to.to_string());
    }
    if !request.references.is_empty() {
        builder = builder.references(request.references.join(" "));
    }
    let body_len = request.body.len();
    let message = builder
        .body(request.body)
        .map_err(|err| anyhow!("failed to build outbound email: {err}"))?;

    let transport = build_transport(config)?;
    transport
        .send(message)
        .await
        .map_err(|err| anyhow!("SMTP send failed: {err}"))?;

    Ok(SendOutcome {
        bytes: body_len,
        recipients: recipient_count,
    })
}

fn parse_mailbox(value: &str) -> anyhow::Result<Mailbox> {
    value
        .trim()
        .parse()
        .with_context(|| format!("invalid recipient address `{value}`"))
}

//! Outbound SMTP helpers.
//!
//! Builds a STARTTLS transport from the current [`EmailConfig`] and sends
//! one plain-text message per `SendMessage` command.

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
}

/// Builds an `AsyncSmtpTransport` configured for the given account.
///
/// Uses `starttls_relay` (submission on port 587 by default) with basic
/// credentials. Callers must still be connected to the tokio runtime.
pub fn build_transport(
    config: &EmailConfig,
) -> anyhow::Result<AsyncSmtpTransport<Tokio1Executor>> {
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
    let from: Mailbox = config
        .from_address
        .parse()
        .with_context(|| format!("invalid from_address `{}`", config.from_address))?;
    let to: Mailbox = peer
        .trim()
        .parse()
        .with_context(|| format!("invalid peer address `{peer}`"))?;

    let message = Message::builder()
        .from(from)
        .to(to)
        .subject("")
        .body(text.to_string())
        .map_err(|err| anyhow!("failed to build outbound email: {err}"))?;

    let transport = build_transport(config)?;
    transport
        .send(message)
        .await
        .map_err(|err| anyhow!("SMTP send failed: {err}"))?;

    Ok(SendOutcome { bytes: text.len() })
}

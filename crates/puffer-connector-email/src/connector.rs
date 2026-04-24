use crate::handler::{handle_command, PLATFORM_ID};
use crate::EmailConfig;
use anyhow::{anyhow, Context, Result};
use futures::StreamExt;
use lettre::message::{Mailbox, Message};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};
use mail_parser::MessageParser;
use puffer_connector_core::{
    CommandOutcome, Connector, ConnectorHandle, ConnectorRuntime, ConnectorStartError,
    InboundMessage, MessageSplitter,
};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;

/// Email connector ready to be started by the puffer connector hub.
pub struct EmailConnector {
    config: EmailConfig,
}

impl EmailConnector {
    pub fn new(config: EmailConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &EmailConfig {
        &self.config
    }
}

impl Connector for EmailConnector {
    fn id(&self) -> &str {
        PLATFORM_ID
    }

    fn start(
        self: Box<Self>,
        runtime: Arc<ConnectorRuntime>,
    ) -> Result<ConnectorHandle, ConnectorStartError> {
        if self.config.imap_host.trim().is_empty()
            || self.config.smtp_host.trim().is_empty()
            || self.config.username.trim().is_empty()
            || self.config.from_address.trim().is_empty()
        {
            return Err(ConnectorStartError::MissingConfig {
                id: PLATFORM_ID.to_string(),
                detail: "imap_host, smtp_host, username, and from_address are required"
                    .to_string(),
            });
        }

        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let (ready_tx, ready_rx) = mpsc::channel::<()>();

        let config = self.config.clone();
        let join = std::thread::Builder::new()
            .name("puffer-connector-email".to_string())
            .spawn(move || -> Result<()> {
                let tokio_runtime = tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .context("failed to build tokio runtime for email connector")?;
                let _ = ready_tx.send(());
                tokio_runtime.block_on(run_poll_loop(config, runtime, shutdown_rx))
            })
            .map_err(|error| ConnectorStartError::other(PLATFORM_ID, error.into()))?;

        ready_rx
            .recv()
            .map_err(|error| ConnectorStartError::other(PLATFORM_ID, error.into()))?;

        let shutdown: Box<dyn FnOnce() + Send> = Box::new(move || {
            let _ = shutdown_tx.send(());
        });

        Ok(ConnectorHandle {
            id: PLATFORM_ID.to_string(),
            shutdown,
            join,
        })
    }
}

/// Main poll loop. Wakes every `poll_interval_secs`, calls `poll_once`,
/// and exits when the shutdown oneshot fires.
async fn run_poll_loop(
    config: EmailConfig,
    runtime: Arc<ConnectorRuntime>,
    mut shutdown_rx: oneshot::Receiver<()>,
) -> Result<()> {
    let interval = Duration::from_secs(config.effective_poll_interval_secs().max(1));

    loop {
        if let Err(error) = poll_once(&config, &runtime).await {
            eprintln!("puffer-connector-email: poll error: {error:#}");
        }

        tokio::select! {
            _ = &mut shutdown_rx => break,
            _ = tokio::time::sleep(interval) => {}
        }
    }
    Ok(())
}

/// Fetches UNSEEN messages from INBOX, dispatches each one through the
/// shared handler, and sends replies.
async fn poll_once(config: &EmailConfig, runtime: &Arc<ConnectorRuntime>) -> Result<()> {
    let messages = fetch_unseen(config).await?;
    if messages.is_empty() {
        return Ok(());
    }

    let transport = build_smtp_transport(config)?;
    let from_address_lc = config.from_address.trim().to_ascii_lowercase();

    for message in messages {
        // Build the shared InboundMessage shape the handler expects.
        // Email is inherently DM-like so `is_group=false` and
        // `bot_mentioned=true` always. `from_bot` is true when the
        // sender address matches our configured `from_address`
        // (case-insensitive), guarding against rare loop scenarios
        // (self-forwards, mailing-list echoes).
        let body_or_subject = if message.body.trim().is_empty() {
            message.subject.clone()
        } else {
            message.body.clone()
        };
        let sender_lc = message.sender.trim().to_ascii_lowercase();
        let from_bot = !from_address_lc.is_empty() && sender_lc == from_address_lc;

        let inbound = InboundMessage {
            conversation_id: message.thread_id.clone(),
            user_id: Some(message.sender.clone()),
            text: body_or_subject,
            thread_id: None,
            is_group: false,
            bot_mentioned: true,
            from_bot,
        };

        let runtime_for_handler = runtime.clone();
        let config_for_handler = config.clone();
        let outcome = tokio::task::spawn_blocking(move || {
            handle_command(&runtime_for_handler, &inbound, &config_for_handler)
        })
        .await
        .map_err(|error| anyhow!("email handler join error: {error}"))??;

        let reply_text = match outcome {
            CommandOutcome::Ignored => continue,
            CommandOutcome::Reply(text) => text,
            CommandOutcome::AgentReply { text, .. } => text,
        };

        if let Err(error) =
            send_reply_chunks(&transport, config, &message, &reply_text).await
        {
            eprintln!(
                "puffer-connector-email: failed to send reply to {}: {error:#}",
                message.sender
            );
        }
    }

    Ok(())
}

/// One inbound email parsed into the shape the handler expects.
#[derive(Debug, Clone)]
struct InboundEmail {
    /// Stable conversation key: the first `Message-ID` in the thread
    /// (derived from `References` / `In-Reply-To` when available, or the
    /// current message's own `Message-ID` for a brand-new thread).
    thread_id: String,
    /// Bare address from the `From:` header.
    sender: String,
    /// Subject line (possibly empty).
    subject: String,
    /// First text/plain body part, UTF-8 decoded.
    body: String,
    /// The full `Message-ID` of this specific email, used as the
    /// `In-Reply-To` of our outbound reply.
    message_id: Option<String>,
}

async fn fetch_unseen(config: &EmailConfig) -> Result<Vec<InboundEmail>> {
    let tcp = tokio::net::TcpStream::connect((config.imap_host.as_str(), config.imap_port))
        .await
        .with_context(|| format!("connect to IMAP {}:{}", config.imap_host, config.imap_port))?;
    let tls = async_native_tls::TlsConnector::new();
    let tls_stream = tls
        .connect(config.imap_host.as_str(), tcp)
        .await
        .context("TLS handshake with IMAP server failed")?;

    let client = async_imap::Client::new(tls_stream);
    let mut session = client
        .login(&config.username, &config.password)
        .await
        .map_err(|(error, _)| anyhow!("IMAP login failed: {error}"))?;

    session
        .select("INBOX")
        .await
        .context("failed to select INBOX")?;

    let uids = session
        .search("UNSEEN")
        .await
        .context("IMAP UNSEEN search failed")?;

    let mut out = Vec::new();
    if !uids.is_empty() {
        let uid_list = uids
            .iter()
            .map(|uid| uid.to_string())
            .collect::<Vec<_>>()
            .join(",");

        let mut stream = session
            .fetch(uid_list, "RFC822")
            .await
            .context("IMAP FETCH failed")?;

        while let Some(item) = stream.next().await {
            let fetch = item.context("IMAP fetch item error")?;
            let Some(body_bytes) = fetch.body() else {
                continue;
            };
            match parse_email(body_bytes) {
                Some(email) => out.push(email),
                None => continue,
            }
        }
    }

    let _ = session.logout().await;
    Ok(out)
}

fn parse_email(raw: &[u8]) -> Option<InboundEmail> {
    let parsed = MessageParser::default().parse(raw)?;

    let sender = parsed
        .from()
        .and_then(|addrs| addrs.first())
        .and_then(|addr| addr.address())
        .unwrap_or("")
        .to_string();
    if sender.is_empty() {
        return None;
    }

    let subject = parsed.subject().unwrap_or("").to_string();
    let body = parsed
        .body_text(0)
        .map(|s| s.into_owned())
        .unwrap_or_default();

    let message_id = parsed.message_id().map(|s| s.to_string());

    // Prefer the first reference (the root of the thread). Fall back to
    // In-Reply-To, then to this message's own Message-ID for a brand-new
    // thread.
    let thread_id = parsed
        .references()
        .as_text_list()
        .and_then(|list| list.first().map(|s| s.to_string()))
        .or_else(|| parsed.in_reply_to().as_text().map(|s| s.to_string()))
        .or_else(|| message_id.clone())
        .unwrap_or_else(|| sender.clone());

    Some(InboundEmail {
        thread_id,
        sender,
        subject,
        body,
        message_id,
    })
}

fn build_smtp_transport(
    config: &EmailConfig,
) -> Result<AsyncSmtpTransport<Tokio1Executor>> {
    let creds = Credentials::new(config.username.clone(), config.password.clone());
    let builder = AsyncSmtpTransport::<Tokio1Executor>::relay(&config.smtp_host)
        .with_context(|| format!("invalid SMTP host {}", config.smtp_host))?
        .port(config.smtp_port)
        .credentials(creds);
    Ok(builder.build())
}

/// Splits a long reply using [`MessageSplitter::EMAIL`] and sends each
/// chunk as a separate email (preserving the threading headers), with
/// per-chunk retries on transient SMTP errors. In practice email's
/// 100_000-char budget almost never triggers a split — but very long
/// agent outputs shouldn't blow up the SMTP session.
async fn send_reply_chunks(
    transport: &AsyncSmtpTransport<Tokio1Executor>,
    config: &EmailConfig,
    inbound: &InboundEmail,
    reply_text: &str,
) -> Result<()> {
    for chunk in MessageSplitter::EMAIL.split(reply_text) {
        send_with_retry(transport, config, inbound, &chunk).await?;
    }
    Ok(())
}

/// Sends a single email chunk with exponential backoff. Retries up to
/// `MAX_ATTEMPTS` times on transient SMTP errors before giving up. Uses
/// a bigger base delay than chat connectors because email servers are
/// slower and more sensitive to burst traffic.
async fn send_with_retry(
    transport: &AsyncSmtpTransport<Tokio1Executor>,
    config: &EmailConfig,
    inbound: &InboundEmail,
    chunk: &str,
) -> Result<()> {
    const MAX_ATTEMPTS: u32 = 3;
    const BASE_DELAY_MS: u64 = 500;

    let mut attempt = 0u32;
    loop {
        match send_reply(transport, config, inbound, chunk).await {
            Ok(()) => return Ok(()),
            Err(error) => {
                attempt += 1;
                if attempt >= MAX_ATTEMPTS {
                    return Err(error);
                }
                let delay_ms = BASE_DELAY_MS * (1u64 << (attempt - 1));
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
        }
    }
}

async fn send_reply(
    transport: &AsyncSmtpTransport<Tokio1Executor>,
    config: &EmailConfig,
    inbound: &InboundEmail,
    reply_text: &str,
) -> Result<()> {
    let from: Mailbox = config
        .from_address
        .parse()
        .with_context(|| format!("invalid from_address {}", config.from_address))?;
    let to: Mailbox = inbound
        .sender
        .parse()
        .with_context(|| format!("invalid reply-to address {}", inbound.sender))?;

    let subject = if inbound.subject.to_ascii_lowercase().starts_with("re:") {
        inbound.subject.clone()
    } else if inbound.subject.is_empty() {
        "Re: (no subject)".to_string()
    } else {
        format!("Re: {}", inbound.subject)
    };

    let mut builder = Message::builder().from(from).to(to).subject(subject);
    if let Some(message_id) = inbound.message_id.as_ref() {
        builder = builder.in_reply_to(message_id.clone());
        builder = builder.references(message_id.clone());
    }

    let email = builder
        .body(reply_text.to_string())
        .context("failed to build reply email")?;

    transport
        .send(email)
        .await
        .context("SMTP send failed")?;
    Ok(())
}

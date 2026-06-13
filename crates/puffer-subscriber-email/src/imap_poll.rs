//! IMAP polling helpers.
//!
//! Provides [`poll_once`], the per-tick routine the main loop invokes: it
//! connects over TLS, logs in, selects INBOX, walks new UIDs, emits one event
//! per new message that passes the sender filter, and persists the new
//! high-water mark.

use anyhow::{anyhow, Context as _};
use async_imap::Session;
use async_native_tls::TlsStream;
use futures::StreamExt;
use mail_parser::MessageParser;
use puffer_subscriber_runtime::Event;
use serde_json::{json, Map, Value};
use tokio::net::TcpStream;
use tracing::{debug, warn};

use crate::config::EmailConfig;
use crate::events::emit_event;
use crate::state::{self, SeenState, INBOX};

/// Convenient alias for the session type returned by async-imap after login.
type ImapSession = Session<TlsStream<TcpStream>>;

/// Runs one IMAP poll:
///
/// * Connects / logs in / selects INBOX.
/// * On the very first poll after a fresh config (no recorded UID),
///   baselines the high-water mark at the mailbox's current max UID without
///   emitting any events — this avoids the config-flood problem.
/// * On subsequent polls, fetches every UID strictly greater than the
///   stored high-water mark, parses, filters by `allowed_senders` and
///   `from_address`, and emits a `message` event for each survivor.
/// * Persists the new high-water mark to `seen.json`.
///
/// Returns the count of events emitted this cycle (for logging only).
pub async fn poll_once(
    topic: &str,
    config: &EmailConfig,
    state_dir: &std::path::Path,
    seen: &mut SeenState,
) -> anyhow::Result<usize> {
    let mut session = connect_and_login(config).await?;

    session
        .select(INBOX)
        .await
        .context("IMAP SELECT INBOX failed")?;

    let mut emitted = 0usize;
    let previous_high = seen.last_uid(INBOX);

    match previous_high {
        None => {
            // Fresh config: baseline at the current max UID without emitting.
            let baseline = fetch_max_uid(&mut session).await?;
            seen.set_last_uid(INBOX, baseline);
            state::save(state_dir, seen).await?;
            debug!(baseline, "seeded email high-water UID on first poll");
        }
        Some(high) => {
            let uids = search_new_uids(&mut session, high).await?;
            if !uids.is_empty() {
                emitted = fetch_and_emit(&mut session, topic, config, &uids, seen).await?;
                state::save(state_dir, seen).await?;
            }
        }
    }

    // Best-effort logout; IMAP servers are forgiving about dropped sessions.
    if let Err(err) = session.logout().await {
        warn!(error = %err, "IMAP LOGOUT returned error; ignoring");
    }
    Ok(emitted)
}

/// Opens a TLS TCP connection to the IMAP server and performs LOGIN. Returns
/// an authenticated [`ImapSession`] ready for SELECT.
async fn connect_and_login(config: &EmailConfig) -> anyhow::Result<ImapSession> {
    let tcp = TcpStream::connect((config.imap_host.as_str(), config.imap_port))
        .await
        .with_context(|| format!("connect to IMAP {}:{}", config.imap_host, config.imap_port))?;
    let tls = async_native_tls::TlsConnector::new();
    let tls_stream = tls
        .connect(config.imap_host.as_str(), tcp)
        .await
        .context("TLS handshake with IMAP server failed")?;

    let client = async_imap::Client::new(tls_stream);
    let session = client
        .login(&config.username, &config.password)
        .await
        .map_err(|(error, _)| anyhow!("IMAP login failed: {error}"))?;
    Ok(session)
}

/// Returns the largest UID currently in the selected mailbox, or 0 when it
/// is empty. Uses `UID SEARCH ALL`.
async fn fetch_max_uid(session: &mut ImapSession) -> anyhow::Result<u32> {
    let uids = session
        .uid_search("ALL")
        .await
        .context("IMAP UID SEARCH ALL failed")?;
    Ok(uids.into_iter().max().unwrap_or(0))
}

/// Returns every UID strictly greater than `high` that is currently present
/// in the mailbox. Uses `UID SEARCH UID high+1:*` so the server filters for
/// us. The result is sorted ascending.
async fn search_new_uids(session: &mut ImapSession, high: u32) -> anyhow::Result<Vec<u32>> {
    let lower = high.saturating_add(1);
    // `UID high+1:*` on an empty-above-high mailbox returns the single UID
    // `*` (the current max); we defensively filter to `uid > high` below.
    let query = format!("UID {lower}:*");
    let found = session
        .uid_search(&query)
        .await
        .with_context(|| format!("IMAP UID SEARCH `{query}` failed"))?;
    let mut uids: Vec<u32> = found.into_iter().filter(|uid| *uid > high).collect();
    uids.sort_unstable();
    Ok(uids)
}

/// Fetches the RFC822 body of each UID in `uids`, parses, filters, emits an
/// event per survivor, and advances `seen` to the largest UID fetched.
async fn fetch_and_emit(
    session: &mut ImapSession,
    topic: &str,
    config: &EmailConfig,
    uids: &[u32],
    seen: &mut SeenState,
) -> anyhow::Result<usize> {
    let uid_list = uids
        .iter()
        .map(|uid| uid.to_string())
        .collect::<Vec<_>>()
        .join(",");

    let mut stream = session
        .uid_fetch(&uid_list, "RFC822")
        .await
        .context("IMAP UID FETCH failed")?;

    let from_lower = config.from_address.trim().to_ascii_lowercase();
    let allowed: Vec<String> = config
        .allowed_senders
        .iter()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect();

    let mut max_uid_seen = seen.last_uid(INBOX).unwrap_or(0);
    let mut emitted = 0usize;

    while let Some(item) = stream.next().await {
        let fetch = item.context("IMAP fetch item error")?;
        let uid = match fetch.uid {
            Some(u) => u,
            None => {
                warn!("IMAP fetch response missing UID; skipping");
                continue;
            }
        };
        if uid > max_uid_seen {
            max_uid_seen = uid;
        }
        let Some(body_bytes) = fetch.body() else {
            continue;
        };
        let Some(parsed) = ParsedEmail::from_raw(body_bytes) else {
            continue;
        };
        if parsed.from.is_empty() {
            continue;
        }
        let from_lc = parsed.from.to_ascii_lowercase();
        if !from_lower.is_empty() && from_lc == from_lower {
            // Don't echo our own outbound mail back to the agent.
            continue;
        }
        if !allowed.is_empty() && !sender_is_allowed(&from_lc, &allowed) {
            continue;
        }

        let event = build_message_event(topic, config, uid, &parsed);
        emit_event(&event)?;
        emitted += 1;
    }

    drop(stream);

    seen.set_last_uid(INBOX, max_uid_seen);
    Ok(emitted)
}

/// Returns `true` when `from_lc` (already lowercased) satisfies at least one
/// entry in the lowercased `allowed` list. An entry beginning with `@` is
/// treated as a domain suffix match; any other entry must match the full
/// address exactly.
pub fn sender_is_allowed(from_lc: &str, allowed: &[String]) -> bool {
    for rule in allowed {
        if let Some(domain) = rule.strip_prefix('@') {
            if from_lc.ends_with(&format!("@{domain}")) || from_lc.ends_with(domain) {
                return true;
            }
            continue;
        }
        if rule == from_lc {
            return true;
        }
    }
    false
}

/// Flattened view of the fields we extract from an inbound email.
struct ParsedEmail {
    from: String,
    sender_name: String,
    subject: String,
    body: String,
    has_attachment: bool,
    message_id: Option<String>,
    thread_id: String,
    date_ms: Option<i64>,
}

impl ParsedEmail {
    /// Parses an RFC822 byte slice into the fields we care about. Returns
    /// `None` if mail-parser rejects the bytes outright.
    fn from_raw(raw: &[u8]) -> Option<Self> {
        let parsed = MessageParser::default().parse(raw)?;

        let from_addr = parsed.from().and_then(|addrs| addrs.first());
        let from = from_addr
            .and_then(|addr| addr.address())
            .unwrap_or("")
            .to_string();
        let sender_name = from_addr
            .and_then(|addr| addr.name())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(&from)
            .to_string();

        let subject = parsed.subject().unwrap_or("").to_string();
        let body = parsed
            .body_text(0)
            .map(|s| s.into_owned())
            .unwrap_or_default();
        let has_attachment = parsed.attachment_count() > 0;
        let message_id = parsed.message_id().map(|s| s.to_string());

        let thread_id = parsed
            .references()
            .as_text_list()
            .and_then(|list| list.first().map(|s| s.to_string()))
            .or_else(|| parsed.in_reply_to().as_text().map(|s| s.to_string()))
            .or_else(|| message_id.clone())
            .unwrap_or_else(|| from.clone());

        let date_ms = parsed.date().map(|d| d.to_timestamp() * 1000);

        Some(Self {
            from,
            sender_name,
            subject,
            body,
            has_attachment,
            message_id,
            thread_id,
            date_ms,
        })
    }
}

/// Assembles the ndjson [`Event`] for one parsed inbound email.
fn build_message_event(topic: &str, config: &EmailConfig, uid: u32, email: &ParsedEmail) -> Event {
    let text = if email.body.is_empty() {
        email.subject.clone()
    } else if email.subject.is_empty() {
        email.body.clone()
    } else {
        format!("{}\n\n{}", email.subject, email.body)
    };

    let preview: String = email.body.chars().take(1024).collect();

    let mut payload = Map::new();
    payload.insert(
        "sender_name".to_string(),
        Value::String(email.sender_name.clone()),
    );
    payload.insert("from".to_string(), Value::String(email.from.clone()));
    payload.insert("subject".to_string(), Value::String(email.subject.clone()));
    payload.insert("body_preview".to_string(), Value::String(preview));
    payload.insert("has_attachment".to_string(), json!(email.has_attachment));
    if let Some(mid) = email.message_id.as_ref() {
        payload.insert("message_id".to_string(), Value::String(mid.clone()));
    }
    payload.insert(
        "thread_id".to_string(),
        Value::String(email.thread_id.clone()),
    );
    if let Some(date_ms) = email.date_ms {
        payload.insert("date_ms".to_string(), json!(date_ms));
    }
    payload.insert("uid".to_string(), json!(uid));
    payload.insert("unread".to_string(), json!(true));

    Event {
        topic: topic.to_string(),
        kind: "message".to_string(),
        control: false,
        dedup_key: Some(format!("{}:{}", config.username, uid)),
        text,
        payload: Value::Object(payload),
    }
}

#[cfg(test)]
mod tests {
    use super::{build_message_event, ParsedEmail};
    use crate::config::EmailConfig;

    fn config() -> EmailConfig {
        EmailConfig {
            imap_host: "imap.example.com".to_string(),
            imap_port: 993,
            smtp_host: "smtp.example.com".to_string(),
            smtp_port: 587,
            username: "me@example.com".to_string(),
            password: "secret".to_string(),
            from_address: "me@example.com".to_string(),
            allowed_senders: Vec::new(),
        }
    }

    #[test]
    fn message_event_exposes_gmail_aligned_sender_and_unread_fields() {
        let raw = b"From: John Smith <john@example.com>\r\n\
Subject: Invoice due\r\n\
Message-ID: <m1@example.com>\r\n\
\r\n\
Please pay this invoice";
        let email = ParsedEmail::from_raw(raw).expect("parsed email");

        let event = build_message_event("email", &config(), 7, &email);

        assert_eq!(event.payload["sender_name"], "John Smith");
        assert_eq!(event.payload["from"], "john@example.com");
        assert_eq!(event.payload["subject"], "Invoice due");
        assert_eq!(event.payload["body_preview"], "Please pay this invoice");
        assert_eq!(event.payload["unread"], true);
        assert_eq!(event.payload["has_attachment"], false);
    }

    #[test]
    fn message_event_marks_mime_attachments() {
        let raw = b"From: John Smith <john@example.com>\r\n\
Subject: Invoice due\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"b\"\r\n\
\r\n\
--b\r\n\
Content-Type: text/plain\r\n\
\r\n\
Please pay this invoice\r\n\
--b\r\n\
Content-Type: application/pdf; name=\"invoice.pdf\"\r\n\
Content-Disposition: attachment; filename=\"invoice.pdf\"\r\n\
\r\n\
PDF bytes\r\n\
--b--";
        let email = ParsedEmail::from_raw(raw).expect("parsed email");

        let event = build_message_event("email", &config(), 8, &email);

        assert_eq!(event.payload["has_attachment"], true);
    }
}

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
    subject: String,
    body: String,
    message_id: Option<String>,
    thread_id: String,
    date_ms: Option<i64>,
}

impl ParsedEmail {
    /// Parses an RFC822 byte slice into the fields we care about. Returns
    /// `None` if mail-parser rejects the bytes outright.
    fn from_raw(raw: &[u8]) -> Option<Self> {
        let parsed = MessageParser::default().parse(raw)?;

        let from = parsed
            .from()
            .and_then(|addrs| addrs.first())
            .and_then(|addr| addr.address())
            .unwrap_or("")
            .to_string();

        let subject = parsed.subject().unwrap_or("").to_string();
        let body = parsed
            .body_text(0)
            .map(|s| s.into_owned())
            .unwrap_or_default();
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
            subject,
            body,
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
    payload.insert("from".to_string(), Value::String(email.from.clone()));
    payload.insert("subject".to_string(), Value::String(email.subject.clone()));
    payload.insert("body_preview".to_string(), Value::String(preview));
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

    Event {
        topic: topic.to_string(),
        kind: "message".to_string(),
        control: false,
        dedup_key: Some(format!("{}:{}", config.username, uid)),
        text,
        payload: Value::Object(payload),
    }
}

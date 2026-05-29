//! Connector action handlers for the IMAP/SMTP email subscriber.
//!
//! The polling loop streams new inbox messages, while this module handles
//! explicit `ConnectorAct` calls such as listing, marking read, drafting,
//! deleting, and sending mail.

use anyhow::{anyhow, Context as _};
use async_imap::types::Flag;
use async_imap::Session;
use async_native_tls::TlsStream;
use futures::pin_mut;
use futures::StreamExt;
use mail_parser::MessageParser;
use serde_json::{json, Map, Value};
use tokio::net::TcpStream;

use crate::config::EmailConfig;
use crate::smtp_send::{self, EmailSendRequest};

type ImapSession = Session<TlsStream<TcpStream>>;

/// Executes one email connector action against IMAP or SMTP.
pub(crate) async fn handle_action(
    config: &EmailConfig,
    action: &str,
    input: &Value,
) -> anyhow::Result<Value> {
    match canonical_action(action) {
        "list_emails" => list_emails(config, action, input).await,
        "mark_read" => mark_read(config, action, input).await,
        "delete" => delete_messages(config, action, input).await,
        "draft_reply" | "draft_forward" => draft_message(config, action, input).await,
        "send_email" => send_email(config, action, input).await,
        other => anyhow::bail!("unsupported email action `{other}`"),
    }
}

fn canonical_action(action: &str) -> &str {
    match action {
        "list_inbox" | "list_category" | "search_emails" => "list_emails",
        other => other,
    }
}

async fn list_emails(config: &EmailConfig, action: &str, input: &Value) -> anyhow::Result<Value> {
    let mailbox = mailbox_from_input(input);
    let limit = integer_input(input, "limit").unwrap_or(30).clamp(1, 100) as usize;
    let scan_limit = integer_input(input, "scan_limit")
        .unwrap_or((limit as u64 * 5).max(100))
        .clamp(limit as u64, 1000) as usize;
    let unread_filter = input.get("unread").and_then(Value::as_bool);

    let mut session = connect_and_login(config).await?;
    session
        .select(&mailbox)
        .await
        .with_context(|| format!("IMAP SELECT `{mailbox}` failed"))?;
    let search = match unread_filter {
        Some(true) => "UNSEEN",
        Some(false) => "SEEN",
        None => "ALL",
    };
    let mut uids = session
        .uid_search(search)
        .await
        .with_context(|| format!("IMAP UID SEARCH `{search}` failed"))?
        .into_iter()
        .collect::<Vec<_>>();
    uids.sort_unstable_by(|left, right| right.cmp(left));
    uids.truncate(scan_limit);

    let mut messages = fetch_messages(&mut session, &uids).await?;
    messages.sort_by(|left, right| right.uid.cmp(&left.uid));
    let filters = EmailFilters::from_input(input);
    let messages = messages
        .into_iter()
        .filter(|message| filters.matches(message))
        .take(limit)
        .map(|message| message.to_value())
        .collect::<Vec<_>>();
    let _ = session.logout().await;
    Ok(json!({
        "action": action,
        "summary": format!("listed {} email(s) from {mailbox}", messages.len()),
        "mailbox": mailbox,
        "messages": messages,
    }))
}

async fn mark_read(config: &EmailConfig, action: &str, input: &Value) -> anyhow::Result<Value> {
    let mailbox = mailbox_from_input(input);
    let mut session = connect_and_login(config).await?;
    session
        .select(&mailbox)
        .await
        .with_context(|| format!("IMAP SELECT `{mailbox}` failed"))?;
    let uids = resolve_message_uids(&mut session, input).await?;
    let uid_set = uid_set(&uids);
    let mut stream = session
        .uid_store(&uid_set, "+FLAGS.SILENT (\\Seen)")
        .await
        .with_context(|| format!("IMAP UID STORE mark read `{uid_set}` failed"))?;
    while let Some(item) = stream.next().await {
        item.context("IMAP mark-read response error")?;
    }
    drop(stream);
    let _ = session.logout().await;
    Ok(json!({
        "action": action,
        "summary": format!("marked {} email(s) read in {mailbox}", uids.len()),
        "mailbox": mailbox,
        "uids": uids,
    }))
}

async fn delete_messages(
    config: &EmailConfig,
    action: &str,
    input: &Value,
) -> anyhow::Result<Value> {
    let mailbox = mailbox_from_input(input);
    let expunge = input
        .get("expunge")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let mut session = connect_and_login(config).await?;
    session
        .select(&mailbox)
        .await
        .with_context(|| format!("IMAP SELECT `{mailbox}` failed"))?;
    let uids = resolve_message_uids(&mut session, input).await?;
    let uid_set = uid_set(&uids);
    let mut stream = session
        .uid_store(&uid_set, "+FLAGS.SILENT (\\Deleted)")
        .await
        .with_context(|| format!("IMAP UID STORE delete `{uid_set}` failed"))?;
    while let Some(item) = stream.next().await {
        item.context("IMAP delete response error")?;
    }
    drop(stream);
    if expunge {
        let expunged = session
            .expunge()
            .await
            .context("IMAP EXPUNGE failed after delete")?;
        pin_mut!(expunged);
        while let Some(item) = expunged.next().await {
            item.context("IMAP expunge response error")?;
        }
    }
    let _ = session.logout().await;
    Ok(json!({
        "action": action,
        "summary": format!("deleted {} email(s) from {mailbox}", uids.len()),
        "mailbox": mailbox,
        "uids": uids,
        "expunged": expunge,
    }))
}

async fn draft_message(config: &EmailConfig, action: &str, input: &Value) -> anyhow::Result<Value> {
    let body = body_from_input(input);
    let subject = draft_subject(action, string_input(input, "subject").unwrap_or_default());
    let raw = raw_email_message(config, input, &subject, &body, action == "draft_reply")?;
    let mut session = connect_and_login(config).await?;
    let mut last_error = None;
    for mailbox in draft_mailbox_candidates(input) {
        match session
            .append(&mailbox, Some("(\\Draft)"), None, raw.as_bytes())
            .await
        {
            Ok(()) => {
                let _ = session.logout().await;
                return Ok(json!({
                    "action": action,
                    "summary": format!("created draft in {mailbox}"),
                    "mailbox": mailbox,
                    "bytes": raw.len(),
                }));
            }
            Err(error) => {
                last_error = Some(format!("{error}"));
            }
        }
    }
    let _ = session.logout().await;
    anyhow::bail!(
        "failed to append draft: {}",
        last_error.unwrap_or_else(|| "no draft mailbox candidates".to_string())
    )
}

async fn send_email(config: &EmailConfig, action: &str, input: &Value) -> anyhow::Result<Value> {
    let request = EmailSendRequest {
        to: address_list_input(input, "to"),
        cc: address_list_input(input, "cc"),
        bcc: address_list_input(input, "bcc"),
        subject: string_input(input, "subject").unwrap_or_default(),
        body: body_from_input(input),
        in_reply_to: string_input(input, "in_reply_to")
            .or_else(|| string_input(input, "message_id")),
        references: references_input(input),
    };
    if request.to.is_empty() {
        anyhow::bail!("send_email requires `to`");
    }
    let outcome = smtp_send::send_email(config, request).await?;
    Ok(json!({
        "action": action,
        "summary": format!("sent email to {} recipient(s)", outcome.recipients),
        "bytes": outcome.bytes,
        "recipients": outcome.recipients,
    }))
}

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
    client
        .login(&config.username, &config.password)
        .await
        .map_err(|(error, _)| anyhow!("IMAP login failed: {error}"))
}

async fn fetch_messages(
    session: &mut ImapSession,
    uids: &[u32],
) -> anyhow::Result<Vec<ListedEmail>> {
    if uids.is_empty() {
        return Ok(Vec::new());
    }
    let uid_list = uid_set(uids);
    let mut stream = session
        .uid_fetch(&uid_list, "(FLAGS RFC822)")
        .await
        .with_context(|| format!("IMAP UID FETCH `{uid_list}` failed"))?;
    let mut messages = Vec::new();
    while let Some(item) = stream.next().await {
        let fetch = item.context("IMAP fetch item error")?;
        let Some(uid) = fetch.uid else {
            continue;
        };
        let flags = fetch.flags().collect::<Vec<_>>();
        let unread = !flags.iter().any(|flag| matches!(flag, Flag::Seen));
        let flag_labels = flags
            .iter()
            .map(|flag| flag_label(flag))
            .collect::<Vec<_>>();
        let Some(body) = fetch.body() else {
            continue;
        };
        if let Some(parsed) = ListedEmail::from_raw(uid, unread, flag_labels, body) {
            messages.push(parsed);
        }
    }
    Ok(messages)
}

async fn resolve_message_uids(
    session: &mut ImapSession,
    input: &Value,
) -> anyhow::Result<Vec<u32>> {
    let mut uids = uid_inputs(input)?;
    if uids.is_empty() {
        if let Some(message_id) = string_input(input, "message_id") {
            let query = format!("HEADER Message-ID {}", imap_quoted(&message_id));
            uids = session
                .uid_search(&query)
                .await
                .with_context(|| format!("IMAP UID SEARCH `{query}` failed"))?
                .into_iter()
                .collect();
        }
    }
    uids.sort_unstable();
    uids.dedup();
    if uids.is_empty() {
        anyhow::bail!("email action requires `uid`, `uids`, `id`, or `message_id`");
    }
    Ok(uids)
}

fn uid_inputs(input: &Value) -> anyhow::Result<Vec<u32>> {
    let mut out = Vec::new();
    for key in ["uid", "uids", "id"] {
        let Some(value) = input.get(key) else {
            continue;
        };
        collect_uids(value, &mut out)?;
    }
    Ok(out)
}

fn collect_uids(value: &Value, out: &mut Vec<u32>) -> anyhow::Result<()> {
    if value.is_null() {
        return Ok(());
    }
    if let Some(items) = value.as_array() {
        for item in items {
            collect_uids(item, out)?;
        }
        return Ok(());
    }
    if let Some(uid) = value.as_u64() {
        out.push(u32::try_from(uid).context("email uid is outside u32 range")?);
        return Ok(());
    }
    if let Some(uid) = value.as_str() {
        let trimmed = uid.trim();
        if !trimmed.is_empty() {
            out.push(
                trimmed
                    .parse::<u32>()
                    .with_context(|| format!("invalid email uid `{trimmed}`"))?,
            );
        }
        return Ok(());
    }
    anyhow::bail!("email uid must be an integer, string, or array")
}

fn uid_set(uids: &[u32]) -> String {
    uids.iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

fn mailbox_from_input(input: &Value) -> String {
    string_input(input, "mailbox")
        .or_else(|| string_input(input, "category"))
        .or_else(|| string_input(input, "label"))
        .map(|value| match value.trim().to_ascii_lowercase().as_str() {
            "" | "inbox" | "primary" => "INBOX".to_string(),
            "draft" | "drafts" => "Drafts".to_string(),
            "sent" | "sent mail" => "Sent".to_string(),
            "spam" | "junk" => "Spam".to_string(),
            "trash" | "bin" => "Trash".to_string(),
            _ => value,
        })
        .unwrap_or_else(|| "INBOX".to_string())
}

fn draft_mailbox_candidates(input: &Value) -> Vec<String> {
    let first = string_input(input, "mailbox")
        .or_else(|| string_input(input, "category"))
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "Drafts".to_string());
    let mut candidates = vec![first, "Drafts".to_string(), "[Gmail]/Drafts".to_string()];
    candidates.dedup();
    candidates
}

fn draft_subject(action: &str, subject: String) -> String {
    let trimmed = subject.trim();
    if action == "draft_forward" {
        if trimmed.to_ascii_lowercase().starts_with("fwd:")
            || trimmed.to_ascii_lowercase().starts_with("fw:")
        {
            return trimmed.to_string();
        }
        return format!("Fwd: {trimmed}");
    }
    if trimmed.to_ascii_lowercase().starts_with("re:") {
        return trimmed.to_string();
    }
    format!("Re: {trimmed}")
}

fn raw_email_message(
    config: &EmailConfig,
    input: &Value,
    subject: &str,
    body: &str,
    is_reply: bool,
) -> anyhow::Result<String> {
    let from = clean_header_value(&config.from_address);
    let to = address_list_input(input, "to").join(", ");
    let cc = address_list_input(input, "cc").join(", ");
    let bcc = address_list_input(input, "bcc").join(", ");
    let mut raw = String::new();
    raw.push_str(&format!("From: {from}\r\n"));
    if !to.is_empty() {
        raw.push_str(&format!("To: {}\r\n", clean_header_value(&to)));
    }
    if !cc.is_empty() {
        raw.push_str(&format!("Cc: {}\r\n", clean_header_value(&cc)));
    }
    if !bcc.is_empty() {
        raw.push_str(&format!("Bcc: {}\r\n", clean_header_value(&bcc)));
    }
    raw.push_str(&format!("Subject: {}\r\n", clean_header_value(subject)));
    raw.push_str(&format!("Date: {}\r\n", rfc2822_now()?));
    if is_reply {
        if let Some(in_reply_to) =
            string_input(input, "in_reply_to").or_else(|| string_input(input, "message_id"))
        {
            raw.push_str(&format!(
                "In-Reply-To: {}\r\n",
                clean_header_value(&in_reply_to)
            ));
        }
        let references = references_input(input);
        if !references.is_empty() {
            raw.push_str(&format!(
                "References: {}\r\n",
                clean_header_value(&references.join(" "))
            ));
        }
    }
    raw.push_str("MIME-Version: 1.0\r\n");
    raw.push_str("Content-Type: text/plain; charset=utf-8\r\n");
    raw.push_str("Content-Transfer-Encoding: 8bit\r\n");
    raw.push_str("\r\n");
    raw.push_str(body);
    Ok(raw)
}

fn rfc2822_now() -> anyhow::Result<String> {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc2822)
        .context("format email date")
}

fn body_from_input(input: &Value) -> String {
    string_input(input, "body")
        .or_else(|| string_input(input, "text"))
        .or_else(|| string_input(input, "message"))
        .unwrap_or_default()
}

fn string_input(input: &Value, key: &str) -> Option<String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn integer_input(input: &Value, key: &str) -> Option<u64> {
    input
        .get(key)
        .and_then(|value| value.as_u64().or_else(|| value.as_str()?.parse().ok()))
}

fn address_list_input(input: &Value, key: &str) -> Vec<String> {
    let Some(value) = input.get(key) else {
        return Vec::new();
    };
    string_list(value)
}

fn references_input(input: &Value) -> Vec<String> {
    input
        .get("references")
        .map(string_list)
        .unwrap_or_else(Vec::new)
}

fn string_list(value: &Value) -> Vec<String> {
    if let Some(items) = value.as_array() {
        return items
            .iter()
            .filter_map(Value::as_str)
            .flat_map(split_addressish)
            .collect();
    }
    value.as_str().map(split_addressish).unwrap_or_default()
}

fn split_addressish(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn clean_header_value(value: &str) -> String {
    value.replace(['\r', '\n'], " ").trim().to_string()
}

fn imap_quoted(value: &str) -> String {
    format!(
        "\"{}\"",
        value.replace('\\', "\\\\").replace('"', "\\\"").trim()
    )
}

fn flag_label(flag: &Flag<'_>) -> String {
    match flag {
        Flag::Seen => "\\Seen".to_string(),
        Flag::Answered => "\\Answered".to_string(),
        Flag::Flagged => "\\Flagged".to_string(),
        Flag::Deleted => "\\Deleted".to_string(),
        Flag::Draft => "\\Draft".to_string(),
        Flag::Recent => "\\Recent".to_string(),
        Flag::MayCreate => "\\*".to_string(),
        Flag::Custom(value) => value.to_string(),
    }
}

#[derive(Default)]
struct EmailFilters {
    from: Option<String>,
    to: Option<String>,
    subject: Option<String>,
    keywords: Vec<String>,
}

impl EmailFilters {
    fn from_input(input: &Value) -> Self {
        let mut keywords = string_input(input, "query")
            .map(|value| vec![value])
            .unwrap_or_default();
        if let Some(value) = input.get("keywords") {
            keywords.extend(string_list(value));
        }
        Self {
            from: string_input(input, "from").map(|value| value.to_ascii_lowercase()),
            to: string_input(input, "to").map(|value| value.to_ascii_lowercase()),
            subject: string_input(input, "subject").map(|value| value.to_ascii_lowercase()),
            keywords: keywords
                .into_iter()
                .map(|value| value.to_ascii_lowercase())
                .filter(|value| !value.is_empty())
                .collect(),
        }
    }

    fn matches(&self, email: &ListedEmail) -> bool {
        if let Some(expected) = &self.from {
            if !email.from.to_ascii_lowercase().contains(expected) {
                return false;
            }
        }
        if let Some(expected) = &self.to {
            if !email.to.join(",").to_ascii_lowercase().contains(expected) {
                return false;
            }
        }
        if let Some(expected) = &self.subject {
            if !email.subject.to_ascii_lowercase().contains(expected) {
                return false;
            }
        }
        let haystack = format!(
            "{}\n{}\n{}\n{}\n{}",
            email.from,
            email.to.join(","),
            email.subject,
            email.body_preview,
            email.message_id.clone().unwrap_or_default()
        )
        .to_ascii_lowercase();
        self.keywords
            .iter()
            .all(|keyword| haystack.contains(keyword))
    }
}

struct ListedEmail {
    uid: u32,
    from: String,
    to: Vec<String>,
    subject: String,
    body_preview: String,
    message_id: Option<String>,
    thread_id: String,
    date_ms: Option<i64>,
    unread: bool,
    flags: Vec<String>,
}

impl ListedEmail {
    fn from_raw(uid: u32, unread: bool, flags: Vec<String>, raw: &[u8]) -> Option<Self> {
        let parsed = MessageParser::default().parse(raw)?;
        let from = parsed
            .from()
            .and_then(|addrs| addrs.first())
            .and_then(|addr| addr.address())
            .unwrap_or("")
            .to_string();
        let to = parsed
            .to()
            .map(|addrs| {
                addrs
                    .iter()
                    .filter_map(|addr| addr.address().map(ToString::to_string))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let subject = parsed.subject().unwrap_or("").to_string();
        let body = parsed
            .body_text(0)
            .map(|value| value.into_owned())
            .unwrap_or_default();
        let message_id = parsed.message_id().map(ToString::to_string);
        let thread_id = parsed
            .references()
            .as_text_list()
            .and_then(|list| list.first().map(ToString::to_string))
            .or_else(|| parsed.in_reply_to().as_text().map(ToString::to_string))
            .or_else(|| message_id.clone())
            .unwrap_or_else(|| from.clone());
        let date_ms = parsed.date().map(|date| date.to_timestamp() * 1000);
        Some(Self {
            uid,
            from,
            to,
            subject,
            body_preview: body.chars().take(1024).collect(),
            message_id,
            thread_id,
            date_ms,
            unread,
            flags,
        })
    }

    fn to_value(&self) -> Value {
        let mut payload = Map::new();
        payload.insert("uid".to_string(), json!(self.uid));
        payload.insert("from".to_string(), json!(self.from));
        payload.insert("to".to_string(), json!(self.to));
        payload.insert("subject".to_string(), json!(self.subject));
        payload.insert("body_preview".to_string(), json!(self.body_preview));
        payload.insert("thread_id".to_string(), json!(self.thread_id));
        payload.insert("unread".to_string(), json!(self.unread));
        payload.insert("flags".to_string(), json!(self.flags));
        if let Some(message_id) = &self.message_id {
            payload.insert("message_id".to_string(), json!(message_id));
        }
        if let Some(date_ms) = self.date_ms {
            payload.insert("date_ms".to_string(), json!(date_ms));
        }
        Value::Object(payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_uid_inputs_from_scalar_and_array() {
        assert_eq!(
            uid_inputs(&json!({"uid": "42", "uids": [7, "8"]})).unwrap(),
            vec![42, 7, 8]
        );
    }

    #[test]
    fn maps_default_mailbox_and_categories() {
        assert_eq!(mailbox_from_input(&json!({})), "INBOX");
        assert_eq!(mailbox_from_input(&json!({"category": "primary"})), "INBOX");
        assert_eq!(mailbox_from_input(&json!({"category": "drafts"})), "Drafts");
        assert_eq!(
            mailbox_from_input(&json!({"label": "[Gmail]/All Mail"})),
            "[Gmail]/All Mail"
        );
    }

    #[test]
    fn filters_email_by_keywords_and_sender() {
        let email = ListedEmail {
            uid: 1,
            from: "alice@example.com".to_string(),
            to: vec!["me@example.com".to_string()],
            subject: "Quarterly plan".to_string(),
            body_preview: "Budget and hiring".to_string(),
            message_id: Some("<m1>".to_string()),
            thread_id: "<m1>".to_string(),
            date_ms: None,
            unread: true,
            flags: Vec::new(),
        };
        let filters = EmailFilters::from_input(&json!({
            "from": "alice",
            "keywords": ["hiring"]
        }));
        assert!(filters.matches(&email));
        let filters = EmailFilters::from_input(&json!({"keywords": ["missing"]}));
        assert!(!filters.matches(&email));
    }
}

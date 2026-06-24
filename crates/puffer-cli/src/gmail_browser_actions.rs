//! Connector action helpers for the Gmail-browser subscriber.

#[path = "gmail_browser_draft.rs"]
mod gmail_browser_draft;

use anyhow::{Context, Result};
use gmail_browser_draft::{
    draft_rows_contain, gmail_reply_draft_script, gmail_reply_draft_verify_script,
    gmail_save_draft_script,
};
use serde_json::{json, Value};
use std::time::Instant;

use super::{
    ensure_browser_daemon, poll_account_at_url, safe_session_part, GmailBrowserConfig,
    SubscriberEnv, BROWSER_HEIGHT, BROWSER_WIDTH, GMAIL_EVALUATE_INTERVAL, GMAIL_LOAD_TIMEOUT,
};

/// Executes one Gmail-browser connector action through the managed Chrome profile.
pub(super) fn handle_action(
    env: &SubscriberEnv,
    config: &GmailBrowserConfig,
    handshake: &mut Option<crate::daemon::Handshake>,
    action: &str,
    input: &Value,
) -> Result<Value> {
    match canonical_gmail_action(action) {
        "list_emails" => gmail_list_emails(env, config, handshake, action, input),
        "mark_read" => gmail_mark_read(env, config, handshake, action, input),
        "delete" => gmail_delete(env, config, handshake, action, input),
        "draft_reply" | "draft_forward" => gmail_draft(env, config, handshake, action, input),
        "send_email" => gmail_send_email(env, config, handshake, action, input),
        other => anyhow::bail!("unsupported gmail-browser action `{other}`"),
    }
}

fn canonical_gmail_action(action: &str) -> &str {
    match action {
        "list_inbox" | "list_category" | "search_emails" => "list_emails",
        other => other,
    }
}

fn gmail_list_emails(
    env: &SubscriberEnv,
    config: &GmailBrowserConfig,
    handshake: &mut Option<crate::daemon::Handshake>,
    action: &str,
    input: &Value,
) -> Result<Value> {
    let account = gmail_action_account(config, input)?;
    let url = gmail_collection_url(&account, input);
    let handshake_ref = ensure_browser_daemon(config, handshake)?;
    let result = poll_account_at_url(env, &account, handshake_ref, &url)?;
    ensure_gmail_action_ready(&account, &result)?;
    let limit = integer_input(input, "limit").unwrap_or(30).clamp(1, 100) as usize;
    let filters = GmailRowFilters::from_input(input);
    let rows = result
        .get("rows")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|row| filters.matches(row))
        .take(limit)
        .collect::<Vec<_>>();
    Ok(json!({
        "action": action,
        "summary": format!("listed {} Gmail email(s) for {account}", rows.len()),
        "account": account,
        "url": url,
        "messages": rows,
    }))
}

fn gmail_mark_read(
    env: &SubscriberEnv,
    config: &GmailBrowserConfig,
    handshake: &mut Option<crate::daemon::Handshake>,
    action: &str,
    input: &Value,
) -> Result<Value> {
    let account = gmail_action_account(config, input)?;
    let thread_id = gmail_thread_id(input)?;
    let url = gmail_thread_url(&account, input, &thread_id);
    let handshake_ref = ensure_browser_daemon(config, handshake)?;
    open_gmail_url(env, &account, handshake_ref, &url)?;
    wait_gmail_thread_ready(env, &account, handshake_ref, &thread_id)?;
    Ok(json!({
        "action": action,
        "summary": format!("opened Gmail thread {thread_id} for {account} to mark it read"),
        "account": account,
        "thread_id": thread_id,
        "url": url,
    }))
}

fn gmail_delete(
    env: &SubscriberEnv,
    config: &GmailBrowserConfig,
    handshake: &mut Option<crate::daemon::Handshake>,
    action: &str,
    input: &Value,
) -> Result<Value> {
    let account = gmail_action_account(config, input)?;
    let thread_id = gmail_thread_id(input)?;
    let url = gmail_thread_url(&account, input, &thread_id);
    let handshake_ref = ensure_browser_daemon(config, handshake)?;
    open_gmail_url(env, &account, handshake_ref, &url)?;
    wait_gmail_thread_ready(env, &account, handshake_ref, &thread_id)?;
    let click = evaluate_gmail_script(env, &account, handshake_ref, GMAIL_DELETE_SCRIPT)?;
    if !click.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        anyhow::bail!(
            "Gmail delete button was not found for thread `{thread_id}`: {}",
            click
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
        );
    }
    Ok(json!({
        "action": action,
        "summary": format!("deleted Gmail thread {thread_id} for {account}"),
        "account": account,
        "thread_id": thread_id,
        "url": url,
    }))
}

fn gmail_draft(
    env: &SubscriberEnv,
    config: &GmailBrowserConfig,
    handshake: &mut Option<crate::daemon::Handshake>,
    action: &str,
    input: &Value,
) -> Result<Value> {
    let fields = GmailComposeFields::from_input(action, input);
    if fields.is_empty_draft() {
        anyhow::bail!("draft actions require at least one recipient, subject, or body field");
    }
    let account = gmail_action_account(config, input)?;
    if action == "draft_reply" {
        if let Some(thread_id) = optional_gmail_thread_id(input) {
            return gmail_thread_reply_draft(
                env, config, handshake, action, input, fields, thread_id,
            );
        }
    }
    let url = gmail_compose_url_for_fields(&account, &fields);
    let handshake_ref = ensure_browser_daemon(config, handshake)?;
    open_gmail_url(env, &account, handshake_ref, &url)?;
    wait_gmail_ready(env, &account, handshake_ref)?;
    let save = evaluate_gmail_script(
        env,
        &account,
        handshake_ref,
        &gmail_save_draft_script(&fields),
    )?;
    if !save.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        anyhow::bail!(
            "Gmail draft was not prepared: {}",
            save.get("reason")
                .and_then(Value::as_str)
                .or_else(|| save.get("status").and_then(Value::as_str))
                .unwrap_or("unknown")
        );
    }
    std::thread::sleep(GMAIL_EVALUATE_INTERVAL);
    let drafts_url = gmail_drafts_url(&account);
    let drafts = poll_account_at_url(env, &account, handshake_ref, &drafts_url)?;
    ensure_gmail_action_not_auth_blocked(&account, &drafts)?;
    if !draft_rows_contain(&fields, &drafts) {
        let status = drafts
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        anyhow::bail!("Gmail draft was prepared but was not visible in Drafts; status `{status}`");
    }
    Ok(json!({
        "action": action,
        "summary": format!("saved Gmail draft for {account}"),
        "account": account,
        "url": url,
        "drafts_url": drafts_url,
        "save": save,
    }))
}

fn gmail_thread_reply_draft(
    env: &SubscriberEnv,
    config: &GmailBrowserConfig,
    handshake: &mut Option<crate::daemon::Handshake>,
    action: &str,
    input: &Value,
    fields: GmailComposeFields,
    thread_id: String,
) -> Result<Value> {
    let account = gmail_action_account(config, input)?;
    let url = gmail_thread_url(&account, input, &thread_id);
    let handshake_ref = ensure_browser_daemon(config, handshake)?;
    open_gmail_url(env, &account, handshake_ref, &url)?;
    wait_gmail_thread_ready(env, &account, handshake_ref, &thread_id)?;
    let save = evaluate_gmail_script(
        env,
        &account,
        handshake_ref,
        &gmail_reply_draft_script(&fields),
    )?;
    if !save.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        anyhow::bail!(
            "Gmail reply draft was not prepared: {}",
            save.get("reason")
                .and_then(Value::as_str)
                .or_else(|| save.get("status").and_then(Value::as_str))
                .unwrap_or("unknown")
        );
    }
    let verification = verify_gmail_reply_draft_in_thread(
        env,
        &account,
        handshake_ref,
        &fields,
        &thread_id,
        &url,
    )?;
    Ok(json!({
        "action": action,
        "summary": format!("saved Gmail reply draft for {account}"),
        "account": account,
        "thread_id": thread_id,
        "url": url,
        "verification_url": verification.url,
        "verification": {
            "matched": true,
            "method": "thread_composer",
            "status": verification.status,
        },
        "save": save,
    }))
}

struct GmailDraftVerification {
    url: String,
    status: String,
}

fn verify_gmail_reply_draft_in_thread(
    env: &SubscriberEnv,
    account: &str,
    handshake: &crate::daemon::Handshake,
    fields: &GmailComposeFields,
    thread_id: &str,
    thread_url: &str,
) -> Result<GmailDraftVerification> {
    // Verify persistence, not just the in-memory composer we just populated.
    let neutral_url = format!("{}#inbox", gmail_base_url(account));
    open_gmail_url(env, account, handshake, &neutral_url)?;
    wait_gmail_ready(env, account, handshake)?;
    open_gmail_url(env, account, handshake, thread_url)?;
    wait_gmail_thread_ready(env, account, handshake, thread_id)?;
    let verification = evaluate_gmail_script(
        env,
        account,
        handshake,
        &gmail_reply_draft_verify_script(fields),
    )?;
    ensure_gmail_action_not_auth_blocked(account, &verification)?;
    if !verification
        .get("ok")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let status = verification
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let reason = verification
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let href = verification
            .get("href")
            .and_then(Value::as_str)
            .unwrap_or(thread_url);
        anyhow::bail!(
            "Gmail reply draft was prepared but was not visible in the thread composer for thread `{thread_id}`; last URL `{href}`, status `{status}`, reason `{reason}`"
        );
    }
    Ok(GmailDraftVerification {
        url: verification
            .get("href")
            .and_then(Value::as_str)
            .unwrap_or(thread_url)
            .to_string(),
        status: verification
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
    })
}

fn gmail_send_email(
    env: &SubscriberEnv,
    config: &GmailBrowserConfig,
    handshake: &mut Option<crate::daemon::Handshake>,
    action: &str,
    input: &Value,
) -> Result<Value> {
    if address_list_input(input, "to").is_empty() {
        anyhow::bail!("send_email requires `to`");
    }
    let account = gmail_action_account(config, input)?;
    let url = gmail_compose_url(&account, action, input);
    let handshake_ref = ensure_browser_daemon(config, handshake)?;
    open_gmail_url(env, &account, handshake_ref, &url)?;
    wait_gmail_ready(env, &account, handshake_ref)?;
    let click = evaluate_gmail_script(env, &account, handshake_ref, GMAIL_SEND_SCRIPT)?;
    if !click.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        anyhow::bail!(
            "Gmail send button was not found: {}",
            click
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
        );
    }
    Ok(json!({
        "action": action,
        "summary": format!("sent Gmail email for {account}"),
        "account": account,
        "url": url,
    }))
}

fn open_gmail_url(
    env: &SubscriberEnv,
    account: &str,
    handshake: &crate::daemon::Handshake,
    url: &str,
) -> Result<()> {
    crate::daemon_browser::send_daemon_request(
        handshake,
        "browser_agent",
        json!({
            "action": "open",
            "sessionId": format!("gmail-browser-{}", safe_session_part(&env.topic)),
            "tabId": safe_session_part(account),
            "label": format!("Gmail {account}"),
            "url": url,
            "width": BROWSER_WIDTH,
            "height": BROWSER_HEIGHT,
            "activate": false,
        }),
    )
    .context("open Gmail browser tab")?;
    Ok(())
}

fn evaluate_gmail_script(
    env: &SubscriberEnv,
    account: &str,
    handshake: &crate::daemon::Handshake,
    script: &str,
) -> Result<Value> {
    let root_session = format!("gmail-browser-{}", safe_session_part(&env.topic));
    let deadline = Instant::now() + GMAIL_LOAD_TIMEOUT;
    loop {
        let value = crate::daemon_browser::send_daemon_request(
            handshake,
            "browser_agent",
            json!({
                "action": "evaluate",
                "sessionId": root_session,
                "tabId": safe_session_part(account),
                "width": BROWSER_WIDTH,
                "height": BROWSER_HEIGHT,
                "script": script,
            }),
        )
        .context("evaluate Gmail action script")?;
        let result = value.get("value").cloned().unwrap_or(Value::Null);
        if result.get("ok").and_then(Value::as_bool).unwrap_or(false) || Instant::now() >= deadline
        {
            return Ok(result);
        }
        std::thread::sleep(GMAIL_EVALUATE_INTERVAL);
    }
}

fn wait_gmail_ready(
    env: &SubscriberEnv,
    account: &str,
    handshake: &crate::daemon::Handshake,
) -> Result<Value> {
    let result = evaluate_gmail_script(env, account, handshake, GMAIL_READY_SCRIPT)?;
    ensure_gmail_action_ready(account, &result)?;
    if !result.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        anyhow::bail!("Gmail page did not become ready for account `{account}`");
    }
    Ok(result)
}

fn wait_gmail_thread_ready(
    env: &SubscriberEnv,
    account: &str,
    handshake: &crate::daemon::Handshake,
    thread_id: &str,
) -> Result<Value> {
    let result = evaluate_gmail_script(
        env,
        account,
        handshake,
        &gmail_thread_ready_script(thread_id),
    )?;
    ensure_gmail_action_ready(account, &result)?;
    if !result.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        anyhow::bail!("Gmail thread `{thread_id}` did not become ready for account `{account}`");
    }
    Ok(result)
}

fn ensure_gmail_action_ready(account: &str, result: &Value) -> Result<()> {
    ensure_gmail_action_not_auth_blocked(account, result)?;
    match result.get("status").and_then(Value::as_str).unwrap_or("ok") {
        "ok" => Ok(()),
        status => anyhow::bail!("Gmail account `{account}` returned status `{status}`"),
    }
}

fn ensure_gmail_action_not_auth_blocked(account: &str, result: &Value) -> Result<()> {
    if result.get("status").and_then(Value::as_str) == Some("auth_required") {
        anyhow::bail!(
            "Gmail account `{account}` is not signed in inside the global Puffer browser profile"
        );
    }
    Ok(())
}

fn gmail_action_account(config: &GmailBrowserConfig, input: &Value) -> Result<String> {
    let requested = string_input(input, "account").or_else(|| string_input(input, "account_slug"));
    if let Some(account) = requested {
        let normalized = account.trim().to_ascii_lowercase();
        if config.accounts.iter().any(|known| known == &normalized) {
            return Ok(normalized);
        }
        anyhow::bail!("Gmail account `{account}` is not configured for this connector");
    }
    config
        .accounts
        .first()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("gmail-browser connector has no configured accounts"))
}

fn gmail_thread_id(input: &Value) -> Result<String> {
    optional_gmail_thread_id(input).ok_or_else(|| {
        anyhow::anyhow!(
            "Gmail action requires `thread_id`, `gmail_thread_id`, `message_id`, or `id`"
        )
    })
}

fn optional_gmail_thread_id(input: &Value) -> Option<String> {
    string_input(input, "thread_id")
        .or_else(|| string_input(input, "gmail_thread_id"))
        .or_else(|| string_input(input, "message_id"))
        .or_else(|| string_input(input, "id"))
        .filter(|value| !value.trim().is_empty())
}

fn gmail_collection_url(account: &str, input: &Value) -> String {
    let base = gmail_base_url(account);
    let keywords = keywords_input(input);
    if let Some(query) = string_input(input, "query").filter(|value| !value.trim().is_empty()) {
        return format!("{base}#search/{}", url_fragment(&query));
    }
    if !keywords.is_empty() {
        return format!("{base}#search/{}", url_fragment(&keywords.join(" ")));
    }
    if let Some(label) = string_input(input, "label").filter(|value| !value.trim().is_empty()) {
        return format!("{base}#label/{}", url_fragment(&label));
    }
    let category = string_input(input, "category")
        .or_else(|| string_input(input, "mailbox"))
        .unwrap_or_else(|| "inbox".to_string());
    let fragment = match category.trim().to_ascii_lowercase().as_str() {
        "" | "inbox" | "primary" => "inbox".to_string(),
        "promotions" | "social" | "updates" | "forums" => {
            format!("category/{}", category.trim().to_ascii_lowercase())
        }
        "sent" | "sent mail" => "sent".to_string(),
        "draft" | "drafts" => "drafts".to_string(),
        "spam" | "junk" => "spam".to_string(),
        "trash" | "bin" => "trash".to_string(),
        "all" | "all mail" => "all".to_string(),
        other => format!("label/{}", url_fragment(other)),
    };
    format!("{base}#{fragment}")
}

fn gmail_thread_url(account: &str, input: &Value, thread_id: &str) -> String {
    if let Some(url) =
        string_input(input, "url").filter(|url| gmail_url_targets_thread(url, thread_id))
    {
        return url;
    }
    let collection = string_input(input, "category")
        .or_else(|| string_input(input, "mailbox"))
        .unwrap_or_else(|| "inbox".to_string());
    let fragment = match collection.trim().to_ascii_lowercase().as_str() {
        "" | "inbox" | "primary" => "inbox".to_string(),
        "all" | "all mail" => "all".to_string(),
        "sent" | "sent mail" => "sent".to_string(),
        "draft" | "drafts" => "drafts".to_string(),
        "trash" | "bin" => "trash".to_string(),
        "spam" | "junk" => "spam".to_string(),
        other => other.to_string(),
    };
    format!(
        "{}#{}/{}",
        gmail_base_url(account),
        url_fragment(&fragment),
        url_fragment(thread_id)
    )
}

fn gmail_url_targets_thread(url: &str, thread_id: &str) -> bool {
    let normalized_url = url.to_ascii_lowercase();
    let normalized_thread_id = thread_id.trim().to_ascii_lowercase();
    if !normalized_thread_id.is_empty() && normalized_url.contains(&normalized_thread_id) {
        return true;
    }
    let Some((_, hash)) = url.split_once('#') else {
        return false;
    };
    let last_segment = hash
        .trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if last_segment.is_empty() {
        return false;
    }
    last_segment.starts_with("fmfc")
        || last_segment.starts_with("thread-a:")
        || last_segment.starts_with("thread-f:")
        || (last_segment.len() >= 12 && last_segment.chars().all(|c| c.is_ascii_hexdigit()))
}

fn gmail_compose_url(account: &str, action: &str, input: &Value) -> String {
    let fields = GmailComposeFields::from_input(action, input);
    gmail_compose_url_for_fields(account, &fields)
}

fn gmail_compose_url_for_fields(account: &str, fields: &GmailComposeFields) -> String {
    let mut pairs = vec![
        ("authuser".to_string(), account.to_string()),
        ("view".to_string(), "cm".to_string()),
        ("fs".to_string(), "1".to_string()),
        ("tf".to_string(), "1".to_string()),
    ];
    let to = fields.to.join(",");
    let cc = fields.cc.join(",");
    let bcc = fields.bcc.join(",");
    if !to.is_empty() {
        pairs.push(("to".to_string(), to));
    }
    if !cc.is_empty() {
        pairs.push(("cc".to_string(), cc));
    }
    if !bcc.is_empty() {
        pairs.push(("bcc".to_string(), bcc));
    }
    if !fields.subject.trim().is_empty() {
        pairs.push(("su".to_string(), fields.subject.clone()));
    }
    if !fields.body.trim().is_empty() {
        pairs.push(("body".to_string(), fields.body.clone()));
    }
    format!(
        "https://mail.google.com/mail/?{}",
        url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(
                pairs
                    .iter()
                    .map(|(key, value)| (key.as_str(), value.as_str()))
            )
            .finish()
    )
}

struct GmailComposeFields {
    to: Vec<String>,
    cc: Vec<String>,
    bcc: Vec<String>,
    subject: String,
    body: String,
}

impl GmailComposeFields {
    fn from_input(action: &str, input: &Value) -> Self {
        Self {
            to: address_list_input(input, "to"),
            cc: address_list_input(input, "cc"),
            bcc: address_list_input(input, "bcc"),
            subject: draft_subject(action, string_input(input, "subject").unwrap_or_default()),
            body: body_input(input),
        }
    }

    fn is_empty_draft(&self) -> bool {
        self.to.is_empty()
            && self.cc.is_empty()
            && self.bcc.is_empty()
            && self.subject.trim().is_empty()
            && self.body.trim().is_empty()
    }
}

fn gmail_base_url(account: &str) -> String {
    let encoded = url::form_urlencoded::byte_serialize(account.as_bytes()).collect::<String>();
    format!("https://mail.google.com/mail/?authuser={encoded}")
}

fn gmail_drafts_url(account: &str) -> String {
    format!("{}#drafts", gmail_base_url(account))
}

fn url_fragment(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect::<String>()
}

fn draft_subject(action: &str, subject: String) -> String {
    let trimmed = subject.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if action == "draft_forward" {
        if trimmed.to_ascii_lowercase().starts_with("fwd:")
            || trimmed.to_ascii_lowercase().starts_with("fw:")
        {
            return trimmed.to_string();
        }
        return format!("Fwd: {trimmed}");
    }
    if action == "draft_reply" && !trimmed.to_ascii_lowercase().starts_with("re:") {
        return format!("Re: {trimmed}");
    }
    trimmed.to_string()
}

fn body_input(input: &Value) -> String {
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
    if let Some(items) = value.as_array() {
        return items
            .iter()
            .filter_map(Value::as_str)
            .flat_map(split_csv)
            .collect();
    }
    value.as_str().map(split_csv).unwrap_or_default()
}

fn keywords_input(input: &Value) -> Vec<String> {
    let mut keywords = Vec::new();
    if let Some(value) = input.get("keywords") {
        if let Some(items) = value.as_array() {
            keywords.extend(
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToString::to_string),
            );
        } else if let Some(keyword) = value
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            keywords.push(keyword.to_string());
        }
    }
    keywords
}

fn split_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(ToString::to_string)
        .collect()
}

struct GmailRowFilters {
    from: Option<String>,
    subject: Option<String>,
    unread: Option<bool>,
    keywords: Vec<String>,
}

impl GmailRowFilters {
    fn from_input(input: &Value) -> Self {
        let mut keywords = keywords_input(input);
        if let Some(query) = string_input(input, "query") {
            keywords.push(query);
        }
        Self {
            from: string_input(input, "from").map(|value| value.to_ascii_lowercase()),
            subject: string_input(input, "subject").map(|value| value.to_ascii_lowercase()),
            unread: input.get("unread").and_then(Value::as_bool),
            keywords: keywords
                .into_iter()
                .map(|value| value.to_ascii_lowercase())
                .collect(),
        }
    }

    fn matches(&self, row: &Value) -> bool {
        if let Some(unread) = self.unread {
            if row.get("unread").and_then(Value::as_bool).unwrap_or(false) != unread {
                return false;
            }
        }
        let from = [
            row.get("sender").and_then(Value::as_str).unwrap_or(""),
            row.get("fromEmail").and_then(Value::as_str).unwrap_or(""),
        ]
        .join(" ")
        .to_ascii_lowercase();
        if let Some(expected) = &self.from {
            if !from.contains(expected) {
                return false;
            }
        }
        let subject = row
            .get("subject")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_ascii_lowercase();
        if let Some(expected) = &self.subject {
            if !subject.contains(expected) {
                return false;
            }
        }
        let haystack = format!(
            "{}\n{}\n{}",
            from,
            subject,
            row.get("snippet").and_then(Value::as_str).unwrap_or("")
        )
        .to_ascii_lowercase();
        self.keywords
            .iter()
            .all(|keyword| haystack.contains(keyword))
    }
}

const GMAIL_READY_SCRIPT: &str = r#"
(() => {
  const href = location.href;
  const title = document.title || "";
  const bodyText = document.body ? document.body.innerText || "" : "";
  const host = location.hostname || "";
  const signinLike =
    host.includes("accounts.google.com") ||
    /ServiceLogin|signin|identifier/.test(href) ||
    (/sign in/i.test(title) && !/gmail/i.test(title));
  if (signinLike) {
    return { ok: false, status: "auth_required", href, title };
  }
  const ready =
    document.querySelector('[role="main"]') ||
    document.querySelector('div[gh]') ||
    document.querySelector('.nH') ||
    /gmail/i.test(title);
  if (!ready) {
    return { ok: false, status: "loading", href, title, bodyText: bodyText.slice(0, 200) };
  }
  return { ok: true, status: "ok", href, title };
})()
"#;

fn gmail_thread_ready_script(thread_id: &str) -> String {
    let expected = json!(thread_id).to_string();
    format!(
        r#"
(() => {{
  const expected = {expected};
  const href = location.href;
  const hash = location.hash || "";
  const title = document.title || "";
  const bodyText = document.body ? document.body.innerText || "" : "";
  const host = location.hostname || "";
  const signinLike =
    host.includes("accounts.google.com") ||
    /ServiceLogin|signin|identifier/.test(href) ||
    (/sign in/i.test(title) && !/gmail/i.test(title));
  if (signinLike) {{
    return {{ ok: false, status: "auth_required", href, title }};
  }}
  const visible = (node) => {{
    if (!node) return false;
    const rect = node.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0;
  }};
  const label = (node) => [
    node.getAttribute("aria-label") || "",
    node.getAttribute("data-tooltip") || "",
    node.getAttribute("title") || "",
    node.textContent || ""
  ].join(" ").trim();
  const hasThreadHash = /\/[^/]+$/.test(hash) || hash.includes(expected);
  const replyButton = Array.from(document.querySelectorAll('[aria-label], [data-tooltip], [role="button"]'))
    .filter(visible)
    .some((node) => /^reply\b/i.test(label(node)) && !/reply all/i.test(label(node)));
  const draftBody = Array.from(document.querySelectorAll('[contenteditable="true"], textarea'))
    .filter(visible)
    .some((node) =>
      /message body|body/i.test(label(node)) ||
      (node.getAttribute("g_editable") === "true" && node.getAttribute("role") === "textbox")
    );
  const threadChrome = /Print all|In new window/i.test(bodyText);
  if (hasThreadHash && (replyButton || draftBody || threadChrome)) {{
    return {{ ok: true, status: "ok", href, title }};
  }}
  return {{
    ok: false,
    status: "loading",
    href,
    title,
    hash,
    bodyText: bodyText.slice(0, 200)
  }};
}})()
"#
    )
}

const GMAIL_DELETE_SCRIPT: &str = r#"
(() => {
  const visible = (node) => {
    if (!node) return false;
    const rect = node.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0;
  };
  const label = (node) => [
    node.getAttribute("aria-label") || "",
    node.getAttribute("data-tooltip") || "",
    node.getAttribute("title") || "",
    node.textContent || ""
  ].join(" ").trim();
  const buttons = Array.from(document.querySelectorAll('[aria-label], [data-tooltip], [role="button"]'))
    .filter(visible);
  const button = buttons.find((node) => /^delete\b/i.test(label(node)) || /\bdelete\b/i.test(label(node)));
  if (!button) {
    return { ok: false, reason: "delete button not visible" };
  }
  button.click();
  return { ok: true, label: label(button) };
})()
"#;

const GMAIL_SEND_SCRIPT: &str = r#"
(() => {
  const visible = (node) => {
    if (!node) return false;
    const rect = node.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0;
  };
  const label = (node) => [
    node.getAttribute("aria-label") || "",
    node.getAttribute("data-tooltip") || "",
    node.getAttribute("title") || "",
    node.textContent || ""
  ].join(" ").trim();
  const buttons = Array.from(document.querySelectorAll('[aria-label], [data-tooltip], [role="button"]'))
    .filter(visible);
  const button = buttons.find((node) => /^send\b/i.test(label(node)) && !/schedule/i.test(label(node)));
  if (!button) {
    return { ok: false, reason: "send button not visible" };
  }
  button.click();
  return { ok: true, label: label(button) };
})()
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn collection_url_supports_query_category_and_label() {
        assert_eq!(
            gmail_collection_url("me@example.com", &json!({"category": "promotions"})),
            "https://mail.google.com/mail/?authuser=me%40example.com#category/promotions"
        );
        assert_eq!(
            gmail_collection_url("me@example.com", &json!({"query": "from:alice has:attachment"})),
            "https://mail.google.com/mail/?authuser=me%40example.com#search/from%3Aalice+has%3Aattachment"
        );
        assert_eq!(
            gmail_collection_url("me@example.com", &json!({"label": "Clients/Acme"})),
            "https://mail.google.com/mail/?authuser=me%40example.com#label/Clients%2FAcme"
        );
    }

    #[test]
    fn thread_url_ignores_collection_only_source_url() {
        assert_eq!(
            gmail_thread_url(
                "me@example.com",
                &json!({
                    "url": "https://mail.google.com/mail/u/0/#inbox",
                    "thread_id": "19ef88112d77ab50"
                }),
                "19ef88112d77ab50",
            ),
            "https://mail.google.com/mail/?authuser=me%40example.com#inbox/19ef88112d77ab50"
        );
    }

    #[test]
    fn thread_url_allows_canonical_thread_source_url() {
        let url = "https://mail.google.com/mail/u/0/#inbox/FMfcgzQgMVlhzDkrbWxFkbcDhlqsqsGW";
        assert_eq!(
            gmail_thread_url(
                "me@example.com",
                &json!({
                    "url": url,
                    "thread_id": "19ef88112d77ab50"
                }),
                "19ef88112d77ab50",
            ),
            url
        );
    }

    #[test]
    fn compose_url_includes_cc_bcc_subject_and_body() {
        let url = gmail_compose_url(
            "me@example.com",
            "draft_reply",
            &json!({
                "to": ["alice@example.com"],
                "cc": "bob@example.com",
                "bcc": "ops@example.com",
                "subject": "Plan",
                "body": "Looks good",
            }),
        );
        assert!(url.contains("to=alice%40example.com"));
        assert!(url.contains("cc=bob%40example.com"));
        assert!(url.contains("bcc=ops%40example.com"));
        assert!(url.contains("su=Re%3A+Plan"));
        assert!(url.contains("body=Looks+good"));
    }

    #[test]
    fn draft_fields_reject_empty_draft_and_generate_save_script() {
        let empty = GmailComposeFields::from_input("draft_reply", &json!({}));
        assert!(empty.is_empty_draft());

        let fields = GmailComposeFields::from_input(
            "draft_reply",
            &json!({
                "to": "alice@example.com",
                "subject": "Plan",
                "body": "Looks good",
            }),
        );
        assert!(!fields.is_empty_draft());
        let script = gmail_save_draft_script(&fields);
        assert!(script.contains("alice@example.com"));
        assert!(script.contains("draft_autosaved"));
        let reply_script = gmail_reply_draft_script(&fields);
        assert!(reply_script.contains("reply_opening"));
        let verify_script = gmail_reply_draft_verify_script(&fields);
        assert!(verify_script.contains("draft_body_not_visible"));
        assert!(verify_script.contains("Gmail is still saving the reply draft"));
        assert!(draft_rows_contain(
            &fields,
            &json!({"rows":[{"subject":"Re: Plan","snippet":"Looks good"}]})
        ));
    }

    #[test]
    fn row_filters_match_sender_subject_unread_and_keywords() {
        let row = json!({
            "sender": "Alice",
            "fromEmail": "alice@example.com",
            "subject": "Launch plan",
            "snippet": "Budget attached",
            "unread": true
        });
        let filters = GmailRowFilters::from_input(&json!({
            "from": "alice",
            "subject": "launch",
            "keywords": ["budget"],
            "unread": true
        }));
        assert!(filters.matches(&row));
        let filters = GmailRowFilters::from_input(&json!({"keywords": ["missing"]}));
        assert!(!filters.matches(&row));
    }

    #[test]
    fn action_account_ignores_connector_connection_slug() {
        let config = GmailBrowserConfig {
            accounts: vec!["me@example.com".to_string()],
            ..GmailBrowserConfig::default()
        };

        assert_eq!(
            gmail_action_account(&config, &json!({"connection_slug": "gmail-browser"})).unwrap(),
            "me@example.com"
        );
    }

    #[test]
    fn action_account_allows_explicit_account_selection() {
        let config = GmailBrowserConfig {
            accounts: vec!["me@example.com".to_string(), "work@example.com".to_string()],
            ..GmailBrowserConfig::default()
        };

        assert_eq!(
            gmail_action_account(&config, &json!({"account": "Work@Example.COM"})).unwrap(),
            "work@example.com"
        );
        assert!(gmail_action_account(&config, &json!({"account": "other@example.com"})).is_err());
    }
}

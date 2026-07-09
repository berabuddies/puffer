//! Connector action helpers for the Gmail-browser subscriber.

#[path = "gmail_browser_draft.rs"]
mod gmail_browser_draft;

use anyhow::{Context, Result};
use gmail_browser_draft::{
    draft_rows_contain, gmail_reply_draft_script, gmail_reply_draft_verify_script,
    gmail_save_draft_script, sent_rows_contain,
};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

use super::{
    ensure_browser_daemon, poll_account_at_url, safe_session_part, GmailBrowserConfig,
    SubscriberEnv, BROWSER_HEIGHT, BROWSER_WIDTH, GMAIL_EVALUATE_INTERVAL, GMAIL_INBOX_SCRIPT,
    GMAIL_LOAD_TIMEOUT,
};

/// Minimum time to let Gmail replace the transient pre-search inbox rows with
/// the actual search results before trusting a scrape (see
/// [`poll_gmail_search_settled`]).
const GMAIL_SEARCH_SETTLE: Duration = Duration::from_millis(2500);

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
    let mut result = poll_account_at_url(env, &account, handshake_ref, &url)?;
    ensure_gmail_action_ready(&account, &result)?;
    if let Some(query) = string_input(input, "query").filter(|value| !value.trim().is_empty()) {
        // A search must clear two async hurdles before its rows can be trusted,
        // and the first scrape loses both races (#582):
        //   1. Route: on a cold tab Gmail boots to `#inbox` and only then
        //      applies the `#search` hash, so the first `href` is `#inbox`.
        //   2. Rows: even once `#search` commits, Gmail swaps in the real
        //      results a beat later, so the rows are still the prior inbox.
        // Reporting either transient state would be a fresh false-success, so
        // wait for the `#search` route to commit AND the result rows to settle.
        // We assert the `#search` route rather than the exact encoded fragment:
        // Gmail re-normalizes the hash (percent-decoding operators like
        // `newer_than:1d`), so an exact-fragment match would false-fail the very
        // operator queries #582 enables.
        // A *cold* tab boots Gmail to `#inbox` and drops the `#search` hash
        // outright, so a direct open of the search URL never reaches search.
        // The hash only commits as a *warm* client-side navigation, so if the
        // first scrape is not yet on `#search`, boot Gmail on the inbox and
        // re-open the search URL as a warm hash change before settling.
        if !href_is_search(&result) {
            let inbox_url = format!("{}#inbox", gmail_base_url(&account));
            poll_account_at_url(env, &account, handshake_ref, &inbox_url)?;
            result = poll_account_at_url(env, &account, handshake_ref, &url)?;
            ensure_gmail_action_not_auth_blocked(&account, &result)?;
        }
        result = poll_gmail_search_settled(env, &account, handshake_ref, action, &query, result)?;
        ensure_gmail_action_ready(&account, &result)?;
    }
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
    // Opening the thread IS Gmail's native mark-as-read mechanism; the defect
    // was reporting success without checking the unread marker actually
    // cleared (#591).
    open_gmail_url(env, &account, handshake_ref, &url)?;
    wait_gmail_thread_ready(env, &account, handshake_ref, &thread_id)?;
    let collection_url = gmail_collection_url(&account, input);
    let outcome =
        poll_gmail_list_until(env, &account, handshake_ref, &collection_url, |listing| {
            mark_read_verification_state(listing_rows(listing).as_slice(), &thread_id)
                == MarkReadVerification::Read
        })?;
    match outcome {
        Ok(_) => Ok(json!({
            "action": action,
            "summary": format!("marked Gmail thread {thread_id} read for {account}"),
            "account": account,
            "thread_id": thread_id,
            "url": url,
            "verification": {
                "matched": true,
                "method": "collection_unread_flag",
                "collection_url": collection_url,
            },
        })),
        Err(listing) => {
            let expectation = match mark_read_verification_state(
                listing_rows(&listing).as_slice(),
                &thread_id,
            ) {
                MarkReadVerification::Read => unreachable!("read state matched before timeout"),
                MarkReadVerification::StillUnread => {
                    format!("thread `{thread_id}` still shows as unread in the list")
                }
                MarkReadVerification::Missing => format!(
                    "thread `{thread_id}` was not visible in the first page of the list, so the read state could not be verified"
                ),
            };
            Err(crate::browser_action_verify::verification_failure(
                action,
                &expectation,
                &listing,
            ))
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MarkReadVerification {
    Read,
    StillUnread,
    Missing,
}

fn mark_read_verification_state(rows: &[Value], thread_id: &str) -> MarkReadVerification {
    match rows
        .iter()
        .find(|row| crate::browser_action_verify::row_matches_thread(row, thread_id))
    {
        Some(row) if row.get("unread").and_then(Value::as_bool).unwrap_or(false) => {
            MarkReadVerification::StillUnread
        }
        Some(_) => MarkReadVerification::Read,
        None => MarkReadVerification::Missing,
    }
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
    // Post-condition: the thread must be visible in Trash. A positive Trash
    // assertion beats "absent from inbox" -- the thread may simply be outside
    // the first page window, which would pass vacuously (#588).
    let trash_url = format!("{}#trash", gmail_base_url(&account));
    match poll_gmail_list_until(env, &account, handshake_ref, &trash_url, |listing| {
        listing_contains_thread(listing, &thread_id)
    })? {
        Ok(_) => Ok(json!({
            "action": action,
            "summary": format!("deleted Gmail thread {thread_id} for {account}"),
            "account": account,
            "thread_id": thread_id,
            "url": url,
            "verification": {
                "matched": true,
                "method": "trash_list",
                "trash_url": trash_url,
            },
        })),
        Err(trash) => Err(crate::browser_action_verify::verification_failure(
            action,
            &format!("thread `{thread_id}` was not visible in Trash after clicking Delete"),
            &trash,
        )),
    }
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
    let fields = GmailComposeFields::from_input(action, input);
    let url = gmail_compose_url_for_fields(&account, &fields);
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
    // Post-condition: the email must show up in the Sent list before we may
    // report success. Clicking Send proves nothing (#578).
    let sent_url = format!("{}#sent", gmail_base_url(&account));
    match poll_gmail_list_until(env, &account, handshake_ref, &sent_url, |listing| {
        sent_rows_contain(&fields, listing)
    })? {
        Ok(_) => Ok(json!({
            "action": action,
            "summary": format!("sent Gmail email for {account}"),
            "account": account,
            "url": url,
            "verification": {
                "matched": true,
                "method": "sent_list",
                "sent_url": sent_url,
            },
        })),
        Err(sent) => Err(crate::browser_action_verify::verification_failure(
            action,
            &format!(
                "email `{}` to {:?} was not visible in Sent after clicking Send",
                fields.subject, fields.to
            ),
            &sent,
        )),
    }
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

/// Re-navigates to `url` and polls its list rows until `matched` accepts the
/// response or [`GMAIL_LOAD_TIMEOUT`] elapses. Change-type actions
/// (send/delete/mark-read) share this loop to assert their post-condition
/// against an authoritative view. Returns the satisfying response, or the last
/// response as `Err` on timeout so the caller can attach action-specific
/// diagnostics through [`verification_failure`].
fn poll_gmail_list_until(
    env: &SubscriberEnv,
    account: &str,
    handshake: &crate::daemon::Handshake,
    url: &str,
    matched: impl Fn(&Value) -> bool,
) -> Result<std::result::Result<Value, Value>> {
    let deadline = Instant::now() + GMAIL_LOAD_TIMEOUT;
    loop {
        std::thread::sleep(GMAIL_EVALUATE_INTERVAL);
        let listing = poll_account_at_url(env, account, handshake, url)?;
        ensure_gmail_action_not_auth_blocked(account, &listing)?;
        if matched(&listing) {
            return Ok(Ok(listing));
        }
        if Instant::now() >= deadline {
            return Ok(Err(listing));
        }
    }
}

/// Extracts the `rows` array from a Gmail list response, defaulting to empty.
fn listing_rows(listing: &Value) -> Vec<Value> {
    listing
        .get("rows")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Returns true when any row in a Gmail list response refers to `thread_id`.
fn listing_contains_thread(listing: &Value, thread_id: &str) -> bool {
    listing_rows(listing)
        .iter()
        .any(|row| crate::browser_action_verify::row_matches_thread(row, thread_id))
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

/// Re-scrapes the current Gmail list tab in place (no re-navigation) with the
/// shared inbox script.
fn rescrape_gmail_list(
    env: &SubscriberEnv,
    account: &str,
    handshake: &crate::daemon::Handshake,
) -> Result<Value> {
    let root_session = format!("gmail-browser-{}", safe_session_part(&env.topic));
    let value = crate::daemon_browser::send_daemon_request(
        handshake,
        "browser_agent",
        json!({
            "action": "evaluate",
            "sessionId": root_session,
            "tabId": safe_session_part(account),
            "width": BROWSER_WIDTH,
            "height": BROWSER_HEIGHT,
            "background": true,
            "script": GMAIL_INBOX_SCRIPT,
        }),
    )
    .context("re-scrape Gmail search results")?;
    Ok(value.get("value").cloned().unwrap_or(Value::Null))
}

/// Ordered thread-id signature of a listing's rows, used to detect when a
/// search's result set has stopped changing.
fn row_signature(result: &Value) -> String {
    listing_rows(result)
        .iter()
        .filter_map(|row| {
            ["threadId", "legacyThreadId", "gmailThreadId", "id"]
                .iter()
                .find_map(|key| row.get(*key).and_then(Value::as_str))
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn href_is_search(result: &Value) -> bool {
    result
        .get("href")
        .and_then(Value::as_str)
        .is_some_and(|href| href.contains("#search"))
}

/// Waits for a Gmail search to commit its `#search` route AND for its result
/// rows to settle before the scrape can be trusted.
///
/// `initial` is the first scrape, which loses both races: on a cold tab its
/// `href` is still `#inbox`, and even on a warm tab the rows are still the
/// prior inbox until Gmail swaps in the results. Re-scrapes in place until the
/// `href` is on `#search` AND the row signature is stable across two
/// consecutive reads AND at least [`GMAIL_SEARCH_SETTLE`] has elapsed since the
/// route committed. On [`GMAIL_LOAD_TIMEOUT`], returns the last scrape if
/// `#search` was reached, otherwise a [`verification_failure`] so a dropped
/// query is reported honestly rather than as stale inbox rows (#582).
fn poll_gmail_search_settled(
    env: &SubscriberEnv,
    account: &str,
    handshake: &crate::daemon::Handshake,
    action: &str,
    query: &str,
    initial: Value,
) -> Result<Value> {
    let deadline = Instant::now() + GMAIL_LOAD_TIMEOUT;
    let mut latest = initial;
    let mut prev_sig = row_signature(&latest);
    // Settle is measured from when `#search` first commits, not from entry, so
    // the cold-tab `#inbox` boot phase does not eat the settle window.
    let mut search_committed_at = href_is_search(&latest).then(Instant::now);
    loop {
        std::thread::sleep(GMAIL_EVALUATE_INTERVAL);
        let next = rescrape_gmail_list(env, account, handshake)?;
        ensure_gmail_action_not_auth_blocked(account, &next)?;
        let on_search = href_is_search(&next);
        let sig = row_signature(&next);
        let stable = on_search && sig == prev_sig;
        if on_search && search_committed_at.is_none() {
            search_committed_at = Some(Instant::now());
        }
        prev_sig = sig;
        latest = next;
        if let Some(committed) = search_committed_at {
            if stable && committed.elapsed() >= GMAIL_SEARCH_SETTLE {
                return Ok(latest);
            }
        }
        if Instant::now() >= deadline {
            if href_is_search(&latest) {
                return Ok(latest);
            }
            return Err(crate::browser_action_verify::verification_failure(
                action,
                &format!("Gmail search view for `{query}` was not reached"),
                &latest,
            ));
        }
    }
}

fn gmail_base_url(account: &str) -> String {
    // Gmail must be addressed by signed-in-account *index* (`/u/N/`), not by the
    // `?authuser=<email>` query form. The query form triggers a full-page
    // redirect that resolves the account and *drops the URL hash fragment* on
    // the way, silently landing on the default `#inbox`. That broke every
    // non-inbox navigation built on this base -- `#search/...` (agentenv/
    // monorepo#582), and the plan-07 post-condition views `#sent` (send),
    // `#trash` (delete), and `#inbox/<thread>` (mark_read / reply) -- because
    // the intended view was never reached. The managed profile hosts a single
    // signed-in account at index 0 (the old `?authuser=` already fell back to
    // it regardless of the requested address), so `/u/0/` selects the same
    // mailbox while preserving the fragment. Verified live: `/u/0/#search/...`
    // reaches the search view where `?authuser=<email>#search/...` did not.
    let _ = account;
    "https://mail.google.com/mail/u/0/".to_string()
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
        let keywords = keywords_input(input);
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
            "https://mail.google.com/mail/u/0/#category/promotions"
        );
        assert_eq!(
            gmail_collection_url(
                "me@example.com",
                &json!({"query": "from:alice has:attachment"})
            ),
            "https://mail.google.com/mail/u/0/#search/from%3Aalice+has%3Aattachment"
        );
        assert_eq!(
            gmail_collection_url("me@example.com", &json!({"label": "Clients/Acme"})),
            "https://mail.google.com/mail/u/0/#label/Clients%2FAcme"
        );
    }

    #[test]
    fn base_url_uses_path_index_so_hash_fragment_survives_navigation() {
        // Regression (agentenv/monorepo#582 + plan-07): the `?authuser=<email>`
        // query form triggered a redirect that dropped the URL hash fragment,
        // so `#search` / `#sent` / `#trash` / `#inbox/<thread>` navigations
        // silently fell back to `#inbox`. Verified live that the `/u/N/` path
        // form preserves the fragment where the query form did not.
        let base = gmail_base_url("me@example.com");
        assert!(
            !base.contains("authuser"),
            "base must not use the fragment-dropping authuser query form: {base}"
        );
        assert!(
            base.starts_with("https://mail.google.com/mail/u/"),
            "base must be a /u/N/ path form: {base}"
        );
        // The plan-07 post-condition views must carry their fragment intact.
        assert_eq!(
            format!("{base}#sent"),
            "https://mail.google.com/mail/u/0/#sent"
        );
        assert_eq!(
            format!("{base}#trash"),
            "https://mail.google.com/mail/u/0/#trash"
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
            "https://mail.google.com/mail/u/0/#inbox/19ef88112d77ab50"
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
        let fields = GmailComposeFields::from_input(
            "draft_reply",
            &json!({
                "to": ["alice@example.com"],
                "cc": "bob@example.com",
                "bcc": "ops@example.com",
                "subject": "Plan",
                "body": "Looks good",
            }),
        );
        let url = gmail_compose_url_for_fields("me@example.com", &fields);
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
    fn sent_rows_require_subject_and_recipient_together() {
        let fields = GmailComposeFields::from_input(
            "send_email",
            &json!({ "to": "bob@example.com", "subject": "Quarterly report", "body": "Numbers attached" }),
        );
        // Subject AND recipient present in the row: match.
        assert!(sent_rows_contain(
            &fields,
            &json!({"rows":[{"sender":"To: Bob","fromEmail":"bob@example.com","subject":"Quarterly report","snippet":"Numbers attached"}]})
        ));
        // Same recipient but a different old email: draft_rows_contain's OR
        // semantics would match this -- sent verification must not.
        assert!(!sent_rows_contain(
            &fields,
            &json!({"rows":[{"sender":"To: Bob","fromEmail":"bob@example.com","subject":"Lunch","snippet":"See you"}]})
        ));
        // Subject matches but recipient absent: no match.
        assert!(!sent_rows_contain(
            &fields,
            &json!({"rows":[{"sender":"To: Carol","fromEmail":"carol@example.com","subject":"Quarterly report","snippet":""}]})
        ));
        // Empty rows / fields without any signal never vacuously match.
        assert!(!sent_rows_contain(&fields, &json!({"rows":[]})));
        let empty = GmailComposeFields::from_input("send_email", &json!({}));
        assert!(!sent_rows_contain(
            &empty,
            &json!({"rows":[{"subject":"anything","snippet":"x"}]})
        ));
    }

    #[test]
    fn mark_read_verification_requires_matching_row_with_unread_cleared() {
        let rows = vec![
            json!({"threadId":"thread-f:read","unread":false}),
            json!({"threadId":"thread-f:unread","unread":true}),
        ];
        assert_eq!(
            mark_read_verification_state(&rows, "#thread-f:read"),
            MarkReadVerification::Read
        );
        assert_eq!(
            mark_read_verification_state(&rows, "thread-f:unread"),
            MarkReadVerification::StillUnread
        );
        assert_eq!(
            mark_read_verification_state(&rows, "thread-f:missing"),
            MarkReadVerification::Missing
        );
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
    fn row_filters_do_not_swallow_query_operators() {
        // #582: `query` is interpreted server-side via the #search URL; treating
        // it as a literal client-side keyword made operator queries like
        // `newer_than:1d` match zero rows.
        let row = json!({
            "sender": "Alice",
            "fromEmail": "alice@example.com",
            "subject": "Launch plan",
            "snippet": "Budget attached",
            "unread": true
        });
        let filters = GmailRowFilters::from_input(&json!({ "query": "newer_than:1d" }));
        assert!(filters.matches(&row));
        // Explicit `keywords` keep their client-side filter contract.
        let filters = GmailRowFilters::from_input(&json!({
            "query": "newer_than:1d",
            "keywords": ["budget"]
        }));
        assert!(filters.matches(&row));
        let filters = GmailRowFilters::from_input(&json!({ "keywords": ["missing"] }));
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

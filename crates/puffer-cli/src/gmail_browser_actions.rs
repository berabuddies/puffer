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
    ensure_browser_daemon, safe_session_part, GmailBrowserConfig, SubscriberEnv, BROWSER_HEIGHT,
    BROWSER_WIDTH, GMAIL_EVALUATE_INTERVAL, GMAIL_INBOX_SCRIPT, GMAIL_LOAD_TIMEOUT,
};

/// Network-idle window that marks a Gmail view's XHR burst as finished.
const GMAIL_NETWORK_IDLE: Duration = Duration::from_millis(600);

/// Upper bound on the network-idle wait. Gmail long-poll heartbeats can keep
/// the connection busy indefinitely, so idle is a best-effort fast path, not
/// a required signal (see [`wait_gmail_network_idle`]).
const GMAIL_NETWORK_IDLE_TIMEOUT: Duration = Duration::from_secs(8);

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
    let deadline = Instant::now() + GMAIL_LOAD_TIMEOUT;
    // All list reads (search included) go through the settled-view primitive;
    // it owns the cold-boot hash-drop retry and the render-completion wait,
    // so a stale pre-navigation view can never be reported as results (#777).
    let result = match open_gmail_view_settled(env, &account, handshake_ref, &url, deadline)? {
        Ok(result) => result,
        Err(latest) => {
            return Err(crate::browser_action_verify::verification_failure(
                action,
                &format!("Gmail view for `{url}` did not settle"),
                &latest,
            ));
        }
    };
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
    // Opening the thread IS Gmail's native mark-as-read mechanism; the defect
    // was reporting success without checking the unread marker actually
    // cleared (#591).
    open_gmail_thread_for_action(env, &account, handshake_ref, input, &url, &thread_id)?;
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
    let navigation_thread_id = gmail_thread_navigation_id(input, &thread_id);
    let url = gmail_thread_url(&account, input, &thread_id);
    let handshake_ref = ensure_browser_daemon(config, handshake)?;
    let ready =
        open_gmail_thread_for_action(env, &account, handshake_ref, input, &url, &thread_id)?;
    let ready_route_id = gmail_thread_ready_route_id(&ready);
    let delete_route_ids =
        gmail_delete_route_ids(&thread_id, &navigation_thread_id, ready_route_id.as_deref());
    let deadline = Instant::now() + GMAIL_LOAD_TIMEOUT;
    loop {
        let click = evaluate_gmail_script(
            env,
            &account,
            handshake_ref,
            &gmail_delete_script(&delete_route_ids),
        )?;
        if click.get("ok").and_then(Value::as_bool).unwrap_or(false) {
            break;
        }
        if !gmail_delete_click_retryable(&click) || Instant::now() >= deadline {
            anyhow::bail!("{}", gmail_delete_click_error_message(&thread_id, &click));
        }
        std::thread::sleep(GMAIL_EVALUATE_INTERVAL);
    }
    // Post-condition: the thread must be visible in Trash. A positive Trash
    // assertion beats "absent from inbox" -- the thread may simply be outside
    // the first page window, which would pass vacuously (#588).
    let trash_url = format!("{}#trash", gmail_base_url(&account));
    match poll_gmail_list_until(env, &account, handshake_ref, &trash_url, |listing| {
        listing_contains_any_thread_id(listing, &delete_route_ids)
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

fn gmail_delete_click_retryable(click: &Value) -> bool {
    !click.get("ok").and_then(Value::as_bool).unwrap_or(false)
        && click
            .get("reason")
            .and_then(Value::as_str)
            .is_some_and(|reason| reason == "delete button not visible")
}

fn gmail_delete_click_error_message(thread_id: &str, click: &Value) -> String {
    let reason = click
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let href = click.get("href").and_then(Value::as_str).unwrap_or("");
    let hash = click.get("hash").and_then(Value::as_str).unwrap_or("");
    let labels = click.get("labels").cloned().unwrap_or(Value::Null);
    let candidates = click.get("candidates").cloned().unwrap_or(Value::Null);
    format!(
        "Gmail delete button was not found for thread `{thread_id}`: {reason}; href={href:?}; hash={hash:?}; labels={labels}; candidates={candidates}"
    )
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
    // Best-effort settle: an unsettled Drafts view falls through to the
    // draft_rows_contain check below, which reports the honest failure.
    let drafts = match open_gmail_view_settled(
        env,
        &account,
        handshake_ref,
        &drafts_url,
        Instant::now() + GMAIL_LOAD_TIMEOUT,
    )? {
        Ok(drafts) => drafts,
        Err(latest) => latest,
    };
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
    open_gmail_thread_for_action(env, &account, handshake_ref, input, &url, &thread_id)?;
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
    open_gmail_thread_ready(env, account, handshake, thread_url, thread_id)?;
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
    let prepare = evaluate_gmail_script(
        env,
        &account,
        handshake_ref,
        &gmail_prepare_send_script(&fields),
    )?;
    if !prepare.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        anyhow::bail!(
            "Gmail send compose was not prepared: {}",
            prepare
                .get("reason")
                .and_then(Value::as_str)
                .or_else(|| prepare.get("status").and_then(Value::as_str))
                .unwrap_or("unknown")
        );
    }
    let click = evaluate_gmail_script(env, &account, handshake_ref, &gmail_send_script(&fields))?;
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
        let value = match crate::daemon_browser::send_daemon_request(
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
        ) {
            Ok(value) => value,
            Err(error) => {
                let error = error.context("evaluate Gmail action script");
                if gmail_evaluate_error_retryable(&error.to_string()) && Instant::now() < deadline {
                    std::thread::sleep(GMAIL_EVALUATE_INTERVAL);
                    continue;
                }
                return Err(error);
            }
        };
        let result = value.get("value").cloned().unwrap_or(Value::Null);
        if result.get("ok").and_then(Value::as_bool).unwrap_or(false) || Instant::now() >= deadline
        {
            return Ok(result);
        }
        std::thread::sleep(GMAIL_EVALUATE_INTERVAL);
    }
}

fn gmail_evaluate_error_retryable(message: &str) -> bool {
    message.contains("timed out waiting for browser evaluation")
        || message.contains("timed out waiting on channel")
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

#[derive(Debug, Clone, PartialEq)]
enum GmailThreadReadyDecision {
    Ready(Value),
    Pending,
}

fn gmail_thread_ready_decision(
    account: &str,
    thread_id: &str,
    result: &Value,
) -> Result<GmailThreadReadyDecision> {
    if result.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        ensure_gmail_action_ready(account, result)?;
        return Ok(GmailThreadReadyDecision::Ready(result.clone()));
    }
    let status = result.get("status").and_then(Value::as_str).unwrap_or("ok");
    if status == "loading" {
        return Ok(GmailThreadReadyDecision::Pending);
    }
    ensure_gmail_action_ready(account, result)?;
    anyhow::bail!("Gmail thread `{thread_id}` did not become ready for account `{account}`")
}

fn open_gmail_thread_for_action(
    env: &SubscriberEnv,
    account: &str,
    handshake: &crate::daemon::Handshake,
    input: &Value,
    url: &str,
    thread_id: &str,
) -> Result<Value> {
    let navigation_thread_id = gmail_thread_navigation_id(input, thread_id);
    let route_ids = gmail_thread_route_ids(thread_id, &navigation_thread_id, None);
    if let Some(source_url) = gmail_thread_source_list_url(input, thread_id) {
        let deadline = Instant::now() + GMAIL_LOAD_TIMEOUT;
        if let Ok(listing) =
            open_gmail_view_settled(env, account, handshake, &source_url, deadline)?
        {
            if listing_contains_any_thread_id(&listing, &route_ids) {
                let click = evaluate_gmail_script(
                    env,
                    account,
                    handshake,
                    &gmail_open_thread_row_script(&route_ids),
                )?;
                if click.get("ok").and_then(Value::as_bool).unwrap_or(false) {
                    return wait_gmail_thread_ready(env, account, handshake, thread_id);
                }
            }
        }
    }
    open_gmail_thread_ready(env, account, handshake, url, thread_id)
}

fn open_gmail_thread_ready(
    env: &SubscriberEnv,
    account: &str,
    handshake: &crate::daemon::Handshake,
    url: &str,
    thread_id: &str,
) -> Result<Value> {
    open_gmail_url(env, account, handshake, url)?;
    wait_gmail_network_idle(env, account, handshake);
    match wait_gmail_thread_ready_result(env, account, handshake, thread_id)? {
        Ok(result) => return Ok(result),
        Err(latest) if !gmail_thread_ready_needs_warm_reopen(&latest) => {
            anyhow::bail!(
                "{}",
                gmail_thread_ready_timeout_message(account, thread_id, &latest)
            );
        }
        Err(_) => {}
    }

    let inbox_url = format!("{}#inbox", gmail_base_url(account));
    open_gmail_url(env, account, handshake, &inbox_url)?;
    wait_gmail_network_idle(env, account, handshake);
    open_gmail_url(env, account, handshake, url)?;
    wait_gmail_network_idle(env, account, handshake);
    wait_gmail_thread_ready(env, account, handshake, thread_id)
}

fn wait_gmail_thread_ready_result(
    env: &SubscriberEnv,
    account: &str,
    handshake: &crate::daemon::Handshake,
    thread_id: &str,
) -> Result<std::result::Result<Value, Value>> {
    let deadline = Instant::now() + GMAIL_LOAD_TIMEOUT;
    loop {
        let result = evaluate_gmail_script(
            env,
            account,
            handshake,
            &gmail_thread_ready_script(thread_id),
        )?;
        match gmail_thread_ready_decision(account, thread_id, &result)? {
            GmailThreadReadyDecision::Ready(result) => return Ok(Ok(result)),
            GmailThreadReadyDecision::Pending => {}
        }
        if Instant::now() >= deadline {
            return Ok(Err(result));
        }
        std::thread::sleep(GMAIL_EVALUATE_INTERVAL);
    }
}

fn wait_gmail_thread_ready(
    env: &SubscriberEnv,
    account: &str,
    handshake: &crate::daemon::Handshake,
    thread_id: &str,
) -> Result<Value> {
    match wait_gmail_thread_ready_result(env, account, handshake, thread_id)? {
        Ok(result) => Ok(result),
        Err(latest) => {
            anyhow::bail!(
                "{}",
                gmail_thread_ready_timeout_message(account, thread_id, &latest)
            )
        }
    }
}

fn gmail_thread_ready_needs_warm_reopen(result: &Value) -> bool {
    if result.get("status").and_then(Value::as_str) != Some("loading") {
        return false;
    }
    if result.get("routeReady").and_then(Value::as_bool) != Some(true) {
        return false;
    }
    if result
        .get("threadDetailReady")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return false;
    }
    let body_text = result
        .get("bodyText")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    body_text.contains("search mail")
        && body_text.contains("compose")
        && body_text.contains("inbox")
}

fn gmail_thread_ready_timeout_message(account: &str, thread_id: &str, result: &Value) -> String {
    let field = |key: &str| result.get(key).and_then(Value::as_str).unwrap_or("unknown");
    let bool_field = |key: &str| {
        result
            .get(key)
            .and_then(Value::as_bool)
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unknown".to_string())
    };
    format!(
        "Gmail thread `{thread_id}` did not become ready for account `{account}`; last URL `{}`, title `{}`, status `{}`, hash `{}`, routeReady={}, threadDetailReady={}, bodyText={:?}",
        field("href"),
        field("title"),
        field("status"),
        field("hash"),
        bool_field("routeReady"),
        bool_field("threadDetailReady"),
        field("bodyText"),
    )
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

/// Re-opens `url` through [`open_gmail_view_settled`] and polls its rows
/// until `matched` accepts the listing or [`GMAIL_LOAD_TIMEOUT`] elapses.
/// Change-type actions (send/delete/mark-read) share this loop to assert
/// their post-condition against an authoritative, settled view -- the settle
/// step guarantees the rows really belong to `url`'s route (#777). Returns
/// the satisfying response, or the last response as `Err` on timeout so the
/// caller can attach action-specific diagnostics through
/// [`verification_failure`].
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
        // The shared deadline caps nested settling: iterations never stack
        // their own timeouts on top of the loop's.
        let listing = match open_gmail_view_settled(env, account, handshake, url, deadline)? {
            Ok(listing) => listing,
            Err(latest) => return Ok(Err(latest)),
        };
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

fn listing_contains_any_thread_id(listing: &Value, thread_ids: &[String]) -> bool {
    thread_ids
        .iter()
        .any(|thread_id| listing_contains_thread(listing, thread_id))
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
    [
        "thread_id",
        "gmail_thread_id",
        "gmailThreadId",
        "threadId",
        "message_id",
        "messageId",
        "id",
    ]
    .iter()
    .find_map(|key| string_input(input, key))
    .map(|value| value.trim().trim_start_matches('#').to_string())
    .filter(|value| !value.is_empty())
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
    let navigation_thread_id = gmail_thread_navigation_id(input, thread_id);
    format!(
        "{}#{}/{}",
        gmail_base_url(account),
        url_fragment(&fragment),
        url_fragment(&navigation_thread_id)
    )
}

fn gmail_thread_source_list_url(input: &Value, thread_id: &str) -> Option<String> {
    string_input(input, "url")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .filter(|value| !gmail_url_targets_thread(value, thread_id))
        .filter(|value| value.contains("mail.google.com") && value.contains('#'))
}

fn gmail_thread_navigation_id(input: &Value, fallback: &str) -> String {
    [
        "threadId",
        "legacyThreadId",
        "id",
        "message_id",
        "messageId",
    ]
    .iter()
    .find_map(|key| string_input(input, key))
    .map(|value| value.trim().trim_start_matches('#').to_string())
    .filter(|value| is_navigable_gmail_thread_fragment(value))
    .unwrap_or_else(|| fallback.trim().trim_start_matches('#').to_string())
}

fn gmail_thread_ready_route_id(result: &Value) -> Option<String> {
    result
        .get("hash")
        .and_then(Value::as_str)
        .and_then(gmail_hash_thread_segment)
        .or_else(|| {
            result
                .get("href")
                .and_then(Value::as_str)
                .and_then(|href| href.split('#').nth(1))
                .and_then(gmail_hash_thread_segment)
        })
}

fn gmail_hash_thread_segment(fragment: &str) -> Option<String> {
    let fragment = fragment.trim().trim_start_matches('#').trim_matches('/');
    if !fragment.contains('/') {
        return None;
    }
    fragment
        .rsplit('/')
        .next()
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .map(ToString::to_string)
}

fn gmail_delete_route_ids(
    thread_id: &str,
    navigation_thread_id: &str,
    ready_route_id: Option<&str>,
) -> Vec<String> {
    gmail_thread_route_ids(thread_id, navigation_thread_id, ready_route_id)
}

fn gmail_thread_route_ids(
    thread_id: &str,
    navigation_thread_id: &str,
    ready_route_id: Option<&str>,
) -> Vec<String> {
    [Some(thread_id), Some(navigation_thread_id), ready_route_id]
        .into_iter()
        .flatten()
        .map(|value| value.trim().trim_start_matches('#').to_string())
        .filter(|value| !value.is_empty())
        .fold(Vec::<String>::new(), |mut ids, value| {
            if !ids.iter().any(|existing| existing == &value) {
                ids.push(value);
            }
            ids
        })
}

fn is_navigable_gmail_thread_fragment(value: &str) -> bool {
    let normalized = value.trim().trim_start_matches('#').to_ascii_lowercase();
    normalized.starts_with("fmfc")
        || (normalized.len() >= 12 && normalized.chars().all(|c| c.is_ascii_hexdigit()))
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

fn gmail_prepare_send_script(fields: &GmailComposeFields) -> String {
    gmail_save_draft_script(fields)
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

/// Best-effort wait for the tab's network to go idle -- the browser-level
/// "this view's XHR burst finished" completion signal (#777). A timeout is
/// not a failure: Gmail heartbeats may never let the network settle, so the
/// caller merely loses the fast path and falls back to its confirmation
/// scrapes. Returns `true` when idle was observed, `false` on timeout; callers
/// skip further idle waits once it has proven unreliable for this view.
fn wait_gmail_network_idle(
    env: &SubscriberEnv,
    account: &str,
    handshake: &crate::daemon::Handshake,
) -> bool {
    let result = crate::daemon_browser::send_daemon_request(
        handshake,
        "browser_agent",
        json!({
            "action": "waitNetworkIdle",
            "sessionId": format!("gmail-browser-{}", safe_session_part(&env.topic)),
            "tabId": safe_session_part(account),
            "width": BROWSER_WIDTH,
            "height": BROWSER_HEIGHT,
            "idleMs": GMAIL_NETWORK_IDLE.as_millis() as u64,
            "timeoutMs": GMAIL_NETWORK_IDLE_TIMEOUT.as_millis() as u64,
        }),
    );
    if result.is_err() {
        crate::gmail_browser_log::line(format!("gmail_network_idle_timeout topic={}", env.topic));
        return false;
    }
    true
}

/// Opens a Gmail list view and waits until it has verifiably rendered.
///
/// This is the sole arbiter of "this list view is ready" for action-path
/// reads. It replaces per-callsite polling that trusted any visible rows and
/// could return the pre-navigation inbox as search results (#777, #582):
///
/// 1. navigate, then wait for network idle (browser-level completion signal;
///    best effort -- see [`wait_gmail_network_idle`]),
/// 2. assert the `href` route matches the URL's fragment route, re-warming
///    through `#inbox` once for the cold-boot hash drop, and
/// 3. require the row signature to hold across two consecutive scrapes.
///
/// Returns `Ok(Ok(result))` once settled, `Ok(Err(latest))` when `deadline`
/// passes first (callers attach action-specific diagnostics), and `Err` for
/// auth or RPC failures. Non-ok/loading statuses (e.g. `temporary_error`)
/// short-circuit as `Ok(Ok(result))` so callers can report them precisely.
fn open_gmail_view_settled(
    env: &SubscriberEnv,
    account: &str,
    handshake: &crate::daemon::Handshake,
    url: &str,
    deadline: Instant,
) -> Result<std::result::Result<Value, Value>> {
    let route = expected_route_of(url);
    let pre_navigation_signature = rescrape_gmail_list(env, account, handshake)
        .ok()
        .and_then(|listing| pre_navigation_row_signature(&listing, url));
    open_gmail_url(env, account, handshake, url)?;
    let mut network_idle_reliable = wait_gmail_network_idle(env, account, handshake);
    let mut warmed = false;
    let mut prev_sig: Option<String> = None;
    let mut latest = rescrape_gmail_list(env, account, handshake)?;
    loop {
        ensure_gmail_action_not_auth_blocked(account, &latest)?;
        let status = latest.get("status").and_then(Value::as_str).unwrap_or("ok");
        if status != "ok" && status != "loading" {
            return Ok(Ok(latest));
        }
        if href_on_route(&latest, &route) {
            if status == "ok" {
                if listing_reuses_pre_navigation_rows(&latest, pre_navigation_signature.as_deref())
                {
                    prev_sig = None;
                } else {
                    let sig = row_signature(&latest);
                    if prev_sig.as_deref() == Some(sig.as_str()) {
                        return Ok(Ok(latest));
                    }
                    prev_sig = Some(sig);
                }
            }
        } else {
            prev_sig = None;
            if !warmed && route != "#inbox" {
                // Cold tab: Gmail boots to `#inbox` and drops the target
                // hash; it only commits as a warm client-side navigation.
                // Boot the inbox, then re-open the target URL (#582).
                warmed = true;
                let inbox_url = format!("{}#inbox", gmail_base_url(account));
                open_gmail_url(env, account, handshake, &inbox_url)?;
                // Skip idle waits once they have proven unreliable for this
                // view: a prior timeout means Gmail heartbeats keep the network
                // busy, so waiting again only burns the shared deadline.
                if network_idle_reliable {
                    network_idle_reliable = wait_gmail_network_idle(env, account, handshake);
                }
                open_gmail_url(env, account, handshake, url)?;
                if network_idle_reliable {
                    wait_gmail_network_idle(env, account, handshake);
                }
                latest = rescrape_gmail_list(env, account, handshake)?;
                if Instant::now() >= deadline {
                    return Ok(Err(latest));
                }
                continue;
            }
        }
        if Instant::now() >= deadline {
            return Ok(Err(latest));
        }
        std::thread::sleep(GMAIL_EVALUATE_INTERVAL);
        latest = rescrape_gmail_list(env, account, handshake)?;
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

/// Returns the pre-navigation row signature when `current` is not already the
/// target Gmail view. Gmail commits `location.hash` before replacing the row
/// grid, so a new `#search/...` hash can briefly expose old rows as if they
/// belonged to the new query. The signature gives [`open_gmail_view_settled`]
/// a concrete stale-grid value to reject.
fn pre_navigation_row_signature(current: &Value, target_url: &str) -> Option<String> {
    let current_href = current.get("href").and_then(Value::as_str)?;
    if same_gmail_view(current_href, target_url) {
        return None;
    }
    let signature = row_signature(current);
    (!signature.is_empty()).then_some(signature)
}

fn listing_reuses_pre_navigation_rows(
    result: &Value,
    pre_navigation_signature: Option<&str>,
) -> bool {
    if result
        .get("empty")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return false;
    }
    let Some(pre_navigation_signature) =
        pre_navigation_signature.filter(|signature| !signature.is_empty())
    else {
        return false;
    };
    row_signature(result) == pre_navigation_signature
}

fn same_gmail_view(left_url: &str, right_url: &str) -> bool {
    normalized_gmail_fragment(left_url) == normalized_gmail_fragment(right_url)
}

fn normalized_gmail_fragment(url: &str) -> Option<String> {
    url.split('#')
        .nth(1)
        .map(percent_decode_gmail_fragment)
        .map(|fragment| fragment.trim_matches('/').to_ascii_lowercase())
}

fn percent_decode_gmail_fragment(fragment: &str) -> String {
    let bytes = fragment.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(high), Some(low)) = (
                hex_digit_value(bytes[index + 1]),
                hex_digit_value(bytes[index + 2]),
            ) {
                decoded.push((high << 4) | low);
                index += 3;
                continue;
            }
        }
        decoded.push(if bytes[index] == b'+' {
            b' '
        } else {
            bytes[index]
        });
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn hex_digit_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Gmail list-view route prefix (`#search`, `#inbox`, ...) that `url` should
/// land on. Only the prefix is meaningful: Gmail re-normalizes the fragment
/// (percent-decoding operators like `newer_than:1d`), so an exact-fragment
/// assertion would false-fail the very operator queries #582 enables.
fn expected_route_of(url: &str) -> String {
    match url
        .split('#')
        .nth(1)
        .map(|f| f.split('/').next().unwrap_or(""))
    {
        Some(route) if !route.is_empty() => format!("#{route}"),
        _ => "#inbox".to_string(),
    }
}

/// Returns true when a scrape's `href` sits on `route` (`#search`, `#sent`,
/// ...). Pages without a committed fragment match no route.
fn href_on_route(result: &Value, route: &str) -> bool {
    result
        .get("href")
        .and_then(Value::as_str)
        .and_then(|href| href.split('#').nth(1))
        .is_some_and(|fragment| {
            let actual_route = format!("#{}", fragment.split('/').next().unwrap_or(""));
            actual_route == route && !fragment_is_thread_detail(fragment)
        })
}

fn fragment_is_thread_detail(fragment: &str) -> bool {
    let Some(last_segment) = fragment.trim_matches('/').rsplit('/').next() else {
        return false;
    };
    let normalized = last_segment
        .trim()
        .trim_start_matches('#')
        .to_ascii_lowercase();
    normalized.starts_with("thread-a:")
        || normalized.starts_with("thread-f:")
        || (normalized.len() >= 12 && normalized.chars().all(|c| c.is_ascii_hexdigit()))
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
  const expectedId = String(expected || "").trim().replace(/^#/, "").toLowerCase();
  const lowerHash = hash.toLowerCase();
  const routeReady = /\/[^/]+$/.test(hash) || (expectedId && lowerHash.includes(expectedId));
  const gmailChrome =
    document.querySelector('[role="main"]') ||
    document.querySelector('div[gh]') ||
    document.querySelector('.nH') ||
    /gmail/i.test(title);
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
  const subjectHeading = Array.from(document.querySelectorAll('.hP'))
    .filter(visible)
    .some((node) => label(node).trim().length > 0);
  const messageRegion = Array.from(document.querySelectorAll('[data-message-id], .adn, .ii.gt'))
    .filter(visible)
    .length > 0;
  const threadDetailReady =
    threadChrome || subjectHeading || messageRegion || replyButton || draftBody;
  if (routeReady && threadDetailReady) {{
    return {{
      ok: true,
      status: "ok",
      href,
      title,
      gmailChrome: Boolean(gmailChrome),
      replyButton,
      draftBody,
      threadChrome,
      subjectHeading,
      messageRegion,
      threadDetailReady
    }};
  }}
  return {{
    ok: false,
    status: "loading",
    href,
    title,
    hash,
    routeReady,
    gmailChrome: Boolean(gmailChrome),
    threadChrome,
    subjectHeading,
    messageRegion,
    bodyText: bodyText.slice(0, 200)
  }};
}})()
"#
    )
}

fn gmail_open_thread_row_script(route_ids: &[String]) -> String {
    let route_ids = json!(route_ids
        .iter()
        .map(|value| value.trim().trim_start_matches('#').to_string())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>())
    .to_string();
    format!(
        r#"
(() => {{
  const routeIds = {route_ids}
    .map((value) => String(value || "").trim().replace(/^#/, "").toLowerCase())
    .filter((value, index, values) => value.length > 0 && values.indexOf(value) === index);
  const href = location.href || "";
  const visible = (node) => {{
    if (!node) return false;
    const rect = node.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0;
  }};
  const normalize = (value) => String(value || "").trim().replace(/^#/, "").toLowerCase();
  const rowIds = (row) => [
    row.getAttribute("data-legacy-thread-id") || "",
    row.getAttribute("data-thread-id") || "",
    row.getAttribute("data-legacy-message-id") || "",
    row.getAttribute("data-message-id") || "",
    row.getAttribute("data-id") || "",
    ...Array.from(row.querySelectorAll("[data-legacy-thread-id], [data-thread-id], [data-legacy-message-id], [data-message-id], [data-id]"))
      .flatMap((node) => [
        node.getAttribute("data-legacy-thread-id") || "",
        node.getAttribute("data-thread-id") || "",
        node.getAttribute("data-legacy-message-id") || "",
        node.getAttribute("data-message-id") || "",
        node.getAttribute("data-id") || ""
      ])
  ].map(normalize).filter((value) => value.length > 0);
  const activate = (node) => {{
    for (const [type, Ctor] of [
      ["pointerdown", window.PointerEvent || window.MouseEvent],
      ["mousedown", window.MouseEvent],
      ["mouseup", window.MouseEvent],
      ["pointerup", window.PointerEvent || window.MouseEvent],
      ["click", window.MouseEvent]
    ]) {{
      node.dispatchEvent(new Ctor(type, {{ bubbles: true, cancelable: true, view: window }}));
    }}
    node.click();
  }};
  const rows = Array.from(document.querySelectorAll('tr[role="row"]')).filter(visible);
  const row = rows.find((candidate) => {{
    const ids = rowIds(candidate);
    return routeIds.some((routeId) => ids.includes(routeId));
  }});
  if (!row) {{
    return {{
      ok: false,
      status: "loading",
      reason: "thread row not visible",
      href,
      routeIds,
      candidates: rows.slice(0, 25).map((candidate) => rowIds(candidate))
    }};
  }}
  const target =
    row.querySelector('a[href], [role="link"], .bog, .y6') ||
    row;
  setTimeout(() => activate(target), 0);
  return {{ ok: true, href, routeIds, rowIds: rowIds(row) }};
}})()
"#
    )
}

fn gmail_delete_script(route_ids: &[String]) -> String {
    let route_ids = json!(route_ids
        .iter()
        .map(|value| value.trim().trim_start_matches('#').to_string())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>())
    .to_string();
    format!(
        r#"
(() => {{
  const routeIds = {route_ids}
    .map((value) => String(value || "").trim().replace(/^#/, "").toLowerCase())
    .filter((value, index, values) => value.length > 0 && values.indexOf(value) === index);
  const expected = routeIds[0] || "";
  const href = location.href || "";
  const hash = location.hash || "";
  const lowerHref = href.toLowerCase();
  const lowerHash = hash.toLowerCase();
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
  ].join(" ").replace(/\s+/g, " ").trim();
  const candidate = (node) => ({{
    tag: node.tagName || "",
    role: node.getAttribute("role") || "",
    ariaLabel: node.getAttribute("aria-label") || "",
    tooltip: node.getAttribute("data-tooltip") || "",
    title: node.getAttribute("title") || "",
    act: node.getAttribute("act") || "",
    text: label(node).slice(0, 120)
  }});
  const activate = (node) => {{
    for (const [type, Ctor] of [
      ["pointerdown", window.PointerEvent || window.MouseEvent],
      ["mousedown", window.MouseEvent],
      ["mouseup", window.MouseEvent],
      ["pointerup", window.PointerEvent || window.MouseEvent],
      ["click", window.MouseEvent]
    ]) {{
      node.dispatchEvent(new Ctor(type, {{ bubbles: true, cancelable: true, view: window }}));
    }}
    node.click();
  }};
  const routeActive = routeIds.some((id) => lowerHref.includes(id) || lowerHash.includes(id));
  if (!routeActive) {{
    return {{ ok: false, reason: "thread route not active", href, hash, expected, routeIds }};
  }}
  const buttons = Array.from(document.querySelectorAll('[aria-label], [data-tooltip], [role="button"], [act="10"]'))
    .filter(visible);
  const button = buttons.find((node) => {{
    const text = label(node).toLowerCase();
    if (/delete forever|delete label|delete draft|delete this message/.test(text)) return false;
    return node.getAttribute("act") === "10" ||
      /^delete\b/.test(text) ||
      /\bmove to (trash|bin)\b/.test(text);
  }});
  if (!button) {{
    return {{
      ok: false,
      reason: "delete button not visible",
      href,
      hash,
      labels: buttons.map(label).filter((value) => /delete|trash/i.test(value)).slice(0, 10),
      candidates: buttons.map(candidate).slice(0, 25)
    }};
  }}
  setTimeout(() => activate(button), 0);
  return {{ ok: true, label: label(button), href, hash }};
}})()
"#
    )
}

fn gmail_send_script(fields: &GmailComposeFields) -> String {
    let expected = json!({
        "to": fields.to.clone(),
        "cc": fields.cc.clone(),
        "bcc": fields.bcc.clone(),
        "subject": fields.subject.clone(),
        "body": fields.body.clone(),
    })
    .to_string();
    format!(
        r#"
(() => {{
  const expected = {expected};
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
  ].join(" ").replace(/\s+/g, " ").trim();
  const normalize = (value) => String(value || "").replace(/\s+/g, " ").trim();
  const expectedRecipients = []
    .concat(expected.to || [], expected.cc || [], expected.bcc || [])
    .map((value) => normalize(value).toLowerCase())
    .filter((value) => value.length > 0);
  const rootText = (root) => normalize([
    root.innerText || "",
    root.innerHTML || ""
  ].join(" ")).toLowerCase();
  const rootMatchesExpected = (root) => {{
    const haystack = rootText(root);
    const subject = normalize(expected.subject || "").toLowerCase();
    const body = normalize(expected.body || "").slice(0, 200).toLowerCase();
    const subjectOk = !subject || haystack.includes(subject);
    const bodyOk = !body || haystack.includes(body);
    const recipientsOk = expectedRecipients.length === 0 ||
      expectedRecipients.some((recipient) => haystack.includes(recipient));
    return subjectOk && bodyOk && recipientsOk;
  }};
  const disabled = (node) =>
    node.getAttribute("aria-disabled") === "true" ||
    node.getAttribute("disabled") !== null ||
    /disabled/i.test(node.getAttribute("aria-label") || "");
  const activate = (node) => {{
    for (const [type, Ctor] of [
      ["pointerdown", window.PointerEvent || window.MouseEvent],
      ["mousedown", window.MouseEvent],
      ["mouseup", window.MouseEvent],
      ["pointerup", window.PointerEvent || window.MouseEvent],
      ["click", window.MouseEvent]
    ]) {{
      node.dispatchEvent(new Ctor(type, {{ bubbles: true, cancelable: true, view: window }}));
    }}
    node.click();
  }};
  const sendButtons = Array.from(document.querySelectorAll('[aria-label], [data-tooltip], [role="button"]'))
    .filter(visible)
    .filter((node) => /^send\b/i.test(label(node)) && !/schedule/i.test(label(node)));
  const inspected = [];
  for (const node of sendButtons) {{
    const root = node.closest('[role="dialog"], .M9, .AD, .nH') || document.body;
    const preview = normalize(root.innerText || "").slice(0, 200);
    inspected.push({{ label: label(node), disabled: disabled(node), preview }});
    if (!rootMatchesExpected(root)) continue;
    if (disabled(node)) {{
      return {{ ok: false, reason: "send button disabled", label: label(node), preview }};
    }}
    activate(node);
    return {{ ok: true, label: label(node), scoped: true, preview }};
  }}
  return {{
    ok: false,
    reason: "send compose with expected content not visible",
    sendButtonCount: sendButtons.length,
    inspected
  }};
}})()
"#
    )
}

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
    fn expected_route_of_extracts_route_prefix() {
        assert_eq!(
            expected_route_of("https://mail.google.com/mail/u/0/#search/from%3Agithub"),
            "#search"
        );
        assert_eq!(
            expected_route_of("https://mail.google.com/mail/u/0/#label/foo%20bar"),
            "#label"
        );
        assert_eq!(
            expected_route_of("https://mail.google.com/mail/u/0/#category/social"),
            "#category"
        );
        assert_eq!(
            expected_route_of("https://mail.google.com/mail/u/0/#inbox"),
            "#inbox"
        );
        assert_eq!(
            expected_route_of("https://mail.google.com/mail/u/0/#sent"),
            "#sent"
        );
        // A bare base URL boots Gmail to the inbox.
        assert_eq!(
            expected_route_of("https://mail.google.com/mail/u/0/"),
            "#inbox"
        );
    }

    #[test]
    fn href_on_route_matches_route_prefix_not_exact_fragment() {
        // Gmail re-normalizes the hash (percent-decoding `newer_than:1d`), so
        // only the route prefix may be asserted (#582).
        let search = json!({"href": "https://mail.google.com/mail/u/0/#search/newer_than:1d"});
        assert!(href_on_route(&search, "#search"));
        assert!(!href_on_route(&search, "#inbox"));
        let inbox = json!({"href": "https://mail.google.com/mail/u/0/#inbox"});
        assert!(href_on_route(&inbox, "#inbox"));
        assert!(!href_on_route(&inbox, "#search"));
        // Mid-load pages without a committed fragment match no route yet.
        assert!(!href_on_route(
            &json!({"href": "https://mail.google.com/mail/u/0/"}),
            "#inbox"
        ));
        assert!(!href_on_route(&json!({}), "#search"));
    }

    #[test]
    fn href_on_route_rejects_thread_detail_for_plain_mailboxes() {
        // A thread-detail URL under a mailbox route is not a settled list view:
        // Gmail may land on `#trash/<thread>` immediately after deletion, but
        // list post-conditions must force the actual Trash row list (#777).
        assert!(!href_on_route(
            &json!({"href": "https://mail.google.com/mail/u/0/#trash/thread-a:r123"}),
            "#trash"
        ));
        assert!(!href_on_route(
            &json!({"href": "https://mail.google.com/mail/u/0/#sent/19f45de75da2cdd7"}),
            "#sent"
        ));
        assert!(href_on_route(
            &json!({"href": "https://mail.google.com/mail/u/0/#category/social"}),
            "#category"
        ));
    }

    #[test]
    fn pre_navigation_signature_blocks_rows_from_previous_gmail_view() {
        let previous = json!({
            "href": "https://mail.google.com/mail/u/0/#inbox",
            "rows": [{
                "id": "19f463fb7872a5a4",
                "threadId": "19f463fb7872a5a4",
            }]
        });
        let stale_search = json!({
            "href": "https://mail.google.com/mail/u/0/#search/from%3Anonexistent-xyz-99",
            "status": "ok",
            "rows": [{
                "id": "19f463fb7872a5a4",
                "threadId": "19f463fb7872a5a4",
            }]
        });

        let pre_navigation_signature = pre_navigation_row_signature(
            &previous,
            "https://mail.google.com/mail/u/0/#search/from%3Anonexistent-xyz-99",
        );

        assert_eq!(
            pre_navigation_signature.as_deref(),
            Some("19f463fb7872a5a4")
        );
        assert!(listing_reuses_pre_navigation_rows(
            &stale_search,
            pre_navigation_signature.as_deref()
        ));
    }

    #[test]
    fn pre_navigation_signature_allows_same_search_hash_after_normalization() {
        let previous = json!({
            "href": "https://mail.google.com/mail/u/0/#search/from:github",
            "rows": [{
                "id": "19f3b300efa7c96d",
                "threadId": "19f3b300efa7c96d",
            }]
        });

        assert_eq!(
            pre_navigation_row_signature(
                &previous,
                "https://mail.google.com/mail/u/0/#search/from%3Agithub"
            ),
            None
        );
    }

    #[test]
    fn empty_search_result_does_not_reuse_pre_navigation_rows() {
        let empty = json!({
            "href": "https://mail.google.com/mail/u/0/#search/from%3Anonexistent-xyz-99",
            "status": "ok",
            "empty": true,
            "rows": []
        });

        assert!(!listing_reuses_pre_navigation_rows(
            &empty,
            Some("19f463fb7872a5a4")
        ));
    }

    #[test]
    fn thread_id_input_prefers_gmail_thread_id_and_trims_hash() {
        let row_input = json!({
            "id": "19f45de75da2cdd7",
            "threadId": "19f45de75da2cdd7",
            "gmailThreadId": "#thread-a:r773037997993629613"
        });
        assert_eq!(
            gmail_thread_id(&row_input).unwrap(),
            "thread-a:r773037997993629613"
        );
    }

    #[test]
    fn thread_url_prefers_legacy_thread_id_for_navigation() {
        let row_input = json!({
            "category": "sent",
            "id": "19f45de75da2cdd7",
            "threadId": "19f45de75da2cdd7",
            "gmailThreadId": "#thread-a:r773037997993629613"
        });
        let raw_thread_id = gmail_thread_id(&row_input).unwrap();
        assert_eq!(raw_thread_id, "thread-a:r773037997993629613");
        assert_eq!(
            gmail_thread_url("me@example.com", &row_input, &raw_thread_id),
            "https://mail.google.com/mail/u/0/#sent/19f45de75da2cdd7"
        );
    }

    #[test]
    fn thread_source_list_url_accepts_search_url_but_not_thread_url() {
        let input = json!({
            "url": "https://mail.google.com/mail/u/0/#search/in%3Asent+%22Puffer%22",
        });
        assert_eq!(
            gmail_thread_source_list_url(&input, "thread-a:r123").as_deref(),
            Some("https://mail.google.com/mail/u/0/#search/in%3Asent+%22Puffer%22")
        );

        let thread_input = json!({
            "url": "https://mail.google.com/mail/u/0/#sent/19f45de75da2cdd7",
        });
        assert_eq!(
            gmail_thread_source_list_url(&thread_input, "thread-a:r123"),
            None
        );
    }

    #[test]
    fn open_thread_row_script_targets_raw_and_legacy_row_ids() {
        let ids = gmail_thread_route_ids("thread-a:r123", "19f45de75da2cdd7", None);
        let script = gmail_open_thread_row_script(&ids);

        assert!(script.contains("thread-a:r123"));
        assert!(script.contains("19f45de75da2cdd7"));
        assert!(script.contains("data-legacy-thread-id"));
        assert!(script.contains("data-thread-id"));
        assert!(script.contains("thread row not visible"));
        assert!(script.contains("setTimeout(() => activate(target), 0)"));
    }

    #[test]
    fn network_idle_timeout_leaves_budget_for_cold_route_confirmation() {
        // Once waitNetworkIdle times out, the idle signal is not reliable for
        // this Gmail view; warm-navigation idle waits are skipped so the shared
        // list-read deadline still leaves room for confirmation scrapes.
        let worst_case_idle_ms = GMAIL_NETWORK_IDLE_TIMEOUT.as_millis();
        let confirmation_ms = GMAIL_EVALUATE_INTERVAL.as_millis() * 2;
        assert!(
            worst_case_idle_ms + confirmation_ms < GMAIL_LOAD_TIMEOUT.as_millis(),
            "network idle timeouts can exhaust Gmail list-read deadline before route/signature confirmation"
        );
    }

    #[test]
    fn inbox_script_recognizes_zero_result_search_as_empty() {
        // A legitimate zero-hit search must yield `status:"ok", rows:[]`, not an
        // eternal `loading` (#777 adjacent defect).
        assert!(GMAIL_INBOX_SCRIPT.contains("no messages matched"));
    }

    #[test]
    fn send_preflight_script_populates_compose_and_waits_for_autosave() {
        let fields = GmailComposeFields {
            to: vec!["me@example.com".to_string()],
            cc: Vec::new(),
            bcc: Vec::new(),
            subject: "Puffer send preflight subject".to_string(),
            body: "Puffer send preflight body".to_string(),
        };
        let script = gmail_prepare_send_script(&fields);
        assert!(script.contains("Puffer send preflight subject"));
        assert!(script.contains("Puffer send preflight body"));
        assert!(script.contains("draft recipients were populated"));
        assert!(script.contains("textarea[name=\"to\"]"));
        assert!(script.contains("draft body was populated"));
        assert!(script.contains("draft subject was populated"));
        assert!(script.contains("Gmail is still saving the draft"));
    }

    #[test]
    fn send_script_refuses_disabled_send_button() {
        let fields = GmailComposeFields {
            to: vec!["me@example.com".to_string()],
            cc: Vec::new(),
            bcc: Vec::new(),
            subject: "Puffer disabled send subject".to_string(),
            body: "Puffer disabled send body".to_string(),
        };
        let script = gmail_send_script(&fields);
        assert!(script.contains("aria-disabled"));
        assert!(script.contains("send button disabled"));
    }

    #[test]
    fn send_script_scopes_click_to_expected_compose() {
        let fields = GmailComposeFields {
            to: vec!["me@example.com".to_string()],
            cc: Vec::new(),
            bcc: Vec::new(),
            subject: "Puffer scoped send subject".to_string(),
            body: "Puffer scoped send body".to_string(),
        };
        let script = gmail_send_script(&fields);
        assert!(script.contains("Puffer scoped send subject"));
        assert!(script.contains("Puffer scoped send body"));
        assert!(script.contains("send compose with expected content not visible"));
        assert!(script.contains("closest('[role=\"dialog\"], .M9, .AD, .nH')"));
        assert!(script.contains("pointerdown"));
    }

    #[test]
    fn gmail_evaluate_retries_browser_channel_timeouts_only() {
        assert!(gmail_evaluate_error_retryable(
            "evaluate Gmail action script: timed out waiting for browser evaluation: timed out waiting on channel"
        ));
        assert!(gmail_evaluate_error_retryable(
            "timed out waiting for browser evaluation"
        ));
        assert!(!gmail_evaluate_error_retryable(
            "evaluate Gmail action script: javascript exception"
        ));
        assert!(!gmail_evaluate_error_retryable(
            "open Gmail browser tab: connection refused"
        ));
    }

    #[test]
    fn delete_script_requires_thread_route_and_precise_delete_button() {
        let ids = gmail_delete_route_ids("thread-a:r123", "thread-a:r123", None);
        let script = gmail_delete_script(&ids);
        assert!(script.contains("thread-a:r123"));
        assert!(script.contains("thread route not active"));
        assert!(script.contains("delete button not visible"));
        assert!(script.contains("pointerdown"));
        assert!(script.contains("delete forever"));
    }

    #[test]
    fn delete_script_accepts_navigation_thread_id_for_route_check() {
        let ids = gmail_delete_route_ids("thread-a:r123", "19f45de75da2cdd7", None);
        let script = gmail_delete_script(&ids);
        assert!(script.contains("thread-a:r123"));
        assert!(script.contains("19f45de75da2cdd7"));
        assert!(script.contains("routeIds.some"));
    }

    #[test]
    fn thread_ready_route_id_extracts_opaque_hash_segment() {
        assert_eq!(
            gmail_thread_ready_route_id(&json!({
                "hash": "#sent/FFNDWNFRqzrJMnlflbszdrLcpnGKtfdF",
            }))
            .as_deref(),
            Some("FFNDWNFRqzrJMnlflbszdrLcpnGKtfdF")
        );
        assert_eq!(
            gmail_thread_ready_route_id(&json!({
                "href": "https://mail.google.com/mail/u/0/#sent/FFNDWNFRqzrJMnlflbszdrLcpnGKtfdF",
            }))
            .as_deref(),
            Some("FFNDWNFRqzrJMnlflbszdrLcpnGKtfdF")
        );
        assert_eq!(
            gmail_thread_ready_route_id(&json!({
                "hash": "#sent",
            })),
            None
        );
    }

    #[test]
    fn delete_route_ids_include_current_ready_hash_segment() {
        let ids = gmail_delete_route_ids(
            "thread-a:r123",
            "19f45de75da2cdd7",
            Some("FFNDWNFRqzrJMnlflbszdrLcpnGKtfdF"),
        );

        assert_eq!(
            ids,
            vec![
                "thread-a:r123".to_string(),
                "19f45de75da2cdd7".to_string(),
                "FFNDWNFRqzrJMnlflbszdrLcpnGKtfdF".to_string(),
            ]
        );
        let script = gmail_delete_script(&ids);
        assert!(script.contains("thread-a:r123"));
        assert!(script.contains("19f45de75da2cdd7"));
        assert!(script.contains("FFNDWNFRqzrJMnlflbszdrLcpnGKtfdF"));
    }

    #[test]
    fn listing_contains_any_thread_id_accepts_delete_route_id_set() {
        let listing = json!({
            "rows": [{
                "id": "19f45de75da2cdd7",
                "threadId": "19f45de75da2cdd7",
                "legacyThreadId": "19f45de75da2cdd7",
            }]
        });
        let ids = gmail_delete_route_ids(
            "thread-a:r773037997993629613",
            "19f45de75da2cdd7",
            Some("FFNDWNFRqzrJMnlflbszdrLcpnGKtfdF"),
        );

        assert!(!listing_contains_thread(
            &listing,
            "thread-a:r773037997993629613"
        ));
        assert!(listing_contains_any_thread_id(&listing, &ids));
    }

    #[test]
    fn delete_script_targets_gmail_delete_action_button() {
        let ids = gmail_delete_route_ids("thread-a:r123", "19f45de75da2cdd7", None);
        let script = gmail_delete_script(&ids);
        assert!(script.contains("[act=\"10\"]"));
        assert!(script.contains("getAttribute(\"act\") === \"10\""));
        assert!(script.contains("move to (trash|bin)"));
    }

    #[test]
    fn delete_script_schedules_destructive_click_after_returning_result() {
        let ids = gmail_delete_route_ids("thread-a:r123", "19f45de75da2cdd7", None);
        let script = gmail_delete_script(&ids);

        assert!(script.contains("setTimeout(() => activate(button), 0)"));
    }

    #[test]
    fn delete_script_reports_visible_button_candidates_when_delete_missing() {
        let ids = gmail_delete_route_ids("thread-a:r123", "19f45de75da2cdd7", None);
        let script = gmail_delete_script(&ids);
        assert!(script.contains("candidates"));
        assert!(script.contains("ariaLabel"));
        assert!(script.contains("tooltip"));
        assert!(script.contains("act"));
    }

    #[test]
    fn delete_click_retries_while_toolbar_button_is_still_loading() {
        assert!(gmail_delete_click_retryable(&json!({
            "ok": false,
            "reason": "delete button not visible",
            "href": "https://mail.google.com/mail/u/0/#sent/19f45de75da2cdd7",
        })));
        assert!(!gmail_delete_click_retryable(&json!({
            "ok": false,
            "reason": "thread route not active",
            "href": "https://mail.google.com/mail/u/0/#sent",
        })));
    }

    #[test]
    fn delete_click_error_message_includes_script_diagnostics() {
        let message = gmail_delete_click_error_message(
            "thread-a:r123",
            &json!({
                "ok": false,
                "reason": "delete button not visible",
                "href": "https://mail.google.com/mail/u/0/#sent/19f45de75da2cdd7",
                "hash": "#sent/19f45de75da2cdd7",
                "labels": ["Archive", "More"],
                "candidates": [{"ariaLabel": "More", "tooltip": "More", "act": "20"}],
            }),
        );
        assert!(message.contains("delete button not visible"));
        assert!(message.contains("labels"));
        assert!(message.contains("Archive"));
        assert!(message.contains("candidates"));
        assert!(message.contains("More"));
        assert!(message.contains("#sent/19f45de75da2cdd7"));
    }

    #[test]
    fn thread_ready_script_is_not_coupled_to_reply_controls() {
        let script = gmail_thread_ready_script("thread-a:r123");
        assert!(script.contains("routeReady"));
        assert!(script.contains("gmailChrome"));
        assert!(!script.contains("replyButton || draftBody || threadChrome"));
    }

    #[test]
    fn thread_ready_script_requires_thread_detail_not_only_gmail_chrome() {
        let script = gmail_thread_ready_script("thread-a:r123");
        assert!(script.contains("threadDetailReady"));
        assert!(script.contains("routeReady && threadDetailReady"));
        assert!(!script.contains("routeReady && gmailChrome"));
    }

    #[test]
    fn thread_ready_script_avoids_generic_gmail_shell_classes() {
        let script = gmail_thread_ready_script("thread-a:r123");
        assert!(!script.contains(".gs"));
    }

    #[test]
    fn thread_ready_script_avoids_generic_page_headings() {
        let script = gmail_thread_ready_script("thread-a:r123");
        assert!(!script.contains("h1, h2"));
        assert!(!script.contains("[role=\"heading\"]"));
        assert!(script.contains(".hP"));
    }

    #[test]
    fn thread_ready_decision_retries_loading_result() {
        let decision = gmail_thread_ready_decision(
            "me@example.com",
            "thread-a:r123",
            &json!({
                "ok": false,
                "status": "loading",
                "href": "https://mail.google.com/mail/u/0/#sent/19f45de75da2cdd7",
                "routeReady": true,
                "threadDetailReady": false,
            }),
        )
        .unwrap();

        assert_eq!(decision, GmailThreadReadyDecision::Pending);
    }

    #[test]
    fn thread_ready_decision_accepts_ready_result() {
        let result = json!({
            "ok": true,
            "status": "ok",
            "href": "https://mail.google.com/mail/u/0/#sent/19f45de75da2cdd7",
            "threadDetailReady": true,
        });
        let decision =
            gmail_thread_ready_decision("me@example.com", "thread-a:r123", &result).unwrap();

        assert_eq!(decision, GmailThreadReadyDecision::Ready(result));
    }

    #[test]
    fn thread_ready_timeout_message_includes_last_probe_diagnostics() {
        let message = gmail_thread_ready_timeout_message(
            "me@example.com",
            "thread-a:r123",
            &json!({
                "status": "loading",
                "href": "https://mail.google.com/mail/u/0/#sent/19f463fb7872a5a4",
                "hash": "#sent/19f463fb7872a5a4",
                "routeReady": true,
                "threadDetailReady": false,
                "bodyText": "Loading conversation"
            }),
        );

        assert!(message.contains("thread-a:r123"));
        assert!(message.contains("me@example.com"));
        assert!(message.contains("https://mail.google.com/mail/u/0/#sent/19f463fb7872a5a4"));
        assert!(message.contains("#sent/19f463fb7872a5a4"));
        assert!(message.contains("routeReady=true"));
        assert!(message.contains("threadDetailReady=false"));
        assert!(message.contains("Loading conversation"));
    }

    #[test]
    fn thread_ready_needs_warm_reopen_for_hash_only_inbox_shell() {
        let result = json!({
            "status": "loading",
            "href": "https://mail.google.com/mail/u/0/#sent/19f463fb7872a5a4",
            "hash": "#sent/19f463fb7872a5a4",
            "routeReady": true,
            "bodyText": "\nSkip to content\nSearch mail\nMail\nCompose\nLabels\nInbox\n1,484"
        });

        assert!(gmail_thread_ready_needs_warm_reopen(&result));
        assert!(!gmail_thread_ready_needs_warm_reopen(&json!({
            "status": "loading",
            "href": "https://mail.google.com/mail/u/0/#sent",
            "hash": "#sent",
            "routeReady": false,
            "bodyText": "\nSkip to content\nSearch mail\nMail\nCompose\nLabels\nInbox\n1,484"
        })));
        assert!(!gmail_thread_ready_needs_warm_reopen(&json!({
            "status": "loading",
            "href": "https://mail.google.com/mail/u/0/#sent/19f463fb7872a5a4",
            "hash": "#sent/19f463fb7872a5a4",
            "routeReady": true,
            "threadDetailReady": true,
            "bodyText": "Puffer Gmail E2E 777 FINAL"
        })));
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

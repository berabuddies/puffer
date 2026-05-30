//! Google Calendar browser action handlers.

use super::{GcalBrowserConfig, SubscriberEnv};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

const ACTION_TIMEOUT: Duration = Duration::from_secs(20);
const ACTION_INTERVAL: Duration = Duration::from_millis(800);

pub(super) fn handle_action(
    env: &SubscriberEnv,
    config: &GcalBrowserConfig,
    handshake: &mut Option<crate::daemon::Handshake>,
    action: &str,
    input: &Value,
) -> Result<Value> {
    let action = canonical_action(action);
    match action {
        "list_events" => list_events(env, config, handshake, input),
        "get_detail" => get_detail(env, config, handshake, input),
        "accept" => rsvp(env, config, handshake, input, "accept"),
        "deny" => rsvp(env, config, handshake, input, "deny"),
        other => bail!("unsupported gcal-browser action `{other}`"),
    }
}

fn canonical_action(action: &str) -> &str {
    match action {
        "list"
        | "list_event"
        | "list_calendar_events"
        | "events"
        | "search"
        | "search_event"
        | "search_events" => "list_events",
        "get_details" | "get_event" | "event_detail" | "detail" => "get_detail",
        "accept_event" | "yes" => "accept",
        "decline" | "reject" | "no" => "deny",
        other => other,
    }
}

fn list_events(
    env: &SubscriberEnv,
    config: &GcalBrowserConfig,
    handshake: &mut Option<crate::daemon::Handshake>,
    input: &Value,
) -> Result<Value> {
    let account = action_account(config, input)?;
    let filter = EventListFilter::from_input(input);
    let handshake = super::ensure_browser_daemon(config, handshake)?;
    open_gcal_url(
        env,
        &account,
        handshake,
        &super::gcal_agenda_url(&account),
        "Google Calendar",
    )?;
    let result = wait_for_events(env, &account, handshake)?;
    let rows = result
        .get("rows")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let events = rows
        .into_iter()
        .filter(|row| filter.matches(row))
        .take(filter.limit)
        .collect::<Vec<_>>();
    Ok(json!({
        "action": "list_events",
        "summary": format!("listed {} Google Calendar events", events.len()),
        "account": account,
        "count": events.len(),
        "events": events,
        "source": {
            "href": result.get("href").cloned().unwrap_or(Value::Null),
            "title": result.get("title").cloned().unwrap_or(Value::Null),
        },
    }))
}

fn get_detail(
    env: &SubscriberEnv,
    config: &GcalBrowserConfig,
    handshake: &mut Option<crate::daemon::Handshake>,
    input: &Value,
) -> Result<Value> {
    let account = action_account(config, input)?;
    let target = EventTarget::from_input(input)?;
    let handshake = super::ensure_browser_daemon(config, handshake)?;
    open_event(env, &account, handshake, &target)?;
    let detail = wait_for_detail(env, &account, handshake)?;
    Ok(json!({
        "action": "get_detail",
        "summary": "loaded Google Calendar event details",
        "account": account,
        "event": detail,
    }))
}

fn rsvp(
    env: &SubscriberEnv,
    config: &GcalBrowserConfig,
    handshake: &mut Option<crate::daemon::Handshake>,
    input: &Value,
    response: &str,
) -> Result<Value> {
    let account = action_account(config, input)?;
    let target = EventTarget::from_input(input)?;
    let handshake = super::ensure_browser_daemon(config, handshake)?;
    open_event(env, &account, handshake, &target)?;
    let detail = wait_for_detail(env, &account, handshake)?;
    let result = evaluate_gcal_script(env, &account, handshake, &gcal_rsvp_script(response))
        .with_context(|| format!("click Google Calendar RSVP `{response}`"))?;
    if !result.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        bail!(
            "Google Calendar RSVP `{response}` button was not found: {}",
            result
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("unknown reason")
        );
    }
    Ok(json!({
        "action": response,
        "summary": format!("set Google Calendar RSVP to {response}"),
        "account": account,
        "clicked": result.get("label").cloned().unwrap_or(Value::Null),
        "event": detail,
    }))
}

fn open_event(
    env: &SubscriberEnv,
    account: &str,
    handshake: &crate::daemon::Handshake,
    target: &EventTarget,
) -> Result<()> {
    if let Some(url) = &target.url {
        open_gcal_url(env, account, handshake, url, "Google Calendar event")?;
        return Ok(());
    }
    open_gcal_url(
        env,
        account,
        handshake,
        &super::gcal_agenda_url(account),
        "Google Calendar",
    )?;
    let result = evaluate_gcal_script(env, account, handshake, &gcal_open_event_script(target))
        .context("open Google Calendar event from agenda")?;
    if result.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        return Ok(());
    }
    bail!(
        "Google Calendar event was not found: {}",
        result
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("no matching visible event")
    )
}

fn open_gcal_url(
    env: &SubscriberEnv,
    account: &str,
    handshake: &crate::daemon::Handshake,
    url: &str,
    label: &str,
) -> Result<()> {
    crate::daemon_browser::send_daemon_request(
        handshake,
        "browser_agent",
        json!({
            "action": "open",
            "sessionId": super::root_session_id(&env.topic),
            "tabId": super::safe_session_part(account),
            "label": label,
            "url": url,
            "width": super::BROWSER_WIDTH,
            "height": super::BROWSER_HEIGHT,
            "activate": false,
        }),
    )
    .with_context(|| format!("open Google Calendar browser tab at {url}"))?;
    wait_gcal_ready(env, account, handshake)
}

fn wait_gcal_ready(
    env: &SubscriberEnv,
    account: &str,
    handshake: &crate::daemon::Handshake,
) -> Result<()> {
    let deadline = Instant::now() + ACTION_TIMEOUT;
    loop {
        let result = evaluate_gcal_script(env, account, handshake, GCAL_READY_SCRIPT)
            .context("inspect Google Calendar browser tab")?;
        match result.get("status").and_then(Value::as_str).unwrap_or("ok") {
            "ok" => return Ok(()),
            "auth_required" => bail!("Google Calendar account `{account}` requires sign-in"),
            _ if Instant::now() >= deadline => {
                bail!(
                    "Google Calendar account `{account}` returned status `{}`",
                    result
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("loading")
                );
            }
            _ => std::thread::sleep(ACTION_INTERVAL),
        }
    }
}

fn wait_for_detail(
    env: &SubscriberEnv,
    account: &str,
    handshake: &crate::daemon::Handshake,
) -> Result<Value> {
    let deadline = Instant::now() + ACTION_TIMEOUT;
    loop {
        let result = evaluate_gcal_script(env, account, handshake, GCAL_DETAIL_SCRIPT)
            .context("read Google Calendar event detail")?;
        if result.get("ok").and_then(Value::as_bool).unwrap_or(false) {
            return Ok(result);
        }
        if Instant::now() >= deadline {
            bail!(
                "Google Calendar event detail was not visible: {}",
                result
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("detail panel not found")
            );
        }
        std::thread::sleep(ACTION_INTERVAL);
    }
}

fn wait_for_events(
    env: &SubscriberEnv,
    account: &str,
    handshake: &crate::daemon::Handshake,
) -> Result<Value> {
    let deadline = Instant::now() + ACTION_TIMEOUT;
    loop {
        let result = evaluate_gcal_script(env, account, handshake, super::GCAL_EVENTS_SCRIPT)
            .context("read Google Calendar event list")?;
        let status = result.get("status").and_then(Value::as_str).unwrap_or("ok");
        match status {
            "auth_required" => bail!("Google Calendar account `{account}` requires sign-in"),
            "ok" => return Ok(result),
            _ if super::gcal_poll_result_ready(&result) => return Ok(result),
            _ if Instant::now() >= deadline => {
                bail!(
                    "Google Calendar account `{account}` returned status `{}`",
                    result
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("loading")
                );
            }
            _ => std::thread::sleep(ACTION_INTERVAL),
        }
    }
}

fn evaluate_gcal_script(
    env: &SubscriberEnv,
    account: &str,
    handshake: &crate::daemon::Handshake,
    script: &str,
) -> Result<Value> {
    let value = crate::daemon_browser::send_daemon_request(
        handshake,
        "browser_agent",
        json!({
            "action": "evaluate",
            "sessionId": super::root_session_id(&env.topic),
            "tabId": super::safe_session_part(account),
            "width": super::BROWSER_WIDTH,
            "height": super::BROWSER_HEIGHT,
            "script": script,
        }),
    )?;
    Ok(value.get("value").cloned().unwrap_or(Value::Null))
}

fn action_account(config: &GcalBrowserConfig, input: &Value) -> Result<String> {
    if let Some(account) = string_field(input, &["account", "account_slug", "email"]) {
        let normalized = account.to_ascii_lowercase();
        if config.accounts.iter().any(|item| item == &normalized) {
            return Ok(normalized);
        }
        bail!("Google Calendar account `{account}` is not configured");
    }
    config
        .accounts
        .first()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("gcal-browser connector has no configured accounts"))
}

fn string_field(input: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| input.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

#[derive(Debug)]
struct EventTarget {
    id: Option<String>,
    title: Option<String>,
    url: Option<String>,
}

impl EventTarget {
    fn from_input(input: &Value) -> Result<Self> {
        let target = Self {
            id: string_field(input, &["event_id", "id", "calendar_event_id"]),
            title: string_field(
                input,
                &["title", "summary", "event_title", "query", "q", "search"],
            ),
            url: string_field(input, &["url", "event_url"]),
        };
        if target.id.is_none() && target.title.is_none() && target.url.is_none() {
            bail!(
                "provide `event_id`, `title`, or `url` for the Google Calendar event, or call `list_events` first"
            );
        }
        Ok(target)
    }
}

#[derive(Debug)]
struct EventListFilter {
    query: Option<String>,
    title: Option<String>,
    id: Option<String>,
    url: Option<String>,
    when: Option<String>,
    location: Option<String>,
    limit: usize,
}

impl EventListFilter {
    fn from_input(input: &Value) -> Self {
        Self {
            query: string_field(input, &["query", "q", "search", "contains", "text"]),
            title: string_field(input, &["title", "summary", "event_title"]),
            id: string_field(input, &["event_id", "id", "calendar_event_id"]),
            url: string_field(input, &["url", "event_url"]),
            when: string_field(input, &["when", "date", "time"]),
            location: string_field(input, &["location", "where"]),
            limit: numeric_field(input, &["limit", "max_results", "max"]).unwrap_or(10),
        }
    }

    fn matches(&self, row: &Value) -> bool {
        contains_optional(row_text(row), self.query.as_deref())
            && contains_optional(row_field(row, "title"), self.title.as_deref())
            && contains_optional(row_field(row, "id"), self.id.as_deref())
            && contains_optional(row_field(row, "url"), self.url.as_deref())
            && contains_optional(row_field(row, "when"), self.when.as_deref())
            && contains_optional(row_field(row, "location"), self.location.as_deref())
    }
}

fn numeric_field(input: &Value, keys: &[&str]) -> Option<usize> {
    keys.iter()
        .find_map(|key| input.get(*key).and_then(Value::as_u64))
        .map(|value| value.clamp(1, 50) as usize)
}

fn row_field(row: &Value, key: &str) -> String {
    row.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn row_text(row: &Value) -> String {
    ["id", "title", "summary", "when", "location", "url"]
        .into_iter()
        .map(|key| row_field(row, key))
        .filter(|value| !value.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn contains_optional(haystack: String, needle: Option<&str>) -> bool {
    let Some(needle) = needle.map(str::trim).filter(|value| !value.is_empty()) else {
        return true;
    };
    haystack
        .to_ascii_lowercase()
        .contains(&needle.to_ascii_lowercase())
}

fn gcal_open_event_script(target: &EventTarget) -> String {
    let target_json = json!({
        "id": target.id,
        "title": target.title,
    })
    .to_string();
    format!(
        r#"
(() => {{
  const target = {target_json};
  const clean = (value) => String(value || "").trim().replace(/\s+/g, " ");
  const visible = (node) => {{
    if (!node) return false;
    const rect = node.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0;
  }};
  const attr = (node, name) => node && node.getAttribute ? clean(node.getAttribute(name)) : "";
  const text = (node) => clean(node && node.textContent);
  const hash = (value) => {{
    let h = 2166136261;
    for (let i = 0; i < value.length; i += 1) {{
      h ^= value.charCodeAt(i);
      h = Math.imul(h, 16777619);
    }}
    return `fallback-${{(h >>> 0).toString(36)}}`;
  }};
  const nodes = Array.from(document.querySelectorAll([
    "[data-eventid]",
    "[data-event-id]",
    "[data-eid]",
    "a[href*='event']",
    "a[href*='eid=']",
    "[role='button'][aria-label]",
    "[role='link'][aria-label]"
  ].join(",")));
  for (const node of nodes) {{
    if (!visible(node)) continue;
    const label = clean([attr(node, "aria-label"), attr(node, "title"), text(node)].filter(Boolean).join(" "));
    const id =
      attr(node, "data-eventid") ||
      attr(node, "data-event-id") ||
      attr(node, "data-eid") ||
      attr(node, "data-id") ||
      attr(node, "href") ||
      hash(label);
    const idMatch = target.id && id && id.includes(target.id);
    const titleMatch = target.title && label.toLowerCase().includes(String(target.title).toLowerCase());
    if (idMatch || titleMatch) {{
      node.dispatchEvent(new MouseEvent("click", {{ bubbles: true, cancelable: true, view: window }}));
      return {{ ok: true, id, label }};
    }}
  }}
  return {{ ok: false, reason: "no matching visible event" }};
}})()
"#
    )
}

fn gcal_rsvp_script(response: &str) -> String {
    let labels = if response == "accept" {
        json!(["yes", "accept", "going"])
    } else {
        json!(["no", "decline", "deny", "not going"])
    }
    .to_string();
    format!(
        r#"
(() => {{
  const wanted = new Set({labels});
  const visible = (node) => {{
    if (!node) return false;
    const rect = node.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0;
  }};
  const clean = (value) => String(value || "").trim().replace(/\s+/g, " ");
  const labelFor = (node) => clean([
    node.getAttribute && node.getAttribute("aria-label"),
    node.getAttribute && node.getAttribute("title"),
    node.textContent
  ].filter(Boolean).join(" ")).toLowerCase();
  const nodes = Array.from(document.querySelectorAll("button,[role='button']"));
  for (const node of nodes) {{
    if (!visible(node)) continue;
    const label = labelFor(node);
    if ([...wanted].some((item) => label === item || label.includes(item))) {{
      node.dispatchEvent(new MouseEvent("click", {{ bubbles: true, cancelable: true, view: window }}));
      return {{ ok: true, label }};
    }}
  }}
  return {{ ok: false, reason: "RSVP button not visible" }};
}})()
"#
    )
}

const GCAL_READY_SCRIPT: &str = r#"
(() => {
  const href = location.href;
  const title = document.title || "";
  const host = location.hostname || "";
  const bodyText = document.body ? document.body.innerText || "" : "";
  const signinLike =
    host.includes("accounts.google.com") ||
    /ServiceLogin|signin|identifier/.test(href) ||
    (/sign in/i.test(title) && !/calendar/i.test(title));
  if (signinLike) return { status: "auth_required", href, title };
  if (host.includes("calendar.google.") && document.readyState === "complete") {
    return { status: "ok", href, title };
  }
  return { status: "loading", href, title, bodyText: bodyText.slice(0, 120) };
})()
"#;

const GCAL_DETAIL_SCRIPT: &str = r#"
(() => {
  const visible = (node) => {
    if (!node) return false;
    const rect = node.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0;
  };
  const clean = (value) => String(value || "").trim().replace(/\s+/g, " ");
  const panels = Array.from(document.querySelectorAll([
    "[role='dialog']",
    "[aria-label*='Event details']",
    "[data-eventid]",
    "[data-eid]"
  ].join(","))).filter(visible);
  const panel = panels.find((node) => /edit|delete|guest|calendar|going|yes|no|meet|location|description/i.test(node.innerText || ""));
  if (!panel) return { ok: false, reason: "detail panel not found" };
  const titleNode = panel.querySelector("[role='heading'], h1, h2, [aria-level='1'], [aria-level='2']");
  const bodyText = clean(panel.innerText || panel.textContent || "");
  return {
    ok: true,
    href: location.href,
    title: clean(titleNode ? titleNode.textContent : "").replace(/^Event details\s*/i, "") || bodyText.split(/\n/)[0] || "",
    bodyText,
    eventId:
      panel.getAttribute("data-eventid") ||
      panel.getAttribute("data-event-id") ||
      panel.getAttribute("data-eid") ||
      ""
  };
})()
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalizes_action_aliases() {
        assert_eq!(canonical_action("search_events"), "list_events");
        assert_eq!(canonical_action("get_details"), "get_detail");
        assert_eq!(canonical_action("decline"), "deny");
        assert_eq!(canonical_action("accept_event"), "accept");
    }

    #[test]
    fn event_target_requires_identifier() {
        assert!(EventTarget::from_input(&json!({})).is_err());
        assert!(EventTarget::from_input(&json!({ "event_id": "abc" })).is_ok());
        assert!(EventTarget::from_input(&json!({ "title": "Planning" })).is_ok());
        assert!(EventTarget::from_input(&json!({ "url": "https://calendar.google.com/" })).is_ok());
    }

    #[test]
    fn event_list_filter_matches_rows_case_insensitively() {
        let filter = EventListFilter::from_input(&json!({
            "query": "planning",
            "limit": 200
        }));
        assert_eq!(filter.limit, 50);
        assert!(filter.matches(&json!({
            "id": "event-1",
            "title": "Planning Review",
            "when": "Tomorrow 10 AM",
            "url": "https://calendar.google.com/event"
        })));
        assert!(!filter.matches(&json!({
            "id": "event-2",
            "title": "Lunch",
            "when": "Tomorrow noon"
        })));
    }
}

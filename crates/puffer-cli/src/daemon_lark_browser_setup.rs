//! Daemon-owned connect/login flow for the Lark/Feishu browser connector.

use crate::daemon::{DaemonState, ServerEnvelope};
use crate::lark_browser::Brand;
use anyhow::{bail, Context, Result};
use puffer_core::{CancelToken, UserQuestionPromptResponse};
use puffer_subscriptions::{ConnectionRecord, ConnectionState};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

const QR_SIGN_IN_QUESTION: &str =
    "Scan the QR shown on the left with your Lark/Feishu mobile app, then choose Continue.";
const SETUP_TAB_ID: &str = "lark-qr";
const BROWSER_WIDTH: u32 = 1100;
const BROWSER_HEIGHT: u32 = 820;
const LOGIN_POLL_TIMEOUT: Duration = Duration::from_secs(120);
const LOGIN_POLL_INTERVAL: Duration = Duration::from_secs(2);

type PendingQuestions = Arc<Mutex<HashMap<String, mpsc::Sender<UserQuestionPromptResponse>>>>;

struct SetupFlow {
    state: Arc<DaemonState>,
    channel: String,
    turn_id: String,
    next_request_id: Arc<AtomicU64>,
    pending_questions: PendingQuestions,
    cancel: CancelToken,
    session_id: String,
    connection_slug: String,
    brand: Brand,
}

pub(crate) fn connect_args_are_lark_browser(connect_args: &str) -> bool {
    brand_from_connect_args(connect_args).is_some()
}

pub(crate) fn brand_from_connect_args(connect_args: &str) -> Option<Brand> {
    connect_args.split_whitespace().next().and_then(Brand::from_slug)
}

/// Executes daemon-native Lark/Feishu browser connector setup.
pub(crate) fn execute_lark_browser_setup(
    state: Arc<DaemonState>,
    channel: String,
    turn_id: String,
    connect_args: String,
    next_request_id: Arc<AtomicU64>,
    pending_questions: PendingQuestions,
    cancel: CancelToken,
) -> Result<String> {
    let brand = brand_from_connect_args(&connect_args)
        .context("unknown lark brand slug")?;
    let (connection_slug, _) = parse_setup_target(&connect_args, brand)?;
    let session_id = format!(
        "lark-browser-setup-{}",
        safe_session_part(&turn_id)
    );
    let mut flow = SetupFlow {
        state,
        channel,
        turn_id,
        next_request_id,
        pending_questions,
        cancel,
        session_id,
        connection_slug,
        brand,
    };
    flow.run()
}

impl SetupFlow {
    fn run(&mut self) -> Result<String> {
        self.cancel.check()?;
        self.open_url(self.brand.web_url(), "Lark/Feishu")?;
        self.poll_until_logged_in()?;
        crate::lark_browser::save_config(
            self.state.config_paths(),
            self.state.cwd_path(),
            self.brand,
            &self.connection_slug,
        )?;
        let registered = upsert_connection(&self.connection_slug, self.brand)?;
        let action = if registered { "created" } else { "updated" };
        Ok(format!(
            "Connected {} ({}) ({action}).",
            self.connection_slug,
            self.brand.slug()
        ))
    }

    fn open_url(&self, url: &str, label: &str) -> Result<()> {
        crate::daemon_browser::handle_browser_agent(
            &self.state,
            &json!({
                "action": "open",
                "sessionId": &self.session_id,
                "tabId": SETUP_TAB_ID,
                "label": label,
                "url": url,
                "width": BROWSER_WIDTH,
                "height": BROWSER_HEIGHT,
                "activate": true,
            }),
        )
        .with_context(|| format!("open Lark/Feishu setup browser at {url}"))?;
        Ok(())
    }

    fn poll_until_logged_in(&mut self) -> Result<()> {
        let deadline = Instant::now() + LOGIN_POLL_TIMEOUT;
        let mut asked = false;
        loop {
            self.cancel.check()?;
            let value = crate::daemon_browser::handle_browser_agent(
                &self.state,
                &json!({
                    "action": "evaluate",
                    "sessionId": &self.session_id,
                    "tabId": SETUP_TAB_ID,
                    "width": BROWSER_WIDTH,
                    "height": BROWSER_HEIGHT,
                    "script": crate::lark_browser_script::LARK_LOGIN_MARKER_JS,
                }),
            )
            .context("evaluate Lark login marker")?;
            let result_str = value
                .get("value")
                .and_then(Value::as_str)
                .unwrap_or("{}");
            let result: Value = serde_json::from_str(result_str).unwrap_or(Value::Null);
            let logged_in = result
                .get("loggedIn")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let href = result
                .get("href")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();

            if logged_in {
                return Ok(());
            }
            if Instant::now() >= deadline {
                bail!(
                    "Lark/Feishu login timed out after {}s (last URL: {})",
                    LOGIN_POLL_TIMEOUT.as_secs(),
                    href
                );
            }
            if !asked {
                asked = true;
                self.ask_sign_in(&href)?;
            }
            std::thread::sleep(LOGIN_POLL_INTERVAL);
        }
    }

    fn ask_sign_in(&self, href: &str) -> Result<()> {
        self.ask_questions(
            json!([{
                "type": "choice",
                "header": "Lark/Feishu QR sign in",
                "question": QR_SIGN_IN_QUESTION,
                "multiSelect": false,
                "options": []
            }]),
            json!({
                "browserSessionId": &self.session_id,
                "browserTabId": SETUP_TAB_ID,
                "browserUrl": href,
            }),
        )?;
        Ok(())
    }

    fn ask_questions(&self, questions: Value, extras: Value) -> Result<UserQuestionPromptResponse> {
        let request_id = self
            .next_request_id
            .fetch_add(1, Ordering::SeqCst)
            .to_string();
        let (tx, rx) = mpsc::channel();
        self.pending_questions
            .lock()
            .unwrap()
            .insert(request_id.clone(), tx);

        let mut payload = Map::new();
        payload.insert("type".to_string(), json!("user-question-request"));
        payload.insert("turnId".to_string(), json!(self.turn_id));
        payload.insert("requestId".to_string(), json!(request_id));
        payload.insert("questions".to_string(), questions);
        if let Some(extra) = extras.as_object() {
            for (key, value) in extra {
                payload.insert(key.clone(), value.clone());
            }
        }
        self.state.publish_event(ServerEnvelope::Event {
            event: self.channel.clone(),
            payload: Value::Object(payload),
        });

        rx.recv()
            .map_err(|_| anyhow::anyhow!("connector setup question channel closed"))
    }
}

/// Parse the connection slug from connect args.
/// Falls back to the brand slug if no second token is given.
fn parse_setup_target(connect_args: &str, brand: Brand) -> Result<(String, Brand)> {
    let mut parts = connect_args.split_whitespace();
    let _connector = parts.next().unwrap_or_default();
    let connection_slug = parts
        .next()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(brand.slug())
        .to_string();
    if parts.next().is_some() {
        bail!(
            "Usage: /connect {} <connection-name>",
            brand.slug()
        );
    }
    Ok((connection_slug, brand))
}

fn upsert_connection(connection: &str, brand: Brand) -> Result<bool> {
    let manager = puffer_core::subscription_manager()?;
    let description = format!("Lark/Feishu Browser ({})", brand.slug());
    let registered = if let Some(existing) = manager.connection_store().get(connection) {
        if existing.connector_slug != brand.slug() {
            bail!(
                "connection `{connection}` already exists for connector `{}`",
                existing.connector_slug
            );
        }
        manager.connection_store().update(connection, |record| {
            record.description = description.clone();
            record.state = ConnectionState::Authenticated;
            record.auth_failure_notified = false;
        })?;
        false
    } else {
        manager
            .connection_store()
            .create(ConnectionRecord::authenticated(
                connection,
                brand.slug(),
                description,
            ))?;
        true
    };
    manager.refresh_connection_consumers()?;
    manager.refresh_connection_auth()?;
    Ok(registered)
}

fn safe_session_part(value: &str) -> String {
    let mut output = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    while output.contains("--") {
        output = output.replace("--", "-");
    }
    let trimmed = output.trim_matches('-');
    if trimmed.is_empty() {
        "setup".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lark_browser::Brand;

    #[test]
    fn matches_both_slugs_only() {
        assert!(connect_args_are_lark_browser("lark-browser"));
        assert!(connect_args_are_lark_browser("feishu-browser work"));
        assert!(!connect_args_are_lark_browser("gmail-browser"));
        assert!(!connect_args_are_lark_browser(""));
    }

    #[test]
    fn infers_brand_from_first_token() {
        assert_eq!(brand_from_connect_args("lark-browser foo"), Some(Brand::Lark));
        assert_eq!(brand_from_connect_args("feishu-browser"), Some(Brand::Feishu));
        assert_eq!(brand_from_connect_args("nope"), None);
    }

    #[test]
    fn parse_setup_target_defaults_to_brand_slug() {
        let (slug, brand) = parse_setup_target("lark-browser", Brand::Lark).unwrap();
        assert_eq!(slug, "lark-browser");
        assert_eq!(brand, Brand::Lark);

        let (slug, brand) = parse_setup_target("feishu-browser", Brand::Feishu).unwrap();
        assert_eq!(slug, "feishu-browser");
        assert_eq!(brand, Brand::Feishu);
    }

    #[test]
    fn parse_setup_target_uses_explicit_connection_name() {
        let (slug, _) = parse_setup_target("lark-browser my-lark", Brand::Lark).unwrap();
        assert_eq!(slug, "my-lark");
    }

    #[test]
    fn parse_setup_target_rejects_extra_args() {
        assert!(parse_setup_target("lark-browser foo bar", Brand::Lark).is_err());
    }

    #[test]
    fn safe_session_part_sanitizes() {
        assert_eq!(safe_session_part("abc-def_123"), "abc-def_123");
        assert_eq!(safe_session_part("a b c"), "a-b-c");
        assert_eq!(safe_session_part("  "), "setup");
        assert_eq!(safe_session_part("a--b"), "a-b");
    }
}

//! Daemon-owned setup flow for the Gmail browser connector.

use crate::daemon::{DaemonState, ServerEnvelope};
use anyhow::{bail, Context, Result};
use puffer_core::{CancelToken, UserQuestionPromptResponse};
use puffer_subscriptions::{ConnectionRecord, ConnectionState};
use serde_json::{json, Map, Value};
use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

const ACCOUNT_SELECT_QUESTION: &str =
    "Which logged-in Gmail accounts should this connection monitor?";
const SIGN_IN_QUESTION: &str = "Sign in to Gmail in the Puffer browser profile, then continue.";
const SIGN_IN_OPTION: &str = "Sign in to another account";
const SETUP_TAB_ID: &str = "google-accounts";
const BROWSER_WIDTH: u32 = 1280;
const BROWSER_HEIGHT: u32 = 900;
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(12);
const DISCOVERY_INTERVAL: Duration = Duration::from_millis(800);
const MAX_SIGN_IN_ROUNDS: usize = 4;

const ACCOUNT_CHOOSER_URL: &str =
    "https://accounts.google.com/AccountChooser?continue=https%3A%2F%2Fmail.google.com%2Fmail%2F&service=mail";
const ADD_SESSION_URL: &str =
    "https://accounts.google.com/AddSession?continue=https%3A%2F%2Fmail.google.com%2Fmail%2F&service=mail";

const DISCOVER_ACCOUNTS_SCRIPT: &str = r#"
(() => {
  const emailRe = /[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}/ig;
  const accounts = new Set();
  const addEmails = (value) => {
    if (!value) return;
    const matches = String(value).match(emailRe) || [];
    for (const match of matches) accounts.add(match.toLowerCase());
  };
  const separatedText = (node) => {
    const walker = document.createTreeWalker(node, NodeFilter.SHOW_TEXT);
    const values = [];
    while (walker.nextNode()) {
      const value = walker.currentNode.nodeValue.trim();
      if (value) values.push(value);
    }
    return values.join("\n");
  };
  const bodyText = document.body ? document.body.innerText || "" : "";
  const href = location.href;
  const host = location.hostname;
  const title = document.title || "";
  const haystack = `${title}\n${bodyText}\n${href}`.toLowerCase();
  const accountIdentityNodes = Array.from(document.querySelectorAll([
    "[data-identifier]",
    "[data-email]",
    "[email]",
    "[aria-label*='@']",
    "[title*='@']",
    "img[alt*='@']",
    "a[href*='authuser=']",
    "a[href*='Email=']",
    "a[href*='SignOutOptions']"
  ].join(",")));
  for (const node of accountIdentityNodes.slice(0, 250)) {
    addEmails(node.getAttribute("data-identifier"));
    addEmails(node.getAttribute("data-email"));
    addEmails(node.getAttribute("email"));
    addEmails(node.getAttribute("aria-label"));
    addEmails(node.getAttribute("title"));
    addEmails(node.getAttribute("alt"));
    addEmails(node.getAttribute("href"));
  }
  if (host.includes("accounts.google.")) {
    const chooserNodes = Array.from(document.querySelectorAll([
      "[data-identifier]",
      "[data-email]",
      "[email]",
      "[role='link']",
      "li",
      "button"
    ].join(",")));
    for (const node of chooserNodes.slice(0, 250)) {
      addEmails(separatedText(node));
      addEmails(node.getAttribute("aria-label"));
      addEmails(node.getAttribute("title"));
      addEmails(node.getAttribute("data-identifier"));
      addEmails(node.getAttribute("data-email"));
      addEmails(node.getAttribute("email"));
    }
  }
  let status = "unknown";
  if (accounts.size > 0) {
    status = "accounts";
  } else if (host.includes("accounts.google.") && /sign in|email or phone|choose an account|use another account|add account|to continue to gmail|couldn.t sign you in/.test(haystack)) {
    status = "login_required";
  } else if (host.includes("mail.google.")) {
    status = "gmail_loaded";
  } else if (document.readyState !== "complete") {
    status = "loading";
  }
  return {
    status,
    accounts: Array.from(accounts).sort(),
    href,
    title,
    bodyText: bodyText.slice(0, 500)
  };
})()
"#;

type PendingQuestions = Arc<Mutex<HashMap<String, mpsc::Sender<UserQuestionPromptResponse>>>>;

#[derive(Debug, Clone, PartialEq, Eq)]
struct SetupTarget {
    connection_slug: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Discovery {
    status: String,
    accounts: Vec<String>,
    href: Option<String>,
    title: Option<String>,
}

struct SetupFlow {
    state: Arc<DaemonState>,
    channel: String,
    turn_id: String,
    next_request_id: Arc<AtomicU64>,
    pending_questions: PendingQuestions,
    cancel: CancelToken,
    session_id: String,
    target: SetupTarget,
}

/// Returns true when connector setup args target `gmail-browser`.
pub(crate) fn connect_args_are_gmail_browser(connect_args: &str) -> bool {
    connect_args
        .split_whitespace()
        .next()
        .is_some_and(|connector| connector == crate::gmail_browser::CONNECTOR_SLUG)
}

/// Executes daemon-native Gmail browser connector setup.
pub(crate) fn execute_gmail_browser_setup(
    state: Arc<DaemonState>,
    channel: String,
    turn_id: String,
    connect_args: String,
    next_request_id: Arc<AtomicU64>,
    pending_questions: PendingQuestions,
    cancel: CancelToken,
) -> Result<String> {
    let target = parse_setup_target(&connect_args)?;
    let session_id = format!("gmail-browser-setup-{}", safe_session_part(&turn_id));
    let mut flow = SetupFlow {
        state,
        channel,
        turn_id,
        next_request_id,
        pending_questions,
        cancel,
        session_id,
        target,
    };
    flow.run()
}

impl SetupFlow {
    fn run(&mut self) -> Result<String> {
        self.cancel.check()?;
        self.open_url(ACCOUNT_CHOOSER_URL, "Google accounts")?;
        let mut discovery = self.discover_accounts()?;
        let mut sign_in_rounds = 0usize;

        loop {
            self.cancel.check()?;
            if discovery.accounts.is_empty() {
                sign_in_rounds += 1;
                if sign_in_rounds > MAX_SIGN_IN_ROUNDS {
                    bail!("no Google accounts were found in the Puffer browser profile");
                }
                self.open_url(ADD_SESSION_URL, "Google sign in")?;
                self.ask_sign_in(&discovery)?;
                self.open_url(ACCOUNT_CHOOSER_URL, "Google accounts")?;
                discovery = self.discover_accounts()?;
                continue;
            }

            let selected = self.ask_account_selection(&discovery.accounts)?;
            if selected.sign_in_another {
                sign_in_rounds += 1;
                if sign_in_rounds > MAX_SIGN_IN_ROUNDS {
                    bail!("Gmail setup reached the sign-in retry limit");
                }
                self.open_url(ADD_SESSION_URL, "Google sign in")?;
                self.ask_sign_in(&discovery)?;
                self.open_url(ACCOUNT_CHOOSER_URL, "Google accounts")?;
                discovery = self.discover_accounts()?;
                continue;
            }
            if selected.accounts.is_empty() {
                bail!("select at least one Gmail account to monitor");
            }
            let config = crate::gmail_browser::save_config(
                self.state.config_paths(),
                self.state.cwd_path(),
                &self.target.connection_slug,
                selected.accounts,
            )?;
            let registered = upsert_connection(&self.target.connection_slug, &config.accounts)?;
            let action = if registered { "created" } else { "updated" };
            return Ok(format!(
                "Configured gmail-browser connection `{}` for {} account(s) using the global Puffer browser profile ({action}).",
                self.target.connection_slug,
                config.accounts.len()
            ));
        }
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
        .with_context(|| format!("open Gmail setup browser at {url}"))?;
        Ok(())
    }

    fn discover_accounts(&self) -> Result<Discovery> {
        let deadline = Instant::now() + DISCOVERY_TIMEOUT;
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
                    "script": DISCOVER_ACCOUNTS_SCRIPT,
                }),
            )
            .context("inspect Google account chooser")?;
            let result = value.get("value").cloned().unwrap_or(Value::Null);
            let discovery = discovery_from_value(&result);
            if !discovery.accounts.is_empty()
                || matches!(discovery.status.as_str(), "login_required" | "gmail_loaded")
                || Instant::now() >= deadline
            {
                return Ok(discovery);
            }
            std::thread::sleep(DISCOVERY_INTERVAL);
        }
    }

    fn ask_sign_in(&self, discovery: &Discovery) -> Result<()> {
        self.ask_questions(
            json!([{
                "type": "choice",
                "header": "Gmail sign in",
                "question": SIGN_IN_QUESTION,
                "multiSelect": false,
                "options": []
            }]),
            json!({
                "browserSessionId": &self.session_id,
                "browserTabId": SETUP_TAB_ID,
                "browserUrl": discovery.href,
            }),
        )?;
        Ok(())
    }

    fn ask_account_selection(&self, accounts: &[String]) -> Result<AccountSelection> {
        let mut options = accounts
            .iter()
            .map(|account| {
                json!({
                    "label": account,
                    "description": "Monitor this Gmail account from the Puffer browser profile."
                })
            })
            .collect::<Vec<_>>();
        options.push(json!({
            "label": SIGN_IN_OPTION,
            "description": "Open Google sign-in in the same Puffer browser profile."
        }));
        let response = self.ask_questions(
            json!([{
                "type": "choice",
                "header": "Gmail accounts",
                "question": ACCOUNT_SELECT_QUESTION,
                "multiSelect": true,
                "options": options
            }]),
            json!({}),
        )?;
        let answers = answer_values(&response, ACCOUNT_SELECT_QUESTION);
        let discovered = accounts.iter().cloned().collect::<BTreeSet<_>>();
        let selected = answers
            .iter()
            .filter(|answer| discovered.contains(*answer))
            .cloned()
            .collect::<Vec<_>>();
        Ok(AccountSelection {
            accounts: selected,
            sign_in_another: answers.iter().any(|answer| answer == SIGN_IN_OPTION),
        })
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct AccountSelection {
    accounts: Vec<String>,
    sign_in_another: bool,
}

fn parse_setup_target(connect_args: &str) -> Result<SetupTarget> {
    let mut parts = connect_args.split_whitespace();
    let connector = parts.next().unwrap_or_default();
    if connector != crate::gmail_browser::CONNECTOR_SLUG {
        bail!("expected gmail-browser connector setup, got `{connector}`");
    }
    let connection_slug = parts
        .next()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(crate::gmail_browser::DEFAULT_CONNECTION);
    if parts.next().is_some() {
        bail!("Usage: /connect gmail-browser <connection-name>");
    }
    Ok(SetupTarget {
        connection_slug: connection_slug.to_string(),
    })
}

fn discovery_from_value(value: &Value) -> Discovery {
    let accounts = value
        .get("accounts")
        .and_then(Value::as_array)
        .map(|items| {
            let mut accounts = items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|account| looks_like_email(account))
                .map(|account| account.to_ascii_lowercase())
                .collect::<Vec<_>>();
            accounts.sort();
            accounts.dedup();
            accounts
        })
        .unwrap_or_default();
    Discovery {
        status: value
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        accounts,
        href: value
            .get("href")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        title: value
            .get("title")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
    }
}

fn answer_values(response: &UserQuestionPromptResponse, question: &str) -> Vec<String> {
    match response.answers.get(question) {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
        Some(Value::String(value)) => value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

fn upsert_connection(connection: &str, accounts: &[String]) -> Result<bool> {
    let manager = puffer_core::subscription_manager()?;
    let description = format!("Gmail Browser ({})", accounts.join(", "));
    let registered = if let Some(existing) = manager.connection_store().get(connection) {
        if existing.connector_slug != crate::gmail_browser::CONNECTOR_SLUG {
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
                crate::gmail_browser::CONNECTOR_SLUG,
                description,
            ))?;
        true
    };
    manager.refresh_connection_consumers()?;
    manager.refresh_connection_auth()?;
    Ok(registered)
}

fn looks_like_email(value: &str) -> bool {
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    !local.is_empty() && domain.contains('.') && !domain.ends_with('.')
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

    #[test]
    fn detects_gmail_browser_connect_args() {
        assert!(connect_args_are_gmail_browser(
            "gmail-browser gmail-browser"
        ));
        assert!(connect_args_are_gmail_browser("gmail-browser"));
        assert!(!connect_args_are_gmail_browser("email email"));
        assert!(!connect_args_are_gmail_browser(""));
    }

    #[test]
    fn setup_target_defaults_connection_slug() {
        let target = parse_setup_target("gmail-browser").unwrap();

        assert_eq!(target.connection_slug, "gmail-browser");
    }

    #[test]
    fn setup_target_parses_explicit_connection_slug() {
        let target = parse_setup_target("gmail-browser work-gmail").unwrap();

        assert_eq!(target.connection_slug, "work-gmail");
    }

    #[test]
    fn discovery_normalizes_accounts() {
        let value = json!({
            "status": "accounts",
            "accounts": ["Me@Example.COM", "bad", "me@example.com", "other@example.com"],
            "href": "https://accounts.google.com/",
            "title": "Choose an account"
        });

        let discovery = discovery_from_value(&value);

        assert_eq!(
            discovery.accounts,
            vec!["me@example.com", "other@example.com"]
        );
    }

    #[test]
    fn answer_values_support_arrays_and_strings() {
        let response = UserQuestionPromptResponse {
            answers: Map::from_iter([(
                ACCOUNT_SELECT_QUESTION.to_string(),
                json!(["a@example.com", SIGN_IN_OPTION]),
            )]),
            annotations: Map::new(),
        };

        assert_eq!(
            answer_values(&response, ACCOUNT_SELECT_QUESTION),
            vec!["a@example.com", SIGN_IN_OPTION]
        );

        let response = UserQuestionPromptResponse {
            answers: Map::from_iter([(
                ACCOUNT_SELECT_QUESTION.to_string(),
                json!("a@example.com, b@example.com"),
            )]),
            annotations: Map::new(),
        };

        assert_eq!(
            answer_values(&response, ACCOUNT_SELECT_QUESTION),
            vec!["a@example.com", "b@example.com"]
        );
    }
}

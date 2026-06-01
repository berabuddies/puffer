//! Deterministic `/connect gmail-browser` setup flow.

use super::{ask_questions, summary, ConnectResult};
use crate::{subscription_manager, AppState};
use anyhow::{anyhow, bail, Context, Result};
use puffer_config::ConfigPaths;
use puffer_resources::LoadedResources;
use puffer_subscriptions::{ConnectionRecord, ConnectionState};
use serde::Serialize;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

const CONNECTOR_SLUG: &str = "gmail-browser";
const STATE_ROOT: &str = "gmail-browser-accounts";
const CONFIG_FILE: &str = "config.toml";

#[derive(Serialize)]
struct GmailBrowserConfig {
    workspace_root: PathBuf,
    accounts: Vec<String>,
}

/// Configures one Gmail browser connection through the standard `/connect` question flow.
pub(super) fn connect_gmail_browser(
    state: &mut AppState,
    resources: &LoadedResources,
    connection: &str,
) -> Result<ConnectResult> {
    let accounts = ask_accounts(state, resources)?;
    let paths = ConfigPaths::discover(&state.cwd);
    save_gmail_config(&paths, &state.cwd, connection, &accounts)?;
    let registered = upsert_connection(connection, &accounts)?;
    let output = json!({
        "status": "configured",
        "registered_connection": registered,
        "profile": "global Puffer browser profile",
        "accounts": accounts,
    });
    Ok(summary(
        CONNECTOR_SLUG,
        connection,
        "Puffer browser profile",
        &output,
    ))
}

fn ask_accounts(state: &mut AppState, resources: &LoadedResources) -> Result<Vec<String>> {
    let question = "Which Gmail accounts should this connection monitor?";
    let output = ask_questions(
        state,
        resources,
        json!([{
            "type": "input",
            "header": "Gmail Accounts",
            "question": question,
            "options": []
        }]),
    )?;
    let accounts = normalize_accounts(split_account_answer(&answer_string(&output, question)?));
    if accounts.is_empty() {
        bail!("provide at least one Gmail account email address");
    }
    Ok(accounts)
}

fn save_gmail_config(
    paths: &ConfigPaths,
    workspace_root: &Path,
    connection: &str,
    accounts: &[String],
) -> Result<()> {
    let state_dir = paths.user_config_dir.join(STATE_ROOT).join(connection);
    fs::create_dir_all(&state_dir).with_context(|| format!("create {}", state_dir.display()))?;
    let config = GmailBrowserConfig {
        workspace_root: workspace_root.to_path_buf(),
        accounts: accounts.to_vec(),
    };
    let raw = toml::to_string_pretty(&config).context("serialize Gmail browser config")?;
    fs::write(state_dir.join(CONFIG_FILE), raw)
        .with_context(|| format!("write {}", state_dir.join(CONFIG_FILE).display()))
}

fn upsert_connection(connection: &str, accounts: &[String]) -> Result<bool> {
    let manager = subscription_manager()?;
    let description = format!("Gmail Browser ({})", accounts.join(", "));
    let registered = if let Some(existing) = manager.connection_store().get(connection) {
        if existing.connector_slug != CONNECTOR_SLUG {
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
                CONNECTOR_SLUG,
                description,
            ))?;
        true
    };
    manager.refresh_connection_consumers()?;
    manager.refresh_connection_auth()?;
    Ok(registered)
}

fn answer_string(output: &Value, question: &str) -> Result<String> {
    output
        .get("answers")
        .and_then(|answers| answers.get(question))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| anyhow!("no answer provided for `{question}`"))
}

fn split_account_answer(answer: &str) -> Vec<String> {
    answer
        .split(|ch: char| ch == ',' || ch == ';' || ch.is_whitespace())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn normalize_accounts(accounts: Vec<String>) -> Vec<String> {
    let mut normalized = accounts
        .into_iter()
        .map(|account| account.trim().to_ascii_lowercase())
        .filter(|account| looks_like_email(account))
        .collect::<Vec<_>>();
    normalized.sort();
    normalized.dedup();
    normalized
}

fn looks_like_email(value: &str) -> bool {
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    !local.is_empty() && domain.contains('.') && !domain.ends_with('.')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_answers_split_and_normalize() {
        let accounts = normalize_accounts(split_account_answer(
            "Me@Example.COM, other@example.com\ninvalid other@example.com",
        ));

        assert_eq!(accounts, vec!["me@example.com", "other@example.com"]);
    }
}

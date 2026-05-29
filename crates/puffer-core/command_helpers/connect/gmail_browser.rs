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

#[derive(Clone, Debug, Eq, PartialEq)]
struct ChromeProfileChoice {
    id: String,
    name: String,
    email: Option<String>,
    google_accounts: Vec<ChromeGoogleAccount>,
    is_last_used: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ChromeGoogleAccount {
    email: String,
    name: Option<String>,
}

#[derive(Serialize)]
struct GmailBrowserConfig {
    workspace_root: PathBuf,
    chrome_profile: String,
    accounts: Vec<String>,
}

/// Configures one Gmail browser connection through the standard `/connect` question flow.
pub(super) fn connect_gmail_browser(
    state: &mut AppState,
    resources: &LoadedResources,
    connection: &str,
) -> Result<ConnectResult> {
    let profiles = discover_chrome_profiles()
        .into_iter()
        .filter(|profile| !profile.google_accounts.is_empty())
        .collect::<Vec<_>>();
    if profiles.is_empty() {
        bail!("no Chrome profiles with Google accounts were found");
    }
    let selected_profile = ask_profile(state, resources, &profiles)?;
    let accounts = ask_accounts(state, resources, selected_profile)?;
    let paths = ConfigPaths::discover(&state.cwd);
    save_gmail_config(&paths, &state.cwd, connection, selected_profile, &accounts)?;
    let registered = upsert_connection(connection, &accounts)?;
    let output = json!({
        "status": "configured",
        "registered_connection": registered,
        "chrome_profile": selected_profile.id,
        "accounts": accounts,
    });
    Ok(summary(
        CONNECTOR_SLUG,
        connection,
        "Chrome profile",
        &output,
    ))
}

fn ask_profile<'a>(
    state: &mut AppState,
    resources: &LoadedResources,
    profiles: &'a [ChromeProfileChoice],
) -> Result<&'a ChromeProfileChoice> {
    let question = "Which Chrome profile should the Gmail connector use?";
    let options = profiles
        .iter()
        .map(|profile| {
            json!({
                "label": profile_label(profile),
                "description": format!(
                    "{} Google account{}",
                    profile.google_accounts.len(),
                    if profile.google_accounts.len() == 1 { "" } else { "s" }
                )
            })
        })
        .collect::<Vec<_>>();
    let output = ask_questions(
        state,
        resources,
        json!([{
            "type": "choice",
            "header": "Chrome Profile",
            "question": question,
            "searchable": true,
            "options": options
        }]),
    )?;
    let label = answer_string(&output, question)?;
    profiles
        .iter()
        .find(|profile| profile_label(profile) == label)
        .ok_or_else(|| anyhow!("unknown Chrome profile answer `{label}`"))
}

fn ask_accounts(
    state: &mut AppState,
    resources: &LoadedResources,
    profile: &ChromeProfileChoice,
) -> Result<Vec<String>> {
    if profile.google_accounts.len() == 1 {
        return Ok(vec![profile.google_accounts[0].email.clone()]);
    }
    let question = "Which Gmail accounts should this connection monitor?";
    let labelled = profile
        .google_accounts
        .iter()
        .map(|account| (account_label(account), account.email.clone()))
        .collect::<Vec<_>>();
    let options = labelled
        .iter()
        .map(|(label, email)| {
            json!({
                "label": label,
                "description": format!("Monitor inbox rows for {email}")
            })
        })
        .collect::<Vec<_>>();
    let output = ask_questions(
        state,
        resources,
        json!([{
            "type": "choice",
            "header": "Gmail Accounts",
            "question": question,
            "multiSelect": true,
            "options": options
        }]),
    )?;
    let selected_labels = answer_strings(&output, question)?;
    let mut accounts = Vec::with_capacity(selected_labels.len());
    for label in selected_labels {
        let Some((_, email)) = labelled.iter().find(|(candidate, _)| candidate == &label) else {
            bail!("unknown Gmail account answer `{label}`");
        };
        accounts.push(email.clone());
    }
    let accounts = normalize_accounts(accounts);
    if accounts.is_empty() {
        bail!("select at least one Gmail account");
    }
    Ok(accounts)
}

fn save_gmail_config(
    paths: &ConfigPaths,
    workspace_root: &Path,
    connection: &str,
    profile: &ChromeProfileChoice,
    accounts: &[String],
) -> Result<()> {
    let state_dir = paths.user_config_dir.join(STATE_ROOT).join(connection);
    fs::create_dir_all(&state_dir).with_context(|| format!("create {}", state_dir.display()))?;
    let config = GmailBrowserConfig {
        workspace_root: workspace_root.to_path_buf(),
        chrome_profile: profile.id.clone(),
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

fn answer_strings(output: &Value, question: &str) -> Result<Vec<String>> {
    let value = output
        .get("answers")
        .and_then(|answers| answers.get(question))
        .ok_or_else(|| anyhow!("no answer provided for `{question}`"))?;
    if let Some(items) = value.as_array() {
        return Ok(items
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .collect());
    }
    if let Some(value) = value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok(vec![value.to_string()]);
    }
    bail!("answer for `{question}` must be text or a text array")
}

fn profile_label(profile: &ChromeProfileChoice) -> String {
    let mut label = profile.name.clone();
    if let Some(email) = profile.email.as_ref().filter(|email| !email.is_empty()) {
        label.push_str(" · ");
        label.push_str(email);
    }
    label.push_str(" (");
    label.push_str(&profile.id);
    label.push(')');
    if profile.is_last_used {
        label.push_str(" · recent");
    }
    label
}

fn account_label(account: &ChromeGoogleAccount) -> String {
    match account.name.as_ref().filter(|name| !name.is_empty()) {
        Some(name) => format!("{} · {name}", account.email),
        None => account.email.clone(),
    }
}

fn discover_chrome_profiles() -> Vec<ChromeProfileChoice> {
    let Some(user_data_dir) = chrome_user_data_dir() else {
        return Vec::new();
    };
    let local_state = read_json(&user_data_dir.join("Local State")).unwrap_or(Value::Null);
    let last_used = last_used_profile_id(&local_state);
    let Some(info_cache) = local_state
        .get("profile")
        .and_then(|profile| profile.get("info_cache"))
        .and_then(Value::as_object)
    else {
        return default_profile(&user_data_dir);
    };
    let mut profiles = Vec::new();
    for (id, value) in info_cache {
        if !valid_profile_id(id) {
            continue;
        }
        let path = user_data_dir.join(id);
        if !path.is_dir() {
            continue;
        }
        profiles.push(ChromeProfileChoice {
            id: id.clone(),
            name: value
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.trim().is_empty())
                .unwrap_or(id)
                .to_string(),
            email: value
                .get("user_name")
                .and_then(Value::as_str)
                .filter(|email| !email.trim().is_empty())
                .map(ToString::to_string),
            google_accounts: google_accounts_from_profile(&path),
            is_last_used: last_used.as_deref() == Some(id.as_str()),
        });
    }
    profiles.sort_by(|left, right| {
        right
            .is_last_used
            .cmp(&left.is_last_used)
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.id.cmp(&right.id))
    });
    profiles
}

fn default_profile(user_data_dir: &Path) -> Vec<ChromeProfileChoice> {
    let path = user_data_dir.join("Default");
    if !path.is_dir() {
        return Vec::new();
    }
    vec![ChromeProfileChoice {
        id: "Default".to_string(),
        name: "Default".to_string(),
        email: None,
        google_accounts: google_accounts_from_profile(&path),
        is_last_used: true,
    }]
}

fn google_accounts_from_profile(profile_path: &Path) -> Vec<ChromeGoogleAccount> {
    let value = read_json(&profile_path.join("Preferences")).unwrap_or(Value::Null);
    let mut accounts = Vec::new();
    collect_google_account_array(value.get("account_info"), &mut accounts);
    collect_google_account_array(value.pointer("/signin/account_info"), &mut accounts);
    accounts.sort_by(|left, right| left.email.cmp(&right.email));
    accounts.dedup_by(|left, right| left.email == right.email);
    accounts
}

fn collect_google_account_array(value: Option<&Value>, accounts: &mut Vec<ChromeGoogleAccount>) {
    let Some(items) = value.and_then(Value::as_array) else {
        return;
    };
    for item in items {
        let Some(email) = item
            .get("email")
            .or_else(|| item.get("user_name"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|email| looks_like_email(email))
        else {
            continue;
        };
        accounts.push(ChromeGoogleAccount {
            email: email.to_ascii_lowercase(),
            name: item
                .get("full_name")
                .or_else(|| item.get("given_name"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(ToString::to_string),
        });
    }
}

fn read_json(path: &Path) -> Result<Value> {
    let raw = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))
}

fn last_used_profile_id(local_state: &Value) -> Option<String> {
    local_state
        .get("profile")
        .and_then(|profile| profile.get("last_used"))
        .and_then(Value::as_str)
        .filter(|id| valid_profile_id(id))
        .map(ToString::to_string)
}

fn chrome_user_data_dir() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PUFFER_CHROME_USER_DATA_DIR")
        .map(PathBuf::from)
        .filter(|path| path.is_dir())
    {
        return Some(path);
    }
    default_chrome_user_data_dirs()
        .into_iter()
        .find(|path| path.is_dir())
}

fn default_chrome_user_data_dirs() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let Some(home) = home_dir() else {
            return Vec::new();
        };
        return vec![home.join("Library/Application Support/Google/Chrome")];
    }
    #[cfg(target_os = "windows")]
    {
        let Some(local_app_data) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) else {
            return Vec::new();
        };
        return vec![local_app_data.join("Google/Chrome/User Data")];
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let Some(home) = home_dir() else {
            return Vec::new();
        };
        vec![
            home.join(".config/google-chrome"),
            home.join(".config/chromium"),
        ]
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
}

fn valid_profile_id(id: &str) -> bool {
    !id.is_empty() && !id.contains('/') && !id.contains('\\') && id != "." && id != ".."
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
    use serde_json::json;

    #[test]
    fn profile_label_includes_stable_id() {
        let profile = ChromeProfileChoice {
            id: "Profile 2".to_string(),
            name: "dev".to_string(),
            email: Some("dev@example.com".to_string()),
            google_accounts: Vec::new(),
            is_last_used: true,
        };

        assert_eq!(
            profile_label(&profile),
            "dev · dev@example.com (Profile 2) · recent"
        );
    }

    #[test]
    fn profile_preferences_expose_google_accounts() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("Preferences"),
            serde_json::to_string(&json!({
                "account_info": [
                    {"email": "Me@Example.COM", "full_name": "Me Example"},
                    {"email": "me@example.com", "full_name": "Duplicate"},
                    {"email": "not-an-email"}
                ]
            }))
            .unwrap(),
        )
        .unwrap();

        let accounts = google_accounts_from_profile(dir.path());

        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].email, "me@example.com");
        assert_eq!(accounts[0].name.as_deref(), Some("Me Example"));
    }
}

//! Slack internal workflow actions.
//!
//! The consolidated internal `Slack` tool handles Slack app credentials,
//! OAuth/browser login credentials, local Slack app import, and read-only
//! lookup operations. Outbound message sends and reactions stay connector
//! actions on `slack-app` or `slack-login`.

use crate::AppState;
use anyhow::{bail, Context, Result};
use puffer_config::ConfigPaths;
use puffer_slack::{
    connection_description, connector_slug_for_auth, credential_path, import_local_slack_session,
    normalize_workspace_url, save_credential, SlackAuthKind, SlackAuthTest, SlackClient,
    SlackCredential, SlackLocalImportOptions,
};
use puffer_subscriptions::{ConnectionRecord, ConnectionState};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

use super::subscription_globals;

const SLACK_APP_CONNECTOR_SLUG: &str = "slack-app";
const SLACK_LOGIN_CONNECTOR_SLUG: &str = "slack-login";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SlackAction {
    ConfigureApp,
    ImportLocal,
    ListConversations,
    LoginBrowser,
    LoginToken,
    ReadMessages,
    SearchConversations,
    SearchMessages,
    SearchUsers,
}

#[derive(Debug, Deserialize)]
struct SlackInput {
    action: SlackAction,
}

#[derive(Debug, Deserialize)]
struct ConfigureAppInput {
    bot_token: String,
    app_token: String,
    #[serde(default)]
    workspace_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LoginTokenInput {
    token: String,
    #[serde(default = "default_token_type")]
    token_type: String,
    #[serde(default)]
    workspace_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LoginBrowserInput {
    workspace_url: String,
    xoxd_token: String,
    xoxc_token: String,
    #[serde(default)]
    workspace_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ImportLocalInput {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    workspace_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ListConversationsInput {
    #[serde(default = "default_conversation_types")]
    types: String,
    #[serde(default = "default_list_limit")]
    limit: usize,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default = "default_true")]
    exclude_archived: bool,
}

#[derive(Debug, Deserialize)]
struct SearchConversationsInput {
    query: String,
    #[serde(default = "default_conversation_types")]
    types: String,
    #[serde(default = "default_search_limit")]
    limit: usize,
}

#[derive(Debug, Deserialize)]
struct SearchUsersInput {
    query: String,
    #[serde(default = "default_search_limit")]
    limit: usize,
}

#[derive(Debug, Deserialize)]
struct ReadMessagesInput {
    channel: String,
    #[serde(default)]
    thread_ts: Option<String>,
    #[serde(default = "default_message_limit")]
    limit: usize,
    #[serde(default)]
    oldest: Option<String>,
    #[serde(default)]
    latest: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SearchMessagesInput {
    query: String,
    #[serde(default = "default_search_limit")]
    limit: usize,
    #[serde(default)]
    page: Option<usize>,
    #[serde(default)]
    sort: Option<String>,
    #[serde(default)]
    sort_dir: Option<String>,
}

/// Executes the consolidated internal `Slack` workflow action.
pub fn execute_slack(_state: &mut AppState, cwd: &Path, input: Value) -> Result<String> {
    let parsed: SlackInput =
        serde_json::from_value(input.clone()).context("invalid Slack input")?;
    match parsed.action {
        SlackAction::ConfigureApp => execute_configure_app(cwd, input),
        SlackAction::ImportLocal => execute_import_local(cwd, input),
        SlackAction::ListConversations => execute_list_conversations(cwd, input),
        SlackAction::LoginBrowser => execute_login_browser(cwd, input),
        SlackAction::LoginToken => execute_login_token(cwd, input),
        SlackAction::ReadMessages => execute_read_messages(cwd, input),
        SlackAction::SearchConversations => execute_search_conversations(cwd, input),
        SlackAction::SearchMessages => execute_search_messages(cwd, input),
        SlackAction::SearchUsers => execute_search_users(cwd, input),
    }
}

fn execute_configure_app(cwd: &Path, input: Value) -> Result<String> {
    let connection_slug = connection_slug_from_input(&input, SLACK_APP_CONNECTOR_SLUG)?;
    let parsed: ConfigureAppInput =
        serde_json::from_value(input).context("invalid Slack configure_app input")?;
    if parsed.bot_token.trim().is_empty() || parsed.app_token.trim().is_empty() {
        bail!("Slack configure_app requires non-empty bot_token and app_token");
    }
    let auth = SlackAuthKind::App {
        bot_token: parsed.bot_token,
        app_token: parsed.app_token,
    };
    persist_authenticated_credential(cwd, connection_slug, auth, parsed.workspace_name)
}

fn execute_login_token(cwd: &Path, input: Value) -> Result<String> {
    let connection_slug = connection_slug_from_input(&input, SLACK_LOGIN_CONNECTOR_SLUG)?;
    let parsed: LoginTokenInput =
        serde_json::from_value(input).context("invalid Slack login_token input")?;
    if parsed.token.trim().is_empty() {
        bail!("Slack login_token requires a non-empty token");
    }
    let auth = SlackAuthKind::Standard {
        token: parsed.token,
        token_type: parsed.token_type,
    };
    persist_authenticated_credential(cwd, connection_slug, auth, parsed.workspace_name)
}

fn execute_login_browser(cwd: &Path, input: Value) -> Result<String> {
    let connection_slug = connection_slug_from_input(&input, SLACK_LOGIN_CONNECTOR_SLUG)?;
    let parsed: LoginBrowserInput =
        serde_json::from_value(input).context("invalid Slack login_browser input")?;
    if parsed.xoxd_token.trim().is_empty() || parsed.xoxc_token.trim().is_empty() {
        bail!("Slack login_browser requires non-empty xoxd_token and xoxc_token");
    }
    let auth = SlackAuthKind::Browser {
        workspace_url: normalize_workspace_url(&parsed.workspace_url)?,
        xoxd_token: parsed.xoxd_token,
        xoxc_token: parsed.xoxc_token,
    };
    persist_authenticated_credential(cwd, connection_slug, auth, parsed.workspace_name)
}

fn execute_import_local(cwd: &Path, input: Value) -> Result<String> {
    let connection_slug = connection_slug_from_input(&input, SLACK_LOGIN_CONNECTOR_SLUG)?;
    let parsed: ImportLocalInput =
        serde_json::from_value(input).context("invalid Slack import_local input")?;
    let imported = import_local_slack_session(SlackLocalImportOptions {
        path: parsed.path.map(PathBuf::from),
        workspace_url: parsed.workspace_url,
    })?;
    let auth = SlackAuthKind::Browser {
        workspace_url: imported.workspace_url,
        xoxd_token: imported.xoxd_token,
        xoxc_token: imported.xoxc_token,
    };
    let mut output: Value = serde_json::from_str(&persist_authenticated_credential(
        cwd,
        connection_slug,
        auth,
        None,
    )?)?;
    output["imported"] = Value::Bool(true);
    output["source_path"] = Value::String(imported.source_path.display().to_string());
    Ok(output.to_string())
}

fn persist_authenticated_credential(
    cwd: &Path,
    connection_slug: String,
    auth: SlackAuthKind,
    workspace_name: Option<String>,
) -> Result<String> {
    let connector_slug = connector_slug_for_auth(&auth).to_string();
    let mut credential = SlackCredential {
        connection_slug: connection_slug.clone(),
        connector_slug: connector_slug.clone(),
        workspace_id: None,
        workspace_name,
        user_id: None,
        user_name: None,
        auth,
    };
    let client = SlackClient::new(credential.clone())?;
    let auth_test = client.test_auth()?;
    hydrate_credential(&mut credential, &auth_test);
    let paths = ConfigPaths::discover(cwd);
    let path = credential_path(&paths.user_config_dir, &connection_slug);
    save_credential(&path, &credential)?;
    let (connection, created) = ensure_slack_connection_record(&credential)?;
    Ok(json!({
        "status": "complete",
        "connection_slug": connection_slug,
        "connector_slug": connector_slug,
        "registered_connection": created,
        "connection": connection,
        "workspace_id": credential.workspace_id,
        "workspace_name": credential.workspace_name,
        "user_id": credential.user_id,
        "user_name": credential.user_name,
        "auth_ok": auth_test.ok,
        "next": "Use this connection_slug in ConnectorAct. Use slack search-conversations or slack search-users before sending to a human name."
    })
    .to_string())
}

fn execute_list_conversations(cwd: &Path, input: Value) -> Result<String> {
    let connection_slug = connection_slug_from_input(&input, SLACK_LOGIN_CONNECTOR_SLUG)?;
    let parsed: ListConversationsInput =
        serde_json::from_value(input).context("invalid Slack list_conversations input")?;
    let client = client_for_connection(cwd, &connection_slug)?;
    let payload = client.list_conversations(
        &parsed.types,
        parsed.limit,
        parsed.cursor.as_deref(),
        parsed.exclude_archived,
    )?;
    Ok(json!({
        "status": "complete",
        "connection_slug": connection_slug,
        "payload": payload,
        "next": "Use conversation `id` as the Slack channel target for ConnectorAct."
    })
    .to_string())
}

fn execute_search_conversations(cwd: &Path, input: Value) -> Result<String> {
    let connection_slug = connection_slug_from_input(&input, SLACK_LOGIN_CONNECTOR_SLUG)?;
    let parsed: SearchConversationsInput =
        serde_json::from_value(input).context("invalid Slack search_conversations input")?;
    let query = normalized_query(&parsed.query, "Slack search_conversations")?;
    let client = client_for_connection(cwd, &connection_slug)?;
    let matches = search_conversations(&client, &query, &parsed.types, parsed.limit)?;
    Ok(json!({
        "status": "complete",
        "connection_slug": connection_slug,
        "count": matches.len(),
        "conversations": matches,
        "next": "Use the returned `id` as `to` or `channel` in ConnectorAct."
    })
    .to_string())
}

fn execute_search_users(cwd: &Path, input: Value) -> Result<String> {
    let connection_slug = connection_slug_from_input(&input, SLACK_LOGIN_CONNECTOR_SLUG)?;
    let parsed: SearchUsersInput =
        serde_json::from_value(input).context("invalid Slack search_users input")?;
    let query = normalized_query(&parsed.query, "Slack search_users")?;
    let client = client_for_connection(cwd, &connection_slug)?;
    let matches = search_users(&client, &query, parsed.limit)?;
    Ok(json!({
        "status": "complete",
        "connection_slug": connection_slug,
        "count": matches.len(),
        "users": matches,
        "next": "Use a returned user `id` as `to` in ConnectorAct; Puffer will open the Slack DM before sending."
    })
    .to_string())
}

fn execute_read_messages(cwd: &Path, input: Value) -> Result<String> {
    let connection_slug = connection_slug_from_input(&input, SLACK_LOGIN_CONNECTOR_SLUG)?;
    let parsed: ReadMessagesInput =
        serde_json::from_value(input).context("invalid Slack read_messages input")?;
    if parsed.channel.trim().is_empty() {
        bail!("Slack read_messages requires a non-empty channel");
    }
    let client = client_for_connection(cwd, &connection_slug)?;
    let payload = if let Some(thread_ts) = parsed.thread_ts.as_deref().filter(|v| !v.is_empty()) {
        client.conversation_replies(&parsed.channel, thread_ts, parsed.limit)?
    } else {
        client.conversation_history(
            &parsed.channel,
            parsed.limit,
            parsed.oldest.as_deref(),
            parsed.latest.as_deref(),
        )?
    };
    Ok(json!({
        "status": "complete",
        "connection_slug": connection_slug,
        "channel": parsed.channel,
        "payload": payload,
        "next": "Use message `ts` as `thread_ts` for replies or reactions."
    })
    .to_string())
}

fn execute_search_messages(cwd: &Path, input: Value) -> Result<String> {
    let connection_slug = connection_slug_from_input(&input, SLACK_LOGIN_CONNECTOR_SLUG)?;
    let parsed: SearchMessagesInput =
        serde_json::from_value(input).context("invalid Slack search_messages input")?;
    if parsed.query.trim().is_empty() {
        bail!("Slack search_messages requires a non-empty query");
    }
    let client = client_for_connection(cwd, &connection_slug)?;
    let payload = client.search_messages(
        &parsed.query,
        parsed.limit,
        parsed.page,
        parsed.sort.as_deref(),
        parsed.sort_dir.as_deref(),
    )?;
    Ok(json!({
        "status": "complete",
        "connection_slug": connection_slug,
        "payload": payload,
        "next": "Use match channel/id and ts for ConnectorAct targets, replies, or reactions."
    })
    .to_string())
}

fn search_conversations(
    client: &SlackClient,
    query: &str,
    types: &str,
    limit: usize,
) -> Result<Vec<Value>> {
    let mut cursor: Option<String> = None;
    let mut matches = Vec::new();
    for _ in 0..10 {
        let payload = client.list_conversations(types, 200, cursor.as_deref(), true)?;
        if let Some(channels) = payload.get("channels").and_then(Value::as_array) {
            for channel in channels {
                if conversation_matches(channel, query) {
                    matches.push(compact_conversation(channel));
                    if matches.len() >= limit.max(1) {
                        return Ok(matches);
                    }
                }
            }
        }
        cursor = next_cursor(&payload);
        if cursor.is_none() {
            break;
        }
    }
    Ok(matches)
}

fn search_users(client: &SlackClient, query: &str, limit: usize) -> Result<Vec<Value>> {
    let mut cursor: Option<String> = None;
    let mut matches = Vec::new();
    for _ in 0..10 {
        let payload = client.list_users(200, cursor.as_deref())?;
        if let Some(members) = payload.get("members").and_then(Value::as_array) {
            for member in members {
                if user_matches(member, query) {
                    matches.push(compact_user(member));
                    if matches.len() >= limit.max(1) {
                        return Ok(matches);
                    }
                }
            }
        }
        cursor = next_cursor(&payload);
        if cursor.is_none() {
            break;
        }
    }
    Ok(matches)
}

fn hydrate_credential(credential: &mut SlackCredential, auth_test: &SlackAuthTest) {
    credential.workspace_id = auth_test.team_id.clone().or(credential.workspace_id.take());
    credential.workspace_name = auth_test.team.clone().or(credential.workspace_name.take());
    credential.user_id = auth_test.user_id.clone().or(credential.user_id.take());
    credential.user_name = auth_test.user.clone().or(credential.user_name.take());
}

fn ensure_slack_connection_record(
    credential: &SlackCredential,
) -> Result<(ConnectionRecord, bool)> {
    let manager = subscription_globals::manager()?;
    let description = connection_description(credential);
    if let Some(connection) = manager.connection_store().get(&credential.connection_slug) {
        if connection.connector_slug != credential.connector_slug {
            bail!(
                "connection `{}` already exists for connector `{}`",
                credential.connection_slug,
                connection.connector_slug
            );
        }
        let updated = manager
            .connection_store()
            .update(&credential.connection_slug, |record| {
                record.description = description.clone();
                record.state = ConnectionState::Authenticated;
                record.auth_failure_notified = false;
                record.set_has_consumer(record.has_consumer);
            })?;
        manager.refresh_connection_consumers()?;
        return Ok((updated, false));
    }
    let record = ConnectionRecord::authenticated(
        &credential.connection_slug,
        &credential.connector_slug,
        description,
    );
    manager.connection_store().create(record.clone())?;
    manager.refresh_connection_consumers()?;
    Ok((record, true))
}

fn client_for_connection(cwd: &Path, connection_slug: &str) -> Result<SlackClient> {
    let paths = ConfigPaths::discover(cwd);
    let path = credential_path(&paths.user_config_dir, connection_slug);
    let credential = puffer_slack::load_credential(&path)?;
    SlackClient::new(credential)
}

fn connection_slug_from_input(input: &Value, default_slug: &str) -> Result<String> {
    let slug = input
        .get("connection_slug")
        .or_else(|| input.get("account_slug"))
        .or_else(|| input.get("connection"))
        .or_else(|| input.get("account"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|slug| !slug.is_empty())
        .unwrap_or(default_slug);
    validate_connection_slug(slug)?;
    Ok(slug.to_string())
}

fn validate_connection_slug(slug: &str) -> Result<()> {
    if slug.is_empty()
        || !slug
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        bail!("Slack connection_slug must be non-empty kebab-case ASCII");
    }
    Ok(())
}

fn normalized_query(query: &str, label: &str) -> Result<String> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        bail!("{label} requires a non-empty query");
    }
    Ok(query)
}

fn next_cursor(payload: &Value) -> Option<String> {
    payload
        .get("response_metadata")
        .and_then(|metadata| metadata.get("next_cursor"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|cursor| !cursor.is_empty())
        .map(ToString::to_string)
}

fn conversation_matches(channel: &Value, query: &str) -> bool {
    [
        string_field(channel, "id"),
        string_field(channel, "name"),
        string_field(channel, "name_normalized"),
        string_field(channel, "user"),
    ]
    .into_iter()
    .flatten()
    .any(|value| value.to_lowercase().contains(query))
}

fn user_matches(member: &Value, query: &str) -> bool {
    let profile = member.get("profile").unwrap_or(&Value::Null);
    [
        string_field(member, "id"),
        string_field(member, "name"),
        string_field(member, "real_name"),
        string_field(profile, "display_name"),
        string_field(profile, "real_name"),
        string_field(profile, "email"),
    ]
    .into_iter()
    .flatten()
    .any(|value| value.to_lowercase().contains(query))
}

fn compact_conversation(channel: &Value) -> Value {
    json!({
        "id": channel.get("id").cloned().unwrap_or(Value::Null),
        "name": channel.get("name").cloned().unwrap_or(Value::Null),
        "name_normalized": channel.get("name_normalized").cloned().unwrap_or(Value::Null),
        "is_channel": channel.get("is_channel").cloned().unwrap_or(Value::Null),
        "is_group": channel.get("is_group").cloned().unwrap_or(Value::Null),
        "is_im": channel.get("is_im").cloned().unwrap_or(Value::Null),
        "is_mpim": channel.get("is_mpim").cloned().unwrap_or(Value::Null),
        "is_archived": channel.get("is_archived").cloned().unwrap_or(Value::Null),
        "user": channel.get("user").cloned().unwrap_or(Value::Null),
    })
}

fn compact_user(member: &Value) -> Value {
    let profile = member.get("profile").unwrap_or(&Value::Null);
    json!({
        "id": member.get("id").cloned().unwrap_or(Value::Null),
        "name": member.get("name").cloned().unwrap_or(Value::Null),
        "real_name": member.get("real_name").cloned().unwrap_or(Value::Null),
        "display_name": profile.get("display_name").cloned().unwrap_or(Value::Null),
        "email": profile.get("email").cloned().unwrap_or(Value::Null),
        "is_bot": member.get("is_bot").cloned().unwrap_or(Value::Null),
        "deleted": member.get("deleted").cloned().unwrap_or(Value::Null),
    })
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn default_token_type() -> String {
    "oauth".to_string()
}

fn default_conversation_types() -> String {
    "public_channel,private_channel,mpim,im".to_string()
}

fn default_list_limit() -> usize {
    100
}

fn default_search_limit() -> usize {
    20
}

fn default_message_limit() -> usize {
    50
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::{connection_slug_from_input, conversation_matches, user_matches};
    use serde_json::json;

    #[test]
    fn connection_slug_defaults_and_validates() {
        assert_eq!(
            connection_slug_from_input(&json!({}), "slack-login").unwrap(),
            "slack-login"
        );
        assert_eq!(
            connection_slug_from_input(&json!({"connection_slug": "work-main"}), "slack-login")
                .unwrap(),
            "work-main"
        );
        assert!(connection_slug_from_input(
            &json!({"connection_slug": "Work Main"}),
            "slack-login"
        )
        .is_err());
    }

    #[test]
    fn search_matches_channel_names_and_ids() {
        let channel = json!({
            "id": "C123",
            "name": "deploys",
            "name_normalized": "deploys"
        });

        assert!(conversation_matches(&channel, "dep"));
        assert!(conversation_matches(&channel, "c123"));
        assert!(!conversation_matches(&channel, "sales"));
    }

    #[test]
    fn search_matches_user_profile_fields() {
        let user = json!({
            "id": "U123",
            "name": "tonyke",
            "profile": {
                "display_name": "Tony",
                "email": "tony@example.com"
            }
        });

        assert!(user_matches(&user, "tony"));
        assert!(user_matches(&user, "example"));
        assert!(!user_matches(&user, "karen"));
    }
}

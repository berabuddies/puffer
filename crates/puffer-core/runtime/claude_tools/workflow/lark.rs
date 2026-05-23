//! Lark internal workflow actions.
//!
//! The consolidated internal `Lark` tool handles Lark app credentials,
//! user-token credentials, environment import, and read-only lookup operations.
//! Outbound message sends, uploads, replies, and reactions stay connector
//! actions on `lark-app` or `lark-login`.

use crate::AppState;
use anyhow::{bail, Context, Result};
use puffer_config::ConfigPaths;
use puffer_lark::{
    connection_description, connector_slug_for_auth, credential_path, save_credential,
    LarkAuthKind, LarkAuthTest, LarkBrand, LarkClient, LarkCredential,
};
use puffer_subscriptions::{ConnectionRecord, ConnectionState};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;

use super::subscription_globals;

const LARK_APP_CONNECTOR_SLUG: &str = "lark-app";
const LARK_LOGIN_CONNECTOR_SLUG: &str = "lark-login";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum LarkAction {
    ConfigureApp,
    ImportEnv,
    ListChats,
    LoginToken,
    MgetMessages,
    ReadMessages,
    SearchChats,
    SearchMessages,
    SearchUsers,
}

#[derive(Debug, Deserialize)]
struct LarkInput {
    action: LarkAction,
}

#[derive(Debug, Deserialize)]
struct ConfigureAppInput {
    app_id: String,
    app_secret: String,
    #[serde(default)]
    brand: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LoginTokenInput {
    user_access_token: String,
    #[serde(default)]
    app_id: Option<String>,
    #[serde(default)]
    brand: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ImportEnvInput {
    #[serde(default)]
    brand: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ListChatsInput {
    #[serde(default = "default_list_limit")]
    page_size: usize,
    #[serde(default)]
    page_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SearchChatsInput {
    query: String,
    #[serde(default = "default_search_limit")]
    page_size: usize,
    #[serde(default)]
    page_token: Option<String>,
    #[serde(default)]
    search_types: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SearchUsersInput {
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    user_ids: Option<String>,
    #[serde(default = "default_search_limit")]
    page_size: usize,
    #[serde(default)]
    has_chatted: bool,
    #[serde(default)]
    exclude_external_users: bool,
}

#[derive(Debug, Deserialize)]
struct ReadMessagesInput {
    #[serde(default)]
    chat_id: Option<String>,
    #[serde(default)]
    thread_id: Option<String>,
    #[serde(default = "default_message_limit")]
    page_size: usize,
    #[serde(default)]
    page_token: Option<String>,
    #[serde(default)]
    sort: Option<String>,
    #[serde(default)]
    start_time: Option<String>,
    #[serde(default)]
    end_time: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SearchMessagesInput {
    query: String,
    #[serde(default = "default_search_limit")]
    page_size: usize,
    #[serde(default)]
    page_token: Option<String>,
    #[serde(default)]
    chat_ids: Option<String>,
    #[serde(default)]
    sender_ids: Option<String>,
    #[serde(default)]
    chat_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MgetMessagesInput {
    message_ids: String,
}

/// Executes the consolidated internal `Lark` workflow action.
pub fn execute_lark(_state: &mut AppState, cwd: &Path, input: Value) -> Result<String> {
    let parsed: LarkInput = serde_json::from_value(input.clone()).context("invalid Lark input")?;
    match parsed.action {
        LarkAction::ConfigureApp => execute_configure_app(cwd, input),
        LarkAction::ImportEnv => execute_import_env(cwd, input),
        LarkAction::ListChats => execute_list_chats(cwd, input),
        LarkAction::LoginToken => execute_login_token(cwd, input),
        LarkAction::MgetMessages => execute_mget_messages(cwd, input),
        LarkAction::ReadMessages => execute_read_messages(cwd, input),
        LarkAction::SearchChats => execute_search_chats(cwd, input),
        LarkAction::SearchMessages => execute_search_messages(cwd, input),
        LarkAction::SearchUsers => execute_search_users(cwd, input),
    }
}

fn execute_configure_app(cwd: &Path, input: Value) -> Result<String> {
    let connection_slug = connection_slug_from_input(&input, LARK_APP_CONNECTOR_SLUG)?;
    let parsed: ConfigureAppInput =
        serde_json::from_value(input).context("invalid Lark configure_app input")?;
    if parsed.app_id.trim().is_empty() || parsed.app_secret.trim().is_empty() {
        bail!("Lark configure_app requires non-empty app_id and app_secret");
    }
    let brand = parse_brand(parsed.brand.as_deref())?;
    let auth = LarkAuthKind::App {
        app_id: parsed.app_id,
        app_secret: parsed.app_secret,
    };
    persist_authenticated_credential(cwd, connection_slug, brand, auth)
}

fn execute_login_token(cwd: &Path, input: Value) -> Result<String> {
    let connection_slug = connection_slug_from_input(&input, LARK_LOGIN_CONNECTOR_SLUG)?;
    let parsed: LoginTokenInput =
        serde_json::from_value(input).context("invalid Lark login_token input")?;
    if parsed.user_access_token.trim().is_empty() {
        bail!("Lark login_token requires a non-empty user_access_token");
    }
    let brand = parse_brand(parsed.brand.as_deref())?;
    let auth = LarkAuthKind::UserToken {
        app_id: parsed.app_id.filter(|value| !value.trim().is_empty()),
        user_access_token: parsed.user_access_token,
    };
    persist_authenticated_credential(cwd, connection_slug, brand, auth)
}

fn execute_import_env(cwd: &Path, input: Value) -> Result<String> {
    let parsed: ImportEnvInput =
        serde_json::from_value(input.clone()).context("invalid Lark import_env input")?;
    let env_brand = env_brand();
    let brand = parse_brand(parsed.brand.as_deref().or(env_brand.as_deref()))?;
    let app_id = env_nonempty("LARK_APP_ID");
    let app_secret = env_nonempty("LARK_APP_SECRET");
    let user_access_token = env_nonempty("LARK_USER_ACCESS_TOKEN");
    let auth = match (app_id, app_secret, user_access_token) {
        (Some(app_id), Some(app_secret), _) => LarkAuthKind::App { app_id, app_secret },
        (app_id, _, Some(user_access_token)) => LarkAuthKind::UserToken {
            app_id,
            user_access_token,
        },
        _ => bail!(
            "Lark import_env requires LARK_APP_ID plus LARK_APP_SECRET, or LARK_USER_ACCESS_TOKEN"
        ),
    };
    let default_slug = connector_slug_for_auth(&auth);
    let connection_slug = connection_slug_from_input(&input, default_slug)?;
    let mut output: Value = serde_json::from_str(&persist_authenticated_credential(
        cwd,
        connection_slug,
        brand,
        auth,
    )?)?;
    output["imported"] = Value::Bool(true);
    output["source"] = Value::String("environment".to_string());
    Ok(output.to_string())
}

fn persist_authenticated_credential(
    cwd: &Path,
    connection_slug: String,
    brand: LarkBrand,
    auth: LarkAuthKind,
) -> Result<String> {
    let connector_slug = connector_slug_for_auth(&auth).to_string();
    let mut credential = LarkCredential {
        connection_slug: connection_slug.clone(),
        connector_slug: connector_slug.clone(),
        brand,
        tenant_key: None,
        user_open_id: None,
        user_name: None,
        auth,
    };
    let client = LarkClient::new(credential.clone())?;
    let auth_test = client.test_auth()?;
    hydrate_credential(&mut credential, &auth_test);
    let paths = ConfigPaths::discover(cwd);
    let path = credential_path(&paths.user_config_dir, &connection_slug);
    save_credential(&path, &credential)?;
    let (connection, created) = ensure_lark_connection_record(&credential)?;
    Ok(json!({
        "status": "complete",
        "connection_slug": connection_slug,
        "connector_slug": connector_slug,
        "registered_connection": created,
        "connection": connection,
        "brand": credential.brand.as_str(),
        "app_id": credential.auth.app_id(),
        "tenant_key": credential.tenant_key,
        "user_open_id": credential.user_open_id,
        "user_name": credential.user_name,
        "auth_ok": auth_test.ok,
        "next": "Use this connection_slug in ConnectorAct. Resolve Lark chat_id/open_id with lark search-chats or lark search-users before sending."
    })
    .to_string())
}

fn execute_list_chats(cwd: &Path, input: Value) -> Result<String> {
    let connection_slug = connection_slug_from_input(&input, LARK_LOGIN_CONNECTOR_SLUG)?;
    let parsed: ListChatsInput =
        serde_json::from_value(input).context("invalid Lark list_chats input")?;
    let client = client_for_connection(cwd, &connection_slug)?;
    let payload = client.list_chats(parsed.page_size, parsed.page_token.as_deref())?;
    Ok(json!({
        "status": "complete",
        "connection_slug": connection_slug,
        "payload": payload,
        "next": "Use returned chat_id values such as oc_... as ConnectorAct `to` targets."
    })
    .to_string())
}

fn execute_search_chats(cwd: &Path, input: Value) -> Result<String> {
    let connection_slug = connection_slug_from_input(&input, LARK_LOGIN_CONNECTOR_SLUG)?;
    let parsed: SearchChatsInput =
        serde_json::from_value(input).context("invalid Lark search_chats input")?;
    let query = normalized_query(&parsed.query, "Lark search_chats")?;
    let search_types = split_csv(parsed.search_types.as_deref());
    let client = client_for_connection(cwd, &connection_slug)?;
    let payload = client.search_chats(
        &query,
        parsed.page_size,
        parsed.page_token.as_deref(),
        &search_types,
    )?;
    Ok(json!({
        "status": "complete",
        "connection_slug": connection_slug,
        "payload": payload,
        "next": "Use returned chat_id values such as oc_... as ConnectorAct `to` targets."
    })
    .to_string())
}

fn execute_search_users(cwd: &Path, input: Value) -> Result<String> {
    let connection_slug = connection_slug_from_input(&input, LARK_LOGIN_CONNECTOR_SLUG)?;
    let parsed: SearchUsersInput =
        serde_json::from_value(input).context("invalid Lark search_users input")?;
    let user_ids = split_csv(parsed.user_ids.as_deref());
    let client = client_for_connection(cwd, &connection_slug)?;
    let payload = client.search_users(
        parsed.query.as_deref(),
        &user_ids,
        parsed.page_size,
        parsed.has_chatted,
        parsed.exclude_external_users,
    )?;
    Ok(json!({
        "status": "complete",
        "connection_slug": connection_slug,
        "payload": payload,
        "next": "Use returned open_id values such as ou_... as ConnectorAct DM targets."
    })
    .to_string())
}

fn execute_read_messages(cwd: &Path, input: Value) -> Result<String> {
    let connection_slug = connection_slug_from_input(&input, LARK_LOGIN_CONNECTOR_SLUG)?;
    let parsed: ReadMessagesInput =
        serde_json::from_value(input).context("invalid Lark read_messages input")?;
    let (container_type, container_id) = read_messages_container(&parsed)?;
    let client = client_for_connection(cwd, &connection_slug)?;
    let payload = if container_type == "thread" {
        client.read_thread_messages(
            &container_id,
            parsed.page_size,
            parsed.page_token.as_deref(),
            parsed.sort.as_deref(),
            parsed.start_time.as_deref(),
            parsed.end_time.as_deref(),
        )?
    } else {
        client.read_messages(
            &container_id,
            parsed.page_size,
            parsed.page_token.as_deref(),
            parsed.sort.as_deref(),
            parsed.start_time.as_deref(),
            parsed.end_time.as_deref(),
        )?
    };
    Ok(json!({
        "status": "complete",
        "connection_slug": connection_slug,
        "container_type": container_type,
        "container_id": container_id,
        "payload": payload,
        "next": "Use message_id values such as om_... for replies, reactions, or mget_messages."
    })
    .to_string())
}

fn execute_search_messages(cwd: &Path, input: Value) -> Result<String> {
    let connection_slug = connection_slug_from_input(&input, LARK_LOGIN_CONNECTOR_SLUG)?;
    let parsed: SearchMessagesInput =
        serde_json::from_value(input).context("invalid Lark search_messages input")?;
    let query = normalized_query(&parsed.query, "Lark search_messages")?;
    let chat_ids = split_csv(parsed.chat_ids.as_deref());
    let sender_ids = split_csv(parsed.sender_ids.as_deref());
    let client = client_for_connection(cwd, &connection_slug)?;
    let payload = client.search_messages(
        &query,
        parsed.page_size,
        parsed.page_token.as_deref(),
        &chat_ids,
        &sender_ids,
        parsed.chat_type.as_deref(),
    )?;
    Ok(json!({
        "status": "complete",
        "connection_slug": connection_slug,
        "payload": payload,
        "next": "Use returned message_id values with mget_messages for full details, replies, or reactions."
    })
    .to_string())
}

fn execute_mget_messages(cwd: &Path, input: Value) -> Result<String> {
    let connection_slug = connection_slug_from_input(&input, LARK_LOGIN_CONNECTOR_SLUG)?;
    let parsed: MgetMessagesInput =
        serde_json::from_value(input).context("invalid Lark mget_messages input")?;
    let message_ids = split_csv(Some(&parsed.message_ids));
    let client = client_for_connection(cwd, &connection_slug)?;
    let payload = client.mget_messages(&message_ids)?;
    Ok(json!({
        "status": "complete",
        "connection_slug": connection_slug,
        "payload": payload,
        "next": "Use message_id values such as om_... for ConnectorAct replies or reactions."
    })
    .to_string())
}

fn hydrate_credential(credential: &mut LarkCredential, auth_test: &LarkAuthTest) {
    credential.tenant_key = auth_test
        .tenant_key
        .clone()
        .or(credential.tenant_key.take());
    credential.user_open_id = auth_test
        .user_open_id
        .clone()
        .or(credential.user_open_id.take());
    credential.user_name = auth_test.user_name.clone().or(credential.user_name.take());
}

fn ensure_lark_connection_record(credential: &LarkCredential) -> Result<(ConnectionRecord, bool)> {
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

fn client_for_connection(cwd: &Path, connection_slug: &str) -> Result<LarkClient> {
    let paths = ConfigPaths::discover(cwd);
    let path = credential_path(&paths.user_config_dir, connection_slug);
    let credential = puffer_lark::load_credential(&path)?;
    LarkClient::new(credential)
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
        bail!("Lark connection_slug must be non-empty kebab-case ASCII");
    }
    Ok(())
}

fn parse_brand(value: Option<&str>) -> Result<LarkBrand> {
    match value.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => LarkBrand::parse(value),
        None => Ok(LarkBrand::default()),
    }
}

fn env_brand() -> Option<String> {
    env_nonempty("LARK_BRAND")
}

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn normalized_query(query: &str, label: &str) -> Result<String> {
    let query = query.trim().to_string();
    if query.is_empty() {
        bail!("{label} requires a non-empty query");
    }
    Ok(query)
}

fn split_csv(value: Option<&str>) -> Vec<String> {
    value
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn read_messages_container(parsed: &ReadMessagesInput) -> Result<(&'static str, String)> {
    let chat_id = parsed
        .chat_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let thread_id = parsed
        .thread_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    match (chat_id, thread_id) {
        (Some(chat_id), None) => Ok(("chat", chat_id.to_string())),
        (None, Some(thread_id)) => Ok(("thread", thread_id.to_string())),
        (Some(_), Some(_)) => bail!("Lark read_messages accepts only one of chat_id or thread_id"),
        (None, None) => bail!("Lark read_messages requires chat_id or thread_id"),
    }
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

#[cfg(test)]
mod tests {
    use super::{connection_slug_from_input, parse_brand, read_messages_container, split_csv};
    use puffer_lark::LarkBrand;
    use serde_json::json;

    #[test]
    fn connection_slug_defaults_and_validates() {
        assert_eq!(
            connection_slug_from_input(&json!({}), "lark-login").unwrap(),
            "lark-login"
        );
        assert_eq!(
            connection_slug_from_input(&json!({"connection_slug": "work-main"}), "lark-login")
                .unwrap(),
            "work-main"
        );
        assert!(
            connection_slug_from_input(&json!({"connection_slug": "Work Main"}), "lark-login")
                .is_err()
        );
    }

    #[test]
    fn brand_defaults_to_lark_and_parses_feishu() {
        assert_eq!(parse_brand(None).unwrap(), LarkBrand::Lark);
        assert_eq!(parse_brand(Some("feishu")).unwrap(), LarkBrand::Feishu);
    }

    #[test]
    fn csv_splitting_trims_empty_entries() {
        assert_eq!(
            split_csv(Some("oc_1, oc_2,, ")),
            vec!["oc_1".to_string(), "oc_2".to_string()]
        );
    }

    #[test]
    fn read_messages_accepts_chat_or_thread_container() {
        let chat = super::ReadMessagesInput {
            chat_id: Some("oc_1".to_string()),
            thread_id: None,
            page_size: 10,
            page_token: None,
            sort: None,
            start_time: None,
            end_time: None,
        };
        let thread = super::ReadMessagesInput {
            chat_id: None,
            thread_id: Some("omt_1".to_string()),
            page_size: 10,
            page_token: None,
            sort: None,
            start_time: None,
            end_time: None,
        };

        assert_eq!(
            read_messages_container(&chat).unwrap(),
            ("chat", "oc_1".to_string())
        );
        assert_eq!(
            read_messages_container(&thread).unwrap(),
            ("thread", "omt_1".to_string())
        );
    }
}

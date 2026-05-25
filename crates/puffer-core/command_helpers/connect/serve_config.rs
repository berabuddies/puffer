use super::{ask_choice, ask_input, ConnectResult};
use crate::AppState;
use anyhow::{anyhow, bail, Context, Result};
use puffer_config::{ensure_workspace_dirs, ConfigPaths};
use puffer_resources::LoadedResources;
use std::fs;
use std::path::{Path, PathBuf};

/// Configures the Telegram bot serve-mode connector from deterministic questions.
pub(super) fn connect_telegram_bot(
    state: &mut AppState,
    resources: &LoadedResources,
    connection: &str,
) -> Result<ConnectResult> {
    let token = ask_input(
        state,
        resources,
        "Bot Token",
        "What Telegram bot token should Puffer use?",
    )?;
    let path = write_workspace_connector_config(
        state,
        "telegram",
        connection,
        &[
            ("token", toml::Value::String(token)),
            ("require_mention", toml::Value::Boolean(true)),
            (
                "group_key_policy",
                toml::Value::String("per_user".to_string()),
            ),
        ],
    )?;
    Ok(serve_summary(
        "telegram-bot",
        connection,
        "Bot token",
        &path,
    ))
}

/// Configures the Discord serve-mode connector from deterministic questions.
pub(super) fn connect_discord_bot(
    state: &mut AppState,
    resources: &LoadedResources,
    connection: &str,
) -> Result<ConnectResult> {
    let token = ask_input(
        state,
        resources,
        "Bot Token",
        "What Discord bot token should Puffer use?",
    )?;
    let path = write_workspace_connector_config(
        state,
        "discord",
        connection,
        &[
            ("token", toml::Value::String(token)),
            ("require_mention", toml::Value::Boolean(true)),
            (
                "group_key_policy",
                toml::Value::String("per_user".to_string()),
            ),
        ],
    )?;
    Ok(serve_summary("discord-bot", connection, "Bot token", &path))
}

/// Configures the Matrix serve-mode connector from deterministic questions.
pub(super) fn connect_matrix_bot(
    state: &mut AppState,
    resources: &LoadedResources,
    connection: &str,
) -> Result<ConnectResult> {
    let homeserver_url = ask_input(
        state,
        resources,
        "Homeserver",
        "What Matrix homeserver URL should Puffer use?",
    )?;
    let username = ask_input(
        state,
        resources,
        "Username",
        "What Matrix username should Puffer use?",
    )?;
    let password = ask_input(
        state,
        resources,
        "Password",
        "What Matrix password should Puffer use?",
    )?;
    let path = write_workspace_connector_config(
        state,
        "matrix",
        connection,
        &[
            ("homeserver_url", toml::Value::String(homeserver_url)),
            ("username", toml::Value::String(username)),
            ("password", toml::Value::String(password)),
            ("require_mention", toml::Value::Boolean(true)),
            (
                "group_key_policy",
                toml::Value::String("per_user".to_string()),
            ),
        ],
    )?;
    Ok(serve_summary(
        "matrix-bot",
        connection,
        "Password login",
        &path,
    ))
}

/// Configures the webhook serve-mode connector from deterministic questions.
pub(super) fn connect_webhook(
    state: &mut AppState,
    resources: &LoadedResources,
    connection: &str,
) -> Result<ConnectResult> {
    let bind_address = ask_input(
        state,
        resources,
        "Bind",
        "What bind address should the webhook listen on?",
    )?;
    let auth_mode = ask_choice(
        state,
        resources,
        "Auth",
        "Should this webhook require bearer-token auth?",
        &[
            (
                "No bearer token",
                "Accept webhook requests without Authorization.",
            ),
            ("Bearer token", "Require Authorization: Bearer <token>."),
        ],
    )?;
    let mut fields = vec![
        ("bind_address", toml::Value::String(bind_address)),
        ("path", toml::Value::String("/puffer".to_string())),
    ];
    if auth_mode == "Bearer token" {
        let token = ask_input(
            state,
            resources,
            "Token",
            "What bearer token should webhook callers send?",
        )?;
        fields.push(("auth_token", toml::Value::String(token)));
    }
    let path = write_workspace_connector_config(state, "webhook", connection, &fields)?;
    Ok(serve_summary("webhook", connection, &auth_mode, &path))
}

/// Configures a named webhook preset through the serve-mode webhook connector.
pub(super) fn connect_webhook_preset(
    state: &mut AppState,
    resources: &LoadedResources,
    connector_slug: &str,
    connection: &str,
) -> Result<ConnectResult> {
    let preset = webhook_preset(connector_slug)?;
    let bind_address = ask_input(
        state,
        resources,
        "Bind",
        &format!(
            "What bind address should the {} webhook listen on?",
            preset.product
        ),
    )?;
    let path_value = ask_input(
        state,
        resources,
        "Path",
        &format!(
            "What URL path should {} post webhook events to?",
            preset.product
        ),
    )?;
    let path_value = normalize_webhook_path_with_default(&path_value, preset.default_path);
    let fields = vec![
        ("bind_address", toml::Value::String(bind_address)),
        ("path", toml::Value::String(path_value)),
        (
            "welcome_message",
            toml::Value::String(format!("{} webhook ready.", preset.product)),
        ),
    ];
    let path = write_workspace_connector_config(state, "webhook", connection, &fields)?;
    Ok(serve_summary(
        connector_slug,
        connection,
        preset.method,
        &path,
    ))
}

struct WebhookPreset {
    product: &'static str,
    default_path: &'static str,
    method: &'static str,
}

fn webhook_preset(connector_slug: &str) -> Result<WebhookPreset> {
    let preset = match connector_slug {
        "asana-webhook" => WebhookPreset {
            product: "Asana",
            default_path: "/asana",
            method: "Asana event webhook",
        },
        "github-webhook" => WebhookPreset {
            product: "GitHub",
            default_path: "/github",
            method: "GitHub event webhook",
        },
        "gitlab-webhook" => WebhookPreset {
            product: "GitLab",
            default_path: "/gitlab",
            method: "GitLab event webhook",
        },
        "jira-webhook" => WebhookPreset {
            product: "Jira",
            default_path: "/jira",
            method: "Jira event webhook",
        },
        "linear-webhook" => WebhookPreset {
            product: "Linear",
            default_path: "/linear",
            method: "Linear event webhook",
        },
        "stripe-webhook" => WebhookPreset {
            product: "Stripe",
            default_path: "/stripe",
            method: "Stripe event webhook",
        },
        _ => bail!("unsupported webhook preset `{connector_slug}`"),
    };
    Ok(preset)
}

fn normalize_webhook_path_with_default(value: &str, default: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return default.to_string();
    }
    if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    }
}

fn write_workspace_connector_config(
    state: &AppState,
    platform: &str,
    display_name: &str,
    fields: &[(&str, toml::Value)],
) -> Result<PathBuf> {
    let paths = ConfigPaths::discover(&state.cwd);
    ensure_workspace_dirs(&paths)?;
    let path = paths.workspace_config_dir.join("connectors.toml");
    let (mut root, prefer_wrapped) = load_connector_document(&path)?;
    upsert_connector_table(&mut root, prefer_wrapped, platform, display_name, fields)?;
    let rendered =
        toml::to_string_pretty(&toml::Value::Table(root)).context("serialize connector config")?;
    fs::write(&path, rendered)
        .with_context(|| format!("failed to write connector config {}", path.display()))?;
    Ok(path)
}

fn load_connector_document(path: &Path) -> Result<(toml::Table, bool)> {
    if !path.exists() {
        return Ok((toml::Table::new(), true));
    }
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read connector config {}", path.display()))?;
    let prefer_wrapped = raw.trim().is_empty();
    let value: toml::Value = if raw.trim().is_empty() {
        toml::Value::Table(toml::Table::new())
    } else {
        toml::from_str(&raw)
            .with_context(|| format!("failed to parse connector config {}", path.display()))?
    };
    match value {
        toml::Value::Table(root) => {
            let prefer_wrapped = prefer_wrapped || root.contains_key("connectors");
            Ok((root, prefer_wrapped))
        }
        other => bail!(
            "connector config root must be a table, got {}",
            other.type_str()
        ),
    }
}

fn upsert_connector_table(
    root: &mut toml::Table,
    prefer_wrapped: bool,
    platform: &str,
    display_name: &str,
    fields: &[(&str, toml::Value)],
) -> Result<()> {
    if prefer_wrapped {
        let connectors = root
            .entry("connectors".to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        let connectors = connectors
            .as_table_mut()
            .ok_or_else(|| anyhow!("`connectors` key must be a table"))?;
        upsert_platform_table(connectors, platform, display_name, fields)
    } else {
        upsert_platform_table(root, platform, display_name, fields)
    }
}

fn upsert_platform_table(
    parent: &mut toml::Table,
    platform: &str,
    display_name: &str,
    fields: &[(&str, toml::Value)],
) -> Result<()> {
    let mut table = match parent.remove(platform) {
        Some(toml::Value::Table(table)) => table,
        Some(other) => bail!(
            "connector `{platform}` config must be a table, got {}",
            other.type_str()
        ),
        None => toml::Table::new(),
    };
    table.insert("enabled".to_string(), toml::Value::Boolean(true));
    table.insert(
        "display_name".to_string(),
        toml::Value::String(display_name.to_string()),
    );
    for (key, value) in fields {
        table.insert((*key).to_string(), value.clone());
    }
    parent.insert(platform.to_string(), toml::Value::Table(table));
    Ok(())
}

fn serve_summary(
    connector_slug: &str,
    connection: &str,
    method: &str,
    path: &Path,
) -> ConnectResult {
    ConnectResult {
        summary: format!(
            "Connector serve configuration written.\nconnector: {connector_slug}\nconnection: {connection}\nmethod: {method}\nconfig: {}\nnext: run `puffer serve` to start the connector.",
            path.display()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{with_user_question_prompt_handler, UserQuestionPromptResponse};
    use puffer_config::PufferConfig;
    use puffer_session_store::SessionMetadata;
    use serde_json::{json, Map, Value};
    use std::sync::{Arc, Mutex};

    fn temp_state() -> AppState {
        let tempdir = tempfile::tempdir().unwrap();
        let cwd = tempdir.keep();
        let session = SessionMetadata {
            id: uuid::Uuid::nil(),
            display_name: None,
            generated_title: None,
            cwd: cwd.clone(),
            created_at_ms: 0,
            updated_at_ms: 0,
            parent_session_id: None,
            slug: None,
            tags: Vec::new(),
            note: None,
        };
        AppState::new(PufferConfig::default(), cwd, session)
    }

    fn answer_request(request: &crate::UserQuestionPromptRequest) -> UserQuestionPromptResponse {
        let question = request.questions[0]["question"]
            .as_str()
            .expect("question text")
            .to_string();
        let answer = match question.as_str() {
            "What Telegram bot token should Puffer use?" => "telegram-token",
            "What Discord bot token should Puffer use?" => "discord-token",
            "What Matrix homeserver URL should Puffer use?" => "https://matrix.example.org",
            "What Matrix username should Puffer use?" => "puffer-bot",
            "What Matrix password should Puffer use?" => "matrix-password",
            "What bind address should the Asana webhook listen on?" => "127.0.0.1:9797",
            "What URL path should Asana post webhook events to?" => "asana",
            "What bind address should the GitHub webhook listen on?" => "127.0.0.1:9292",
            "What URL path should GitHub post webhook events to?" => "/github",
            "What bind address should the GitLab webhook listen on?" => "127.0.0.1:9494",
            "What URL path should GitLab post webhook events to?" => "gitlab",
            "What bind address should the Jira webhook listen on?" => "127.0.0.1:9595",
            "What URL path should Jira post webhook events to?" => "jira",
            "What bind address should the Linear webhook listen on?" => "127.0.0.1:9393",
            "What URL path should Linear post webhook events to?" => "linear",
            "What bind address should the Stripe webhook listen on?" => "127.0.0.1:9696",
            "What URL path should Stripe post webhook events to?" => "stripe",
            "What bind address should the webhook listen on?" => "127.0.0.1:9191",
            "Should this webhook require bearer-token auth?" => "No bearer token",
            other => panic!("unexpected question: {other}"),
        };
        UserQuestionPromptResponse {
            answers: Map::from_iter([(question, json!(answer))]),
            annotations: Map::new(),
        }
    }

    #[test]
    fn telegram_bot_connect_writes_wrapped_workspace_config() {
        let mut state = temp_state();
        let resources = LoadedResources::default();
        let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
        let request_log = Arc::clone(&requests);

        let result = with_user_question_prompt_handler(
            move |request| {
                request_log.lock().unwrap().push(request.questions.clone());
                answer_request(&request)
            },
            || connect_telegram_bot(&mut state, &resources, "telegram-bot"),
        )
        .expect("connect result");

        assert!(result.summary.contains("connector: telegram-bot"));
        assert!(result.summary.contains("run `puffer serve`"));
        let path = state.cwd.join(".puffer/connectors.toml");
        let raw = fs::read_to_string(path).expect("connector config");
        assert!(raw.contains("[connectors.telegram]"));
        assert!(raw.contains("display_name = \"telegram-bot\""));
        assert!(raw.contains("token = \"telegram-token\""));
        assert!(raw.contains("require_mention = true"));
        assert!(raw.contains("group_key_policy = \"per_user\""));
        assert_eq!(requests.lock().unwrap()[0][0]["type"], "input");
    }

    #[test]
    fn discord_bot_connect_writes_wrapped_workspace_config() {
        let mut state = temp_state();
        let resources = LoadedResources::default();
        let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
        let request_log = Arc::clone(&requests);

        let result = with_user_question_prompt_handler(
            move |request| {
                request_log.lock().unwrap().push(request.questions.clone());
                answer_request(&request)
            },
            || connect_discord_bot(&mut state, &resources, "discord-bot"),
        )
        .expect("connect result");

        assert!(result.summary.contains("run `puffer serve`"));
        let path = state.cwd.join(".puffer/connectors.toml");
        let raw = fs::read_to_string(path).expect("connector config");
        assert!(raw.contains("[connectors.discord]"));
        assert!(raw.contains("display_name = \"discord-bot\""));
        assert!(raw.contains("token = \"discord-token\""));
        assert!(raw.contains("require_mention = true"));
        assert!(raw.contains("group_key_policy = \"per_user\""));
        assert_eq!(requests.lock().unwrap()[0][0]["type"], "input");
    }

    #[test]
    fn matrix_bot_connect_writes_required_config() {
        let mut state = temp_state();
        let resources = LoadedResources::default();

        let result = with_user_question_prompt_handler(
            |request| answer_request(&request),
            || connect_matrix_bot(&mut state, &resources, "matrix-bot"),
        )
        .expect("connect result");

        assert!(result.summary.contains("method: Password login"));
        let path = state.cwd.join(".puffer/connectors.toml");
        let raw = fs::read_to_string(path).expect("connector config");
        assert!(raw.contains("[connectors.matrix]"));
        assert!(raw.contains("homeserver_url = \"https://matrix.example.org\""));
        assert!(raw.contains("username = \"puffer-bot\""));
        assert!(raw.contains("password = \"matrix-password\""));
        assert!(raw.contains("require_mention = true"));
        assert!(raw.contains("group_key_policy = \"per_user\""));
    }

    #[test]
    fn webhook_connect_writes_no_auth_config() {
        let mut state = temp_state();
        let resources = LoadedResources::default();

        let result = with_user_question_prompt_handler(
            |request| answer_request(&request),
            || connect_webhook(&mut state, &resources, "local-hook"),
        )
        .expect("connect result");

        assert!(result.summary.contains("connection: local-hook"));
        let path = state.cwd.join(".puffer/connectors.toml");
        let raw = fs::read_to_string(path).expect("connector config");
        assert!(raw.contains("[connectors.webhook]"));
        assert!(raw.contains("display_name = \"local-hook\""));
        assert!(raw.contains("bind_address = \"127.0.0.1:9191\""));
        assert!(raw.contains("path = \"/puffer\""));
        assert!(!raw.contains("auth_token"));
    }

    #[test]
    fn github_webhook_connect_writes_webhook_preset_config() {
        let mut state = temp_state();
        let resources = LoadedResources::default();

        let result = with_user_question_prompt_handler(
            |request| answer_request(&request),
            || connect_webhook_preset(&mut state, &resources, "github-webhook", "github-webhook"),
        )
        .expect("connect result");

        assert!(result.summary.contains("connector: github-webhook"));
        assert!(result.summary.contains("run `puffer serve`"));
        let path = state.cwd.join(".puffer/connectors.toml");
        let raw = fs::read_to_string(path).expect("connector config");
        assert!(raw.contains("[connectors.webhook]"));
        assert!(raw.contains("display_name = \"github-webhook\""));
        assert!(raw.contains("bind_address = \"127.0.0.1:9292\""));
        assert!(raw.contains("path = \"/github\""));
        assert!(raw.contains("welcome_message = \"GitHub webhook ready.\""));
    }

    #[test]
    fn asana_webhook_connect_writes_webhook_preset_config() {
        let mut state = temp_state();
        let resources = LoadedResources::default();

        let result = with_user_question_prompt_handler(
            |request| answer_request(&request),
            || connect_webhook_preset(&mut state, &resources, "asana-webhook", "asana-webhook"),
        )
        .expect("connect result");

        assert!(result.summary.contains("connector: asana-webhook"));
        assert!(result.summary.contains("run `puffer serve`"));
        let path = state.cwd.join(".puffer/connectors.toml");
        let raw = fs::read_to_string(path).expect("connector config");
        assert!(raw.contains("[connectors.webhook]"));
        assert!(raw.contains("display_name = \"asana-webhook\""));
        assert!(raw.contains("bind_address = \"127.0.0.1:9797\""));
        assert!(raw.contains("path = \"/asana\""));
        assert!(raw.contains("welcome_message = \"Asana webhook ready.\""));
    }

    #[test]
    fn gitlab_webhook_connect_writes_webhook_preset_config() {
        let mut state = temp_state();
        let resources = LoadedResources::default();

        let result = with_user_question_prompt_handler(
            |request| answer_request(&request),
            || connect_webhook_preset(&mut state, &resources, "gitlab-webhook", "gitlab-webhook"),
        )
        .expect("connect result");

        assert!(result.summary.contains("connector: gitlab-webhook"));
        assert!(result.summary.contains("run `puffer serve`"));
        let path = state.cwd.join(".puffer/connectors.toml");
        let raw = fs::read_to_string(path).expect("connector config");
        assert!(raw.contains("[connectors.webhook]"));
        assert!(raw.contains("display_name = \"gitlab-webhook\""));
        assert!(raw.contains("bind_address = \"127.0.0.1:9494\""));
        assert!(raw.contains("path = \"/gitlab\""));
        assert!(raw.contains("welcome_message = \"GitLab webhook ready.\""));
    }

    #[test]
    fn jira_webhook_connect_writes_webhook_preset_config() {
        let mut state = temp_state();
        let resources = LoadedResources::default();

        let result = with_user_question_prompt_handler(
            |request| answer_request(&request),
            || connect_webhook_preset(&mut state, &resources, "jira-webhook", "jira-webhook"),
        )
        .expect("connect result");

        assert!(result.summary.contains("connector: jira-webhook"));
        assert!(result.summary.contains("run `puffer serve`"));
        let path = state.cwd.join(".puffer/connectors.toml");
        let raw = fs::read_to_string(path).expect("connector config");
        assert!(raw.contains("[connectors.webhook]"));
        assert!(raw.contains("display_name = \"jira-webhook\""));
        assert!(raw.contains("bind_address = \"127.0.0.1:9595\""));
        assert!(raw.contains("path = \"/jira\""));
        assert!(raw.contains("welcome_message = \"Jira webhook ready.\""));
    }

    #[test]
    fn linear_webhook_connect_writes_webhook_preset_config() {
        let mut state = temp_state();
        let resources = LoadedResources::default();

        let result = with_user_question_prompt_handler(
            |request| answer_request(&request),
            || connect_webhook_preset(&mut state, &resources, "linear-webhook", "linear-webhook"),
        )
        .expect("connect result");

        assert!(result.summary.contains("connector: linear-webhook"));
        assert!(result.summary.contains("run `puffer serve`"));
        let path = state.cwd.join(".puffer/connectors.toml");
        let raw = fs::read_to_string(path).expect("connector config");
        assert!(raw.contains("[connectors.webhook]"));
        assert!(raw.contains("display_name = \"linear-webhook\""));
        assert!(raw.contains("bind_address = \"127.0.0.1:9393\""));
        assert!(raw.contains("path = \"/linear\""));
        assert!(raw.contains("welcome_message = \"Linear webhook ready.\""));
    }

    #[test]
    fn stripe_webhook_connect_writes_webhook_preset_config() {
        let mut state = temp_state();
        let resources = LoadedResources::default();

        let result = with_user_question_prompt_handler(
            |request| answer_request(&request),
            || connect_webhook_preset(&mut state, &resources, "stripe-webhook", "stripe-webhook"),
        )
        .expect("connect result");

        assert!(result.summary.contains("connector: stripe-webhook"));
        assert!(result.summary.contains("run `puffer serve`"));
        let path = state.cwd.join(".puffer/connectors.toml");
        let raw = fs::read_to_string(path).expect("connector config");
        assert!(raw.contains("[connectors.webhook]"));
        assert!(raw.contains("display_name = \"stripe-webhook\""));
        assert!(raw.contains("bind_address = \"127.0.0.1:9696\""));
        assert!(raw.contains("path = \"/stripe\""));
        assert!(raw.contains("welcome_message = \"Stripe webhook ready.\""));
    }

    #[test]
    fn writer_preserves_bare_config_shape() {
        let state = temp_state();
        let path = state.cwd.join(".puffer/connectors.toml");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "[telegram]\ntoken = \"old\"\n").unwrap();
        write_workspace_connector_config(
            &state,
            "webhook",
            "hook",
            &[(
                "bind_address",
                toml::Value::String("127.0.0.1:9191".to_string()),
            )],
        )
        .expect("write config");

        let raw = fs::read_to_string(path).expect("connector config");
        assert!(raw.contains("[telegram]"));
        assert!(raw.contains("[webhook]"));
        assert!(!raw.contains("[connectors.webhook]"));
    }
}

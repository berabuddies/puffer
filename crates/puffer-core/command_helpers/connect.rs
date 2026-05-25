use crate::runtime::claude_tools::execute_workflow_tool;
use crate::{AppState, TurnExecution};
use anyhow::{anyhow, bail, Context, Result};
use puffer_resources::LoadedResources;
use serde_json::{json, Value};

mod catalog;
mod serve_config;

/// Runs the deterministic `/connect` connector-auth flow without a provider turn.
pub fn execute_connect_flow(
    state: &mut AppState,
    resources: &LoadedResources,
    args: &str,
) -> Result<TurnExecution> {
    let target = parse_or_ask_target(state, resources, args)?;
    let result = match target.connector_slug.as_str() {
        "telegram-login" => connect_telegram(state, resources, &target.connection_name)?,
        "slack-app" => connect_slack_app(state, resources, &target.connection_name)?,
        "slack-login" => connect_slack_login(state, resources, &target.connection_name)?,
        "lark-app" => connect_lark_app(state, resources, &target.connection_name)?,
        "lark-login" => connect_lark_login(state, resources, &target.connection_name)?,
        "email" => connect_email(state, resources, &target.connection_name)?,
        "telegram-bot" => {
            serve_config::connect_telegram_bot(state, resources, &target.connection_name)?
        }
        "discord-bot" => {
            serve_config::connect_discord_bot(state, resources, &target.connection_name)?
        }
        "matrix-bot" => {
            serve_config::connect_matrix_bot(state, resources, &target.connection_name)?
        }
        "github-webhook" => {
            serve_config::connect_github_webhook(state, resources, &target.connection_name)?
        }
        "gitlab-webhook" => {
            serve_config::connect_gitlab_webhook(state, resources, &target.connection_name)?
        }
        "jira-webhook" => {
            serve_config::connect_jira_webhook(state, resources, &target.connection_name)?
        }
        "linear-webhook" => {
            serve_config::connect_linear_webhook(state, resources, &target.connection_name)?
        }
        "stripe-webhook" => {
            serve_config::connect_stripe_webhook(state, resources, &target.connection_name)?
        }
        "webhook" => serve_config::connect_webhook(state, resources, &target.connection_name)?,
        _ => connect_generic(state, resources, &target)?,
    };
    Ok(TurnExecution {
        assistant_text: result.summary,
        tool_invocations: Vec::new(),
        reflection_traces: Vec::new(),
    })
}

struct ConnectTarget {
    connector_slug: String,
    connection_name: String,
}

struct ConnectResult {
    summary: String,
}

fn parse_or_ask_target(
    state: &mut AppState,
    resources: &LoadedResources,
    args: &str,
) -> Result<ConnectTarget> {
    let mut parts = args.split_whitespace();
    let mut connector_slug = parts.next().unwrap_or_default().to_string();
    let mut connection_name = parts.next().unwrap_or_default().to_string();
    let extra = parts.collect::<Vec<_>>().join(" ");

    connector_slug = if connector_slug.is_empty() {
        catalog::ask_connector_slug(state, resources)?
    } else {
        catalog::resolve_connector_slug(state, resources, &connector_slug)?
    };
    if connection_name.is_empty() || !extra.is_empty() {
        if !extra.is_empty() {
            connection_name.clear();
        }
        connection_name = ask_input(
            state,
            resources,
            "Connection",
            "What exact connection name should Puffer use?",
        )?;
    }

    Ok(ConnectTarget {
        connector_slug,
        connection_name,
    })
}

fn connect_telegram(
    state: &mut AppState,
    resources: &LoadedResources,
    connection: &str,
) -> Result<ConnectResult> {
    let method = ask_choice(
        state,
        resources,
        "Method",
        "How should Telegram authenticate this connection?",
        &[
            (
                "Telegram Desktop import",
                "Import an existing local Telegram Desktop session.",
            ),
            (
                "QR login",
                "Approve a login URL from a logged-in Telegram app.",
            ),
            (
                "Phone login",
                "Request a Telegram login code for a phone number.",
            ),
        ],
    )?;
    let output = match method.as_str() {
        "Telegram Desktop import" => call_tool(
            state,
            resources,
            "Telegram",
            json!({
                "action": "import_desktop",
                "connection_slug": connection
            }),
        )?,
        "QR login" => telegram_qr_login(state, resources, connection)?,
        "Phone login" => telegram_phone_login(state, resources, connection)?,
        _ => bail!("unsupported Telegram auth method `{method}`"),
    };
    Ok(summary("telegram-login", connection, &method, &output))
}

fn telegram_qr_login(
    state: &mut AppState,
    resources: &LoadedResources,
    connection: &str,
) -> Result<Value> {
    let mut output = call_tool(
        state,
        resources,
        "Telegram",
        json!({
            "action": "login_qr",
            "connection_slug": connection
        }),
    )?;
    for _ in 0..3 {
        if status(&output) != Some("qr_pending") {
            return Ok(output);
        }
        let url = output
            .pointer("/payload/url")
            .and_then(Value::as_str)
            .unwrap_or("<missing QR URL>");
        let answer = ask_choice_with_preview(
            state,
            resources,
            "Approve",
            "Approve this Telegram QR login URL from a logged-in Telegram app.",
            &[
                ("Approved", "I approved the login request.", Some(url)),
                ("Cancel", "Stop this login attempt.", None),
            ],
        )?;
        if answer != "Approved" {
            bail!("Telegram QR login cancelled");
        }
        output = call_tool(
            state,
            resources,
            "Telegram",
            json!({
                "action": "login_qr_wait",
                "connection_slug": connection
            }),
        )?;
    }
    Ok(output)
}

fn telegram_phone_login(
    state: &mut AppState,
    resources: &LoadedResources,
    connection: &str,
) -> Result<Value> {
    let phone = ask_input(
        state,
        resources,
        "Phone",
        "What E.164 phone number should Telegram use?",
    )?;
    let mut output = call_tool(
        state,
        resources,
        "Telegram",
        json!({
            "action": "login_start",
            "connection_slug": connection,
            "phone": phone
        }),
    )?;
    if status(&output) == Some("awaiting_code") {
        let code = ask_input(
            state,
            resources,
            "Code",
            "What login code did Telegram send?",
        )?;
        output = call_tool(
            state,
            resources,
            "Telegram",
            json!({
                "action": "login_submit_code",
                "connection_slug": connection,
                "code": code
            }),
        )?;
    }
    if status(&output) == Some("awaiting_password") {
        let password = ask_input(
            state,
            resources,
            "Password",
            "What is the Telegram 2FA cloud password?",
        )?;
        output = call_tool(
            state,
            resources,
            "Telegram",
            json!({
                "action": "login_submit_password",
                "connection_slug": connection,
                "password": password
            }),
        )?;
    }
    Ok(output)
}

fn connect_slack_app(
    state: &mut AppState,
    resources: &LoadedResources,
    connection: &str,
) -> Result<ConnectResult> {
    let bot_token = ask_input(
        state,
        resources,
        "Bot Token",
        "What Slack bot token should Puffer use?",
    )?;
    let app_token = ask_input(
        state,
        resources,
        "App Token",
        "What Slack app-level token should Puffer use?",
    )?;
    let output = call_tool(
        state,
        resources,
        "Slack",
        json!({
            "action": "configure_app",
            "connection_slug": connection,
            "bot_token": bot_token,
            "app_token": app_token
        }),
    )?;
    Ok(summary("slack-app", connection, "App credentials", &output))
}

fn connect_slack_login(
    state: &mut AppState,
    resources: &LoadedResources,
    connection: &str,
) -> Result<ConnectResult> {
    let method = ask_choice(
        state,
        resources,
        "Method",
        "How should Slack authenticate this connection?",
        &[
            ("OAuth token", "Use an existing Slack OAuth token."),
            (
                "Local app import",
                "Import local Slack app browser credentials.",
            ),
            ("Browser tokens", "Use xoxd and xoxc browser tokens."),
        ],
    )?;
    let output = match method.as_str() {
        "OAuth token" => {
            let token = ask_input(
                state,
                resources,
                "Token",
                "What Slack OAuth token should Puffer use?",
            )?;
            call_tool(
                state,
                resources,
                "Slack",
                json!({
                    "action": "login_token",
                    "connection_slug": connection,
                    "token": token
                }),
            )?
        }
        "Local app import" => call_tool(
            state,
            resources,
            "Slack",
            json!({
                "action": "import_local",
                "connection_slug": connection
            }),
        )?,
        "Browser tokens" => {
            let workspace_url = ask_input(
                state,
                resources,
                "Workspace",
                "What Slack workspace URL should Puffer use?",
            )?;
            let xoxd = ask_input(
                state,
                resources,
                "xoxd",
                "What Slack xoxd browser cookie should Puffer use?",
            )?;
            let xoxc = ask_input(
                state,
                resources,
                "xoxc",
                "What Slack xoxc browser token should Puffer use?",
            )?;
            call_tool(
                state,
                resources,
                "Slack",
                json!({
                    "action": "login_browser",
                    "connection_slug": connection,
                    "workspace_url": workspace_url,
                    "xoxd_token": xoxd,
                    "xoxc_token": xoxc
                }),
            )?
        }
        _ => bail!("unsupported Slack login method `{method}`"),
    };
    Ok(summary("slack-login", connection, &method, &output))
}

fn connect_lark_app(
    state: &mut AppState,
    resources: &LoadedResources,
    connection: &str,
) -> Result<ConnectResult> {
    let method = ask_choice(
        state,
        resources,
        "Method",
        "How should Lark authenticate this app connection?",
        &[
            ("Manual credentials", "Enter app_id and app_secret."),
            (
                "Environment import",
                "Read Lark credentials from the process environment.",
            ),
        ],
    )?;
    let output = match method.as_str() {
        "Manual credentials" => {
            let brand = ask_brand(state, resources)?;
            let app_id = ask_input(
                state,
                resources,
                "App ID",
                "What Lark app_id should Puffer use?",
            )?;
            let app_secret = ask_input(
                state,
                resources,
                "Secret",
                "What Lark app_secret should Puffer use?",
            )?;
            call_tool(
                state,
                resources,
                "Lark",
                json!({
                    "action": "configure_app",
                    "connection_slug": connection,
                    "brand": brand,
                    "app_id": app_id,
                    "app_secret": app_secret
                }),
            )?
        }
        "Environment import" => call_tool(
            state,
            resources,
            "Lark",
            json!({
                "action": "import_env",
                "connection_slug": connection
            }),
        )?,
        _ => bail!("unsupported Lark app method `{method}`"),
    };
    Ok(summary("lark-app", connection, &method, &output))
}

fn connect_lark_login(
    state: &mut AppState,
    resources: &LoadedResources,
    connection: &str,
) -> Result<ConnectResult> {
    let brand = ask_brand(state, resources)?;
    let token = ask_input(
        state,
        resources,
        "Token",
        "What Lark user_access_token should Puffer use?",
    )?;
    let output = call_tool(
        state,
        resources,
        "Lark",
        json!({
            "action": "login_token",
            "connection_slug": connection,
            "brand": brand,
            "user_access_token": token
        }),
    )?;
    Ok(summary(
        "lark-login",
        connection,
        "User access token",
        &output,
    ))
}

fn connect_email(
    state: &mut AppState,
    resources: &LoadedResources,
    connection: &str,
) -> Result<ConnectResult> {
    let imap_host = ask_input(
        state,
        resources,
        "IMAP",
        "What IMAP host should Puffer use?",
    )?;
    let imap_port = ask_port(
        state,
        resources,
        "IMAP Port",
        "What IMAP port should Puffer use?",
    )?;
    let smtp_host = ask_input(
        state,
        resources,
        "SMTP",
        "What SMTP host should Puffer use?",
    )?;
    let smtp_port = ask_port(
        state,
        resources,
        "SMTP Port",
        "What SMTP port should Puffer use?",
    )?;
    let username = ask_input(
        state,
        resources,
        "Username",
        "What email username should Puffer use?",
    )?;
    let password = ask_input(
        state,
        resources,
        "Password",
        "What email password or app password should Puffer use?",
    )?;
    let from_address = ask_input(
        state,
        resources,
        "From",
        "What from address should outbound email use?",
    )?;
    let output = call_tool(
        state,
        resources,
        "Email",
        json!({
            "action": "configure",
            "imap_host": imap_host,
            "imap_port": imap_port,
            "smtp_host": smtp_host,
            "smtp_port": smtp_port,
            "username": username,
            "password": password,
            "from_address": from_address
        }),
    )?;
    let connection_output = call_tool(
        state,
        resources,
        "ConnectionCreate",
        json!({
            "slug": connection,
            "connector_slug": "email",
            "description": "Email connection configured by /connect"
        }),
    )?;
    let mut merged = output;
    merged["connection"] = connection_output;
    Ok(summary("email", connection, "Email credentials", &merged))
}

fn connect_generic(
    state: &mut AppState,
    resources: &LoadedResources,
    target: &ConnectTarget,
) -> Result<ConnectResult> {
    let output = call_tool(state, resources, "ConnectorList", json!({}))?;
    let connector = output
        .get("connectors")
        .and_then(Value::as_array)
        .and_then(|connectors| {
            connectors.iter().find(|connector| {
                connector.get("connector_slug").and_then(Value::as_str)
                    == Some(target.connector_slug.as_str())
            })
        })
        .ok_or_else(|| anyhow!("connector `{}` not found", target.connector_slug))?;
    if connector
        .get("requires_auth")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        bail!(
            "/connect does not yet have a deterministic auth flow for connector `{}`",
            target.connector_slug
        );
    }
    let output = call_tool(
        state,
        resources,
        "ConnectionCreate",
        json!({
            "slug": target.connection_name,
            "connector_slug": target.connector_slug,
            "description": "Connection registered by /connect"
        }),
    )?;
    Ok(summary(
        &target.connector_slug,
        &target.connection_name,
        "No-auth connection",
        &output,
    ))
}

fn ask_brand(state: &mut AppState, resources: &LoadedResources) -> Result<String> {
    let brand = ask_choice(
        state,
        resources,
        "Brand",
        "Which Lark endpoint family should Puffer use?",
        &[
            ("lark", "Use the international Lark endpoints."),
            ("feishu", "Use the China Feishu endpoints."),
        ],
    )?;
    Ok(brand)
}

fn ask_port(
    state: &mut AppState,
    resources: &LoadedResources,
    header: &str,
    question: &str,
) -> Result<u16> {
    let value = ask_input(state, resources, header, question)?;
    value
        .parse::<u16>()
        .with_context(|| format!("`{value}` is not a valid TCP port"))
}

fn ask_input(
    state: &mut AppState,
    resources: &LoadedResources,
    header: &str,
    question: &str,
) -> Result<String> {
    let output = ask_questions(
        state,
        resources,
        json!([{
            "type": "input",
            "header": header,
            "question": question
        }]),
    )?;
    answer_string(&output, question)
}

fn ask_searchable_choice(
    state: &mut AppState,
    resources: &LoadedResources,
    header: &str,
    question: &str,
    options: &[(String, String)],
) -> Result<String> {
    let options = options
        .iter()
        .map(|(label, description)| {
            json!({
                "label": label,
                "description": description
            })
        })
        .collect::<Vec<_>>();
    let output = ask_questions(
        state,
        resources,
        json!([{
            "type": "choice",
            "header": header,
            "question": question,
            "searchable": true,
            "options": options
        }]),
    )?;
    answer_string(&output, question)
}

fn ask_choice(
    state: &mut AppState,
    resources: &LoadedResources,
    header: &str,
    question: &str,
    options: &[(&str, &str)],
) -> Result<String> {
    ask_choice_with_preview(
        state,
        resources,
        header,
        question,
        &options
            .iter()
            .map(|(label, description)| (*label, *description, None))
            .collect::<Vec<_>>(),
    )
}

fn ask_choice_with_preview(
    state: &mut AppState,
    resources: &LoadedResources,
    header: &str,
    question: &str,
    options: &[(&str, &str, Option<&str>)],
) -> Result<String> {
    let options = options
        .iter()
        .map(|(label, description, preview)| {
            let mut option = json!({
                "label": label,
                "description": description
            });
            if let Some(preview) = preview {
                option["preview"] = Value::String((*preview).to_string());
            }
            option
        })
        .collect::<Vec<_>>();
    let output = ask_questions(
        state,
        resources,
        json!([{
            "type": "choice",
            "header": header,
            "question": question,
            "options": options
        }]),
    )?;
    answer_string(&output, question)
}

fn ask_questions(
    state: &mut AppState,
    resources: &LoadedResources,
    questions: Value,
) -> Result<Value> {
    let output = call_tool(
        state,
        resources,
        "AskUserQuestion",
        json!({ "questions": questions }),
    )?;
    if output
        .get("pending")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        bail!("interactive AskUserQuestion response is required for /connect");
    }
    Ok(output)
}

fn answer_string(output: &Value, question: &str) -> Result<String> {
    let value = output
        .get("answers")
        .and_then(|answers| answers.get(question))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("no answer provided for `{question}`"))?;
    Ok(value.to_string())
}

fn call_tool(
    state: &mut AppState,
    resources: &LoadedResources,
    tool_id: &str,
    input: Value,
) -> Result<Value> {
    let cwd = state.cwd.clone();
    let output = execute_workflow_tool(state, resources, &cwd, tool_id, input, None)?;
    serde_json::from_str(&output).or_else(|_| Ok(json!({ "status": "complete", "output": output })))
}

fn summary(connector_slug: &str, connection: &str, method: &str, output: &Value) -> ConnectResult {
    let status = status(output).unwrap_or("complete");
    let registered = output
        .get("registered_connection")
        .or_else(|| output.pointer("/connection/registered_connection"))
        .and_then(Value::as_bool);
    let registered_text = registered
        .map(|value| if value { "created" } else { "already existed" })
        .unwrap_or("not reported");
    ConnectResult {
        summary: format!(
            "Connector connection configured.\nconnector: {connector_slug}\nconnection: {connection}\nmethod: {method}\nstatus: {status}\nconnection record: {registered_text}"
        ),
    }
}

fn status(output: &Value) -> Option<&str> {
    output.get("status").and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{
        with_user_question_prompt_handler, UserQuestionPromptRequest, UserQuestionPromptResponse,
    };
    use puffer_config::PufferConfig;
    use puffer_session_store::SessionMetadata;
    use serde_json::Map;
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

    fn connector_config(state: &AppState) -> String {
        std::fs::read_to_string(state.cwd.join(".puffer/connectors.toml")).expect("config")
    }

    fn answer_connect_question(request: &UserQuestionPromptRequest) -> UserQuestionPromptResponse {
        let question = request.questions[0]["question"]
            .as_str()
            .expect("question text")
            .to_string();
        let answer = match question.as_str() {
            "What bind address should the webhook listen on?" => "127.0.0.1:9191",
            "Should this webhook require bearer-token auth?" => "No bearer token",
            "What bind address should the GitHub webhook listen on?" => "127.0.0.1:9292",
            "What URL path should GitHub post webhook events to?" => "/github",
            "What bind address should the Linear webhook listen on?" => "127.0.0.1:9393",
            "What URL path should Linear post webhook events to?" => "linear",
            "What bind address should the Stripe webhook listen on?" => "127.0.0.1:9696",
            "What URL path should Stripe post webhook events to?" => "stripe",
            "What Telegram bot token should Puffer use?" => "telegram-token",
            other => panic!("unexpected question: {other}"),
        };
        UserQuestionPromptResponse {
            answers: Map::from_iter([(question, json!(answer))]),
            annotations: Map::new(),
        }
    }

    #[test]
    fn parse_target_uses_two_args_without_questions() {
        let mut state = temp_state();
        let resources = LoadedResources::default();

        let target = parse_or_ask_target(&mut state, &resources, "telegram-login telegram-user")
            .expect("target");

        assert_eq!(target.connector_slug, "telegram-login");
        assert_eq!(target.connection_name, "telegram-user");
    }

    #[test]
    fn parse_target_resolves_unique_connector_search_term() {
        let mut state = temp_state();
        let resources = LoadedResources::default();

        let target =
            parse_or_ask_target(&mut state, &resources, "matrix matrix-main").expect("target");

        assert_eq!(target.connector_slug, "matrix-bot");
        assert_eq!(target.connection_name, "matrix-main");
    }

    #[test]
    fn parse_target_resolves_unique_action_search_term() {
        let mut state = temp_state();
        let resources = LoadedResources::default();

        let target =
            parse_or_ask_target(&mut state, &resources, "vote telegram-user").expect("target");

        assert_eq!(target.connector_slug, "telegram-login");
        assert_eq!(target.connection_name, "telegram-user");
    }

    #[test]
    fn parse_target_asks_for_missing_values_with_input_questions() {
        let mut state = temp_state();
        let resources = LoadedResources::default();
        let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
        let request_log = Arc::clone(&requests);

        let target = with_user_question_prompt_handler(
            move |request| {
                let question = request.questions[0]["question"]
                    .as_str()
                    .expect("question text")
                    .to_string();
                request_log.lock().unwrap().push(request.questions.clone());
                let answer = if question.contains("connector") {
                    "telegram-login"
                } else {
                    "telegram-user"
                };
                UserQuestionPromptResponse {
                    answers: Map::from_iter([(question, json!(answer))]),
                    annotations: Map::new(),
                }
            },
            || parse_or_ask_target(&mut state, &resources, ""),
        )
        .expect("target");

        assert_eq!(target.connector_slug, "telegram-login");
        assert_eq!(target.connection_name, "telegram-user");
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0][0]["type"], "choice");
        assert_eq!(requests[0][0]["searchable"], true);
        assert!(requests[0][0]["options"]
            .as_array()
            .is_some_and(|options| options
                .iter()
                .any(|option| option["label"] == "telegram-login")));
        assert_eq!(requests[1][0]["type"], "input");
    }

    #[test]
    fn execute_connect_flow_dispatches_webhook_setup() {
        let mut state = temp_state();
        let resources = LoadedResources::default();

        let turn = with_user_question_prompt_handler(
            |request| answer_connect_question(&request),
            || execute_connect_flow(&mut state, &resources, "webhook local-hook"),
        )
        .expect("connect turn");

        assert!(turn.assistant_text.contains("connection: local-hook"));
        assert!(turn.assistant_text.contains("run `puffer serve`"));
        let raw = connector_config(&state);
        assert!(raw.contains("[connectors.webhook]"));
    }

    #[test]
    fn execute_connect_flow_dispatches_github_webhook_setup() {
        let mut state = temp_state();
        let resources = LoadedResources::default();

        let turn = with_user_question_prompt_handler(
            |request| answer_connect_question(&request),
            || execute_connect_flow(&mut state, &resources, "github-webhook github-events"),
        )
        .expect("connect turn");

        assert!(turn.assistant_text.contains("connector: github-webhook"));
        assert!(turn.assistant_text.contains("connection: github-events"));
        assert!(turn.assistant_text.contains("run `puffer serve`"));
        let raw = connector_config(&state);
        assert!(raw.contains("[connectors.webhook]"));
        assert!(raw.contains("display_name = \"github-events\""));
        assert!(raw.contains("path = \"/github\""));
    }

    #[test]
    fn execute_connect_flow_dispatches_linear_webhook_setup() {
        let mut state = temp_state();
        let resources = LoadedResources::default();

        let turn = with_user_question_prompt_handler(
            |request| answer_connect_question(&request),
            || execute_connect_flow(&mut state, &resources, "linear-webhook linear-events"),
        )
        .expect("connect turn");

        assert!(turn.assistant_text.contains("connector: linear-webhook"));
        assert!(turn.assistant_text.contains("connection: linear-events"));
        assert!(turn.assistant_text.contains("run `puffer serve`"));
        let raw = connector_config(&state);
        assert!(raw.contains("[connectors.webhook]"));
        assert!(raw.contains("display_name = \"linear-events\""));
        assert!(raw.contains("path = \"/linear\""));
    }

    #[test]
    fn execute_connect_flow_dispatches_stripe_webhook_setup() {
        let mut state = temp_state();
        let resources = LoadedResources::default();

        let turn = with_user_question_prompt_handler(
            |request| answer_connect_question(&request),
            || execute_connect_flow(&mut state, &resources, "stripe-webhook stripe-events"),
        )
        .expect("connect turn");

        assert!(turn.assistant_text.contains("connector: stripe-webhook"));
        assert!(turn.assistant_text.contains("connection: stripe-events"));
        assert!(turn.assistant_text.contains("run `puffer serve`"));
        let raw = connector_config(&state);
        assert!(raw.contains("[connectors.webhook]"));
        assert!(raw.contains("display_name = \"stripe-events\""));
        assert!(raw.contains("path = \"/stripe\""));
    }

    #[test]
    fn execute_connect_flow_dispatches_telegram_bot_setup() {
        let mut state = temp_state();
        let resources = LoadedResources::default();

        let turn = with_user_question_prompt_handler(
            |request| answer_connect_question(&request),
            || execute_connect_flow(&mut state, &resources, "telegram-bot telegram-bot"),
        )
        .expect("connect turn");

        assert!(turn.assistant_text.contains("connector: telegram-bot"));
        assert!(turn.assistant_text.contains("run `puffer serve`"));
        let raw = connector_config(&state);
        assert!(raw.contains("[connectors.telegram]"));
        assert!(raw.contains("token = \"telegram-token\""));
    }
}

use crate::runtime::claude_tools::execute_workflow_tool;
use crate::{AppState, TurnExecution};
use anyhow::{anyhow, bail, Context, Result};
use puffer_resources::LoadedResources;
use serde_json::{json, Value};

use super::common::render_svg_qr_data_uri;

mod catalog;
mod gcal_browser;
mod gmail_browser;
mod serve_config;

const TELEGRAM_QR_APPROVAL_ATTEMPTS: usize = 3;
const TELEGRAM_QR_APPROVAL_CHECK_SECONDS: u64 = 10;

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
        "lark-login" | "lark-bot" => connect_lark_cli(
            state,
            resources,
            &target.connector_slug,
            &target.connection_name,
        )?,
        "gmail-browser" => {
            gmail_browser::connect_gmail_browser(state, resources, &target.connection_name)?
        }
        "gcal-browser" => {
            gcal_browser::connect_gcal_browser(state, resources, &target.connection_name)?
        }
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
    if !extra.is_empty() {
        bail!("Usage: /connect <connector-slug> <connection-name>");
    }
    if connection_name.is_empty() {
        connection_name = puffer_subscriptions::suggested_connection_slug(&connector_slug);
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
    for _ in 0..TELEGRAM_QR_APPROVAL_ATTEMPTS {
        if status(&output) != Some("qr_pending") {
            return submit_telegram_password_if_needed(state, resources, connection, output);
        }
        let url = output
            .pointer("/payload/url")
            .and_then(Value::as_str)
            .unwrap_or("<missing QR URL>");
        let question = telegram_qr_approval_question(url);
        let answer = ask_choice(
            state,
            resources,
            "Approve",
            &question,
            &[
                ("Approved", "I approved the login request."),
                ("Cancel", "Stop this login attempt."),
            ],
        )?;
        if answer != "Approved" {
            bail!("Telegram QR login cancelled");
        }
        output = call_tool(
            state,
            resources,
            "Telegram",
            telegram_qr_wait_input(connection),
        )?;
    }
    bail!(
        "Telegram QR login was not approved after {TELEGRAM_QR_APPROVAL_ATTEMPTS} checks; restart setup to try again"
    )
}

fn telegram_qr_approval_question(url: &str) -> String {
    let qr_body = match render_svg_qr_data_uri(url) {
        Some(data_uri) => format!("![Telegram QR code]({data_uri})\n\n{url}"),
        None => url.to_string(),
    };
    format!("Approve this Telegram QR login URL from a logged-in Telegram app.\n\n{qr_body}")
}

fn telegram_qr_wait_input(connection: &str) -> Value {
    json!({
        "action": "login_qr_wait",
        "connection_slug": connection,
        "timeout_seconds": TELEGRAM_QR_APPROVAL_CHECK_SECONDS
    })
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
        output = submit_telegram_password_if_needed(state, resources, connection, output)?;
    }
    Ok(output)
}

fn submit_telegram_password_if_needed(
    state: &mut AppState,
    resources: &LoadedResources,
    connection: &str,
    mut output: Value,
) -> Result<Value> {
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

/// Connects a Lark account backed by the official `lark-cli`.
///
/// Auth is delegated to `lark-cli`: installs it on consent if missing, then runs
/// the device-flow login (showing the verification URL/QR) if not logged in, and
/// records a `ConnectionCreate`. Puffer never stores a Lark token.
fn connect_lark_cli(
    state: &mut AppState,
    resources: &LoadedResources,
    connector_slug: &str,
    connection: &str,
) -> Result<ConnectResult> {
    let bin = lark_cli_bin();
    // If the CLI is missing, offer to install it (with consent) before auth.
    if !lark_cli_available(&bin) {
        ensure_lark_cli_installed(state, resources, &bin)?;
    }
    // Both connectors verify their lark-cli identity before connecting. When it
    // is missing, the recovery differs: lark-login (user identity) drives the
    // device-flow OAuth login; lark-bot (app/bot identity) has no OAuth login in
    // lark-cli, so it drives `config init` to set the app id/secret instead.
    let identity = lark_connector_identity(connector_slug);
    if !lark_cli_identity_ready(&bin, identity) {
        match identity {
            "user" => ensure_lark_cli_logged_in(state, resources, &bin)?,
            _ => ensure_lark_cli_app_configured(state, resources, &bin)?,
        }
    }
    // Idempotent: re-running /connect with an existing connection name just
    // re-verifies auth (done above) instead of failing on a duplicate record.
    if lark_connection_exists(state, resources, connection)? {
        return Ok(summary(
            connector_slug,
            connection,
            "lark-cli auth status (existing connection)",
            &json!({ "status": "complete" }),
        ));
    }
    let output = call_tool(
        state,
        resources,
        "ConnectionCreate",
        json!({
            "slug": connection,
            "connector_slug": connector_slug,
            "description": "Lark connection backed by lark-cli, configured by /connect"
        }),
    )?;
    Ok(summary(
        connector_slug,
        connection,
        "lark-cli auth status",
        &output,
    ))
}

/// Returns the `lark-cli` binary name, honoring the `LARK_CLI_BIN` override.
fn lark_cli_bin() -> String {
    std::env::var("LARK_CLI_BIN")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "lark-cli".to_string())
}

/// Returns whether the `lark-cli` binary can be launched (i.e. is installed).
fn lark_cli_available(bin: &str) -> bool {
    std::process::Command::new(bin)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Runs `lark-cli <args>` capturing output, force-killing it after `timeout` so
/// a stalled device-flow login cannot hang the synchronous `/connect` turn.
fn lark_cli_output_timed(
    bin: &str,
    args: &[&str],
    timeout: std::time::Duration,
) -> Result<std::process::Output> {
    let mut child = std::process::Command::new(bin)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .with_context(|| format!("spawn `{bin} {}`", args.join(" ")))?;
    let start = std::time::Instant::now();
    loop {
        if child.try_wait().context("wait for lark-cli")?.is_some() {
            return child.wait_with_output().context("collect lark-cli output");
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            bail!("`{bin} {}` timed out", args.join(" "));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// Returns whether a connection named `slug` is already registered, so a re-run
/// of `/connect` can short-circuit instead of failing on a duplicate.
fn lark_connection_exists(
    state: &mut AppState,
    resources: &LoadedResources,
    slug: &str,
) -> Result<bool> {
    let listed = call_tool(state, resources, "ConnectionList", json!({}))?;
    Ok(listed
        .get("connections")
        .and_then(Value::as_array)
        .is_some_and(|connections| {
            connections
                .iter()
                .any(|connection| connection.get("slug").and_then(Value::as_str) == Some(slug))
        }))
}

/// Maps a Lark connector slug to the `lark-cli` identity it authenticates as.
fn lark_connector_identity(connector_slug: &str) -> &'static str {
    match connector_slug {
        "lark-login" => "user",
        _ => "bot",
    }
}

/// Returns whether `lark-cli auth status` reports `identity` ("user"/"bot") as
/// available, parsing `identities.<identity>.available` and trusting a
/// successful exit when that field is absent (older `lark-cli`).
fn lark_cli_identity_ready(bin: &str, identity: &str) -> bool {
    let output = match std::process::Command::new(bin)
        .args(["auth", "status"])
        .stdin(std::process::Stdio::null())
        .output()
    {
        Ok(output) => output,
        Err(_) => return false,
    };
    if !output.status.success() {
        return false;
    }
    match serde_json::from_slice::<Value>(&output.stdout) {
        Ok(status) => status
            .get("identities")
            .and_then(|identities| identities.get(identity))
            .and_then(|entry| entry.get("available"))
            .and_then(Value::as_bool)
            .unwrap_or(true),
        Err(_) => true,
    }
}

/// Generates a scannable PNG QR for `url` via `lark-cli auth qrcode --output`
/// (written to the temp dir, since the CLI only accepts a cwd-relative path) and
/// returns its absolute path, or `None` on failure. Callers point the user at
/// the saved file because terminal hosts cannot render an inline image.
fn lark_cli_png_qr(bin: &str, url: &str) -> Option<String> {
    let dir = std::env::temp_dir();
    let path = dir.join("lark-qr.png");
    // Drop any stale QR first so a no-op/failed run can't surface a prior URL's image.
    let _ = std::fs::remove_file(&path);
    let status = std::process::Command::new(bin)
        .args(["auth", "qrcode", url, "--output", "lark-qr.png"])
        .current_dir(&dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok()?;
    if !status.success() {
        return None;
    }
    path.exists().then(|| path.to_string_lossy().into_owned())
}

/// Opens `url` in the user's default browser (best-effort; failure is ignored
/// because the URL and a QR are also shown in the prompt).
fn open_url_in_browser(url: &str) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "explorer"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(opener)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// Builds the option preview shown beside an authorization prompt: the full URL
/// (the single-line prompt title cannot render a long URL untruncated) plus the
/// saved QR image path.
fn build_auth_preview(url: &str, qr_path: Option<&str>) -> String {
    let mut preview = format!("Authorization URL:\n{url}");
    if let Some(path) = qr_path {
        preview.push_str(&format!("\n\nOr scan the QR image:\n{path}"));
    }
    preview
}

/// Extracts the first whitespace-delimited `http(s)` URL from `line`, trimming
/// trailing punctuation. Used to scrape the verification URL `config init --new`
/// prints to stderr.
fn first_http_url(line: &str) -> Option<String> {
    let start = line.find("http")?;
    let url = line[start..]
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .trim_end_matches(['"', ')', ']', ',', '.'])
        .to_string();
    url.starts_with("http").then_some(url)
}

/// Drives the `lark-cli` device-flow login: show the verification URL/QR, wait
/// for the user to authorize in a browser, then complete with the device code.
fn ensure_lark_cli_logged_in(
    state: &mut AppState,
    resources: &LoadedResources,
    bin: &str,
) -> Result<()> {
    let started = lark_cli_output_timed(
        bin,
        &["auth", "login", "--no-wait", "--json", "--domain", "im"],
        std::time::Duration::from_secs(30),
    )?;
    if !started.status.success() {
        let stderr = String::from_utf8_lossy(&started.stderr);
        bail!("`{bin} auth login` could not start: {}", stderr.trim());
    }
    let payload: Value = serde_json::from_slice(&started.stdout)
        .context("parse `lark-cli auth login --no-wait --json` output")?;
    let url = payload
        .get("verification_url")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("lark-cli did not return a verification_url"))?;
    let device_code = payload
        .get("device_code")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("lark-cli did not return a device_code"))?;

    // Open the URL directly and put the full URL + QR path in the option preview:
    // the single-line prompt title cannot render newlines or a long URL cleanly.
    open_url_in_browser(url);
    let qr_path = lark_cli_png_qr(bin, url);
    let preview = build_auth_preview(url, qr_path.as_deref());
    let choice = ask_choice_with_preview(
        state,
        resources,
        "Authorize",
        "Opened the Lark authorization page in your browser — authorize there, then choose Done. (URL & QR in the preview.)",
        &[
            (
                "Done",
                "I authorized Lark in the browser.",
                Some(preview.as_str()),
            ),
            ("Cancel", "Stop the Lark login.", None),
        ],
    )?;
    if choice != "Done" {
        bail!("Lark login cancelled");
    }

    let completed = lark_cli_output_timed(
        bin,
        &["auth", "login", "--device-code", device_code],
        std::time::Duration::from_secs(180),
    )?;
    if !completed.status.success() {
        let stderr = String::from_utf8_lossy(&completed.stderr);
        bail!(
            "Lark login did not complete: {}. Finish the browser authorization, then retry /connect.",
            stderr.trim()
        );
    }
    if !lark_cli_identity_ready(bin, "user") {
        bail!(
            "Lark login finished but `{bin} auth status` still reports no user identity; retry /connect."
        );
    }
    Ok(())
}

/// Sets up the app credentials backing the bot identity: either creates a new
/// Lark app in the browser (`config init --new`) or configures an existing app
/// id + secret. lark-cli stores credentials in the OS keychain — Puffer never
/// persists them.
fn ensure_lark_cli_app_configured(
    state: &mut AppState,
    resources: &LoadedResources,
    bin: &str,
) -> Result<()> {
    let method = ask_choice(
        state,
        resources,
        "Bot app",
        "How should the bot's Lark app be set up?",
        &[
            (
                "Create new app",
                "Create a new Lark app in the browser (config init --new).",
            ),
            (
                "Use existing app",
                "Enter an existing app id and secret (no browser).",
            ),
        ],
    )?;
    if method == "Create new app" {
        lark_cli_config_init_new(state, resources, bin)?;
    } else {
        lark_cli_config_init_existing(state, resources, bin)?;
    }
    if !lark_cli_identity_ready(bin, "bot") {
        bail!(
            "Lark app configured but `{bin} auth status` still reports no bot identity; \
             retry `/connect lark-bot`."
        );
    }
    Ok(())
}

/// Configures an existing Lark app by prompting for its id, brand, and secret;
/// the secret is piped via stdin so it stays out of the process list.
fn lark_cli_config_init_existing(
    state: &mut AppState,
    resources: &LoadedResources,
    bin: &str,
) -> Result<()> {
    let app_id = ask_input(
        state,
        resources,
        "App ID",
        "What Lark app id (cli_...) should the bot use?",
    )?;
    let brand = ask_choice(
        state,
        resources,
        "Brand",
        "Which Lark brand is this app on?",
        &[
            ("feishu", "Feishu (China)."),
            ("lark", "Lark (international)."),
        ],
    )?;
    let app_secret = ask_input(
        state,
        resources,
        "App secret",
        "What is the Lark app secret? lark-cli stores it in your OS keychain; Puffer does not keep it.",
    )?;
    let output = lark_cli_config_init(bin, app_id.trim(), &brand, app_secret.trim())?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("`{bin} config init` failed: {}", stderr.trim());
    }
    Ok(())
}

/// Creates a new Lark app via `config init --new`, which prints a verification
/// URL to stderr and blocks until the user finishes app creation in the browser.
/// Opens the URL, shows it (and a QR) in the prompt, and waits for completion.
fn lark_cli_config_init_new(
    state: &mut AppState,
    resources: &LoadedResources,
    bin: &str,
) -> Result<()> {
    use std::io::{BufRead, BufReader};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    let mut child = std::process::Command::new(bin)
        .args(["config", "init", "--new"])
        .stdin(std::process::Stdio::null())
        // stdout is unused (URL + QR go to stderr); null it so a chatty stdout
        // can't fill the pipe and deadlock the command while it waits.
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .with_context(|| format!("spawn `{bin} config init --new`"))?;
    // The URL (and a QR) are printed to stderr before it blocks; read stderr on a
    // thread so the pipe never fills while the command waits for the browser.
    let stderr = child
        .stderr
        .take()
        .context("open config init --new stderr")?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut sent = false;
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if !sent {
                if let Some(url) = first_http_url(&line) {
                    let _ = tx.send(url);
                    sent = true;
                }
            }
        }
    });
    let url = match rx.recv_timeout(Duration::from_secs(30)) {
        Ok(url) => url,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            bail!("`{bin} config init --new` did not print a verification URL");
        }
    };

    open_url_in_browser(&url);
    let qr_path = lark_cli_png_qr(bin, &url);
    let preview = build_auth_preview(&url, qr_path.as_deref());
    let choice = ask_choice_with_preview(
        state,
        resources,
        "Create app",
        "Opened the Lark app-creation page in your browser — finish creating the app there, then choose Done. (URL & QR in the preview.)",
        &[
            (
                "Done",
                "I finished creating the app in the browser.",
                Some(preview.as_str()),
            ),
            ("Cancel", "Stop creating the Lark app.", None),
        ],
    )?;
    if choice != "Done" {
        let _ = child.kill();
        let _ = child.wait();
        bail!("Lark app creation cancelled");
    }

    // The command self-completes once the browser flow finishes; wait for it.
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().context("wait for config init --new")? {
            break status;
        }
        if start.elapsed() >= Duration::from_secs(180) {
            let _ = child.kill();
            let _ = child.wait();
            bail!(
                "`{bin} config init --new` did not finish; complete the browser steps, then retry `/connect lark-bot`"
            );
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    if !status.success() {
        bail!("`{bin} config init --new` failed; retry `/connect lark-bot`");
    }
    Ok(())
}

/// Runs `lark-cli config init`, feeding the app secret over stdin.
fn lark_cli_config_init(
    bin: &str,
    app_id: &str,
    brand: &str,
    app_secret: &str,
) -> Result<std::process::Output> {
    use std::io::Write as _;
    let mut child = std::process::Command::new(bin)
        .args([
            "config",
            "init",
            "--app-id",
            app_id,
            "--app-secret-stdin",
            "--brand",
            brand,
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .with_context(|| format!("spawn `{bin} config init`"))?;
    // Write the secret, then always reap the child — even if the write fails
    // (e.g. lark-cli rejected the app id and exited before reading stdin, so the
    // pipe broke), so we never leak a zombie. The real outcome is its exit status.
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(app_secret.as_bytes());
    }
    child
        .wait_with_output()
        .context("collect config init output")
}

/// Returns whether a helper program (e.g. `npm`, `go`) is on `PATH`.
fn program_available(name: &str) -> bool {
    std::process::Command::new(name)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// The official larksuite/cli one-liner installer args (needs Node.js for `npx`).
const LARK_CLI_INSTALL_ARGS: &[&str] = &["--yes", "@larksuite/cli@latest", "install"];

/// Manual install instructions shown when auto-install is declined or impossible.
fn lark_cli_install_instructions(bin: &str) -> String {
    format!(
        "Install the official larksuite/cli (needs Node.js for `npx`), then retry \
         `/connect lark-login` or `/connect lark-bot`:\n  npx @larksuite/cli@latest install\n\
         Then run `{bin} auth login` to sign in. Set LARK_CLI_BIN to point at a \
         non-PATH binary."
    )
}

/// Asks for consent then runs the official installer; bails (with guidance) if `npx` is missing or declined.
fn ensure_lark_cli_installed(
    state: &mut AppState,
    resources: &LoadedResources,
    bin: &str,
) -> Result<()> {
    let choice = ask_choice(
        state,
        resources,
        "Install",
        &format!("`{bin}` is not installed. Install the official larksuite/cli now?"),
        &[
            (
                "Install now",
                "Run `npx @larksuite/cli@latest install` (requires Node.js).",
            ),
            (
                "Show instructions",
                "Do not install; print the manual install command.",
            ),
        ],
    );
    // Headless (no interactive prompt): show guidance instead of an opaque error.
    let choice = match choice {
        Ok(choice) => choice,
        Err(_) => bail!(
            "`{bin}` is not installed.\n{}",
            lark_cli_install_instructions(bin)
        ),
    };
    if choice != "Install now" {
        bail!("{}", lark_cli_install_instructions(bin));
    }

    if !program_available("npx") {
        bail!(
            "`npx` (Node.js) is not available. Install Node.js first, then retry \
             `/connect lark-login` or `/connect lark-bot`.\n{}",
            lark_cli_install_instructions(bin)
        );
    }
    let output = std::process::Command::new("npx")
        .args(LARK_CLI_INSTALL_ARGS)
        .output()
        .with_context(|| format!("run `npx {}`", LARK_CLI_INSTALL_ARGS.join(" ")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "`npx {}` failed to install lark-cli: {}\n{}",
            LARK_CLI_INSTALL_ARGS.join(" "),
            stderr.trim(),
            lark_cli_install_instructions(bin)
        );
    }
    if !lark_cli_available(bin) {
        bail!(
            "Installer ran, but `{bin}` is still not runnable. It may not be on \
             PATH — set LARK_CLI_BIN to its path, then retry /connect.\n{}",
            lark_cli_install_instructions(bin)
        );
    }
    Ok(())
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

    #[test]
    fn first_http_url_extracts_clean_url() {
        let line = "  https://open.feishu.cn/page/cli?user_code=2FVM-NAV3&from=cli";
        assert_eq!(
            first_http_url(line).as_deref(),
            Some("https://open.feishu.cn/page/cli?user_code=2FVM-NAV3&from=cli")
        );
        assert_eq!(first_http_url("no url here"), None);
        // Trailing punctuation from a surrounding log line is trimmed.
        assert_eq!(
            first_http_url("see (https://x.test/a).").as_deref(),
            Some("https://x.test/a")
        );
    }

    #[test]
    fn build_auth_preview_includes_url_and_optional_qr() {
        let with_qr = build_auth_preview("https://x.test/auth", Some("/tmp/lark-qr.png"));
        assert!(with_qr.contains("https://x.test/auth"));
        assert!(with_qr.contains("/tmp/lark-qr.png"));
        let without_qr = build_auth_preview("https://x.test/auth", None);
        assert!(without_qr.contains("https://x.test/auth"));
        assert!(!without_qr.contains("QR image"));
    }

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
    fn telegram_qr_approval_question_embeds_qr_markdown_in_question_body() {
        let question = telegram_qr_approval_question("tg://login?token=abc");

        assert!(question
            .starts_with("Approve this Telegram QR login URL from a logged-in Telegram app."));
        assert!(question.contains("![Telegram QR code](data:image/svg+xml;base64,"));
        assert!(question.contains("tg://login?token=abc"));
    }

    #[test]
    fn telegram_qr_wait_input_uses_short_retry_timeout() {
        let input = telegram_qr_wait_input("telegram-user");

        assert_eq!(input["action"], "login_qr_wait");
        assert_eq!(input["connection_slug"], "telegram-user");
        assert_eq!(input["timeout_seconds"], TELEGRAM_QR_APPROVAL_CHECK_SECONDS);
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
    fn parse_target_uses_default_connection_name_for_connector_only() {
        let mut state = temp_state();
        let resources = LoadedResources::default();

        let target = parse_or_ask_target(&mut state, &resources, "email").expect("target");

        assert_eq!(target.connector_slug, "email");
        assert_eq!(target.connection_name, "email");
    }

    #[test]
    fn parse_target_asks_for_connector_and_uses_default_connection_name() {
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
                UserQuestionPromptResponse {
                    answers: Map::from_iter([(question, json!("telegram-login"))]),
                    annotations: Map::new(),
                }
            },
            || parse_or_ask_target(&mut state, &resources, ""),
        )
        .expect("target");

        assert_eq!(target.connector_slug, "telegram-login");
        assert_eq!(target.connection_name, "telegram-user");
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0][0]["type"], "choice");
        assert_eq!(requests[0][0]["searchable"], true);
        assert!(requests[0][0]["options"]
            .as_array()
            .is_some_and(|options| options
                .iter()
                .any(|option| option["label"] == "telegram-login")));
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

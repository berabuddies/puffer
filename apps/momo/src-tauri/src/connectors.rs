//! Tauri RPCs that drive `puffer connect …` from the Connected Apps UI.
//!
//! Telegram and Email setup require two different shapes of process
//! lifetime:
//!
//! - Telegram personal-account login holds an opaque grammers
//!   `LoginToken` in memory between `login_start` and `submit_code`.
//!   The CLI subprocess therefore stays alive across multiple Tauri RPC
//!   calls; we keep a stdin/stdout pair per session indexed by a UUID
//!   we hand back to the frontend.
//! - Email config is a one-shot `puffer connect email configure …` that
//!   writes the TOML config file and exits.
//!
//! No internal-permission broker, no /connect REPL — the underlying
//! `puffer connect` subcommand calls the subscriber crates' login /
//! config APIs directly.

use crate::backend::{
    ensure_provider_command, home_dir, optional_trimmed_string_param, string_param,
};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use uuid::Uuid;

const DEFAULT_TELEGRAM_CONNECTION: &str = "telegram-user";

/// Dispatches connector-setup RPCs from `BackendState::handle`. Returns
/// `None` when `method` is not a connector method so the caller can
/// fall through to the rest of the dispatch table.
pub(crate) fn handle(method: &str, params: &Value) -> Option<Result<Value>> {
    Some(match method {
        "telegram_start" => telegram_start(params),
        "telegram_submit_code" => telegram_submit_code(params),
        "telegram_submit_password" => telegram_submit_password(params),
        "telegram_cancel" => telegram_cancel(params),
        "email_configure" => email_configure(params),
        "connector_status" => Ok(connector_status(params)),
        "connector_disconnect" => connector_disconnect(params),
        _ => return None,
    })
}

struct ChildSession {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    connection_slug: String,
}

fn sessions() -> &'static Mutex<HashMap<String, ChildSession>> {
    static SESSIONS: OnceLock<Mutex<HashMap<String, ChildSession>>> = OnceLock::new();
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Returns `{ telegram: bool, email: bool }` reflecting whether each
/// connector has authenticated credentials on disk. Telegram is
/// considered connected once `credentials.json` exists (which is only
/// written after a successful sign-in, not after the pre-auth code
/// request). Email is connected once `config.toml` exists.
///
/// As a self-heal: if credentials are present but the matching record
/// is missing from `~/.puffer/connections.json`, this also registers
/// the record so puffer's agent tools can find the connection. This
/// covers users who authenticated under an earlier build that wrote
/// credentials without registering the record.
fn connector_status(params: &Value) -> Value {
    let connection_slug = optional_trimmed_string_param(params, &["connectionSlug", "connection_slug"])
        .unwrap_or_else(|| DEFAULT_TELEGRAM_CONNECTION.to_string());
    let telegram_creds = telegram_state_dir(&connection_slug).join("credentials.json");
    let email_config = email_state_dir().join("config.toml");
    let telegram_connected = telegram_creds.exists();
    let email_connected = email_config.exists();
    if telegram_connected {
        let _ = ensure_connection_record(
            &connection_slug,
            "telegram-login",
            "Telegram personal account",
        );
    }
    if email_connected {
        let _ = ensure_connection_record("email", "email", "Email (IMAP/SMTP)");
    }
    json!({
        "telegram": telegram_connected,
        "email": email_connected,
    })
}

/// Spawns a `puffer connect telegram` child, kicks off `login_start`,
/// stashes the live process under a fresh sessionId, and returns the
/// first event (typically `login_awaiting_code`).
fn telegram_start(params: &Value) -> Result<Value> {
    let phone = string_param(params, &["phone"])?;
    let connection_slug = optional_trimmed_string_param(params, &["connectionSlug", "connection_slug"])
        .unwrap_or_else(|| DEFAULT_TELEGRAM_CONNECTION.to_string());

    let command = ensure_provider_command("puffer")?;
    let mut child = Command::new(&command)
        .arg("connect")
        .arg("telegram")
        .arg("--connection")
        .arg(&connection_slug)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to spawn `{command} connect telegram`"))?;
    let mut stdin = child.stdin.take().context("missing child stdin")?;
    let stdout = child.stdout.take().context("missing child stdout")?;
    let mut reader = BufReader::new(stdout);

    let cmd = json!({ "action": "login_start", "phone": phone });
    writeln!(stdin, "{cmd}").context("write login_start to child stdin")?;

    let event = read_next_event(&mut reader)?;
    let session_id = Uuid::new_v4().to_string();
    let kind = event_kind(&event);
    let is_terminal = is_terminal_event(kind);

    if is_terminal {
        let _ = child.kill();
        return Ok(build_response(&session_id, &connection_slug, event));
    }

    sessions().lock().unwrap().insert(
        session_id.clone(),
        ChildSession {
            child,
            stdin,
            stdout: reader,
            connection_slug: connection_slug.clone(),
        },
    );
    Ok(build_response(&session_id, &connection_slug, event))
}

/// Writes a `submit_code` command into the matching child's stdin and
/// returns the next emitted event.
fn telegram_submit_code(params: &Value) -> Result<Value> {
    let session_id = string_param(params, &["sessionId", "session_id"])?;
    let code = string_param(params, &["code"])?;
    submit_to_child(&session_id, json!({ "action": "submit_code", "code": code }))
}

/// Writes a `submit_password` command into the matching child's stdin
/// and returns the next emitted event.
fn telegram_submit_password(params: &Value) -> Result<Value> {
    let session_id = string_param(params, &["sessionId", "session_id"])?;
    let password = string_param(params, &["password"])?;
    submit_to_child(
        &session_id,
        json!({ "action": "submit_password", "password": password }),
    )
}

/// Best-effort: kills and removes the child for `sessionId`.
fn telegram_cancel(params: &Value) -> Result<Value> {
    let session_id = string_param(params, &["sessionId", "session_id"])?;
    if let Some(mut entry) = sessions().lock().unwrap().remove(&session_id) {
        let _ = entry.child.kill();
    }
    Ok(json!({ "sessionId": session_id }))
}

fn connector_disconnect(params: &Value) -> Result<Value> {
    let kind = string_param(params, &["kind"])?;
    match kind.as_str() {
        "telegram" => {
            let connection_slug =
                optional_trimmed_string_param(params, &["connectionSlug", "connection_slug"])
                    .unwrap_or_else(|| DEFAULT_TELEGRAM_CONNECTION.to_string());
            let dir = telegram_state_dir(&connection_slug);
            if dir.exists() {
                std::fs::remove_dir_all(&dir)
                    .with_context(|| format!("remove telegram state {}", dir.display()))?;
            }
            let _ = remove_connection_record(&connection_slug);
            Ok(json!({ "kind": "telegram", "connectionSlug": connection_slug }))
        }
        "email" => {
            let config = email_state_dir().join("config.toml");
            if config.exists() {
                std::fs::remove_file(&config)
                    .with_context(|| format!("remove email config {}", config.display()))?;
            }
            let _ = remove_connection_record("email");
            Ok(json!({ "kind": "email" }))
        }
        other => bail!("unknown connector kind `{other}`"),
    }
}

fn submit_to_child(session_id: &str, command: Value) -> Result<Value> {
    let mut sessions_lock = sessions().lock().unwrap();
    let entry = sessions_lock
        .get_mut(session_id)
        .ok_or_else(|| anyhow!("unknown session `{session_id}`"))?;
    writeln!(entry.stdin, "{command}").context("write command to child stdin")?;
    let event = read_next_event(&mut entry.stdout)?;
    let kind = event_kind(&event);
    let connection_slug = entry.connection_slug.clone();
    if is_terminal_event(kind) {
        let _ = writeln!(entry.stdin, "{}", json!({ "action": "exit" }));
        let mut removed = sessions_lock.remove(session_id).unwrap();
        let _ = removed.child.kill();
    }
    Ok(build_response(session_id, &connection_slug, event))
}

fn read_next_event(reader: &mut BufReader<ChildStdout>) -> Result<Value> {
    let mut line = String::new();
    loop {
        line.clear();
        let bytes = reader
            .read_line(&mut line)
            .context("read event line from `puffer connect` child")?;
        if bytes == 0 {
            bail!("`puffer connect` child closed before emitting an event");
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        return serde_json::from_str::<Value>(trimmed)
            .with_context(|| format!("parse `puffer connect` event JSON: {trimmed}"));
    }
}

fn event_kind(event: &Value) -> &str {
    event.get("kind").and_then(Value::as_str).unwrap_or("")
}

fn is_terminal_event(kind: &str) -> bool {
    matches!(kind, "login_complete" | "login_error" | "connect_error")
}

fn build_response(session_id: &str, connection_slug: &str, event: Value) -> Value {
    let kind = event_kind(&event).to_string();
    json!({
        "sessionId": session_id,
        "connectionSlug": connection_slug,
        "status": map_status_from_kind(&kind),
        "event": event,
    })
}

fn map_status_from_kind(kind: &str) -> &'static str {
    match kind {
        "login_awaiting_code" => "awaiting_code",
        "login_awaiting_password" => "awaiting_password",
        "login_complete" => "complete",
        "login_error" => "failed",
        "connect_error" => "failed",
        _ => "unknown",
    }
}

/// One-shot `puffer connect email configure …` invocation. Passes the
/// password via `--password-stdin` so it never appears in argv listings.
fn email_configure(params: &Value) -> Result<Value> {
    let imap_host = string_param(params, &["imapHost", "imap_host"])?;
    let smtp_host = string_param(params, &["smtpHost", "smtp_host"])?;
    let username = string_param(params, &["username"])?;
    let password = string_param(params, &["password"])?;
    let from_address = string_param(params, &["fromAddress", "from_address"])?;
    let imap_port = optional_port(params, &["imapPort", "imap_port"])?;
    let smtp_port = optional_port(params, &["smtpPort", "smtp_port"])?;
    let imap_port_str = imap_port.to_string();
    let smtp_port_str = smtp_port.to_string();

    let command = ensure_provider_command("puffer")?;
    let mut child = Command::new(&command)
        .arg("connect")
        .arg("email")
        .arg("configure")
        .args([
            "--imap-host",
            imap_host.as_str(),
            "--imap-port",
            imap_port_str.as_str(),
            "--smtp-host",
            smtp_host.as_str(),
            "--smtp-port",
            smtp_port_str.as_str(),
            "--username",
            username.as_str(),
            "--from-address",
            from_address.as_str(),
        ])
        .arg("--password-stdin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to spawn `{command} connect email configure`"))?;
    {
        let mut stdin = child.stdin.take().context("missing child stdin")?;
        writeln!(stdin, "{password}").context("write email password to child stdin")?;
    }
    let output = child
        .wait_with_output()
        .context("wait for `puffer connect email configure`")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let message = if !stderr.is_empty() {
            stderr
        } else if !stdout.is_empty() {
            stdout
        } else {
            format!(
                "`{command} connect email configure` exited with {:?}",
                output.status.code()
            )
        };
        bail!(message);
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(json!({ "status": "configured", "output": stdout }))
}

fn optional_port(params: &Value, names: &[&str]) -> Result<u16> {
    for name in names {
        if let Some(value) = params.get(*name) {
            if value.is_null() {
                continue;
            }
            let parsed = value
                .as_u64()
                .ok_or_else(|| anyhow!("`{name}` must be a non-negative integer"))?;
            if parsed > u16::MAX as u64 {
                bail!("`{name}` must fit in a u16");
            }
            return Ok(parsed as u16);
        }
    }
    Ok(0)
}

fn telegram_state_dir(connection_slug: &str) -> PathBuf {
    if connection_slug == DEFAULT_TELEGRAM_CONNECTION {
        home_dir()
            .join(".puffer")
            .join("subscribers")
            .join(DEFAULT_TELEGRAM_CONNECTION)
            .join("state")
    } else {
        home_dir()
            .join(".puffer")
            .join("telegram-accounts")
            .join(connection_slug)
    }
}

fn email_state_dir() -> PathBuf {
    home_dir()
        .join(".puffer")
        .join("subscribers")
        .join("email")
        .join("state")
}

fn connections_path() -> PathBuf {
    home_dir().join(".puffer").join("connections.json")
}

/// Adds a `{slug, connector_slug, state: "authenticated", …}` record to
/// `~/.puffer/connections.json` if one is missing. Idempotent.
fn ensure_connection_record(
    slug: &str,
    connector_slug: &str,
    description: &str,
) -> Result<()> {
    let path = connections_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create {}", parent.display()))?;
    }
    let mut doc = read_connections_doc(&path)?;
    let array = ensure_connections_array(&mut doc);
    let already = array.iter().any(|entry| {
        entry.get("slug").and_then(Value::as_str) == Some(slug)
            && entry.get("connector_slug").and_then(Value::as_str) == Some(connector_slug)
    });
    if already {
        return Ok(());
    }
    array.push(json!({
        "slug": slug,
        "connector_slug": connector_slug,
        "state": "authenticated",
        "description": description,
    }));
    write_connections_doc(&path, &doc)
}

fn remove_connection_record(slug: &str) -> Result<()> {
    let path = connections_path();
    if !path.exists() {
        return Ok(());
    }
    let mut doc = read_connections_doc(&path)?;
    let array = ensure_connections_array(&mut doc);
    let before = array.len();
    array.retain(|entry| entry.get("slug").and_then(Value::as_str) != Some(slug));
    if array.len() == before {
        return Ok(());
    }
    write_connections_doc(&path, &doc)
}

fn read_connections_doc(path: &std::path::Path) -> Result<Value> {
    if !path.exists() {
        return Ok(json!({ "connections": [] }));
    }
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("read {}", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(json!({ "connections": [] }));
    }
    serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))
}

fn ensure_connections_array(doc: &mut Value) -> &mut Vec<Value> {
    if !doc.is_object() {
        *doc = json!({ "connections": [] });
    }
    let obj = doc.as_object_mut().unwrap();
    obj.entry("connections")
        .or_insert_with(|| Value::Array(Vec::new()));
    obj.get_mut("connections")
        .and_then(Value::as_array_mut)
        .expect("connections entry just set to an array")
}

fn write_connections_doc(path: &std::path::Path, doc: &Value) -> Result<()> {
    let body = serde_json::to_string_pretty(doc).context("serialize connections doc")?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| {
        format!("rename {} -> {}", tmp.display(), path.display())
    })
}

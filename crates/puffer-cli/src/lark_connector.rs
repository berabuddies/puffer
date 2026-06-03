//! `puffer __connector lark-user|lark-bot` protocol bridge over the official
//! `lark-cli`: `auth-ok`/`act`/`subscribe`. The id picks the default `--as`
//! identity (user/bot) for `act`/`auth-ok`. `subscribe` always consumes as the
//! bot (`lark-cli event consume` is bot-only). `LARK_CLI_BIN` overrides the
//! binary; `LARK_CLI_AS` overrides the subscribe identity; `act` uses the
//! per-call `as` field or the connector default.

use anyhow::{anyhow, bail, Context, Result};
use puffer_subscriptions::{ConnectorSubscribeCommand, ConnectorSubscribeFrame};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

/// Lark open-platform event key for incoming IM messages.
const EVENT_KEY: &str = "im.message.receive_v1";
/// Timeout for the one-shot `auth-ok` probe.
const AUTH_TIMEOUT: Duration = Duration::from_secs(20);
/// Timeout for a single `act` command.
const ACT_TIMEOUT: Duration = Duration::from_secs(60);
/// How long to wait for the `lark-cli event consume` ready marker before streaming anyway.
const READY_TIMEOUT: Duration = Duration::from_secs(30);
/// Stderr marker `lark-cli event consume` prints once the websocket is live.
const READY_MARKER: &str = "[event] ready";
/// How long to wait for `lark-cli event consume` to exit gracefully (after
/// closing its stdin) before force-killing it on shutdown.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// Runs one `puffer __connector lark-user|lark-bot <op> ...` invocation.
/// `identity` ("user"/"bot") is the default `--as` for act/subscribe.
pub(crate) async fn run(identity: &str, args: &[String]) -> Result<()> {
    let (op, rest) = args
        .split_first()
        .ok_or_else(|| anyhow!("missing connector op; expected auth-ok|act|subscribe"))?;
    match op.as_str() {
        "auth-ok" => run_auth_ok(identity).await,
        "act" => {
            let action = rest
                .get(1)
                .map(String::as_str)
                .ok_or_else(|| anyhow!("`act` requires an action slug"))?;
            run_act(identity, action).await
        }
        "subscribe" => run_subscribe().await,
        other => bail!("unknown connector op `{other}`; expected auth-ok|act|subscribe"),
    }
}

/// Returns the `lark-cli` binary name, honoring the `LARK_CLI_BIN` override.
fn lark_cli_bin() -> String {
    env_or("LARK_CLI_BIN", "lark-cli")
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

/// Prints `{"ok":bool}` reflecting whether `lark-cli auth status` reports the
/// connector's identity (`user`/`bot`) as available.
async fn run_auth_ok(identity: &str) -> Result<()> {
    let ok = match tokio::time::timeout(
        AUTH_TIMEOUT,
        Command::new(lark_cli_bin())
            .args(["auth", "status"])
            .stdin(Stdio::null())
            .output(),
    )
    .await
    {
        Ok(Ok(output)) if output.status.success() => identity_available(&output.stdout, identity),
        Ok(Ok(_)) => false,
        // A missing binary or timeout means "not authenticated", not a bridge
        // error — report false, but log so a broken install is diagnosable.
        Ok(Err(error)) => {
            eprintln!(
                "lark auth-ok: could not run `{} auth status`: {error}",
                lark_cli_bin()
            );
            false
        }
        Err(_) => {
            eprintln!("lark auth-ok: `{} auth status` timed out", lark_cli_bin());
            false
        }
    };
    println!("{}", json!({ "ok": ok }));
    Ok(())
}

/// Parses `auth status` JSON for `identities.<identity>.available`; trusts a
/// successful exit when the payload predates that field.
fn identity_available(stdout: &[u8], identity: &str) -> bool {
    match serde_json::from_slice::<Value>(stdout) {
        Ok(status) => status
            .get("identities")
            .and_then(|identities| identities.get(identity))
            .and_then(|entry| entry.get("available"))
            .and_then(Value::as_bool)
            .unwrap_or(true),
        Err(_) => true,
    }
}

/// Reads one JSON object from stdin, runs the action, and prints the response.
async fn run_act(identity: &str, action: &str) -> Result<()> {
    let mut raw = String::new();
    tokio::io::stdin().read_to_string(&mut raw).await?;
    let input: Value = if raw.trim().is_empty() {
        Value::Null
    } else {
        serde_json::from_str(raw.trim()).context("parse act input JSON")?
    };

    let outcome = match action {
        "send_message" => send_message(&input, identity).await,
        "react" | "send_reaction" => react(&input, identity).await,
        "remove_reaction" => remove_reaction(&input, identity).await,
        other => Err(anyhow!(
            "lark-cli connector does not support action `{other}`"
        )),
    };
    let response = match outcome {
        Ok(outcome) => json!({
            "success": true,
            "summary": outcome.summary,
            "output": outcome.output,
        }),
        Err(error) => {
            let summary = format!("{error:#}");
            // Timeouts are transient — let the host retry them.
            let retryable = summary.contains("timed out");
            json!({ "success": false, "summary": summary, "retryable": retryable })
        }
    };
    // Always exit 0 with a structured body so the host surfaces the summary.
    println!("{response}");
    Ok(())
}

/// Result of a successful `act` command.
struct ActOutcome {
    summary: String,
    output: Value,
}

/// Runs `lark-cli` with `args`, returning the parsed JSON output on success.
async fn exec_lark(args: &[String], cwd: Option<&Path>) -> Result<Value> {
    let mut command = Command::new(lark_cli_bin());
    command.args(args).stdin(Stdio::null());
    // Media uploads run from the file's parent dir, since lark-cli's
    // `--file/--image/--audio` only accept a path relative to the cwd.
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let output = tokio::time::timeout(ACT_TIMEOUT, command.output())
        .await
        .map_err(|_| anyhow!("lark-cli timed out after {ACT_TIMEOUT:?}"))?
        .with_context(|| format!("run `{} {}`", lark_cli_bin(), args.join(" ")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("lark-cli failed: {}", stderr.trim());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stdout = stdout.trim();
    let body =
        serde_json::from_str::<Value>(stdout).unwrap_or_else(|_| json!({ "stdout": stdout }));
    // Lark OpenAPI often returns exit 0 with a logical error in the body.
    let logical_error = body.get("ok") == Some(&Value::Bool(false))
        || body
            .get("code")
            .and_then(Value::as_i64)
            .is_some_and(|code| code != 0);
    if logical_error {
        let msg = body
            .get("error")
            .and_then(|error| error.get("message").or(Some(error)))
            .or_else(|| body.get("msg"))
            .and_then(Value::as_str)
            .unwrap_or("lark-cli reported an error");
        bail!("lark-cli error: {msg}");
    }
    Ok(body)
}

/// Sends a message, or replies via `+messages-reply` when `reply_to` is set.
async fn send_message(input: &Value, identity: &str) -> Result<ActOutcome> {
    let identity = send_identity(input, identity);
    let (args, cwd, summary) = if let Some(message_id) = reply_message_id(input) {
        let (args, cwd) = build_reply_args(input, &identity, &message_id)?;
        (
            args,
            cwd,
            format!("Replied to Lark message {message_id} as {identity} via lark-cli"),
        )
    } else {
        let (_, recipient) = resolve_recipient(input)?;
        let (args, cwd) = build_send_args(input, &identity)?;
        (
            args,
            cwd,
            format!("Sent Lark message to {recipient} as {identity} via lark-cli"),
        )
    };
    let output = exec_lark(&args, cwd.as_deref()).await?;
    Ok(ActOutcome { summary, output })
}

/// Adds a reaction to a Lark message via `lark-cli im reactions create`.
async fn react(input: &Value, identity: &str) -> Result<ActOutcome> {
    let identity = send_identity(input, identity);
    let args = build_react_args(input, &identity)?;
    let output = exec_lark(&args, None).await?;
    let message_id = first_str(input, &["message_id", "id"]).unwrap_or_default();
    Ok(ActOutcome {
        summary: format!("Reacted to Lark message {message_id} as {identity} via lark-cli"),
        output,
    })
}

/// Removes a reaction from a Lark message via `lark-cli im reactions delete`.
async fn remove_reaction(input: &Value, identity: &str) -> Result<ActOutcome> {
    let identity = send_identity(input, identity);
    let args = build_remove_reaction_args(input, &identity)?;
    let output = exec_lark(&args, None).await?;
    let reaction_id = first_str(input, &["reaction_id"]).unwrap_or_default();
    Ok(ActOutcome {
        summary: format!("Removed Lark reaction {reaction_id} as {identity} via lark-cli"),
        output,
    })
}

/// Acting identity: `as`/`identity` field, else the connector's `default`.
fn send_identity(input: &Value, default: &str) -> String {
    first_str(input, &["as", "identity"]).unwrap_or_else(|| default.to_string())
}

/// Returns the message id to reply to, from `reply_to_message_id` or `reply_to`
/// (a string id or an object with `message_id`). `None` means a normal send.
fn reply_message_id(input: &Value) -> Option<String> {
    if let Some(id) = first_str(input, &["reply_to_message_id"]) {
        return Some(id);
    }
    match input.get("reply_to") {
        Some(Value::String(value)) if !value.trim().is_empty() => Some(value.trim().to_string()),
        Some(Value::Object(map)) => map
            .get("message_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string),
        _ => None,
    }
}

/// Appends `--text` and/or one media flag (`image`/`audio`/`file`); requires at least one.
fn push_body_args(input: &Value, args: &mut Vec<String>) -> Result<Option<PathBuf>> {
    let text = first_str(input, &["text", "message", "caption"]);
    let media: Option<(&str, String)> = first_str(input, &["image"])
        .map(|value| ("--image", value))
        .or_else(|| first_str(input, &["audio"]).map(|value| ("--audio", value)))
        .or_else(|| first_str(input, &["file", "media"]).map(|value| ("--file", value)));
    if text.is_none() && media.is_none() {
        bail!("requires a `text` (or `message`) or a media field (image/file/audio)");
    }
    if let Some(text) = text {
        args.push("--text".to_string());
        args.push(text);
    }
    let mut media_cwd = None;
    if let Some((flag, value)) = media {
        let (value, cwd) = resolve_media_value(&value);
        args.push(flag.to_string());
        args.push(value);
        media_cwd = cwd;
    }
    Ok(media_cwd)
}

/// Resolves a media value for lark-cli's `--file/--image/--audio` flags, which
/// only accept a path relative to the cwd. A readable local file becomes its
/// file name plus the parent dir to run lark-cli from; anything else (a file
/// key or URL) is passed through unchanged.
fn resolve_media_value(value: &str) -> (String, Option<PathBuf>) {
    let path = Path::new(value);
    if path.is_file() {
        if let Ok(absolute) = path.canonicalize() {
            if let (Some(parent), Some(name)) = (absolute.parent(), absolute.file_name()) {
                return (
                    name.to_string_lossy().into_owned(),
                    Some(parent.to_path_buf()),
                );
            }
        }
    }
    (value.to_string(), None)
}

/// Builds the `lark-cli im +messages-send` argv (recipient + `--as` + body).
fn build_send_args(input: &Value, identity: &str) -> Result<(Vec<String>, Option<PathBuf>)> {
    let (flag, id) = resolve_recipient(input)?;
    let mut args = vec![
        "im".to_string(),
        "+messages-send".to_string(),
        "--as".to_string(),
        identity.to_string(),
        flag.to_string(),
        id,
    ];
    let cwd = push_body_args(input, &mut args)?;
    Ok((args, cwd))
}

/// Builds the `lark-cli im +messages-reply` argv for replying to `message_id`.
fn build_reply_args(
    input: &Value,
    identity: &str,
    message_id: &str,
) -> Result<(Vec<String>, Option<PathBuf>)> {
    let mut args = vec![
        "im".to_string(),
        "+messages-reply".to_string(),
        "--as".to_string(),
        identity.to_string(),
        "--message-id".to_string(),
        message_id.to_string(),
    ];
    let cwd = push_body_args(input, &mut args)?;
    if input.get("reply_in_thread").and_then(Value::as_bool) == Some(true) {
        args.push("--reply-in-thread".to_string());
    }
    Ok((args, cwd))
}

/// Builds the `lark-cli im reactions create` argv (add a reaction).
fn build_react_args(input: &Value, identity: &str) -> Result<Vec<String>> {
    let message_id = first_str(input, &["message_id", "id"])
        .ok_or_else(|| anyhow!("react requires a `message_id`"))?;
    let emoji = first_str(input, &["emoji_type", "emoji", "reaction"])
        .ok_or_else(|| anyhow!("react requires an `emoji_type` (Lark EmojiType, e.g. SMILE)"))?;
    let params = json!({ "message_id": message_id }).to_string();
    let data = json!({ "reaction_type": { "emoji_type": emoji } }).to_string();
    Ok(vec![
        "im".to_string(),
        "reactions".to_string(),
        "create".to_string(),
        "--as".to_string(),
        identity.to_string(),
        "--params".to_string(),
        params,
        "--data".to_string(),
        data,
    ])
}

/// Builds the `lark-cli im reactions delete` argv (remove a reaction by id).
fn build_remove_reaction_args(input: &Value, identity: &str) -> Result<Vec<String>> {
    let message_id = first_str(input, &["message_id", "id"])
        .ok_or_else(|| anyhow!("remove_reaction requires a `message_id`"))?;
    let reaction_id = first_str(input, &["reaction_id"])
        .ok_or_else(|| anyhow!("remove_reaction requires a `reaction_id`"))?;
    let params = json!({ "message_id": message_id, "reaction_id": reaction_id }).to_string();
    Ok(vec![
        "im".to_string(),
        "reactions".to_string(),
        "delete".to_string(),
        "--as".to_string(),
        identity.to_string(),
        "--params".to_string(),
        params,
    ])
}

/// Resolves the recipient flag and id for `lark-cli im +messages-send`.
fn resolve_recipient(input: &Value) -> Result<(&'static str, String)> {
    if let Some(chat) = first_str(input, &["chat_id", "chat", "channel"]) {
        return Ok(("--chat-id", chat));
    }
    if let Some(user) = first_str(input, &["open_id", "user_id", "user"]) {
        return Ok(("--user-id", user));
    }
    if let Some(target) = first_str(input, &["receive_id", "to", "target"]) {
        // Honor an explicit receive_id_type; otherwise infer from the `oc_` prefix.
        let flag = match first_str(input, &["receive_id_type"]).as_deref() {
            Some("chat_id") => "--chat-id",
            Some("open_id" | "user_id" | "union_id" | "email") => "--user-id",
            _ if target.starts_with("oc_") => "--chat-id",
            _ => "--user-id",
        };
        return Ok((flag, target));
    }
    bail!("send_message requires a recipient (chat_id, open_id/user_id, or to/target)")
}

/// Returns the first present, non-empty string value among `keys`.
fn first_str(input: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        input
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
    })
}

// ---------------------------------------------------------------------------
// subscribe
// ---------------------------------------------------------------------------

/// Streams incoming Lark messages as puffer frames via `lark-cli event consume`.
///
/// Holds the child's stdin open for the stream's lifetime — `lark-cli` treats
/// stdin EOF as shutdown, so the host's stdin must not be forwarded to it.
async fn run_subscribe() -> Result<()> {
    // `lark-cli event consume` only supports `--as bot` — `im.message.receive_v1`
    // is an app-level stream — so subscribe always consumes as the bot regardless
    // of the connector identity. `LARK_CLI_AS` can still override it.
    let consume_as = env_or("LARK_CLI_AS", "bot");
    // 1. Consume the host's `{"op":"subscribe",…}` line.
    let mut host_stdin = BufReader::new(tokio::io::stdin());
    let mut first_line = String::new();
    host_stdin.read_line(&mut first_line).await?;
    if !first_line.trim().is_empty() {
        // The cursor is ignored anyway, so a malformed line degrades (logged)
        // rather than taking down the whole stream.
        if let Err(error) = serde_json::from_str::<ConnectorSubscribeCommand>(first_line.trim()) {
            eprintln!("lark subscribe: ignoring unparsable subscribe command: {error}");
        }
    }
    // Lark's websocket has no resumable cursor; any host cursor is ignored.

    // 2. Spawn `lark-cli event consume` with an owned stdin we never close.
    let mut child = Command::new(lark_cli_bin())
        .args(["event", "consume", EVENT_KEY, "--as", &consume_as])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("spawn `{} event consume {EVENT_KEY}`", lark_cli_bin()))?;
    let child_stdin = child.stdin.take().context("event consume stdin missing")?;
    let child_stdout = child
        .stdout
        .take()
        .context("event consume stdout missing")?;
    let child_stderr = child
        .stderr
        .take()
        .context("event consume stderr missing")?;

    // Single writer task owns our stdout so frames never interleave.
    let (frame_tx, mut frame_rx) =
        tokio::sync::mpsc::unbounded_channel::<ConnectorSubscribeFrame>();
    let writer = tokio::spawn(async move {
        let mut out = tokio::io::stdout();
        while let Some(frame) = frame_rx.recv().await {
            if let Ok(mut line) = serde_json::to_string(&frame) {
                line.push('\n');
                if out.write_all(line.as_bytes()).await.is_err() {
                    break;
                }
                let _ = out.flush().await;
            }
        }
    });

    // 3. Drain stderr (forwarded to the host as logs) and signal readiness.
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<()>();
    let stderr_task = tokio::spawn(async move {
        let mut ready_tx = Some(ready_tx);
        let mut lines = BufReader::new(child_stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if line.contains(READY_MARKER) {
                if let Some(tx) = ready_tx.take() {
                    let _ = tx.send(());
                }
            }
            eprintln!("{line}");
        }
    });
    let _ = tokio::time::timeout(READY_TIMEOUT, ready_rx).await;

    // If event consume already exited, it failed to start (unsupported identity,
    // auth/scope error, …) — report that instead of a misleading "ok".
    if let Ok(Some(status)) = child.try_wait() {
        let _ = frame_tx.send(ConnectorSubscribeFrame::Health {
            status: "error".to_string(),
            detail: Some(format!(
                "lark-cli event consume {EVENT_KEY} exited before streaming ({status})"
            )),
        });
        drop(frame_tx);
        let _ = writer.await;
        stderr_task.abort();
        bail!("lark-cli event consume {EVENT_KEY} failed to start ({status})");
    }
    let _ = frame_tx.send(ConnectorSubscribeFrame::Health {
        status: "ok".to_string(),
        detail: Some(format!("lark-cli event consume {EVENT_KEY} streaming")),
    });

    // Drain host stdin (acks are no-ops); EOF signals graceful shutdown.
    let stdin_task = tokio::spawn(async move {
        let mut line = String::new();
        loop {
            line.clear();
            match host_stdin.read_line(&mut line).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
    });

    // 4. Pump events until the child exits or the host closes our stdin.
    let mut stdout_lines = BufReader::new(child_stdout).lines();
    let mut stdin_task = stdin_task;
    loop {
        tokio::select! {
            line = stdout_lines.next_line() => {
                match line {
                    Ok(Some(line)) => {
                        if let Some(frame) = event_line_to_frame(&line) {
                            let _ = frame_tx.send(frame);
                        }
                    }
                    _ => break,
                }
            }
            _ = &mut stdin_task => break,
        }
    }

    let _ = frame_tx.send(ConnectorSubscribeFrame::Health {
        status: "error".to_string(),
        detail: Some("lark-cli event stream ended".to_string()),
    });
    drop(frame_tx);
    // Graceful stop: closing the child's stdin makes `lark-cli event consume`
    // unsubscribe server-side and exit. A SIGKILL would skip that and leak the
    // subscription, so only force-kill if it does not exit within the grace window.
    drop(child_stdin);
    if tokio::time::timeout(SHUTDOWN_GRACE, child.wait())
        .await
        .is_err()
    {
        let _ = child.start_kill();
    }
    let _ = writer.await;
    stderr_task.abort();
    stdin_task.abort();
    Ok(())
}

/// Converts one `im.message.receive_v1` NDJSON line into a puffer event frame (None if unparsable / no id).
fn event_line_to_frame(line: &str) -> Option<ConnectorSubscribeFrame> {
    let value: Value = serde_json::from_str(line.trim()).ok()?;
    let event = value.get("event").unwrap_or(&value);
    let message = event.get("message").unwrap_or(event);

    let message_id = message
        .get("message_id")
        .and_then(Value::as_str)
        .or_else(|| value.get("message_id").and_then(Value::as_str))?;

    let text = extract_text(message);
    let chat_id = message.get("chat_id").and_then(Value::as_str);
    let sender = extract_sender(event, message);
    let create_time = message.get("create_time").and_then(Value::as_str);
    let message_type = message
        .get("message_type")
        .and_then(Value::as_str)
        .or_else(|| message.get("msg_type").and_then(Value::as_str));

    let payload = json!({
        "kind": "message",
        // `text` and `message` both set so the host's payload_text() finds the body.
        "text": text,
        "message": text,
        "message_id": message_id,
        "chat_id": chat_id,
        "sender_open_id": sender,
        "create_time": create_time,
        "message_type": message_type,
    });

    Some(ConnectorSubscribeFrame::Event {
        id: message_id.to_string(),
        cursor: message_id.to_string(),
        payload,
    })
}

/// Extracts the sender open id (flat `sender_id`, or nested `sender.sender_id.open_id`).
fn extract_sender(event: &Value, message: &Value) -> Option<String> {
    if let Some(open_id) = event.get("sender_id").and_then(Value::as_str) {
        return Some(open_id.to_string());
    }
    event
        .get("sender")
        .and_then(|sender| sender.get("sender_id"))
        .and_then(|sender_id| sender_id.get("open_id"))
        .and_then(Value::as_str)
        .or_else(|| {
            event
                .get("sender")
                .and_then(|sender| sender.get("open_id"))
                .and_then(Value::as_str)
        })
        .or_else(|| message.get("sender_id").and_then(Value::as_str))
        .map(ToString::to_string)
}

/// Extracts text from a Lark message (`content` plain or JSON `{"text":..}`, else `text`).
fn extract_text(message: &Value) -> String {
    if let Some(content) = message.get("content").and_then(Value::as_str) {
        // Lark text content is a JSON string `{"text":"…"}`; pull `.text` out,
        // falling back to the raw content for shapes without it (e.g. cards).
        if let Ok(parsed) = serde_json::from_str::<Value>(content) {
            if let Some(text) = parsed.get("text").and_then(Value::as_str) {
                return text.to_string();
            }
        }
        return content.to_string();
    }
    message
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_available_reads_per_identity_status() {
        let status = br#"{"identities":{"bot":{"available":true},"user":{"available":false}}}"#;
        assert!(identity_available(status, "bot"));
        assert!(!identity_available(status, "user"));
        // Payload without the identities map (older lark-cli) trusts the exit code.
        assert!(identity_available(b"{\"appId\":\"x\"}", "user"));
    }

    #[test]
    fn resolve_media_value_passes_through_keys_and_urls() {
        let (value, cwd) = resolve_media_value("img_v2_abc");
        assert_eq!(value, "img_v2_abc");
        assert!(cwd.is_none());
        let (value, cwd) = resolve_media_value("https://example.test/x.png");
        assert_eq!(value, "https://example.test/x.png");
        assert!(cwd.is_none());
    }

    #[test]
    fn resolve_media_value_splits_local_file_into_name_and_cwd() {
        let file = std::env::temp_dir().join("puffer-lark-media-test.txt");
        std::fs::write(&file, b"x").unwrap();
        let (name, cwd) = resolve_media_value(file.to_str().unwrap());
        assert_eq!(name, "puffer-lark-media-test.txt");
        // lark-cli runs from `cwd`, so `cwd/name` must resolve to the file.
        assert!(cwd.expect("local file yields a cwd").join(&name).is_file());
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn send_args_prefer_explicit_chat_id() {
        let args = build_send_args(&json!({"chat_id": "oc_1", "text": "hi"}), "bot")
            .unwrap()
            .0;
        assert_eq!(
            args,
            [
                "im",
                "+messages-send",
                "--as",
                "bot",
                "--chat-id",
                "oc_1",
                "--text",
                "hi"
            ]
        );
    }

    #[test]
    fn send_args_route_open_id_to_user_flag() {
        let args = build_send_args(&json!({"open_id": "ou_9", "message": "yo"}), "bot")
            .unwrap()
            .0;
        assert_eq!(
            args,
            [
                "im",
                "+messages-send",
                "--as",
                "bot",
                "--user-id",
                "ou_9",
                "--text",
                "yo"
            ]
        );
    }

    #[test]
    fn send_args_infer_kind_from_generic_target() {
        let chat = build_send_args(&json!({"to": "oc_x", "text": "a"}), "bot")
            .unwrap()
            .0;
        assert_eq!(chat[4], "--chat-id");
        let user = build_send_args(&json!({"target": "ou_x", "text": "a"}), "bot")
            .unwrap()
            .0;
        assert_eq!(user[4], "--user-id");
    }

    #[test]
    fn send_args_carry_requested_identity() {
        let args = build_send_args(&json!({"chat_id": "oc_1", "text": "hi"}), "user")
            .unwrap()
            .0;
        assert_eq!(args[2], "--as");
        assert_eq!(args[3], "user");
    }

    #[test]
    fn send_identity_uses_connector_default_and_honors_override() {
        assert_eq!(send_identity(&json!({"text": "hi"}), "bot"), "bot");
        assert_eq!(send_identity(&json!({"text": "hi"}), "user"), "user");
        assert_eq!(send_identity(&json!({"as": "user"}), "bot"), "user");
        assert_eq!(send_identity(&json!({"identity": "bot"}), "user"), "bot");
    }

    #[test]
    fn send_args_require_text_or_media_and_recipient() {
        assert!(build_send_args(&json!({"chat_id": "oc_1"}), "bot").is_err());
        assert!(build_send_args(&json!({"text": "hi"}), "bot").is_err());
    }

    #[test]
    fn send_args_support_media_without_text() {
        let args = build_send_args(&json!({"chat_id": "oc_1", "image": "img.png"}), "bot")
            .unwrap()
            .0;
        assert_eq!(
            args,
            [
                "im",
                "+messages-send",
                "--as",
                "bot",
                "--chat-id",
                "oc_1",
                "--image",
                "img.png"
            ]
        );
    }

    #[test]
    fn reply_message_id_reads_string_object_and_explicit() {
        assert_eq!(
            reply_message_id(&json!({"reply_to": "om_a"})).as_deref(),
            Some("om_a")
        );
        assert_eq!(
            reply_message_id(&json!({"reply_to": {"message_id": "om_b"}})).as_deref(),
            Some("om_b")
        );
        assert_eq!(
            reply_message_id(&json!({"reply_to_message_id": "om_c"})).as_deref(),
            Some("om_c")
        );
        assert_eq!(reply_message_id(&json!({"text": "hi"})), None);
    }

    #[test]
    fn reply_args_target_message_and_thread_flag() {
        let args = build_reply_args(
            &json!({"text": "yo", "reply_in_thread": true}),
            "user",
            "om_x",
        )
        .unwrap()
        .0;
        assert_eq!(
            args,
            [
                "im",
                "+messages-reply",
                "--as",
                "user",
                "--message-id",
                "om_x",
                "--text",
                "yo",
                "--reply-in-thread"
            ]
        );
    }

    #[test]
    fn react_args_carry_message_and_emoji() {
        let args =
            build_react_args(&json!({"message_id": "om_1", "emoji_type": "SMILE"}), "bot").unwrap();
        assert_eq!(args[..5], ["im", "reactions", "create", "--as", "bot"]);
        assert_eq!(args[5], "--params");
        assert!(args[6].contains("om_1"));
        assert_eq!(args[7], "--data");
        assert!(args[8].contains("SMILE") && args[8].contains("emoji_type"));
    }

    #[test]
    fn react_args_require_message_and_emoji() {
        assert!(build_react_args(&json!({"message_id": "om_1"}), "bot").is_err());
        assert!(build_react_args(&json!({"emoji_type": "SMILE"}), "bot").is_err());
    }

    #[test]
    fn remove_reaction_args_carry_ids() {
        let args =
            build_remove_reaction_args(&json!({"message_id": "om_1", "reaction_id": "r9"}), "bot")
                .unwrap();
        assert_eq!(args[..5], ["im", "reactions", "delete", "--as", "bot"]);
        assert_eq!(args[5], "--params");
        assert!(args[6].contains("om_1") && args[6].contains("r9"));
    }

    #[test]
    fn event_line_extracts_text_and_ids() {
        let line = json!({
            "event": {
                "message": {
                    "message_id": "om_1",
                    "chat_id": "oc_2",
                    "message_type": "text",
                    "create_time": "1700000000000",
                    "content": "{\"text\":\"hello\"}"
                },
                "sender": {"sender_id": {"open_id": "ou_3"}}
            }
        })
        .to_string();
        let frame = event_line_to_frame(&line).unwrap();
        match frame {
            ConnectorSubscribeFrame::Event {
                id,
                cursor,
                payload,
            } => {
                assert_eq!(id, "om_1");
                assert_eq!(cursor, "om_1");
                assert_eq!(payload["text"], "hello");
                assert_eq!(payload["message"], "hello");
                assert_eq!(payload["chat_id"], "oc_2");
                assert_eq!(payload["sender_open_id"], "ou_3");
            }
            other => panic!("expected event frame, got {other:?}"),
        }
    }

    #[test]
    fn event_line_parses_flat_lark_cli_schema() {
        // The real `lark-cli event consume` agent-friendly format: flat fields,
        // top-level `sender_id` string, plain-text `content`.
        let line = json!({
            "type": "im.message.receive_v1",
            "message_id": "om_real",
            "chat_id": "oc_p2p",
            "chat_type": "p2p",
            "message_type": "text",
            "create_time": "1780384954938",
            "sender_id": "ou_alice",
            "content": "hello there"
        })
        .to_string();
        let frame = event_line_to_frame(&line).unwrap();
        match frame {
            ConnectorSubscribeFrame::Event { id, payload, .. } => {
                assert_eq!(id, "om_real");
                assert_eq!(payload["text"], "hello there");
                assert_eq!(payload["chat_id"], "oc_p2p");
                assert_eq!(payload["sender_open_id"], "ou_alice");
                assert_eq!(payload["message_type"], "text");
            }
            other => panic!("expected event frame, got {other:?}"),
        }
    }

    #[test]
    fn event_line_without_message_id_is_skipped() {
        assert!(event_line_to_frame("not json").is_none());
        assert!(event_line_to_frame(&json!({"event": {"message": {}}}).to_string()).is_none());
    }

    #[test]
    fn extract_text_falls_back_to_raw_content() {
        let message = json!({"content": "{\"title\":\"t\"}", "message_type": "post"});
        assert_eq!(extract_text(&message), "{\"title\":\"t\"}");
    }
}

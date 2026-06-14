//! `puffer __connector wechat-user` protocol bridge for a WeChat personal
//! account running inside a managed Docker desktop (KasmVNC + native WeChat).
//!
//! Unlike API-backed connectors, WeChat has no message API: this bridge drives
//! the *running* client through `docker`/`container exec` (Docker or Apple
//! `container`) — `xdotool`/`xclip` for input, the AT-SPI accessibility tree for
//! locating UI, and the decrypted chat DB for reads (screen-grab vision is an
//! opt-in fallback) — with human-like timing to stay unobtrusive. Protocol ops:
//!
//! * `auth-ok`   — is the container up and WeChat logged in? prints `{"ok":bool}`
//! * `act`       — read one JSON action from stdin, perform it, print a response
//! * `subscribe` — stream inbound messages as NDJSON [`ConnectorSubscribeFrame`]s
//! * `db-probe` / `bringup` — diagnostics (validate the DB reader; ensure the
//!   container is up); see [`run`].
//!
//! The target instance is selected by the CONNECTION SLUG passed to each op
//! (e.g. `wechat-user`), which maps to a container named `puffer-wechat-<slug>`
//! (see [`WechatInstance::for_connection`]). `DOCKER_BIN` / `WECHAT_CONTAINER_BIN`
//! override the runtime binary.

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use tokio::io::AsyncReadExt;

mod act;
mod atspi;
mod config;
mod dbread;
mod docker;
mod human;
mod policy;
mod read;
mod subscribe;

use human::Human;
pub(crate) use config::InstanceConfig;
pub(crate) use docker::{teardown_runtimes, WechatInstance};

/// Runs one `puffer __connector wechat-user <op> ...` invocation. `identity`
/// is currently always `"user"` (personal WeChat has no bot identity); it is
/// accepted for parity with the other bridges and future use.
pub(crate) async fn run(identity: &str, args: &[String]) -> Result<()> {
    let _ = identity;
    let (op, rest) = args
        .split_first()
        .ok_or_else(|| anyhow!("missing connector op; expected auth-ok|act|subscribe"))?;
    match op.as_str() {
        "auth-ok" => {
            // `auth-ok <connection>` — the connection slug selects the instance.
            let instance = WechatInstance::for_connection(rest.first().map_or("", String::as_str));
            run_auth_ok(&instance).await
        }
        "act" => {
            // `act <connection> <action>` — slug selects the instance/container.
            let instance = WechatInstance::for_connection(rest.first().map_or("", String::as_str));
            let action = rest.get(1).map(String::as_str).unwrap_or("send_message");
            run_act(&instance, action).await
        }
        // The subscribe connection arrives in the stdin subscribe command.
        "subscribe" => subscribe::run().await,
        // Diagnostic: validate the direct chat-DB reader (needs login + SYS_PTRACE).
        "db-probe" => {
            let instance = WechatInstance::for_connection(rest.first().map_or("", String::as_str));
            run_db_probe(&instance).await
        }
        // Ensure the container is up with the current env (recreates it with the
        // SYS_PTRACE cap when WECHAT_ENABLE_DB_READ=1). Used by the enable script.
        "bringup" => {
            let instance = WechatInstance::for_connection(rest.first().map_or("", String::as_str));
            run_bringup(&instance).await
        }
        other => {
            bail!("unknown connector op `{other}`; expected auth-ok|act|subscribe|db-probe|bringup")
        }
    }
}

/// Prints `{"ok":bool}` reflecting whether the instance container is running and
/// WeChat appears logged in. A missing container or docker error means "not
/// authenticated" (false), not a bridge failure.
async fn run_auth_ok(instance: &WechatInstance) -> Result<()> {
    let ok = match instance.is_logged_in().await {
        Ok(value) => value,
        Err(error) => {
            eprintln!("wechat auth-ok: {error:#}");
            false
        }
    };
    println!("{}", json!({ "ok": ok }));
    Ok(())
}

/// Diagnostic for the optional direct chat-DB reader: extracts the key from
/// WeChat memory, decrypts `session.db`, and prints a summary. Run logged-in
/// (with `WECHAT_ENABLE_DB_READ=1`) to validate the DB path on this WeChat
/// version: `puffer __connector wechat-user db-probe <connection>`.
async fn run_db_probe(instance: &WechatInstance) -> Result<()> {
    match dbread::read_session(instance, 0.0).await {
        Ok(res) => {
            let sample: Vec<Value> = res
                .messages
                .iter()
                .take(5)
                .map(|m| json!({ "chat": m.chat, "sender": m.sender, "text": m.text }))
                .collect();
            println!(
                "{}",
                json!({ "ok": true, "chats": res.messages.len(), "cursor": res.cursor, "sample": sample })
            );
        }
        Err(error) => println!("{}", json!({ "ok": false, "error": format!("{error:#}") })),
    }
    Ok(())
}

/// `read_history`: returns the last `limit` messages of a chat from the
/// decrypted message DBs (read-only; requires the DB reader). `chat` may be a
/// contact/group display name (resolved via contact.db) or a wxid/`*@chatroom`.
async fn read_history_action(instance: &WechatInstance, input: &Value) -> Result<act::ActOutcome> {
    if !dbread::enabled() {
        bail!(
            "read_history needs the direct DB reader, which is disabled \
             (WECHAT_ENABLE_DB_READ=0); re-enable it and recreate the container"
        );
    }
    let chat = ["chat", "to", "contact", "target", "name"]
        .iter()
        .find_map(|k| input.get(*k).and_then(Value::as_str))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("read_history requires `chat` (a contact/group name or wxid)"))?;
    let limit = input.get("limit").and_then(Value::as_u64).unwrap_or(50) as u32;
    let messages = dbread::read_history(instance, chat, limit).await?;
    Ok(act::ActOutcome {
        summary: format!("Read {} messages from {chat}", messages.len()),
        output: json!({ "chat": chat, "messages": messages }),
    })
}

/// Ensures the instance container is up with the current environment (so it
/// gets the SYS_PTRACE cap when `WECHAT_ENABLE_DB_READ=1`), then prints a status
/// JSON. Lets a script recreate the container without the GUI/daemon.
async fn run_bringup(instance: &WechatInstance) -> Result<()> {
    let cfg = match InstanceConfig::load_or_create(instance.name()) {
        Ok(cfg) => cfg,
        Err(error) => {
            println!("{}", json!({ "ok": false, "error": format!("{error:#}") }));
            return Ok(());
        }
    };
    match instance.ensure_container(&cfg).await {
        Ok(()) => {
            let logged_in = instance.is_logged_in().await.unwrap_or(false);
            let url = format!("http://{}/", instance.desktop_authority(&cfg).await);
            println!(
                "{}",
                json!({
                    "ok": true,
                    "container": cfg.container_name(),
                    "url": url,
                    "logged_in": logged_in,
                    "db_read_enabled": dbread::enabled(),
                })
            );
        }
        Err(error) => println!("{}", json!({ "ok": false, "error": format!("{error:#}") })),
    }
    Ok(())
}

/// Reads one JSON object from stdin, runs the action, and prints the response.
/// Always exits 0 with a structured body so the host surfaces the summary.
async fn run_act(instance: &WechatInstance, action: &str) -> Result<()> {
    let mut raw = String::new();
    tokio::io::stdin().read_to_string(&mut raw).await?;
    let input: Value = if raw.trim().is_empty() {
        Value::Null
    } else {
        serde_json::from_str(raw.trim()).context("parse act input JSON")?
    };

    // `action` field in the body overrides the positional arg when present.
    let action = input
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or(action);

    // Hard safety interlock (code, not docs): money + mass-broadcast are blocked
    // outright and never retried — a human must handle those.
    if let Some(reason) = forbidden_reason(action, &input) {
        println!("{}", json!({ "success": false, "summary": reason, "retryable": false }));
        return Ok(());
    }

    let human = Human::from_env();
    let cfg = match InstanceConfig::load_or_create(instance.name()) {
        Ok(cfg) => cfg,
        Err(error) => {
            println!(
                "{}",
                json!({ "success": false, "summary": format!("{error:#}"), "retryable": false })
            );
            return Ok(());
        }
    };
    // Serialize UI-driving actions per instance: they manipulate the container's
    // single X clipboard / keyboard focus, so two concurrent acts would race
    // (wrong content or wrong recipient). Held for the whole action. Read-only
    // actions (read_history) and the bridge's other ops do not need it. The
    // vision monitor reads only the framebuffer, so it is unaffected.
    let _ui_guard = match action {
        "send_message" | "send" | "reply" | "send_file" | "send_image" | "send_media"
        | "send_photo" | "send_document" | "mention" | "send_mention" | "react"
        | "send_reaction" | "pat" | "nudge" | "mark_read" | "open_chat" | "read_chat"
        | "logout" | "sign_out" => Some(policy::UiLock::acquire(instance.name())),
        _ => None,
    };

    let outcome = match action {
        "send_message" | "send" | "reply" | "send_file" | "send_image" | "send_media"
        | "send_photo" | "send_document" => {
            act::send_message(instance, &cfg, &input, &human).await
        }
        "mention" | "send_mention" => act::mention(instance, &cfg, &input, &human).await,
        "react" | "send_reaction" | "pat" | "nudge" => {
            act::react(instance, &cfg, &input, &human).await
        }
        "mark_read" | "open_chat" | "read_chat" => {
            act::mark_read(instance, &cfg, &input, &human).await
        }
        "logout" | "sign_out" => act::logout(instance, &cfg, &input, &human).await,
        "read_history" => read_history_action(instance, &input).await,
        other => Err(anyhow!("wechat connector does not support action `{other}`")),
    };
    // Per-step vision token cost (0 for DB-backed ops like read_history).
    let (tp, tc, tt, tn) = read::take_token_usage();
    let tokens = json!({ "prompt": tp, "completion": tc, "total": tt, "vision_calls": tn });
    let response = match outcome {
        Ok(outcome) => json!({
            "success": true,
            "summary": outcome.summary,
            "output": outcome.output,
            "tokens": tokens,
        }),
        Err(error) => {
            let summary = format!("{error:#}");
            // Transient docker/exec hiccups are worth a retry; logical errors are not.
            // A PARTIAL send (some messages already delivered) must NEVER be retried —
            // a retry would re-blast the messages that already went out. It is marked
            // by an explicit sentinel so other Display strings don't trip this.
            let partial = summary.contains("__WECHAT_PARTIAL_SEND__");
            let retryable = !partial
                && (summary.contains("timed out") || summary.contains("Cannot connect"));
            json!({ "success": false, "summary": summary, "retryable": retryable, "tokens": tokens })
        }
    };
    println!("{response}");
    Ok(())
}

/// Hard safety interlock: returns a block reason for operations that must never
/// be automated on a personal account — money movement and mass/broadcast
/// messaging — checked against both the action name and the input payload.
fn forbidden_reason(action: &str, input: &Value) -> Option<String> {
    let a = action.to_ascii_lowercase();
    // The ACTION NAME may use the broad list (a `transfer`/`pay` action is always
    // money), since action names are code-ish, not prose.
    const MONEY_ACTION: &[&str] = &[
        "transfer", "pay", "payment", "redpacket", "red_packet", "hongbao", "money", "wallet",
        "收款", "转账", "红包", "付款",
    ];
    if MONEY_ACTION.iter().any(|k| a.contains(k)) {
        return Some(
            "money operations (transfer / red packet / payment) are blocked for safety; \
             a human must perform these"
                .to_string(),
        );
    }
    // Money-solicitation CONTENT is also blocked. Only HIGH-SIGNAL terms (bare
    // English pay/transfer/money over-match benign text). Body is normalized
    // (lowercase, separators/whitespace/zero-width stripped) so spaced-out or
    // `red-packet`-style evasions collapse to a keyword — so keywords carry no
    // separators. The zh-CN terms below are matched as-is.
    const MONEY_CONTENT: &[&str] = &["redpacket", "hongbao", "收款", "转账", "红包", "付款"];
    let body_normalized: String = ["text", "message", "caption"]
        .iter()
        .filter_map(|k| input.get(*k).and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        // Drop whitespace (incl. full-width U+3000), zero-width/format chars, AND
        // ASCII punctuation so spacing / `-` / `_` / `.` / U+200B evasions of the
        // high-signal terms are caught.
        .filter(|c| {
            !c.is_whitespace()
                && *c != '\u{3000}'
                && !c.is_ascii_punctuation()
                && !matches!(*c, '\u{200B}'..='\u{200D}' | '\u{2060}' | '\u{FEFF}')
        })
        .flat_map(char::to_lowercase)
        .collect();
    if let Some(word) = MONEY_CONTENT.iter().find(|w| body_normalized.contains(*w)) {
        return Some(format!(
            "message contains money-solicitation content (`{word}`); blocked for safety"
        ));
    }

    let mass = ["broadcast", "mass", "群发", "qunfa", "bulk"];
    if mass.iter().any(|k| a.contains(k)) {
        return Some(
            "mass/broadcast messaging is blocked to protect the account from spam bans".to_string(),
        );
    }
    // M14: fan-out via a multi-recipient array under ANY recipient key (unified
    // with resolve_recipient's key set) is broadcast.
    for key in [
        "recipients", "to", "targets", "contacts", "target", "contact", "chat", "chat_id", "user",
        "receive_id", "name",
    ] {
        if let Some(Value::Array(items)) = input.get(key) {
            if items.len() > 1 {
                return Some(format!(
                    "sending to {} recipients at once is a broadcast and is blocked; send \
                     individually",
                    items.len()
                ));
            }
        }
    }
    None
}


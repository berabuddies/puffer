//! Locates WeChat UI elements through the AT-SPI accessibility tree.
//!
//! Drives `a11y_locate.py` inside the container (pushed once) to find elements by
//! role/name and return their pixel click-center, states, and bounds. This is the
//! primary way the connector drives the client; when the accessibility tree is
//! not reachable (e.g. a WeChat build that does not expose AT-SPI) the caller
//! reads the screen with the vision model instead. Gated by [`available`].
//!
//! Requires an AT-SPI-capable WeChat build with the accessibility environment set
//! up by the image (see `wechat-vz/`). Verified on 4.1.1.4 AND on the current
//! latest 4.1.1.7 — both expose the full tree; the image defaults to 4.1.1.7.

use anyhow::{Context, Result};
use serde_json::Value;

use super::docker::WechatInstance;

/// The locator program, embedded so the connector is self-contained.
const LOCATOR_PY: &str = include_str!("../../wechat-vz/a11y_locate.py");
const LOCATOR_PATH: &str = "/tmp/wechat-a11y-locate.py";

/// Where the image's startup publishes the session D-Bus address that the AT-SPI
/// bus is registered on (so a fresh `exec` can reach the same a11y bus). The
/// startup writes the primary path, or the fallback if `/run` is not writable;
/// the reader mirrors both.
const DBUS_ADDR_FILE: &str = "/run/wechat/dbus-addr";
const DBUS_ADDR_FALLBACK: &str = "/config/.cache/wechat-dbus-addr";

/// Whether to drive via the accessibility tree: not force-disabled with
/// `WECHAT_NO_VISION=0`, and the WeChat application is present in the live tree
/// (so locating will actually work on this image).
pub(crate) async fn available(instance: &WechatInstance) -> bool {
    if matches!(std::env::var("WECHAT_NO_VISION").as_deref(), Ok("0")) {
        return false;
    }
    if ensure_locator(instance).await.is_err() {
        return false;
    }
    run(instance, &["find", "--role", "frame", "--app", "wechat"])
        .await
        .ok()
        .and_then(|v| v.get("found").and_then(Value::as_bool))
        .unwrap_or(false)
}

/// Click-center of the recipient's row in the LEFT conversation list. `--max-x`
/// keeps the match in the left panel, away from the search-dropdown rows (a
/// search "web result" row, when clicked, opens a full-window web view). The name
/// match is substring because conversation rows append a last-message preview.
pub(crate) async fn conversation_row(
    instance: &WechatInstance,
    recipient: &str,
) -> Result<Option<(i32, i32)>> {
    let v = run(
        instance,
        &["center", "--role", "list-item", "--name", recipient, "--contains", "--max-x", "280"],
    )
    .await?;
    point(&v)
}

/// Verifies the requested chat is the one currently OPEN, via the LEFT
/// conversation list: the open chat's row carries the `SELECTED` state (verified
/// live) and its name (chat title + last-message preview) contains the recipient.
/// Matching the left-list ROW — not a right-pane label — is the safety here: the
/// right pane exposes the Send button, sender names, and message text as labels
/// too, any of which can contain the recipient string and would falsely confirm a
/// different chat (a wrong-recipient send). Substring (`--contains`) stays
/// consistent with how `conversation_row` opens the chat (the title may be a full
/// display name while `recipient` is the searched substring).
pub(crate) async fn open_chat_is(instance: &WechatInstance, recipient: &str) -> Result<bool> {
    let v = run(
        instance,
        &[
            "find", "--role", "list-item", "--name", recipient, "--contains",
            "--max-x", "280", "--state", "SELECTED",
        ],
    )
    .await?;
    Ok(found(&v))
}

/// Confirms delivery: the just-sent `body` appears as a message bubble in the
/// chat history (right pane; bubbles begin around x ~= 270, matching `--min-x`).
pub(crate) async fn body_in_history(instance: &WechatInstance, body: &str) -> Result<bool> {
    // Match a leading slice of the body (timestamps/suffixes may differ). Restrict
    // to a message bubble (list-item) in the right pane, so the input-box echo or
    // other labels can't be mistaken for a delivered message.
    let needle: String = body.chars().take(16).collect();
    let v = run(
        instance,
        &["find", "--role", "list-item", "--name", needle.trim(), "--contains", "--min-x", "270"],
    )
    .await?;
    Ok(found(&v))
}

/// Click-center of a right-click context-menu item by its label. WeChat exposes
/// context menus as `menu-item` nodes with real bounds.
pub(crate) async fn menu_item(instance: &WechatInstance, label: &str) -> Result<Option<(i32, i32)>> {
    let v = run(instance, &["center", "--role", "menu-item", "--name", label]).await?;
    point(&v)
}

/// Bounds (x, y, w, h) of the chat-history message bubble whose text contains
/// `snippet` (right pane). The row is full-width; the caller right-clicks the
/// actual bubble (left for incoming, right for outgoing). `None` if not found.
pub(crate) async fn message_bounds(
    instance: &WechatInstance,
    snippet: &str,
) -> Result<Option<(i32, i32, i32, i32)>> {
    let v = run(
        instance,
        &["find", "--role", "list-item", "--name", snippet, "--contains", "--min-x", "270"],
    )
    .await?;
    if !found(&v) {
        return Ok(None);
    }
    let Some(b) = v.get("bounds") else { return Ok(None) };
    let g = |k: &str| b.get(k).and_then(Value::as_i64).map(|n| n as i32);
    match (g("x"), g("y"), g("w"), g("h")) {
        (Some(x), Some(y), Some(w), Some(h)) => Ok(Some((x, y, w, h))),
        _ => Ok(None),
    }
}

fn found(v: &Value) -> bool {
    v.get("found").and_then(Value::as_bool) == Some(true)
}

/// Extracts the `{x, y}` click-center from a locator result, if present.
fn point(v: &Value) -> Result<Option<(i32, i32)>> {
    if !found(v) {
        return Ok(None);
    }
    match (v.get("x").and_then(Value::as_i64), v.get("y").and_then(Value::as_i64)) {
        (Some(x), Some(y)) => Ok(Some((x as i32, y as i32))),
        _ => Ok(None),
    }
}

/// Pushes the locator script into the container (idempotent — cheap to repeat).
async fn ensure_locator(instance: &WechatInstance) -> Result<()> {
    instance
        .exec_bash_stdin(&format!("cat > {LOCATOR_PATH}"), LOCATOR_PY.as_bytes())
        .await
        .context("push a11y_locate.py into container")?;
    Ok(())
}

/// Runs an `a11y_locate.py` subcommand and parses its JSON line. Points the call
/// at the WeChat session's a11y bus (a fresh exec has no D-Bus env of its own).
async fn run(instance: &WechatInstance, args: &[&str]) -> Result<Value> {
    let argline = args.iter().map(|a| sh_quote(a)).collect::<Vec<_>>().join(" ");
    // `|| true`: a11y_locate.py exits 3 when an element isn't found (a valid
    // result — the `{"found":false}` JSON is still on stdout). Without this the
    // non-zero exit makes exec_bash treat a normal "not found" as a hard failure.
    let script = format!(
        "if [ -f {DBUS_ADDR_FILE} ]; then export DBUS_SESSION_BUS_ADDRESS=\"$(cat {DBUS_ADDR_FILE})\"; \
         elif [ -f {DBUS_ADDR_FALLBACK} ]; then export DBUS_SESSION_BUS_ADDRESS=\"$(cat {DBUS_ADDR_FALLBACK})\"; fi; \
         python3 {LOCATOR_PATH} {argline} 2>/dev/null || true"
    );
    let out = instance.exec_bash(&script).await?;
    let line = out.lines().last().unwrap_or("{}");
    serde_json::from_str(line).with_context(|| format!("parse a11y_locate output: {out}"))
}

/// Single-quotes an argument for the shell (names may contain spaces, CJK, or
/// punctuation), escaping embedded single quotes.
fn sh_quote(arg: &str) -> String {
    format!("'{}'", arg.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::sh_quote;

    #[test]
    fn quotes_plain_and_special() {
        assert_eq!(sh_quote("Alice"), "'Alice'");
        assert_eq!(sh_quote("two words"), "'two words'");
        assert_eq!(sh_quote("it's"), r"'it'\''s'");
    }
}

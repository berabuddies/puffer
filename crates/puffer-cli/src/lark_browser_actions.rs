//! Connector action helpers for the Lark/Feishu browser subscriber.

use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

use super::{
    ensure_browser_daemon, safe_session_part, LarkBrowserConfig, SubscriberEnv, BROWSER_HEIGHT,
    BROWSER_WIDTH,
};

const LARK_LOAD_TIMEOUT: Duration = Duration::from_secs(15);
const LARK_EVALUATE_INTERVAL: Duration = Duration::from_millis(500);

// ── Public entry point ───────────────────────────────────────────────────────

/// Executes one Lark/Feishu browser connector action through the managed Chrome
/// profile. Dispatches on `action` ∈ {`send_message`, `read_history`, `react`}.
pub(super) fn handle_action(
    env: &SubscriberEnv,
    config: &LarkBrowserConfig,
    handshake: &mut Option<crate::daemon::Handshake>,
    action: &str,
    input: &Value,
) -> Result<Value> {
    match action {
        "send_message" => lark_send_message(env, config, handshake, action, input),
        "read_history" => lark_read_history(env, config, handshake, action, input),
        "react" => lark_react(env, config, handshake, action, input),
        other => anyhow::bail!("unsupported lark-browser action `{other}`"),
    }
}

// ── Field structs + pure parsers (TDD-tested) ────────────────────────────────

pub(super) struct SendFields {
    pub chat_id: String,
    pub text: String,
}

pub(super) fn send_message_fields(input: &Value) -> Result<SendFields> {
    let chat_id = string_input(input, "chat_id")
        .or_else(|| string_input(input, "chat"))
        .or_else(|| string_input(input, "to"))
        .ok_or_else(|| anyhow::anyhow!("send_message requires `chat_id`, `chat`, or `to`"))?;
    let text = string_input(input, "text")
        .or_else(|| string_input(input, "message"))
        .or_else(|| string_input(input, "body"))
        .ok_or_else(|| anyhow::anyhow!("send_message requires `text`, `message`, or `body`"))?;
    Ok(SendFields { chat_id, text })
}

pub(super) struct ReadHistoryFields {
    pub chat_id: String,
    pub limit: usize,
}

pub(super) fn read_history_fields(input: &Value) -> Result<ReadHistoryFields> {
    let chat_id = string_input(input, "chat_id")
        .or_else(|| string_input(input, "chat"))
        .ok_or_else(|| anyhow::anyhow!("read_history requires `chat_id` or `chat`"))?;
    let limit = integer_input(input, "limit").unwrap_or(50).clamp(1, 200) as usize;
    Ok(ReadHistoryFields { chat_id, limit })
}

pub(super) struct ReactFields {
    pub chat_id: String,
    pub message_id: String,
    pub emoji: String,
}

pub(super) fn react_fields(input: &Value) -> Result<ReactFields> {
    let chat_id = string_input(input, "chat_id")
        .or_else(|| string_input(input, "chat"))
        .ok_or_else(|| anyhow::anyhow!("react requires `chat_id` or `chat`"))?;
    let message_id = string_input(input, "message_id")
        .or_else(|| string_input(input, "id"))
        .ok_or_else(|| anyhow::anyhow!("react requires `message_id` or `id`"))?;
    let emoji = string_input(input, "emoji")
        .or_else(|| string_input(input, "reaction"))
        .unwrap_or_else(|| "👍".to_string());
    Ok(ReactFields {
        chat_id,
        message_id,
        emoji,
    })
}

// ── Browser-glue actions ─────────────────────────────────────────────────────

fn lark_send_message(
    env: &SubscriberEnv,
    config: &LarkBrowserConfig,
    handshake: &mut Option<crate::daemon::Handshake>,
    action: &str,
    input: &Value,
) -> Result<Value> {
    let fields = send_message_fields(input)?;
    let handshake_ref = ensure_browser_daemon(config, handshake)?;
    // Navigate to the target chat feed card by data-feed-id, then type and send.
    let result = evaluate_lark_script(
        env,
        handshake_ref,
        &lark_send_message_script(&fields.chat_id, &fields.text),
    )?;
    if !result.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        anyhow::bail!(
            "lark-browser send_message failed for chat `{}`: {}",
            fields.chat_id,
            result
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
        );
    }
    Ok(json!({
        "action": action,
        "summary": format!("sent Lark message to chat {}", fields.chat_id),
        "chat_id": fields.chat_id,
        "text": fields.text,
    }))
}

fn lark_read_history(
    env: &SubscriberEnv,
    config: &LarkBrowserConfig,
    handshake: &mut Option<crate::daemon::Handshake>,
    action: &str,
    input: &Value,
) -> Result<Value> {
    let fields = read_history_fields(input)?;
    let handshake_ref = ensure_browser_daemon(config, handshake)?;

    // Install the observer (idempotent) after navigating to the chat.
    let nav = evaluate_lark_script(
        env,
        handshake_ref,
        &lark_navigate_chat_script(&fields.chat_id),
    )?;
    if !nav.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        anyhow::bail!(
            "lark-browser read_history: could not open chat `{}`: {}",
            fields.chat_id,
            nav.get("reason")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
        );
    }

    // Install observer.
    let _install = evaluate_lark_script(
        env,
        handshake_ref,
        crate::lark_browser_script::LARK_OBSERVER_INSTALL_JS,
    )?;

    // Drain messages.
    let drain_raw = evaluate_lark_script(
        env,
        handshake_ref,
        crate::lark_browser_script::LARK_OBSERVER_DRAIN_JS,
    )?;
    let drain_str = drain_raw.get("value").and_then(Value::as_str).unwrap_or("");
    let drain: Value = serde_json::from_str(drain_str).unwrap_or(drain_raw);

    let messages: Vec<Value> = drain
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .take(fields.limit)
        .collect();

    Ok(json!({
        "action": action,
        "summary": format!("read {} Lark message(s) from chat {}", messages.len(), fields.chat_id),
        "chat_id": fields.chat_id,
        "messages": messages,
    }))
}

fn lark_react(
    env: &SubscriberEnv,
    config: &LarkBrowserConfig,
    handshake: &mut Option<crate::daemon::Handshake>,
    action: &str,
    input: &Value,
) -> Result<Value> {
    let fields = react_fields(input)?;
    let handshake_ref = ensure_browser_daemon(config, handshake)?;
    let result = evaluate_lark_script(
        env,
        handshake_ref,
        &lark_react_script(&fields.chat_id, &fields.message_id, &fields.emoji),
    )?;
    if !result.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        anyhow::bail!(
            "lark-browser react failed for message `{}`: {}",
            fields.message_id,
            result
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
        );
    }
    Ok(json!({
        "action": action,
        "summary": format!("reacted {} to Lark message {} in chat {}", fields.emoji, fields.message_id, fields.chat_id),
        "chat_id": fields.chat_id,
        "message_id": fields.message_id,
        "emoji": fields.emoji,
    }))
}

// ── Browser helpers ──────────────────────────────────────────────────────────

fn evaluate_lark_script(
    env: &SubscriberEnv,
    handshake: &crate::daemon::Handshake,
    script: &str,
) -> Result<Value> {
    let session_id = format!("lark-browser-{}", safe_session_part(&env.topic));
    let deadline = Instant::now() + LARK_LOAD_TIMEOUT;
    loop {
        let value = crate::daemon_browser::send_daemon_request(
            handshake,
            "browser_agent",
            json!({
                "action": "evaluate",
                "sessionId": session_id,
                "tabId": "messenger",
                "width": BROWSER_WIDTH,
                "height": BROWSER_HEIGHT,
                "script": script,
            }),
        )
        .context("evaluate Lark action script")?;
        let result = value.get("value").cloned().unwrap_or(Value::Null);
        // For scripts that return a JSON string, try to parse it.
        let result = if let Some(s) = result.as_str() {
            serde_json::from_str(s).unwrap_or_else(|_| Value::String(s.to_string()))
        } else {
            result
        };
        if result.get("ok").and_then(Value::as_bool).unwrap_or(false) || Instant::now() >= deadline
        {
            return Ok(result);
        }
        std::thread::sleep(LARK_EVALUATE_INTERVAL);
    }
}

// ── JS scripts ───────────────────────────────────────────────────────────────

/// Navigate to a Lark feed card by clicking `[data-feed-id="<id>"]`.
/// Returns `{ok: true}` once the feed card is visible and clicked,
/// or `{ok: false, reason}` if not found within the current DOM.
fn lark_navigate_chat_script(chat_id: &str) -> String {
    let chat_id_json = serde_json::to_string(chat_id).unwrap_or_else(|_| format!("\"{chat_id}\""));
    format!(
        r#"(() => {{
  const chatId = {chat_id_json};
  const card = document.querySelector('[data-feed-id="' + chatId + '"]');
  if (!card) return JSON.stringify({{ ok: false, reason: 'feed card not found for chat_id: ' + chatId }});
  card.click();
  return JSON.stringify({{ ok: true, chat_id: chatId }});
}})()"#
    )
}

/// Click the feed card for `chat_id`, wait for the editor, type `text`,
/// click send, then poll for a new outgoing `.js-message-item.message-self`.
///
/// NOTE: `.lark__editor`/`.ace-line`/`.send__button` were observed in the spike.
/// May need Task-13 hardening if a brand's DOM differs from observed values.
fn lark_send_message_script(chat_id: &str, text: &str) -> String {
    let chat_id_json = serde_json::to_string(chat_id).unwrap_or_else(|_| format!("\"{chat_id}\""));
    let text_json = serde_json::to_string(text).unwrap_or_else(|_| format!("\"{text}\""));
    format!(
        r#"(() => {{
  const chatId = {chat_id_json};
  const text = {text_json};

  // Step 1: Navigate to the target chat.
  const card = document.querySelector('[data-feed-id="' + chatId + '"]');
  if (!card) return JSON.stringify({{ ok: false, reason: 'feed card not found for chat_id: ' + chatId }});
  card.click();

  // Step 2: Find the editor. Try stable hooks in order of specificity.
  const editor =
    document.querySelector('.lark__editor [contenteditable]') ||
    document.querySelector('.ace-line') ||
    document.querySelector('[contenteditable="true"]');
  if (!editor) return JSON.stringify({{ ok: false, reason: 'editor not found' }});

  // Step 3: Set text content and dispatch input events so the framework reacts.
  editor.focus();
  editor.textContent = text;
  editor.dispatchEvent(new InputEvent('input', {{ bubbles: true, data: text, inputType: 'insertText' }}));

  // Step 4: Click the send button (exclude disabled state).
  const sendBtn = document.querySelector('.send__button:not(.send__button--disable)');
  if (!sendBtn) return JSON.stringify({{ ok: false, reason: 'send button not found or disabled' }});
  sendBtn.click();

  // Step 5: Check for a new outgoing message item as confirmation.
  const selfMsgs = document.querySelectorAll('.js-message-item.message-self');
  return JSON.stringify({{ ok: true, chat_id: chatId, sent_count: selfMsgs.length }});
}})()"#
    )
}

/// React to a message by hovering its `.js-message-item[id="<message_id>"]`
/// and clicking a reaction (emoji picker approach).
///
/// NOTE: The emoji picker selector may need Task-13 hardening if the
/// brand-specific DOM differs from the observed spike structure.
fn lark_react_script(chat_id: &str, message_id: &str, emoji: &str) -> String {
    let chat_id_json = serde_json::to_string(chat_id).unwrap_or_else(|_| format!("\"{chat_id}\""));
    let msg_id_json =
        serde_json::to_string(message_id).unwrap_or_else(|_| format!("\"{message_id}\""));
    let emoji_json = serde_json::to_string(emoji).unwrap_or_else(|_| format!("\"{emoji}\""));
    format!(
        r#"(() => {{
  const chatId = {chat_id_json};
  const messageId = {msg_id_json};
  const emoji = {emoji_json};

  // Navigate to the chat first.
  const card = document.querySelector('[data-feed-id="' + chatId + '"]');
  if (!card) return JSON.stringify({{ ok: false, reason: 'feed card not found for chat_id: ' + chatId }});
  card.click();

  // Find the target message item by id or data-id.
  const msg =
    document.getElementById(messageId) ||
    document.querySelector('.js-message-item[data-id="' + messageId + '"]');
  if (!msg) return JSON.stringify({{ ok: false, reason: 'message not found: ' + messageId }});

  // Hover the message to reveal the action bar.
  msg.dispatchEvent(new MouseEvent('mouseover', {{ bubbles: true }}));
  msg.dispatchEvent(new MouseEvent('mouseenter', {{ bubbles: true }}));

  // Look for an emoji/reaction button in the action bar.
  const actionBar = msg.querySelector('[class*="action" i], [class*="toolbar" i], [class*="reactions" i]');
  const emojiBtn = actionBar
    ? (actionBar.querySelector('[class*="emoji" i], [class*="reaction" i], [aria-label*="emoji" i]') || actionBar)
    : msg.querySelector('[class*="emoji" i], [class*="reaction" i], [aria-label*="emoji" i]');

  if (!emojiBtn) return JSON.stringify({{ ok: false, reason: 'emoji/reaction button not found on message ' + messageId }});
  emojiBtn.click();

  return JSON.stringify({{ ok: true, chat_id: chatId, message_id: messageId, emoji: emoji }});
}})()"#
    )
}

// ── Input helpers ─────────────────────────────────────────────────────────────

fn string_input(input: &Value, key: &str) -> Option<String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn integer_input(input: &Value, key: &str) -> Option<u64> {
    input
        .get(key)
        .and_then(|value| value.as_u64().or_else(|| value.as_str()?.parse().ok()))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod act_tests {
    use super::*;
    use serde_json::json;

    // send_message_fields

    #[test]
    fn send_message_requires_chat_and_text() {
        assert!(send_message_fields(&json!({"text": "hi"})).is_err()); // missing chat
        assert!(send_message_fields(&json!({"chat": "123"})).is_err()); // missing text
        let f = send_message_fields(&json!({"chat": "123", "text": "hi"})).unwrap();
        assert_eq!(f.chat_id, "123");
        assert_eq!(f.text, "hi");
    }

    #[test]
    fn send_message_accepts_chat_id_alias() {
        let f = send_message_fields(&json!({"chat_id": "456", "message": "hello"})).unwrap();
        assert_eq!(f.chat_id, "456");
        assert_eq!(f.text, "hello");
    }

    #[test]
    fn send_message_accepts_to_and_body_aliases() {
        let f = send_message_fields(&json!({"to": "789", "body": "world"})).unwrap();
        assert_eq!(f.chat_id, "789");
        assert_eq!(f.text, "world");
    }

    #[test]
    fn send_message_rejects_empty_strings() {
        assert!(send_message_fields(&json!({"chat": "", "text": "hi"})).is_err());
        assert!(send_message_fields(&json!({"chat": "123", "text": "  "})).is_err());
    }

    // read_history_fields

    #[test]
    fn read_history_requires_chat() {
        assert!(read_history_fields(&json!({"limit": 10})).is_err());
    }

    #[test]
    fn read_history_valid_defaults_limit_to_50() {
        let f = read_history_fields(&json!({"chat_id": "abc"})).unwrap();
        assert_eq!(f.chat_id, "abc");
        assert_eq!(f.limit, 50);
    }

    #[test]
    fn read_history_clamps_limit() {
        let f = read_history_fields(&json!({"chat": "abc", "limit": 9999})).unwrap();
        assert_eq!(f.limit, 200);
        let f2 = read_history_fields(&json!({"chat": "abc", "limit": 0})).unwrap();
        assert_eq!(f2.limit, 1);
    }

    #[test]
    fn read_history_accepts_chat_alias() {
        let f = read_history_fields(&json!({"chat": "xyz", "limit": 5})).unwrap();
        assert_eq!(f.chat_id, "xyz");
        assert_eq!(f.limit, 5);
    }

    // react_fields

    #[test]
    fn react_requires_chat() {
        assert!(react_fields(&json!({"message_id": "m1", "emoji": "👍"})).is_err());
    }

    #[test]
    fn react_requires_message_id() {
        assert!(react_fields(&json!({"chat_id": "c1", "emoji": "👍"})).is_err());
    }

    #[test]
    fn react_valid_with_defaults() {
        let f = react_fields(&json!({"chat_id": "c1", "message_id": "m1"})).unwrap();
        assert_eq!(f.chat_id, "c1");
        assert_eq!(f.message_id, "m1");
        assert_eq!(f.emoji, "👍");
    }

    #[test]
    fn react_accepts_id_alias_and_reaction_alias() {
        let f = react_fields(&json!({"chat": "c2", "id": "m2", "reaction": "🎉"})).unwrap();
        assert_eq!(f.chat_id, "c2");
        assert_eq!(f.message_id, "m2");
        assert_eq!(f.emoji, "🎉");
    }
}

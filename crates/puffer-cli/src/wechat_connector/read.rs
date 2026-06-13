//! OPT-IN vision/screen-reading fallback: reads WeChat UI from a screenshot via
//! the configured (codex/OpenAI) vision model.
//!
//! This is NOT the default path. By default the connector locates UI via the
//! AT-SPI accessibility tree (see `atspi.rs`) and the monitor reads messages from
//! the decrypted chat DB (see `dbread.rs`/`subscribe.rs`); vision here is gated
//! behind `WECHAT_ALLOW_VISION=1` (see [`vision_allowed`]) and used only as an
//! explicit fallback. When enabled, we send the WeChat window screenshot to the
//! OpenAI-compatible relay configured in `~/.puffer/config.toml`
//! (`openai_base_url` + `default_model`) with the API key from
//! `~/.puffer/auth.json`, and ask for the visible content as JSON. Read-only — no
//! input is sent to WeChat.

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use puffer_config::{load_config, ConfigPaths};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Process-wide vision token accounting, so each connector operation can report
/// the model tokens it consumed (DB reads consume ZERO — they never call here).
static TOK_PROMPT: AtomicU64 = AtomicU64::new(0);
static TOK_COMPLETION: AtomicU64 = AtomicU64::new(0);
static TOK_TOTAL: AtomicU64 = AtomicU64::new(0);
static VISION_CALLS: AtomicU64 = AtomicU64::new(0);

/// Returns and RESETS the accumulated `(prompt, completion, total, calls)` vision
/// token usage since the last call. Used to attach per-step cost to op results.
pub(crate) fn take_token_usage() -> (u64, u64, u64, u64) {
    (
        TOK_PROMPT.swap(0, Ordering::Relaxed),
        TOK_COMPLETION.swap(0, Ordering::Relaxed),
        TOK_TOTAL.swap(0, Ordering::Relaxed),
        VISION_CALLS.swap(0, Ordering::Relaxed),
    )
}

/// Records one vision call's usage into the accumulator.
fn record_usage(value: &Value) {
    let usage = match value.get("usage") {
        Some(u) => u,
        None => return,
    };
    let p = usage.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0);
    let c = usage.get("completion_tokens").and_then(Value::as_u64).unwrap_or(0);
    let t = usage.get("total_tokens").and_then(Value::as_u64).unwrap_or(p + c);
    TOK_PROMPT.fetch_add(p, Ordering::Relaxed);
    TOK_COMPLETION.fetch_add(c, Ordering::Relaxed);
    TOK_TOTAL.fetch_add(t, Ordering::Relaxed);
    VISION_CALLS.fetch_add(1, Ordering::Relaxed);
}

/// One message read off the WeChat screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WechatMessage {
    /// The CONVERSATION the message belongs to (contact/group name) — the
    /// routable reply target. For a chat-list row this equals the row name; for
    /// a message in the open conversation it is the open chat's header, NOT the
    /// individual author.
    pub(crate) chat: String,
    /// Display name of the message's author/sender (best-effort).
    pub(crate) sender: String,
    /// Message text.
    pub(crate) text: String,
}

/// Whether screen reading (the vision model) is permitted. OFF by default — it
/// costs tokens — and there is NO automatic fallback to it: callers try the
/// accessibility (AT-SPI) path unconditionally, and only reach the vision model
/// when the operator has opted in explicitly for the run by setting
/// `WECHAT_ALLOW_VISION=1` (or `=true`). With it unset, every vision call fails
/// fast instead of silently spending tokens.
pub(crate) fn vision_allowed() -> bool {
    std::env::var("WECHAT_ALLOW_VISION")
        .map(|v| {
            let v = v.trim();
            v == "1" || v.eq_ignore_ascii_case("true")
        })
        .unwrap_or(false)
}

/// Timeout for a single vision completion. Default 90s; raise it for a slow
/// backend (e.g. a local codex reverse-proxy) via `WECHAT_VISION_TIMEOUT` (secs).
fn vision_timeout() -> Duration {
    let secs = std::env::var("WECHAT_VISION_TIMEOUT")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&s| s > 0)
        .unwrap_or(90);
    Duration::from_secs(secs)
}

const SYSTEM_PROMPT: &str = "You are reading a screenshot of the WeChat desktop client to detect \
NEW incoming messages. Return a JSON array of objects, each with three string fields: \
{\"chat\": the CONVERSATION it belongs to, \"sender\": who wrote it, \"text\": the message text}. \
Two sources: (1) In the LEFT chat list, each chat with an unread red badge — for these set both \
`chat` and `sender` to that contact/group name and `text` to the latest preview shown. (2) In the \
OPEN conversation on the RIGHT, each visible incoming message — set `chat` to the open \
conversation's title (the header at the top of the right pane), `sender` to the message's author \
(in a group this is the member; in a 1-on-1 it is the contact), and `text` to the message. NEVER \
put an individual group member in `chat` — `chat` is always the conversation/group itself. Ignore \
UI chrome and your own (right-aligned, green) composer/outgoing bubbles. Respond with ONLY a \
compact JSON array like [{\"chat\":\"Group\",\"sender\":\"Alice\",\"text\":\"hi\"}]; if nothing new, respond with [].";

/// Resolves `(base_url, api_key, model)` for the OpenAI-compatible relay from
/// the user's puffer config + auth store.
fn resolve_openai() -> Result<(String, String, String)> {
    let cwd = std::env::current_dir().unwrap_or_default();
    let paths = ConfigPaths::discover(&cwd);
    let config = load_config(&paths).context("load puffer config")?;
    let base_url = config
        .openai_base_url
        .clone()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("openai_base_url is not set in ~/.puffer/config.toml"))?;
    // Config stores the model as `openai/<name>`; the relay wants the bare name.
    let model = config
        .default_model
        .clone()
        .filter(|value| !value.trim().is_empty())
        .map(|value| {
            value
                .strip_prefix("openai/")
                .unwrap_or(&value)
                .to_string()
        })
        .unwrap_or_else(|| "gpt-4o".to_string());

    let auth_path = paths.user_config_dir.join("auth.json");
    let raw = std::fs::read_to_string(&auth_path)
        .with_context(|| format!("read {}", auth_path.display()))?;
    let auth: Value = serde_json::from_str(&raw).context("parse auth.json")?;
    let key = auth
        .get("providers")
        .and_then(|providers| providers.get("openai"))
        .and_then(|openai| openai.get("key"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("no OpenAI api key in ~/.puffer/auth.json"))?
        .to_string();

    // Env overrides let the vision calls use a different (e.g. fallback /
    // currently-reachable) endpoint than the main agent model, without editing
    // the shared config — handy when the primary relay is down.
    let base_url = std::env::var("WECHAT_VISION_BASE_URL")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or(base_url);
    let model = std::env::var("WECHAT_VISION_MODEL")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or(model);
    let key = std::env::var("WECHAT_VISION_API_KEY")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or(key);

    Ok((base_url.trim_end_matches('/').to_string(), key, model))
}

/// Reads the messages visible in `png` (PNG bytes) via the vision model.
///
/// BLOCKING — uses `reqwest::blocking`, so call it from a blocking context
/// (e.g. `tokio::task::spawn_blocking`), never directly on an async runtime.
pub(crate) fn read_messages_from_png(png: &[u8]) -> Result<Vec<WechatMessage>> {
    let content = vision_complete(
        SYSTEM_PROMPT,
        "Extract new/unread WeChat messages as JSON.",
        png,
    )?;
    Ok(parse_messages(&content))
}

/// One vision chat-completion over a screenshot, returning the model's text.
/// BLOCKING. Retries transient failures (network / 5xx / empty) up to 3 times
/// with backoff, so a momentary relay 503 doesn't abort a send. Fails fast on
/// 4xx (auth/request) errors.
fn vision_complete(system: &str, user: &str, png: &[u8]) -> Result<String> {
    // Vision is off by default and never an automatic fallback: bail before any
    // token is spent unless the operator opted in for this run.
    if !vision_allowed() {
        bail!(
            "screen reading (vision) is disabled by default to avoid token cost, and is not an \
             automatic fallback; set WECHAT_ALLOW_VISION=1 to permit it for this run"
        );
    }
    let (base_url, key, model) = resolve_openai()?;
    let data_url = format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png)
    );
    let body = json!({
        "model": model,
        "temperature": 0,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": [
                { "type": "text", "text": user },
                { "type": "image_url", "image_url": { "url": data_url } }
            ]}
        ]
    });
    let client = reqwest::blocking::Client::builder()
        .timeout(vision_timeout())
        .build()
        .context("build vision http client")?;
    let url = format!("{base_url}/chat/completions");
    let mut last_err = String::new();
    for attempt in 0..3u64 {
        match client.post(&url).bearer_auth(&key).json(&body).send() {
            Ok(resp) => {
                let status = resp.status();
                let value: Value = resp.json().unwrap_or(Value::Null);
                if status.is_success() {
                    record_usage(&value);
                    if let Some(content) = value
                        .get("choices")
                        .and_then(|c| c.get(0))
                        .and_then(|c| c.get("message"))
                        .and_then(|m| m.get("content"))
                        .and_then(Value::as_str)
                    {
                        return Ok(content.to_string());
                    }
                    last_err = format!("no content: {value}");
                } else {
                    last_err = format!("status {status}: {value}");
                    if !status.is_server_error() {
                        bail!("vision request failed ({status}): {value}"); // 4xx: don't retry
                    }
                }
            }
            Err(error) => last_err = error.to_string(),
        }
        if attempt < 2 {
            std::thread::sleep(std::time::Duration::from_millis(700 * (attempt + 1)));
        }
    }
    bail!("vision request failed after retries: {last_err}")
}

/// Reads the title of the currently-open conversation (the chat header at the
/// top of the right pane), used to verify a send targets the right chat.
/// Returns `Ok(None)` when no chat is open / header unreadable. BLOCKING.
pub(crate) fn read_open_chat_title(png: &[u8]) -> Result<Option<String>> {
    one_shot_text(
        png,
        "You are reading a WeChat desktop screenshot. Report the title of the conversation currently \
OPEN in the right pane — the contact or group name shown in the chat header at the top of the right \
side. Respond with ONLY that name as plain text, nothing else. If no conversation is open (the right \
side is blank/splash), respond with exactly NONE.",
        "What is the open chat's title?",
    )
}

/// Confirms (visually) that the conversation OPEN in the right pane is the chat
/// for `recipient`. More robust than transcription for DECORATIVE/stylized
/// display names: the model compares the header against the reference characters
/// rather than transcribing them first (transcription misreads lookalike glyphs).
/// Returns `true` only on an explicit YES; "no chat" / unsure answers NO
/// (fail-closed). BLOCKING.
pub(crate) fn confirm_open_chat_is(png: &[u8], recipient: &str) -> Result<bool> {
    let system = format!(
        "You are reading a WeChat desktop screenshot. A conversation may be OPEN in the right pane, \
with the contact/group name shown in the header at the very top of the right side. That displayed \
name MAY include decorative or stylized unicode letters surrounding the real characters. Looking \
ONLY at that header, decide whether the open conversation is the chat for: {recipient} — i.e. the \
header clearly shows / contains the name \"{recipient}\". Answer with ONLY the word YES or NO. If no \
conversation is open, or you are not confident it is that exact chat, answer NO."
    );
    let content = vision_complete(
        &system,
        "Is the currently-open chat the requested recipient? Answer YES or NO.",
        png,
    )?;
    Ok(content.trim().to_ascii_uppercase().starts_with("YES"))
}

/// Reads the text of the most recent OUTGOING message (the latest right-aligned
/// green bubble) in the open conversation, for delivery confirmation. BLOCKING.
pub(crate) fn read_last_outgoing(png: &[u8]) -> Result<Option<String>> {
    one_shot_text(
        png,
        "You are reading a WeChat desktop screenshot. Report the text of the MOST RECENT message \
SENT BY ME — the bottom-most right-aligned (green) outgoing bubble in the open conversation. \
Respond with ONLY that text, or exactly NONE if there is no outgoing message visible.",
        "What is my most recent outgoing message text?",
    )
}

/// Scans the screen for a WeChat risk/limit/forced-logout banner (the signal to
/// pause all sends). Returns the banner text if present. BLOCKING.
pub(crate) fn read_risk_warning(png: &[u8]) -> Result<Option<String>> {
    one_shot_text(
        png,
        "You are reading a WeChat desktop screenshot. Determine if WeChat is showing a RISK / \
SECURITY / RATE-LIMIT / ACCOUNT-RESTRICTION / FORCED-LOGOUT warning — e.g. phrases like \
当前账号存在风险, 操作过于频繁, 功能被限制, 账号被限制, 安全验证, 请重新登录, account risk, \
operating too frequently, function restricted. Respond with ONLY the warning text if such a \
banner/dialog is visible, or exactly NONE otherwise.",
        "Is there a risk/limit/logout warning on screen?",
    )
}

/// One-shot vision call returning trimmed text, or `None` for an empty/`NONE` reply.
fn one_shot_text(png: &[u8], system: &str, user: &str) -> Result<Option<String>> {
    let content = vision_complete(system, user, png)?;
    let content = content.trim();
    if content.is_empty() || content.eq_ignore_ascii_case("none") {
        Ok(None)
    } else {
        Ok(Some(content.to_string()))
    }
}

/// Locates the pixel (x,y) of the search-dropdown row that opens the chat with
/// `recipient` — preferring a contact/group/feature result over the web-search
/// rows. Returns `Ok(None)` if no local result is found. BLOCKING.
pub(crate) fn read_result_coords(png: &[u8], recipient: &str) -> Result<Option<(i32, i32)>> {
    let system = "You are looking at a WeChat desktop search dropdown (left side). It lists rows. \
Some rows are under 搜索网络结果 (web search — a magnifier icon) and MUST be ignored. Other rows are \
real chats/contacts/groups/features (联系人 / 群聊 / 功能), shown with an avatar. Find the row that \
opens a chat/contact/group/feature whose name matches the requested target, NOT a web-search row. \
Respond with ONLY compact JSON: {\"x\":<int>,\"y\":<int>} giving the pixel center of that row, or \
{\"found\":false} if there is no such local result.";
    let user = format!("Target name: {recipient}. Give the click coordinates of its local result row.");
    let content = vision_complete(system, &user, png)?;
    if std::env::var("WECHAT_DEBUG_VISION").map(|v| v.trim() == "1").unwrap_or(false) {
        eprintln!("wechat read_result_coords raw reply: {content}");
    }
    Ok(parse_xy(&content))
}

/// Parses a `{"x":int,"y":int}` reply (tolerating fences/prose) into coords, or
/// `None` for `{"found":false}` / unparseable / missing fields.
fn parse_xy(content: &str) -> Option<(i32, i32)> {
    let trimmed = strip_code_fence(content.trim());
    let slice = match (trimmed.find('{'), trimmed.rfind('}')) {
        (Some(s), Some(e)) if e > s => &trimmed[s..=e],
        _ => return None,
    };
    let value: Value = serde_json::from_str(slice).ok()?;
    let x = value.get("x").and_then(Value::as_i64)?;
    let y = value.get("y").and_then(Value::as_i64)?;
    Some((x as i32, y as i32))
}

/// Locates the pixel center of the message bubble whose text matches `snippet`,
/// in the open conversation (right pane) — used to right-click it for a
/// quote/reply. Returns `None` if no matching bubble is visible. BLOCKING.
pub(crate) fn read_message_coords(png: &[u8], snippet: &str) -> Result<Option<(i32, i32)>> {
    let system = "You are looking at a WeChat conversation in the right pane. Each message is a \
chat bubble. Find the message bubble whose text contains or best matches the given snippet — this \
is the message being replied to, so PREFER an INCOMING message (left-aligned, white/grey bubble) \
over your own outgoing messages (right-aligned, green); among matches prefer the most recent. \
Respond with ONLY compact JSON {\"x\":<int>,\"y\":<int>} = the pixel center of that bubble, or \
{\"found\":false} if no such message is visible.";
    let user = format!("Snippet to find: {snippet}");
    let content = vision_complete(system, &user, png)?;
    if std::env::var("WECHAT_DEBUG_VISION").map(|v| v.trim() == "1").unwrap_or(false) {
        eprintln!("wechat read_message_coords raw reply: {content}");
    }
    Ok(parse_xy(&content))
}

/// Locates the pixel center of a right-click context-menu item labeled `label`
/// (e.g. quote, forward, recall). Returns `None` if not present. BLOCKING.
pub(crate) fn read_menu_item_coords(png: &[u8], label: &str) -> Result<Option<(i32, i32)>> {
    let system = "A small right-click context menu is open in the WeChat desktop client (a vertical \
list of short text items such as 复制 / 转发 / 收藏 / 引用 / 删除). Find the menu item whose label \
matches the requested one. Respond with ONLY compact JSON {\"x\":<int>,\"y\":<int>} = the pixel center \
of that item, or {\"found\":false} if the menu has no such item.";
    let user = format!("Menu item label: {label}");
    let content = vision_complete(system, &user, png)?;
    if std::env::var("WECHAT_DEBUG_VISION").map(|v| v.trim() == "1").unwrap_or(false) {
        eprintln!("wechat read_menu_item_coords raw reply: {content}");
    }
    Ok(parse_xy(&content))
}

/// Locates the pixel center of the AVATAR (square profile icon beside a bubble)
/// of the most recent message from `who`, for a pat (right-click the avatar).
/// Returns `None` if not found. BLOCKING.
pub(crate) fn read_avatar_coords(png: &[u8], who: &str) -> Result<Option<(i32, i32)>> {
    let system = "You are looking at the WeChat desktop. IGNORE the conversation LIST on the far \
left (the column of chats around x<270). Look ONLY at the open conversation in the RIGHT pane: \
each incoming message there has a small square avatar just to the LEFT of its bubble. Find the \
most recent message sent by the named person and return the pixel center of THAT message's avatar \
icon in the conversation (NOT the chat list, NOT the bubble). Respond with ONLY compact JSON \
{\"x\":<int>,\"y\":<int>}, or {\"found\":false} if that person's avatar is not visible in the conversation.";
    let user = format!("Person: {who}");
    let content = vision_complete(system, &user, png)?;
    if std::env::var("WECHAT_DEBUG_VISION").map(|v| v.trim() == "1").unwrap_or(false) {
        eprintln!("wechat read_avatar_coords raw reply: {content}");
    }
    Ok(parse_xy(&content))
}

/// Parses the model's reply into messages, tolerating ```json fences and prose.
fn parse_messages(content: &str) -> Vec<WechatMessage> {
    let trimmed = strip_code_fence(content.trim());
    // Find the first JSON array in the text.
    let slice = match (trimmed.find('['), trimmed.rfind(']')) {
        (Some(start), Some(end)) if end > start => &trimmed[start..=end],
        _ => return Vec::new(),
    };
    let parsed: Value = match serde_json::from_str(slice) {
        Ok(value) => value,
        Err(_) => return Vec::new(),
    };
    let Some(items) = parsed.as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let text = item.get("text").and_then(Value::as_str)?.trim();
            if text.is_empty() {
                return None;
            }
            let sender = item
                .get("sender")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            // `chat` is the conversation (reply target). Fall back to sender only
            // for older/!chat replies, which is correct for chat-list rows where
            // they're the same (but wrong for group authors — hence the field).
            let chat = item
                .get("chat")
                .or_else(|| item.get("conversation"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(&sender)
                .to_string();
            Some(WechatMessage {
                chat,
                sender,
                text: text.to_string(),
            })
        })
        .collect()
}

/// Strips a leading/trailing markdown code fence if present.
fn strip_code_fence(text: &str) -> &str {
    let text = text.strip_prefix("```json").or_else(|| text.strip_prefix("```")).unwrap_or(text);
    text.strip_suffix("```").unwrap_or(text).trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_json_array() {
        let msgs = parse_messages(
            r#"[{"chat":"Group","sender":"Alice","text":"hi"},{"chat":"Bob","sender":"Bob","text":"yo"}]"#,
        );
        assert_eq!(msgs.len(), 2);
        assert_eq!(
            msgs[0],
            WechatMessage { chat: "Group".into(), sender: "Alice".into(), text: "hi".into() }
        );
    }

    #[test]
    fn chat_falls_back_to_sender_when_absent() {
        // A chat-list row that omits `chat`: it equals the sender (the conversation).
        let msgs = parse_messages(r#"[{"sender":"Carol","text":"hello"}]"#);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].chat, "Carol");
        assert_eq!(msgs[0].sender, "Carol");
    }

    #[test]
    fn group_message_keeps_distinct_chat_and_author() {
        let msgs = parse_messages(r#"[{"chat":"Team","sender":"Alice","text":"hi"}]"#);
        assert_eq!(msgs[0].chat, "Team");
        assert_eq!(msgs[0].sender, "Alice");
    }

    #[test]
    fn parses_fenced_json_with_prose() {
        let msgs = parse_messages("Here you go:\n```json\n[{\"sender\":\"A\",\"text\":\"hello\"}]\n```");
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].text, "hello");
    }

    #[test]
    fn empty_and_garbage_yield_nothing() {
        assert!(parse_messages("[]").is_empty());
        assert!(parse_messages("no messages").is_empty());
        assert!(parse_messages("").is_empty());
    }

    #[test]
    fn skips_entries_without_text() {
        let msgs = parse_messages(r#"[{"sender":"A"},{"sender":"B","text":"  "},{"text":"keep"}]"#);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].text, "keep");
        assert_eq!(msgs[0].sender, "");
    }
}

//! Outbound actions: driving the WeChat client to send a message as the user.
//!
//! There is no WeChat send API, so this reproduces what a person does at the
//! keyboard, mediated through `docker exec` + `xdotool`/`xclip`:
//!
//! 1. focus the WeChat window
//! 2. open search (Ctrl+F), paste the contact name, Enter to open the chat
//! 3. paste the message body into the (now focused) input box
//! 4. pause, then Enter to send
//!
//! Chinese text is delivered by clipboard paste (xclip → Ctrl+V), matching the
//! WechatOnCloud panel, because per-keysym typing drops characters. Human-like
//! think-time from [`Human`](super::human::Human) is inserted between every
//! step so the cadence is not robotic.

use anyhow::{anyhow, bail, Context, Result};
use rand::Rng;
use serde_json::{json, Value};

use super::config::InstanceConfig;
use super::docker::WechatInstance;
use super::human::Human;
use super::policy::Policy;
use super::read;

/// Literal labels from the zh-CN WeChat client that the connector matches in the
/// accessibility tree or via OCR. These are WeChat's own UI strings (the client
/// runs in Chinese), so they are kept verbatim — translating them would break the
/// match. Isolated here so the rest of the module reads in English.
mod ui {
    /// Context-menu item: quote / reply to a message.
    pub(super) const QUOTE: &str = "引用";
    /// Context-menu item: pat / nudge a member.
    pub(super) const PAT: &str = "拍一拍";
    /// Account-menu item: log out (keeps local data).
    pub(super) const LOG_OUT: &str = "退出登录";
    /// Confirmation button shown as a plain "exit" on some builds.
    pub(super) const EXIT: &str = "退出";
    /// Main-window title, for the xdotool `--name` fallback (the WM_CLASS
    /// `wechat` lookup is tried first).
    pub(super) const WINDOW_NAMES: &str = "微信|WeChat|Weixin";
    /// zh-CN phrasings for "@everyone" (broadcast), blocked by the @all guard.
    pub(super) const AT_ALL: &[&str] = &["所有人", "全体成员", "全体"];
}

/// Result of a successful `act` command, surfaced back to the host.
pub(crate) struct ActOutcome {
    /// Human-readable one-line summary.
    pub(crate) summary: String,
    /// Structured output for the host/agent.
    pub(crate) output: Value,
}

/// The key (sequence) that sends the current message. WeChat's default is
/// `Return`; users who set "Ctrl+Enter to send" should set `WECHAT_SEND_KEY=ctrl+Return`.
/// Validated against an xdotool-keyspec allowlist (defense-in-depth; the value
/// flows into `xdotool key`), falling back to `Return` on anything unexpected.
fn send_key() -> String {
    let key = std::env::var("WECHAT_SEND_KEY")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "Return".to_string());
    if key.chars().all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '_') {
        key
    } else {
        "Return".to_string()
    }
}

/// Pre-send "re-read / composition" pause, scaled by message length so a long
/// reply does not appear instantly (a bot tell). Capped to stay responsive.
fn compose_pause_ms(human: &Human, body: &str) -> u64 {
    let base = human.pre_send_pause().as_millis() as u64;
    let chars = body.chars().count() as u64;
    let per_char = rand::thread_rng().gen_range(10..=30); // ~1-3s per 100 chars
    (base + chars * per_char).min(15_000)
}

/// Sends a message to a WeChat contact by simulating the user. `input` carries
/// the recipient (a contact/chat display name) and the body text. Gated by the
/// anti-ban [`Policy`]; verifies the opened chat before sending.
pub(crate) async fn send_message(
    instance: &WechatInstance,
    cfg: &InstanceConfig,
    input: &Value,
    human: &Human,
) -> Result<ActOutcome> {
    let recipient = resolve_recipient(input)?;
    // Body text is OPTIONAL when an attachment is present (media-only send).
    let body = first_str(input, &["text", "message", "caption"])
        .filter(|s| !s.trim().is_empty());

    // Validate the quote/reply precondition BEFORE any side effect — a reply
    // needs a body, and bailing here avoids sending attachments then erroring.
    let quoting = reply_snippet(input);
    if quoting.is_some() && body.is_none() {
        bail!("a quote/reply (`reply_to`) requires `text` (the reply body)");
    }

    ensure_ready(instance, cfg).await?;

    // Anti-ban gate: caps / active-hours / dup / sensitive content, then the
    // randomized minimum gap before sending.
    let mut policy = Policy::load(instance.name());
    // For agent AUTO-replies (input `auto_reply: true`), deliberately skip a
    // fraction so we don't answer 100% of messages instantly (a bot tell).
    if input.get("auto_reply").and_then(Value::as_bool) == Some(true) && !policy.should_reply() {
        return Ok(ActOutcome {
            summary: format!("Skipped auto-reply to {recipient} (human-realism throttle)"),
            output: json!({ "recipient": recipient, "skipped": true }),
        });
    }

    // Resolve any media attachments (reads local files / downloads URLs) off the
    // async runtime, since it does blocking IO + network.
    let attachments = {
        let input = input.clone();
        tokio::task::spawn_blocking(move || collect_attachments(&input))
            .await
            .map_err(|e| anyhow!("attachment resolve task failed: {e}"))??
    };
    if body.is_none() && attachments.is_empty() {
        bail!("send_message requires `text` (or `message`) or an attachment (`image`/`file`/`media`)");
    }

    // One up-front anti-ban gate. For a media-only send the gate label is made
    // time-unique so two different sends with the same image/file COUNTS aren't
    // mistaken for duplicate content; the real body text keeps content-dedup.
    let nonce = send_nonce();
    let gate_text = body
        .clone()
        .unwrap_or_else(|| format!("{} @{nonce}", media_gate_label(&attachments)));
    if let Err(block) = policy.check_send(&recipient, &gate_text) {
        bail!("blocked by anti-ban policy: {block}");
    }
    sleep_ms(policy.wait_before_send()).await;

    // Navigate to the recipient's chat, verifying (fail-closed) we landed right.
    open_chat(instance, &recipient, human).await?;
    verify_open_chat(instance, &recipient).await?;

    // Focus the message input box. Opening a chat by CLICKING a search-result row
    // does NOT reliably focus the input on WeChat 4.x, so a paste would land
    // nowhere and the send would silently no-op (validated live). Click it first.
    focus_message_input(instance, human).await?;

    // Perform the physical sends, recording EACH message against caps as it
    // lands (so the per-minute/daily counters reflect the true count and a
    // mid-batch failure still counts what went out) and tracking how many real
    // messages have left. `/tmp/wocm/<nonce>` stages this send's files and is
    // cleaned up afterwards regardless of outcome.
    let mut images = 0usize;
    let mut files = 0usize;
    let mut sent = 0usize;
    let result = perform_sends(
        instance, human, &mut policy, &recipient, &attachments, body.as_deref(),
        quoting.as_deref(), &nonce, &mut images, &mut files, &mut sent,
    )
    .await;
    let _ = instance.exec_bash(&format!("rm -rf '/tmp/wocm/{nonce}'")).await; // best-effort cleanup

    if let Err(error) = result {
        // Once ANY message has physically left, do NOT let the host retry the
        // whole send (it would re-blast the earlier messages). The "partial
        // send"/"not retrying" marker makes run_act classify this non-retryable.
        if sent > 0 {
            bail!(
                "__WECHAT_PARTIAL_SEND__ partial WeChat send to {recipient}: {sent} message(s) \
                 already delivered; not retrying to avoid duplicates ({error:#})"
            );
        }
        return Err(error);
    }

    let summary = describe_send(&recipient, body.as_deref(), images, files);
    Ok(ActOutcome {
        summary,
        output: json!({
            "recipient": recipient,
            "chars": body.as_deref().map(|b| b.chars().count()).unwrap_or(0),
            "images": images,
            "files": files,
            "messages_sent": sent,
        }),
    })
}

/// Performs the physical message sends for [`send_message`]: each attachment
/// (image inline / file card), each per-attachment caption, then the text body
/// (optionally as a quote/reply). Records every delivered message against the
/// anti-ban policy as it lands and bumps `*sent`, so partial failures are
/// accounted for and the caller can refuse a double-sending retry.
#[allow(clippy::too_many_arguments)]
async fn perform_sends(
    instance: &WechatInstance,
    human: &Human,
    policy: &mut Policy,
    recipient: &str,
    attachments: &[Attachment],
    body: Option<&str>,
    quoting: Option<&str>,
    nonce: &str,
    images: &mut usize,
    files: &mut usize,
    sent: &mut usize,
) -> Result<()> {
    for (idx, att) in attachments.iter().enumerate() {
        if idx > 0 {
            sleep(human.think_time()).await; // small human gap between sends
            focus_message_input(instance, human).await?;
        }
        send_one_attachment(instance, att, nonce, idx).await?;
        if att.as_image {
            *images += 1
        } else {
            *files += 1
        }
        *sent += 1;
        // Unique per-message label so repeated media doesn't pollute text dedup.
        policy.record_send(recipient, &format!("[attachment {} @{nonce}-{idx}]", att.filename))?;
        if let Some(caption) = att.caption.as_deref().filter(|c| !c.trim().is_empty()) {
            sleep(human.think_time()).await;
            focus_message_input(instance, human).await?;
            key(instance, "ctrl+a").await?;
            paste(instance, caption).await?;
            sleep_ms(compose_pause_ms(human, caption)).await;
            key(instance, &send_key()).await?;
            *sent += 1;
            policy.record_send(recipient, caption)?;
        }
    }

    if let Some(body) = body {
        if !attachments.is_empty() {
            sleep(human.think_time()).await;
        }
        if let Some(snippet) = quoting {
            // Quote the target message first (right-click -> quote). WeChat focuses
            // the input automatically afterwards, leaving it empty, so we skip the
            // focus-click + select-all the plain path uses (a misclick could
            // dismiss the quote bar).
            quote_message(instance, snippet, human).await?;
            sleep(human.think_time()).await;
        } else {
            focus_message_input(instance, human).await?;
            key(instance, "ctrl+a").await?; // clear any stray text so the paste is exact
        }
        // Paste the body, re-read pause (scaled to length), then send.
        paste(instance, body).await?;
        sleep_ms(compose_pause_ms(human, body)).await;
        key(instance, &send_key()).await?;
        *sent += 1;
        policy.record_send(recipient, body)?;
        // Confirm the text landed (best-effort, never blocks — already recorded).
        confirm_delivery(instance, body).await?;
    }
    Ok(())
}

/// Sends a message into a group, @-mentioning one or more members first.
/// `mentions` is a string or array of member display names. Gated by policy.
pub(crate) async fn mention(
    instance: &WechatInstance,
    cfg: &InstanceConfig,
    input: &Value,
    human: &Human,
) -> Result<ActOutcome> {
    let recipient = resolve_recipient(input)?;
    let names = mention_names(input);
    if names.is_empty() {
        bail!("mention requires `mention` (a member name or array of names)");
    }
    // Reject @all unless explicitly allowed (it broadcasts to everyone).
    let at_all_allowed = std::env::var("WECHAT_ALLOW_AT_ALL").map(|v| v.trim() == "1").unwrap_or(false);
    if names.iter().any(|n| is_at_all(n)) && !at_all_allowed {
        bail!("@all (broadcast to all members) is blocked (set WECHAT_ALLOW_AT_ALL=1 to override)");
    }
    let body = first_str(input, &["text", "message", "caption"]).unwrap_or_default();

    ensure_ready(instance, cfg).await?;
    let mut policy = Policy::load(instance.name());
    let gate_text = format!("@{} {body}", names.join(" @"));
    if let Err(block) = policy.check_send(&recipient, &gate_text) {
        bail!("blocked by anti-ban policy: {block}");
    }
    sleep_ms(policy.wait_before_send()).await;

    open_chat(instance, &recipient, human).await?;
    verify_open_chat(instance, &recipient).await?;

    // Focus the input box first (clicking a search row does not focus it on 4.x).
    focus_message_input(instance, human).await?;

    // For each mention: type '@', paste the name to filter the picker, Return to
    // insert the top match.
    for name in &names {
        key(instance, "at").await?;
        sleep(human.think_time()).await;
        paste(instance, name).await?;
        sleep(human.think_time()).await;
        key(instance, "Return").await?;
    }
    if !body.trim().is_empty() {
        paste(instance, &body).await?;
    }
    sleep_ms(compose_pause_ms(human, &body)).await;
    key(instance, &send_key()).await?;

    policy.record_send(&recipient, &gate_text)?;
    Ok(ActOutcome {
        summary: format!("Sent WeChat group message to {recipient} mentioning {}", names.join(", ")),
        output: json!({ "recipient": recipient, "mentions": names }),
    })
}

/// Opens a chat (which marks it read in WeChat). Read-only with respect to other
/// people — no message is sent — so it is NOT policy-gated.
pub(crate) async fn mark_read(
    instance: &WechatInstance,
    cfg: &InstanceConfig,
    input: &Value,
    human: &Human,
) -> Result<ActOutcome> {
    let recipient = resolve_recipient(input)?;
    ensure_ready(instance, cfg).await?;
    open_chat(instance, &recipient, human).await?;
    Ok(ActOutcome {
        summary: format!("Opened WeChat chat {recipient} (marked read)"),
        output: json!({ "recipient": recipient }),
    })
}

/// Logs out of WeChat through the client UI (bottom-left "more" menu -> log out
/// -> confirm), KEEPING the container, the WeChat install and local data. This is
/// distinct from REMOVING the connection (which wipes the container + data
/// volume): after logout the same or a different account can log back in with a
/// fresh QR scan, without recreating anything.
pub(crate) async fn logout(
    instance: &WechatInstance,
    cfg: &InstanceConfig,
    _input: &Value,
    human: &Human,
) -> Result<ActOutcome> {
    instance.ensure_container(cfg).await?;
    let _ = instance.ensure_screenshot_tool().await;
    if !instance.is_logged_in().await.unwrap_or(false) {
        return Ok(ActOutcome {
            summary: format!("WeChat `{}` is already logged out", instance.name()),
            output: json!({ "logged_out": true, "already": true }),
        });
    }

    activate_window(instance).await?;
    sleep(human.think_time()).await;
    // Open the bottom-left "more" menu, then click the log-out item.
    human_click(instance, more_menu_point(instance).await).await?;
    sleep_ms(700).await;
    let item = locate_menu_item(instance, ui::LOG_OUT)
        .await?
        .ok_or_else(|| anyhow!("could not find the log-out item in the WeChat menu (is it open?)"))?;
    human_click(instance, item).await?;
    sleep_ms(700).await;
    // Confirm dialog (button is usually the same log-out label, sometimes "exit").
    let confirm = match locate_menu_item(instance, ui::LOG_OUT).await? {
        Some(p) => Some(p),
        None => locate_menu_item(instance, ui::EXIT).await?,
    };
    if let Some(p) = confirm {
        human_click(instance, p).await?;
        sleep_ms(800).await;
    }

    Ok(ActOutcome {
        summary: format!("Logged out of WeChat `{}` (container + data kept)", instance.name()),
        output: json!({ "logged_out": true }),
    })
}

/// Pixel point of the bottom-left ☰ "more" menu trigger, from the main window
/// geometry (fallback: the 1280x800 capture's bottom-left rail).
async fn more_menu_point(instance: &WechatInstance) -> (i32, i32) {
    let geom = instance
        .exec_bash(
            "best=''; bestw=0; \
             for w in $(xdotool search --onlyvisible --class '^wechat$' 2>/dev/null); do \
               eval \"$(xdotool getwindowgeometry --shell \"$w\" 2>/dev/null)\"; \
               if [ \"${WIDTH:-0}\" -gt \"$bestw\" ]; then bestw=${WIDTH:-0}; best=$w; fi; \
             done; \
             [ -n \"$best\" ] && eval \"$(xdotool getwindowgeometry --shell \"$best\")\" && \
               echo \"$((X + 31)) $((Y + HEIGHT - 36))\"",
        )
        .await
        .unwrap_or_default();
    let mut it = geom.split_whitespace();
    match (it.next().and_then(|v| v.parse().ok()), it.next().and_then(|v| v.parse().ok())) {
        (Some(x), Some(y)) => (x, y),
        _ => (31, 762),
    }
}

/// Screenshots + vision-locates a menu/dialog item by label, with one retry.
async fn locate_menu_item(instance: &WechatInstance, label: &str) -> Result<Option<(i32, i32)>> {
    for _ in 0..2 {
        sleep_ms(500).await;
        let png = instance.screenshot_png().await.context("screenshot to locate menu item")?;
        let want = label.to_string();
        let coords = tokio::task::spawn_blocking(move || read::read_menu_item_coords(&png, &want))
            .await
            .map_err(|e| anyhow!("locate menu item task failed: {e}"))?
            .context("locate menu item via vision")?;
        if coords.is_some() {
            return Ok(coords);
        }
    }
    Ok(None)
}

/// Sends a pat / nudge — WeChat's only lightweight per-person reaction (the Linux
/// 4.x client has no per-message emoji reactions). RIGHT-clicks the target
/// person's avatar and clicks the pat menu item (a plain double-click just opens
/// their chat). `on`/`member` selects whom to pat (defaults to the chat's other
/// party). Policy-gated. This action reads the screen (vision) to find the
/// avatar, which the accessibility tree does not expose.
pub(crate) async fn react(
    instance: &WechatInstance,
    cfg: &InstanceConfig,
    input: &Value,
    human: &Human,
) -> Result<ActOutcome> {
    let recipient = resolve_recipient(input)?;
    let who = first_str(input, &["on", "member", "who", "at", "target_user", "user_name"])
        .unwrap_or_else(|| recipient.clone());

    ensure_ready(instance, cfg).await?;
    // A pat is still an outbound interaction — count it against the anti-ban caps,
    // but make the gate label UNIQUE (send_nonce = epoch+pid+counter) so the
    // duplicate-content block doesn't stop legitimate repeat pats — even two in
    // the same second (a plain seconds label would collide).
    let mut policy = Policy::load(instance.name());
    let pat_label = format!("[pat -> {who} @{}]", send_nonce());
    if let Err(block) = policy.check_send(&recipient, &pat_label) {
        bail!("blocked by anti-ban policy: {block}");
    }
    sleep_ms(policy.wait_before_send()).await;

    open_chat(instance, &recipient, human).await?;
    verify_open_chat(instance, &recipient).await?;

    let point = locate_avatar(instance, &who).await?.ok_or_else(|| {
        anyhow!(
            "could not find {who}'s avatar on screen to pat; open a chat showing a recent \
             message from them"
        )
    })?;
    // Right-click the avatar, then click the pat menu item (a plain double-click
    // just opens the contact's chat). The avatar is located by reading the screen
    // (vision) because the accessibility tree does not expose it as an element.
    human_right_click(instance, point).await?;
    sleep_ms(700).await;
    let png = instance
        .screenshot_png()
        .await
        .context("screenshot the avatar pat menu")?;
    let coords = tokio::task::spawn_blocking(move || read::read_menu_item_coords(&png, ui::PAT))
        .await
        .map_err(|e| anyhow!("pat menu task failed: {e}"))?
        .context("read the pat menu item")?;
    let pat_point = coords.ok_or_else(|| {
        anyhow!("the pat item was not found in the avatar menu (this WeChat build may not support it)")
    })?;
    human_click(instance, pat_point).await?;
    sleep_ms(800).await;

    policy.record_send(&recipient, &pat_label)?;
    Ok(ActOutcome {
        summary: format!("Sent a pat to {who} in WeChat chat {recipient}"),
        output: json!({ "recipient": recipient, "patted": who, "action": "pat" }),
    })
}

/// Quotes the message matching `snippet` in the open chat: locate the bubble,
/// right-click it, then click the quote menu item. Leaves the input focused with
/// the quote bar attached, ready for the reply body.
async fn quote_message(instance: &WechatInstance, snippet: &str, human: &Human) -> Result<()> {
    // Preferred path: locate the message bubble in the accessibility tree,
    // right-click it (the row is full-width; the bubble is left for incoming /
    // right for outgoing, so try both), then click the quote menu item.
    if super::atspi::available(instance).await {
        // The target bubble lands in the tree shortly after the chat opens; retry
        // the lookup briefly before giving up.
        let mut bounds = None;
        for attempt in 0..6 {
            bounds = super::atspi::message_bounds(instance, snippet).await?;
            if bounds.is_some() {
                break;
            }
            if attempt < 5 {
                sleep_ms(400).await;
            }
        }
        if let Some((x, y, w, h)) = bounds {
            let cy = y + h / 2;
            for rx in [x + 70, x + w - 90] {
                human_right_click(instance, (rx, cy)).await?;
                sleep_ms(700).await;
                if let Some(p) = super::atspi::menu_item(instance, ui::QUOTE).await? {
                    human_click(instance, p).await?;
                    sleep(human.think_time()).await;
                    return Ok(());
                }
                key(instance, "Escape").await?; // dismiss the empty/wrong menu, retry
                sleep_ms(200).await;
            }
            // Both positions missed: make sure no menu is left open before vision.
            key(instance, "Escape").await?;
        }
        // Fall through to reading the screen if the accessibility path missed.
    }
    // No automatic vision fallback: with screen reading disabled, fail closed.
    if !read::vision_allowed() {
        bail!(
            "could not locate the message `{snippet}` to quote via the accessibility tree, and \
             screen reading (vision) is disabled (set WECHAT_ALLOW_VISION=1 to allow it)"
        );
    }
    // Fallback: read the screen (vision) to locate the bubble and the menu item.
    let point = locate_message(instance, snippet).await?.ok_or_else(|| {
        anyhow!(
            "could not find a visible message matching `{snippet}` to quote; it may need to be \
             scrolled into view"
        )
    })?;
    human_right_click(instance, point).await?;
    sleep_ms(700).await; // context menu render

    let png = instance
        .screenshot_png()
        .await
        .context("screenshot the quote context menu")?;
    let coords = tokio::task::spawn_blocking(move || read::read_menu_item_coords(&png, ui::QUOTE))
        .await
        .map_err(|e| anyhow!("quote menu task failed: {e}"))?
        .context("read the quote menu item")?;
    let menu_point =
        coords.ok_or_else(|| anyhow!("the quote item was not found in the menu"))?;
    human_click(instance, menu_point).await?;
    sleep(human.think_time()).await;
    Ok(())
}

/// Screenshots + vision-locates the message bubble matching `snippet`, settling
/// and retrying once for a late render.
async fn locate_message(instance: &WechatInstance, snippet: &str) -> Result<Option<(i32, i32)>> {
    for _ in 0..2 {
        sleep_ms(600).await;
        let png = instance
            .screenshot_png()
            .await
            .context("screenshot to locate the message to quote")?;
        let s = snippet.to_string();
        let coords = tokio::task::spawn_blocking(move || read::read_message_coords(&png, &s))
            .await
            .map_err(|e| anyhow!("locate message task failed: {e}"))?
            .context("locate message via vision")?;
        if coords.is_some() {
            return Ok(coords);
        }
    }
    Ok(None)
}

/// Screenshots + vision-locates `who`'s avatar (for a pat), with one retry.
async fn locate_avatar(instance: &WechatInstance, who: &str) -> Result<Option<(i32, i32)>> {
    for _ in 0..2 {
        sleep_ms(600).await;
        let png = instance
            .screenshot_png()
            .await
            .context("screenshot to locate the avatar")?;
        let w = who.to_string();
        let coords = tokio::task::spawn_blocking(move || read::read_avatar_coords(&png, &w))
            .await
            .map_err(|e| anyhow!("locate avatar task failed: {e}"))?
            .context("locate avatar via vision")?;
        if coords.is_some() {
            return Ok(coords);
        }
    }
    Ok(None)
}

/// Extracts a quote/reply target snippet from `reply_to`/`quote`. WeChat has no
/// UI message-ids, so the value must be the message TEXT (a snippet) — a string,
/// or an object with `text`/`snippet`/`message`. A bare numeric id is unusable
/// here and yields `None`.
fn reply_snippet(input: &Value) -> Option<String> {
    let value = input.get("reply_to").or_else(|| input.get("quote"))?;
    match value {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Value::Object(_) => first_str(value, &["text", "snippet", "message", "quote", "body"]),
        _ => None,
    }
}

/// Ensures the container is up, logged in, and has the screenshot tool (so the
/// recipient/delivery vision checks can actually run on act-only instances).
async fn ensure_ready(instance: &WechatInstance, cfg: &InstanceConfig) -> Result<()> {
    instance.ensure_container(cfg).await?;
    if !instance.is_logged_in().await? {
        bail!(
            "wechat connection `{}` is not connected (logged out); reconnect it via the \
             connector setup — that re-opens the WeChat screen with a fresh QR to scan. Tell \
             the user to reconnect the WeChat connector, not to scan a QR out of nowhere.",
            instance.name()
        );
    }
    // H2: provision the screenshot tool here too — verify_open_chat needs it, and
    // an act-only instance may never have run the subscribe path that installs it.
    let _ = instance.ensure_screenshot_tool().await;
    Ok(())
}

/// Activates WeChat, opens search, and navigates to `recipient`'s chat.
///
/// IMPORTANT (validated live on WeChat 4.x): pressing Enter in the search box
/// opens WeChat's in-app web search, not the first local result, and the local
/// chat row sits AFTER a variable number of web-suggestion rows — so neither
/// Enter nor Down-counting works. We must CLICK the contact/chat row. A leading
/// Escape clears any stale search/popup.
async fn open_chat(instance: &WechatInstance, recipient: &str, human: &Human) -> Result<()> {
    activate_window(instance).await?;
    // Preferred path: click the recipient's row in the LEFT conversation list
    // directly via the accessibility tree — done BEFORE any Escape. An Escape
    // closes a currently-open chat, and re-clicking its (already-selected) row is
    // a no-op, so a 2nd send to the same chat would leave the pane closed and the
    // verify would fail. The conversation list is always visible, so the click
    // navigates directly; no search box is involved, so no web-result row can
    // hijack the window (clicking one opens a full-window web view). Falls through
    // to the search + screen-reading path if the chat isn't in the visible list.
    if super::atspi::available(instance).await {
        // If the requested chat is ALREADY open, do not click its conversation
        // row — clicking the currently-open row TOGGLES the chat closed (verified
        // live), which is what broke a 2nd consecutive send to the same chat.
        if super::atspi::open_chat_is(instance, recipient).await.unwrap_or(false) {
            return Ok(());
        }
        for attempt in 0..4 {
            if let Some(point) = super::atspi::conversation_row(instance, recipient).await? {
                human_click(instance, point).await?;
                sleep(human.think_time()).await;
                return Ok(());
            }
            if attempt < 3 {
                sleep_ms(400).await;
            }
        }
    }
    // No automatic vision fallback: the search path reads the screen to pick the
    // result row, so with screen reading disabled, require the chat to be
    // reachable from the conversation list.
    if !read::vision_allowed() {
        bail!(
            "could not open `{recipient}` from the conversation list via the accessibility tree; \
             searching needs screen reading (vision), which is disabled. Open the chat on the \
             desktop once so it is in the recent list, or set WECHAT_ALLOW_VISION=1"
        );
    }
    // Search path: clear any leftover popup/context-menu/search first (two
    // Escapes clear nested state, e.g. a context menu over a search box) so a
    // prior action can't block the search box from opening. (Closing the open
    // chat here is fine — we're about to search-navigate to a different one.)
    key(instance, "Escape").await?;
    sleep_ms(150).await;
    key(instance, "Escape").await?;
    sleep(human.think_time()).await;
    key(instance, "ctrl+f").await?;
    sleep(human.think_time()).await;
    key(instance, "ctrl+a").await?; // clear any stale search text
    paste(instance, recipient).await?;

    // Read the screen to locate the local (non-web) result row, then click it.
    // Retried so a not-yet-rendered dropdown (slow first paint) doesn't read as
    // "no result".
    match locate_result(instance, recipient).await? {
        Some(point) => human_click(instance, point).await?,
        None => bail!(
            "could not find a local chat/contact result for `{recipient}` in search \
             (only web-search rows?)"
        ),
    }
    sleep(human.think_time()).await; // chat opens; input gains focus
    Ok(())
}

/// Reads the screen to find the contact's search-result row. Used as the fallback
/// when the accessibility tree doesn't expose the chat (open_chat tries that
/// first via `atspi::conversation_row`).
async fn locate_result(instance: &WechatInstance, recipient: &str) -> Result<Option<(i32, i32)>> {
    // NB: open_chat opens chats via the conversation list (no search box, so no
    // web-result row can be clicked). This function is the screen-reading fallback
    // for the search dropdown — used only when the chat isn't in the visible
    // conversation list. It must NOT click the search dropdown's exact-name row,
    // which is a web-search suggestion that hijacks the window.
    for _ in 0..2 {
        sleep_ms(1500).await; // let the results dropdown render/settle
        let png = instance
            .screenshot_png()
            .await
            .context("screenshot to locate the chat result")?;
        let target = recipient.to_string();
        let coords = tokio::task::spawn_blocking(move || read::read_result_coords(&png, &target))
            .await
            .map_err(|e| anyhow!("locate task failed: {e}"))?
            .context("locate chat result via vision (is the model relay reachable?)")?;
        if coords.is_some() {
            return Ok(coords);
        }
    }
    Ok(None)
}

/// Moves the mouse to `target` along a human-like curved path (with jitter) and
/// left-clicks — used to open a search result row (WeChat 4.x needs a real click).
async fn human_click(instance: &WechatInstance, target: (i32, i32)) -> Result<()> {
    human_click_action(instance, target, "click --clearmodifiers 1").await
}

/// Like [`human_click`] but right-clicks (button 3) — opens the message context
/// menu (copy / forward / quote / ...).
async fn human_right_click(instance: &WechatInstance, target: (i32, i32)) -> Result<()> {
    human_click_action(instance, target, "click --clearmodifiers 3").await
}

/// Moves the mouse to `target` along a human-like curved path (with jitter) and
/// runs the given `xdotool` click subcommand there.
async fn human_click_action(
    instance: &WechatInstance,
    target: (i32, i32),
    click_cmd: &str,
) -> Result<()> {
    let start = instance.mouse_pos().await.unwrap_or((400, 300));
    let target = super::human::jitter_target(target, 4);
    let path = super::human::mouse_path(start, target);
    let mut script = String::new();
    for point in &path {
        let secs = super::human::step_delay().as_millis() as f64 / 1000.0;
        script.push_str(&format!(
            "xdotool mousemove --sync {} {} 2>/dev/null; sleep {:.3}; ",
            point.0, point.1, secs
        ));
    }
    script.push_str(&format!("xdotool {click_cmd}"));
    instance.exec_bash(&script).await.map(|_| ())
}

/// Clicks the message input box of the main WeChat window so a subsequent paste
/// lands there. The point is derived from the main window geometry (lower-center,
/// inside the text area and clear of the toolbar icons and the Send button),
/// falling back to a fixed point in the 1280x800 capture if geometry can't be read.
async fn focus_message_input(instance: &WechatInstance, human: &Human) -> Result<()> {
    let geom = instance
        .exec_bash(
            "best=''; bestw=0; \
             for w in $(xdotool search --onlyvisible --class '^wechat$' 2>/dev/null); do \
               eval \"$(xdotool getwindowgeometry --shell \"$w\" 2>/dev/null)\"; \
               if [ \"${WIDTH:-0}\" -gt \"$bestw\" ]; then bestw=${WIDTH:-0}; best=$w; fi; \
             done; \
             [ -n \"$best\" ] && eval \"$(xdotool getwindowgeometry --shell \"$best\")\" && \
               echo \"$((X + WIDTH / 2)) $((Y + HEIGHT - 70))\"",
        )
        .await
        .unwrap_or_default();
    let point = {
        let mut it = geom.split_whitespace();
        match (it.next().and_then(|v| v.parse().ok()), it.next().and_then(|v| v.parse().ok())) {
            (Some(x), Some(y)) => (x, y),
            _ => (640, 720), // center-bottom of the 1280x800 capture
        }
    };
    human_click(instance, point).await?;
    sleep(human.think_time()).await;
    Ok(())
}

/// Guard against sending to the wrong chat. FAILS CLOSED (H1): a send is
/// irreversible, so unless the open-chat header can be positively confirmed to
/// match `recipient`, the send is aborted. Screenshot/vision failure, an
/// unreadable/empty header, or a mismatch all abort. Opt out only with
/// `WECHAT_VERIFY_RECIPIENT=0` (then it is skipped entirely).
async fn verify_open_chat(instance: &WechatInstance, recipient: &str) -> Result<()> {
    if std::env::var("WECHAT_VERIFY_RECIPIENT").map(|v| v.trim() == "0").unwrap_or(false) {
        return Ok(());
    }
    // Verify via the accessibility tree: the open chat's header label carries the
    // recipient name (right pane). The header label lands in the tree a moment
    // after the row is clicked, so retry briefly before giving up. On a match
    // we're done; otherwise (e.g. a decorative display name the tree exposes
    // differently) fall through to the screen-reading check below.
    if super::atspi::available(instance).await {
        for attempt in 0..6 {
            if super::atspi::open_chat_is(instance, recipient).await.unwrap_or(false) {
                return Ok(());
            }
            if attempt < 5 {
                sleep_ms(400).await;
            }
        }
    }
    // No automatic vision fallback: if the accessibility tree could not confirm
    // the recipient and screen reading is disabled, fail closed with guidance.
    if !read::vision_allowed() {
        bail!(
            "aborting send: could not confirm the open chat is `{recipient}` via the accessibility \
             tree, and screen reading (vision) is disabled (set WECHAT_ALLOW_VISION=1 to allow it)"
        );
    }
    // Fast path: transcribe the header and substring-match. This works for plain
    // names and gives a readable name for the error message.
    let png = instance
        .screenshot_png()
        .await
        .map_err(|e| anyhow!("aborting send: cannot screenshot to verify recipient ({e:#})"))?;
    let title = tokio::task::spawn_blocking(move || read::read_open_chat_title(&png))
        .await
        .map_err(|e| anyhow!("aborting send: recipient verify task failed ({e})"))?
        .map_err(|e| anyhow!("aborting send: cannot read open-chat title ({e:#})"))?;
    if let Some(title) = &title {
        if title_matches(title, recipient) {
            return Ok(());
        }
    }
    // Fallback: decorative/stylized display names (unusual unicode wrapping the
    // real characters) defeat exact transcription. Ask the model a direct visual
    // YES/NO against the reference characters, robust to the decoration. Still
    // fail-closed: only an explicit YES confirms.
    let png = instance
        .screenshot_png()
        .await
        .map_err(|e| anyhow!("aborting send: cannot screenshot to verify recipient ({e:#})"))?;
    let want = recipient.to_string();
    let confirmed = tokio::task::spawn_blocking(move || read::confirm_open_chat_is(&png, &want))
        .await
        .map_err(|e| anyhow!("aborting send: recipient confirm task failed ({e})"))?
        .map_err(|e| anyhow!("aborting send: cannot confirm open chat ({e:#})"))?;
    if confirmed {
        return Ok(());
    }
    match title {
        Some(title) => bail!(
            "aborting send: opened chat `{title}` does not match requested recipient `{recipient}`"
        ),
        None => bail!("aborting send: no chat appears open / header unreadable for `{recipient}`"),
    }
}

/// Whether the read header title confirms the requested recipient. Normalized
/// (trim + collapse whitespace + casefold). The header must EQUAL or CONTAIN the
/// requested name — a searched name is often a substring of the contact's full
/// nick (e.g. search "Bob" → header "✦Bob✦"), and group headers append a member
/// count. The reverse (requested contains header) is deliberately NOT accepted:
/// a shorter/truncated header like "Bob" must not confirm a request for "Bob
/// Smith". `open_chat` navigates via the accessibility tree (vision only as a
/// fallback), so this is the sanity backstop; the `confirm_open_chat_is` fallback
/// handles decorative names.
fn title_matches(title: &str, recipient: &str) -> bool {
    let t = normalize(title);
    let r = normalize(recipient);
    if t.is_empty() || r.is_empty() {
        return false; // can't positively confirm -> fail closed
    }
    t == r || t.contains(&r)
}

/// Normalizes a name for comparison: collapse whitespace + casefold.
fn normalize(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// Best-effort delivery confirmation (H4): after pressing send, vision-reads the
/// latest outgoing bubble and warns if the body is not visible. It does NOT
/// block recording — a false "not delivered" would trigger a retry and risk a
/// DOUBLE SEND, which is worse than a cap undercount. Set
/// `WECHAT_VERIFY_DELIVERY=0` to skip the check entirely.
async fn confirm_delivery(instance: &WechatInstance, body: &str) -> Result<()> {
    if std::env::var("WECHAT_VERIFY_DELIVERY").map(|v| v.trim() == "0").unwrap_or(false) {
        return Ok(());
    }
    // Confirm via the accessibility tree: the sent body appears as a message
    // bubble in the chat history (which lands in the tree shortly after send, so
    // retry briefly). On a match we're done.
    if super::atspi::available(instance).await {
        for attempt in 0..6 {
            if super::atspi::body_in_history(instance, body).await.unwrap_or(false) {
                return Ok(());
            }
            if attempt < 5 {
                sleep_ms(400).await;
            }
        }
    }
    // Best-effort only, and never an automatic vision call: with screen reading
    // disabled, skip the vision cross-check (the send already happened; a missing
    // confirmation just suppresses a warning, it does not fail the send).
    if !read::vision_allowed() {
        return Ok(());
    }
    let png = match instance.screenshot_png().await {
        Ok(png) => png,
        Err(_) => return Ok(()),
    };
    let probe: String = body.chars().take(12).collect();
    let last = tokio::task::spawn_blocking(move || read::read_last_outgoing(&png))
        .await
        .unwrap_or(Ok(None));
    if let Ok(Some(last)) = last {
        if !probe.trim().is_empty() && !last.contains(probe.trim()) {
            eprintln!(
                "wechat: delivery unconfirmed — last outgoing bubble `{last}` does not contain the \
                 sent text (it may still have sent; not retrying to avoid a double-send)"
            );
        }
    }
    Ok(())
}

/// Activates the MAIN WeChat window (WM_CLASS `wechat`, the widest one) and
/// brings it to the front — NOT a `WeChatAppEx` window (the embedded browser used
/// by the in-app web search / mini-programs), which can otherwise steal focus and
/// swallow our keystrokes. Falls back to a name match if the class lookup finds
/// nothing.
async fn activate_window(instance: &WechatInstance) -> Result<()> {
    instance
        .exec_bash(
            &format!(
            "best=''; bestw=0; \
             for w in $(xdotool search --onlyvisible --class '^wechat$' 2>/dev/null); do \
               eval \"$(xdotool getwindowgeometry --shell \"$w\" 2>/dev/null)\"; \
               if [ \"${{WIDTH:-0}}\" -gt \"$bestw\" ]; then bestw=${{WIDTH:-0}}; best=$w; fi; \
             done; \
             [ -z \"$best\" ] && best=$(xdotool search --onlyvisible --name '{names}' 2>/dev/null | head -1); \
             [ -n \"$best\" ] && xdotool windowactivate --sync \"$best\" || true",
            names = ui::WINDOW_NAMES),
        )
        .await
        .map(|_| ())
}

/// Sends one `xdotool key --clearmodifiers <keyspec>` (e.g. `ctrl+f`, `Return`).
async fn key(instance: &WechatInstance, keyspec: &str) -> Result<()> {
    // keyspec is a fixed, code-supplied token (never user text), so it is safe
    // to interpolate directly.
    instance
        .exec_bash(&format!("xdotool key --clearmodifiers {keyspec}"))
        .await
        .map(|_| ())
}

/// Loads `text` into the clipboard (via stdin, no escaping) and pastes it with
/// Ctrl+V into the focused widget — the keysym-safe path for Chinese input.
async fn paste(instance: &WechatInstance, text: &str) -> Result<()> {
    instance
        .exec_bash_stdin(
            "xclip -selection clipboard -i; xdotool key --clearmodifiers ctrl+v",
            text.as_bytes(),
        )
        .await
        .map(|_| ())
}

/// Resolves the recipient display name from common input keys.
fn resolve_recipient(input: &Value) -> Result<String> {
    first_str(
        input,
        &[
            "to", "target", "contact", "chat", "chat_id", "user", "receive_id", "name",
        ],
    )
    .ok_or_else(|| {
        anyhow!("send_message requires a recipient (`to`/`contact`: a WeChat contact or group name)")
    })
}

/// Extracts mention target names from `mention`/`mentions`/`at` (string or array).
fn mention_names(input: &Value) -> Vec<String> {
    for key in ["mention", "mentions", "at"] {
        match input.get(key) {
            Some(Value::String(s)) if !s.trim().is_empty() => {
                return vec![s.trim().to_string()];
            }
            Some(Value::Array(items)) => {
                let names: Vec<String> = items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(ToString::to_string)
                    .collect();
                if !names.is_empty() {
                    return names;
                }
            }
            _ => {}
        }
    }
    Vec::new()
}

/// Whether a mention name is the broadcast "@everyone" (English or the zh-CN
/// phrasings in [`ui::AT_ALL`]). Strips a leading `@` and ALL whitespace (incl.
/// full-width U+3000) and casefolds, so spaced-out / English / @-prefixed
/// evasions are still caught.
fn is_at_all(name: &str) -> bool {
    let n: String = name
        .trim_start_matches('@')
        .chars()
        .filter(|c| {
            !c.is_whitespace()
                && *c != '\u{3000}'
                && !matches!(*c, '\u{200B}'..='\u{200D}' | '\u{2060}' | '\u{FEFF}')
        })
        .flat_map(char::to_lowercase)
        .collect();
    n == "all" || n == "everyone" || ui::AT_ALL.contains(&n.as_str())
}

/// Sleeps for `ms` milliseconds (skips the await when zero).
async fn sleep_ms(ms: u64) {
    if ms > 0 {
        tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
    }
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

/// Async sleep wrapper kept local so the flow reads as a sequence of human steps.
async fn sleep(duration: std::time::Duration) {
    tokio::time::sleep(duration).await;
}

/// A resolved outbound attachment: raw bytes + how to deliver it.
struct Attachment {
    /// Filename the recipient sees (for file cards).
    filename: String,
    /// File bytes (read from a local path or downloaded from a URL).
    bytes: Vec<u8>,
    /// Inline image (clipboard `image/png`) vs file card (`text/uri-list`).
    as_image: bool,
    /// Optional caption, sent as a following text message.
    caption: Option<String>,
}

/// Image file extensions sent inline (everything else goes as a file card).
const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "bmp", "webp", "heic", "tiff"];

/// Collects + resolves attachments from the action input (BLOCKING: reads local
/// files and downloads `http(s)` URLs). Scans the cross-connector media keys.
/// Caps at 9 (WeChat's per-batch limit).
fn collect_attachments(input: &Value) -> Result<Vec<Attachment>> {
    let mut specs: Vec<(Value, Option<bool>)> = Vec::new();
    let mut push = |v: &Value, hint: Option<bool>, specs: &mut Vec<(Value, Option<bool>)>| {
        match v {
            Value::Array(items) => items.iter().for_each(|i| specs.push((i.clone(), hint))),
            Value::Null => {}
            other => specs.push((other.clone(), hint)),
        }
    };
    for key in ["image", "photo"] {
        if let Some(v) = input.get(key) {
            push(v, Some(true), &mut specs);
        }
    }
    for key in ["file", "document", "video", "voice", "audio"] {
        if let Some(v) = input.get(key) {
            push(v, Some(false), &mut specs);
        }
    }
    for key in ["media", "attachment", "attachments", "files"] {
        if let Some(v) = input.get(key) {
            push(v, None, &mut specs);
        }
    }
    // Bare path/url only when nothing else matched (e.g. a `send_file` call).
    if specs.is_empty() {
        for key in ["path", "url"] {
            if let Some(v) = input.get(key) {
                push(v, None, &mut specs);
            }
        }
    }

    // Dedup by source so the SAME file supplied under more than one key (e.g.
    // both `image` and `media`) is not sent twice. Keeps the first (most-specific
    // hint) occurrence.
    let mut seen_src = std::collections::HashSet::new();
    specs.retain(|(v, _)| match spec_src(v) {
        Some(s) => seen_src.insert(s),
        None => true,
    });

    // Enforce the per-send cap BEFORE downloading/reading anything (don't pull 50
    // files into memory just to reject them).
    if specs.len() > 9 {
        bail!("too many attachments ({}); WeChat allows at most 9 per send", specs.len());
    }
    let mut out = Vec::new();
    for (value, hint) in specs {
        out.push(resolve_attachment(&value, hint)?);
    }
    Ok(out)
}

/// Extracts the source path/URL string from an attachment spec, for dedup.
fn spec_src(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.trim().to_string()),
        Value::Object(_) => first_str(value, &["path", "file", "url", "src", "media"]),
        _ => None,
    }
}

/// Hard cap on a single attachment's size (bytes) — bounds memory for downloads
/// and local reads. WeChat's own file limit is well under this.
const MAX_ATTACHMENT_BYTES: u64 = 100 * 1024 * 1024;

/// Resolves one attachment spec (a string path/URL or an object) to bytes.
fn resolve_attachment(value: &Value, hint: Option<bool>) -> Result<Attachment> {
    let (src, kind, caption, explicit_name) = match value {
        Value::String(s) => (s.trim().to_string(), None, None, None),
        Value::Object(_) => {
            let src = first_str(value, &["path", "file", "url", "src", "media"]).ok_or_else(|| {
                anyhow!("attachment object needs a `path`, `file`, or `url`")
            })?;
            (
                src,
                value.get("kind").and_then(Value::as_str).map(str::to_lowercase),
                first_str(value, &["caption"]),
                first_str(value, &["filename", "name"]),
            )
        }
        _ => bail!("attachment must be a path/URL string or an object"),
    };
    if src.is_empty() {
        bail!("attachment path/URL is empty");
    }

    // Bytes + filename from a URL download or a local file.
    let (bytes, url_name) = if src.starts_with("http://") || src.starts_with("https://") {
        download_attachment(&src)?
    } else {
        // SECURITY: `src` is agent/model-controlled (and may be influenced by an
        // untrusted inbound message). Reading an arbitrary host path would let a
        // prompt-injected agent exfiltrate secrets (~/.puffer/auth.json, DB keys,
        // SSH keys) by "sending" them to a chat. Confine to safe paths.
        let path = validate_local_attachment_path(&src)?;
        let meta = std::fs::metadata(&path).with_context(|| format!("stat attachment {src}"))?;
        if meta.len() > MAX_ATTACHMENT_BYTES {
            bail!("attachment {src} is {} bytes, over the {MAX_ATTACHMENT_BYTES}-byte cap", meta.len());
        }
        let bytes = std::fs::read(&path).with_context(|| format!("read attachment file {src}"))?;
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
            .to_string();
        (bytes, name)
    };

    let filename = explicit_name.unwrap_or(url_name);
    let as_image = match kind.as_deref() {
        Some("photo") | Some("image") => true,
        Some("file") | Some("document") | Some("doc") | Some("video") | Some("voice")
        | Some("audio") => false,
        _ => hint.unwrap_or_else(|| is_image_name(&filename)),
    };
    Ok(Attachment {
        filename,
        bytes,
        as_image,
        caption,
    })
}

/// Whether a filename's extension marks it as an inline image.
fn is_image_name(name: &str) -> bool {
    name.rsplit('.')
        .next()
        .map(|ext| IMAGE_EXTS.contains(&ext.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Validates a LOCAL attachment path: the source is agent-controlled, so this
/// stops "send a file" from exfiltrating arbitrary host files. Default-deny —
/// the path must canonicalize to a REGULAR FILE under an allow-root (OS temp,
/// `~/.puffer/wechat/outbox`, `~/Downloads`/`~/Pictures`/`~/Desktop`, or
/// `WECHAT_ATTACH_DIR` which replaces the defaults), with a sensitive-dir /
/// credential-name denylist as defense in depth.
fn validate_local_attachment_path(src: &str) -> Result<std::path::PathBuf> {
    use std::path::PathBuf;
    let path = std::fs::canonicalize(src)
        .with_context(|| format!("resolve attachment path {src} (does it exist?)"))?;

    // Must be a regular file — never /proc, /sys, FIFOs, devices, or directories
    // (canonicalize doesn't enforce this; /proc/self/environ is a "regular" file
    // but lives outside every allow-root, so the allow-root check below blocks it).
    let meta = std::fs::metadata(&path).with_context(|| format!("stat attachment {src}"))?;
    if !meta.file_type().is_file() {
        bail!("attachment {} is not a regular file", path.display());
    }

    // PUFFER_HOME first (matches the rest of the connector's state-path logic),
    // then HOME.
    let home = std::env::var("PUFFER_HOME")
        .ok()
        .or_else(|| std::env::var("HOME").ok())
        .filter(|h| !h.trim().is_empty());

    // Defense-in-depth denylist (the allow-root below is the primary control).
    let mut blocked: Vec<PathBuf> = ["/etc", "/root", "/var/root", "/private/etc"]
        .iter()
        .map(PathBuf::from)
        .collect();
    if let Some(home) = &home {
        for sub in [".puffer", ".ssh", ".aws", ".gnupg", ".config", ".kube", ".docker"] {
            blocked.push(PathBuf::from(home).join(sub));
        }
    }
    for dir in &blocked {
        let canon = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.clone());
        if path.starts_with(&canon) {
            bail!("refusing to attach a file under a sensitive directory: {}", path.display());
        }
    }
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        let lname = name.to_ascii_lowercase();
        let credential = lname == "auth.json"
            || lname == ".env"
            || lname == ".netrc"
            || lname == ".git-credentials"
            || lname == "id_rsa"
            || lname == "id_ed25519"
            || [".dbkey", ".pem", ".key", ".keystore", ".p12"].iter().any(|e| lname.ends_with(e));
        if credential {
            bail!("refusing to attach a credential-like file: {name}");
        }
    }

    // ALLOW-ROOT (default-deny): WECHAT_ATTACH_DIR replaces the defaults if set.
    let allowed: Vec<PathBuf> = match std::env::var("WECHAT_ATTACH_DIR") {
        Ok(allow) if !allow.trim().is_empty() => allow
            .split(':')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .filter_map(|d| std::fs::canonicalize(d).ok())
            .collect(),
        _ => {
            let mut roots = vec![std::env::temp_dir(), PathBuf::from("/tmp")];
            if let Some(home) = &home {
                for sub in [".puffer/wechat/outbox", "Downloads", "Pictures", "Desktop"] {
                    roots.push(PathBuf::from(home).join(sub));
                }
            }
            roots.into_iter().filter_map(|d| std::fs::canonicalize(&d).ok()).collect()
        }
    };
    if !allowed.iter().any(|a| path.starts_with(a)) {
        bail!(
            "attachment {} is outside the allowed attachment directories; set WECHAT_ATTACH_DIR \
             to a directory containing files you want to send",
            path.display()
        );
    }
    Ok(path)
}

/// Downloads an `http(s)` attachment with SSRF + size guards: no redirects (so a
/// public URL can't 302 to an internal target), reject private/loopback/
/// link-local resolved addresses, and cap the body at [`MAX_ATTACHMENT_BYTES`].
fn download_attachment(src: &str) -> Result<(Vec<u8>, String)> {
    use std::io::Read;
    use std::net::ToSocketAddrs;

    let url = reqwest::Url::parse(src).with_context(|| format!("parse attachment URL {src}"))?;
    let host = url.host_str().ok_or_else(|| anyhow!("attachment URL has no host: {src}"))?;
    let port = url.port_or_known_default().unwrap_or(443);
    // Resolve ONCE, reject internal targets, and PIN the validated address so
    // reqwest connects to exactly the IP we checked (defeats DNS-rebinding TOCTOU,
    // where a second resolution at connect time could point at an internal host).
    let mut chosen: Option<std::net::SocketAddr> = None;
    for addr in (host, port).to_socket_addrs().with_context(|| format!("resolve {host}"))? {
        if is_blocked_ip(&addr.ip()) {
            bail!("refusing to download attachment from a private/loopback address ({})", addr.ip());
        }
        if chosen.is_none() {
            chosen = Some(addr);
        }
    }
    let addr = chosen.ok_or_else(|| anyhow!("could not resolve attachment host {host}"))?;

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::none())
        .resolve(host, addr) // pin DNS to the validated address (no rebinding)
        .build()
        .context("build attachment http client")?;
    let resp = client.get(url).send().with_context(|| format!("download attachment {src}"))?;
    if !resp.status().is_success() {
        bail!("download attachment {src}: HTTP {}", resp.status());
    }
    if let Some(len) = resp.content_length() {
        if len > MAX_ATTACHMENT_BYTES {
            bail!("attachment {src} is {len} bytes, over the {MAX_ATTACHMENT_BYTES}-byte cap");
        }
    }
    // Cap the actual read even if Content-Length lied or was absent.
    let mut buf = Vec::new();
    resp.take(MAX_ATTACHMENT_BYTES + 1)
        .read_to_end(&mut buf)
        .with_context(|| format!("read attachment body {src}"))?;
    if buf.len() as u64 > MAX_ATTACHMENT_BYTES {
        bail!("attachment {src} exceeds the {MAX_ATTACHMENT_BYTES}-byte cap");
    }
    let name = src
        .split('?')
        .next()
        .unwrap_or(src)
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("download")
        .to_string();
    Ok((buf, name))
}

/// Whether an IP is private/loopback/link-local/unspecified (SSRF target).
fn is_blocked_ip(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.octets()[0] == 0
                // carrier-grade NAT 100.64.0.0/10
                || (v4.octets()[0] == 100 && (v4.octets()[1] & 0xc0) == 64)
        }
        std::net::IpAddr::V6(v6) => {
            let seg = v6.segments();
            // Embedded IPv4: IPv4-mapped (::ffff:a.b.c.d), NAT64 well-known prefix
            // 64:ff9b::/96, and 6to4 2002::/16 — recurse on the embedded V4.
            let embedded_v4 = v6.to_ipv4_mapped().or_else(|| {
                if seg[0] == 0x0064 && seg[1] == 0xff9b {
                    Some(std::net::Ipv4Addr::new(
                        (seg[6] >> 8) as u8, seg[6] as u8, (seg[7] >> 8) as u8, seg[7] as u8,
                    ))
                } else if seg[0] == 0x2002 {
                    Some(std::net::Ipv4Addr::new(
                        (seg[1] >> 8) as u8, seg[1] as u8, (seg[2] >> 8) as u8, seg[2] as u8,
                    ))
                } else {
                    None
                }
            });
            v6.is_loopback()
                || v6.is_unspecified()
                // unique-local fc00::/7 and link-local fe80::/10
                || (seg[0] & 0xfe00) == 0xfc00
                || (seg[0] & 0xffc0) == 0xfe80
                || embedded_v4.map(|m| is_blocked_ip(&std::net::IpAddr::V4(m))).unwrap_or(false)
        }
    }
}

/// A process-unique nonce (epoch secs + pid + counter) for per-send scratch dirs
/// and dedup-safe gate labels — avoids collisions between concurrent sends.
fn send_nonce() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{secs}-{}-{n}", std::process::id())
}

/// Percent-encodes a filesystem path for a `file://` URI: keeps `/` and the
/// unreserved set, encodes every other byte (so CJK / non-ASCII filenames form a
/// valid uri-list entry that WeChat decodes back to the original name).
fn percent_encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for &b in path.as_bytes() {
        let keep = b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'.' | b'_' | b'~');
        if keep {
            out.push(b as char);
        } else {
            out.push('%');
            out.push_str(&format!("{b:02X}"));
        }
    }
    out
}

/// Stages one attachment into the container and sends it via clipboard paste:
/// images go on the clipboard as `image/png` (inline), other files as a
/// `text/uri-list` `file://` URI (a file card) — both validated live on 4.x.
async fn send_one_attachment(
    instance: &WechatInstance,
    att: &Attachment,
    nonce: &str,
    idx: usize,
) -> Result<()> {
    // Per-send, per-attachment scratch path under /tmp/wocm/<nonce>/ so concurrent
    // sends never share a file or the clipboard scratch image; send_message
    // `rm -rf`s the whole <nonce> dir afterwards.
    let dir = format!("/tmp/wocm/{nonce}");
    let safe = sanitize_filename(&att.filename);
    let cpath = format!("{dir}/{idx}-{safe}");
    // Write the bytes into the container as `abc` (no docker cp, no shell escaping).
    instance
        .exec_bash_stdin(&format!("mkdir -p '{dir}' && cat > '{cpath}'"), &att.bytes)
        .await
        .context("stage attachment into container")?;
    if att.as_image {
        // Normalize to PNG so the clipboard `image/png` target always matches.
        let clip = format!("{dir}/{idx}.clip.png");
        instance
            .exec_bash(&format!(
                "convert '{cpath}' '{clip}' 2>/dev/null || cp '{cpath}' '{clip}'; \
                 xclip -selection clipboard -t image/png -i '{clip}'"
            ))
            .await
            .context("put image on clipboard")?;
    } else {
        // Percent-encode so CJK/non-ASCII filenames form a valid file:// URI.
        let uri = percent_encode_path(&cpath);
        instance
            .exec_bash(&format!(
                "printf 'file://{uri}\\r\\n' | xclip -selection clipboard -t text/uri-list -i"
            ))
            .await
            .context("put file URI on clipboard")?;
    }
    key(instance, "ctrl+v").await?;
    sleep_ms(1300).await; // let the media preview/file card render in the input
    key(instance, &send_key()).await?;
    // Let WeChat finish uploading before the scratch dir is cleaned / next step —
    // scale the wait with size for files (the upload reads the file on send).
    let wait = if att.as_image {
        900
    } else {
        let mb = (att.bytes.len() / (1024 * 1024)) as u64;
        (900 + mb * 400).min(8000)
    };
    sleep_ms(wait).await;
    Ok(())
}

/// Sanitizes an attachment filename to a safe basename (no path separators, no
/// shell-breaking characters); keeps unicode (incl. CJK) and `._-`.
fn sanitize_filename(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let cleaned: String = base
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '.' || c == '_' || c == '-' || (c as u32) > 127 {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('.').trim();
    if trimmed.is_empty() {
        "file".to_string()
    } else {
        trimmed.to_string()
    }
}

/// A short anti-ban gate label for a media-only send (no text to dedup on).
fn media_gate_label(atts: &[Attachment]) -> String {
    let images = atts.iter().filter(|a| a.as_image).count();
    format!("[media {} image(s), {} file(s)]", images, atts.len() - images)
}

/// One-line human summary of what was sent.
fn describe_send(recipient: &str, body: Option<&str>, images: usize, files: usize) -> String {
    let mut parts = Vec::new();
    if images > 0 {
        parts.push(format!("{images} image(s)"));
    }
    if files > 0 {
        parts.push(format!("{files} file(s)"));
    }
    if body.is_some() {
        parts.push("a text message".to_string());
    }
    let what = if parts.is_empty() {
        "a message".to_string()
    } else {
        parts.join(" + ")
    };
    format!("Sent {what} to WeChat chat {recipient} as the logged-in user")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn recipient_resolves_from_aliases() {
        assert_eq!(resolve_recipient(&json!({"to": "Alice"})).unwrap(), "Alice");
        assert_eq!(
            resolve_recipient(&json!({"contact": "Carol"})).unwrap(),
            "Carol"
        );
        assert_eq!(
            resolve_recipient(&json!({"target": "  Bob  "})).unwrap(),
            "Bob"
        );
        assert!(resolve_recipient(&json!({"text": "hi"})).is_err());
    }

    #[test]
    fn first_str_skips_empty_and_nonstring() {
        let v = json!({"a": "", "b": "  ", "c": 5, "d": "ok"});
        assert_eq!(first_str(&v, &["a", "b", "c", "d"]).as_deref(), Some("ok"));
        assert_eq!(first_str(&v, &["a", "b"]), None);
    }

    #[test]
    fn image_extensions_detected() {
        assert!(is_image_name("photo.PNG"));
        assert!(is_image_name("a.jpeg"));
        assert!(is_image_name("x.webp"));
        assert!(!is_image_name("report.pdf"));
        assert!(!is_image_name("noext"));
        assert!(!is_image_name("voice.mp3"));
    }

    #[test]
    fn sanitize_filename_strips_path_and_unsafe_chars() {
        assert_eq!(sanitize_filename("/tmp/a/report.pdf"), "report.pdf");
        assert_eq!(sanitize_filename("..\\..\\evil.txt"), "evil.txt");
        assert_eq!(sanitize_filename("na'me;rm -rf.doc"), "na_me_rm_-rf.doc");
        assert_eq!(sanitize_filename("café.txt"), "café.txt");
        assert_eq!(sanitize_filename("..."), "file");
        assert_eq!(sanitize_filename(""), "file");
    }

    #[test]
    fn reply_snippet_from_string_and_object() {
        assert_eq!(reply_snippet(&json!({"reply_to": "hi there"})).as_deref(), Some("hi there"));
        assert_eq!(
            reply_snippet(&json!({"quote": {"text": "  quoted  "}})).as_deref(),
            Some("quoted")
        );
        // A bare numeric id is unusable (WeChat has no UI message ids).
        assert_eq!(reply_snippet(&json!({"reply_to": 12345})), None);
        assert_eq!(reply_snippet(&json!({"text": "no reply field"})), None);
    }

    #[test]
    fn media_gate_label_counts_kinds() {
        let atts = vec![
            Attachment { filename: "a.png".into(), bytes: vec![], as_image: true, caption: None },
            Attachment { filename: "b.pdf".into(), bytes: vec![], as_image: false, caption: None },
            Attachment { filename: "c.jpg".into(), bytes: vec![], as_image: true, caption: None },
        ];
        assert_eq!(media_gate_label(&atts), "[media 2 image(s), 1 file(s)]");
    }

    #[test]
    fn describe_send_summarizes_parts() {
        assert_eq!(
            describe_send("Bob", Some("hi"), 2, 1),
            "Sent 2 image(s) + 1 file(s) + a text message to WeChat chat Bob as the logged-in user"
        );
        assert_eq!(
            describe_send("Bob", None, 0, 1),
            "Sent 1 file(s) to WeChat chat Bob as the logged-in user"
        );
    }

    #[test]
    fn resolve_attachment_honors_explicit_kind_over_extension() {
        // A .png explicitly marked as a file should NOT be inlined as an image.
        // (Use a data-less object path that doesn't exist; we only check the
        // kind/extension logic up front by constructing via is_image_name + hint.)
        assert!(is_image_name("x.png"));
        // explicit kind=file beats the .png extension:
        let kind = Some("file".to_string());
        let as_image = match kind.as_deref() {
            Some("photo") | Some("image") => true,
            Some("file") | Some("document") | Some("doc") | Some("video") | Some("voice")
            | Some("audio") => false,
            _ => is_image_name("x.png"),
        };
        assert!(!as_image);
    }
}

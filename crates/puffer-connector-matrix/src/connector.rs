use crate::handler::{handle_command, PLATFORM_ID};
use crate::MatrixConfig;
use anyhow::{Context, Result};
use matrix_sdk::config::SyncSettings;
use matrix_sdk::ruma::events::room::message::{
    MessageType, OriginalSyncRoomMessageEvent, RoomMessageEventContent,
};
use matrix_sdk::{Client, Room, RoomState};
use puffer_connector_core::{
    CommandOutcome, Connector, ConnectorHandle, ConnectorRuntime, ConnectorStartError,
    InboundMessage, MessageSplitter,
};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;

/// Matrix connector ready to be started by the puffer connector hub.
pub struct MatrixConnector {
    config: MatrixConfig,
}

impl MatrixConnector {
    pub fn new(config: MatrixConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &MatrixConfig {
        &self.config
    }
}

impl Connector for MatrixConnector {
    fn id(&self) -> &str {
        PLATFORM_ID
    }

    fn start(
        self: Box<Self>,
        runtime: Arc<ConnectorRuntime>,
    ) -> Result<ConnectorHandle, ConnectorStartError> {
        if self.config.homeserver_url.trim().is_empty() {
            return Err(ConnectorStartError::MissingConfig {
                id: PLATFORM_ID.to_string(),
                detail: "homeserver_url is empty".to_string(),
            });
        }
        if self.config.username.trim().is_empty() {
            return Err(ConnectorStartError::MissingConfig {
                id: PLATFORM_ID.to_string(),
                detail: "username is empty".to_string(),
            });
        }
        if self.config.password.is_empty() {
            return Err(ConnectorStartError::MissingConfig {
                id: PLATFORM_ID.to_string(),
                detail: "password is empty".to_string(),
            });
        }

        // One-shot channel that fires a cooperative shutdown. The worker
        // tokio runtime picks it up and drops the sync task.
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        // Signal back from the worker thread once the Tokio runtime is
        // ready to be interrupted.
        let (ready_tx, ready_rx) = mpsc::channel::<()>();

        let config = self.config.clone();
        let join = std::thread::Builder::new()
            .name("puffer-connector-matrix".to_string())
            .spawn(move || -> Result<()> {
                let tokio_runtime = tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .context("failed to build tokio runtime for matrix connector")?;
                let _ = ready_tx.send(());
                tokio_runtime.block_on(run_client(config, runtime, shutdown_rx))
            })
            .map_err(|error| ConnectorStartError::other(PLATFORM_ID, error.into()))?;

        ready_rx
            .recv()
            .map_err(|error| ConnectorStartError::other(PLATFORM_ID, error.into()))?;

        let shutdown: Box<dyn FnOnce() + Send> = Box::new(move || {
            let _ = shutdown_tx.send(());
        });

        Ok(ConnectorHandle {
            id: PLATFORM_ID.to_string(),
            shutdown,
            join,
        })
    }
}

async fn run_client(
    config: MatrixConfig,
    runtime: Arc<ConnectorRuntime>,
    shutdown_rx: oneshot::Receiver<()>,
) -> Result<()> {
    let client = Client::builder()
        .homeserver_url(&config.homeserver_url)
        .build()
        .await
        .context("failed to build matrix client")?;

    client
        .matrix_auth()
        .login_username(&config.username, &config.password)
        .send()
        .await
        .context("failed to log in to matrix homeserver")?;

    // An initial sync populates the client store before we start
    // dispatching messages — without it, `get_room` returns None for
    // rooms the bot is in.
    client
        .sync_once(SyncSettings::default())
        .await
        .context("initial matrix sync failed")?;

    // Stash the config + runtime so the event handler can read them by
    // `Ctx` lookup rather than capturing them in the closure (which would
    // force a `Send + Sync + 'static` clone on every call).
    client.add_event_handler_context(HandlerCtx {
        config: Arc::new(config.clone()),
        runtime: runtime.clone(),
    });
    client.add_event_handler(on_room_message);

    // Drive the long-poll sync loop in a background task so we can race
    // it against the shutdown signal.
    let sync_client = client.clone();
    let sync_settings = SyncSettings::default();
    let sync_handle = tokio::spawn(async move {
        // `sync` only returns on error; we treat a clean shutdown as Ok.
        let _ = sync_client.sync(sync_settings).await;
    });

    let _ = shutdown_rx.await;
    sync_handle.abort();
    // `abort` drops the future, which in turn drops the sync stream. We
    // still await the handle so the thread's JoinHandle reflects the
    // final state accurately.
    let _ = sync_handle.await;
    Ok(())
}

/// Context passed into matrix-sdk event handlers via
/// [`Client::add_event_handler_context`].
#[derive(Clone)]
struct HandlerCtx {
    config: Arc<MatrixConfig>,
    runtime: Arc<ConnectorRuntime>,
}

async fn on_room_message(
    event: OriginalSyncRoomMessageEvent,
    room: Room,
    client: Client,
    ctx: matrix_sdk::event_handler::Ctx<HandlerCtx>,
) {
    // Only react inside rooms the bot has actually joined. Invites and
    // left rooms should stay silent until someone accepts on our behalf.
    if room.state() != RoomState::Joined {
        return;
    }

    let raw_text = match &event.content.msgtype {
        MessageType::Text(content) => content.body.clone(),
        _ => return,
    };

    let me = client.user_id().map(|id| id.to_owned());

    // Short-circuit on bot-self messages before building an
    // InboundMessage — saves a spawn_blocking round-trip and prevents
    // any accidental self-loop if the handler filter is ever removed.
    if let Some(bot_id) = me.as_ref() {
        if event.sender == *bot_id {
            return;
        }
    }

    let from_bot = false; // already early-returned above

    // Heuristic: treat rooms with more than 2 active members as groups.
    // `active_members_count` returns joined + invited members; for a DM
    // this is 2 (bot + one user). Default to `true` on the edge case
    // where the store hasn't populated yet so we favor mention-gating.
    let active_count = room.active_members_count();
    let is_group = active_count == 0 || active_count > 2;

    // Mention detection + stripping:
    //  * full MXID substring (`@bot:example.org`) in the body, or
    //  * localpart substring (`bot`) in the body — covers the common
    //    `@bot` display-name mention Matrix clients produce, and
    //  * in DMs the bot is implicitly always "mentioned".
    //
    // TODO(connector-matrix): parse `event.content.mentions` /
    // `m.relates_to` for precise m.in_reply_to detection; the current
    // substring check is good enough for v1 and never wrongly ignores
    // in DMs thanks to the `!is_group` fallback.
    let (bot_mentioned, cleaned_text) = if let Some(bot_id) = me.as_ref() {
        let mxid = bot_id.as_str();
        let localpart = bot_id.localpart();
        let body_lower = raw_text.to_ascii_lowercase();
        let mentioned = !is_group
            || body_lower.contains(&mxid.to_ascii_lowercase())
            || body_lower.contains(&localpart.to_ascii_lowercase());
        let cleaned = strip_matrix_mentions(&raw_text, mxid, localpart);
        (mentioned, cleaned)
    } else {
        (!is_group, raw_text.trim().to_string())
    };

    // TODO(connector-matrix): extract a stable thread id from
    // `event.content.relates_to` once we decide how to map Matrix
    // threads onto Puffer sessions. For now every message in a room
    // belongs to the same session regardless of thread.
    let thread_id: Option<String> = None;

    let inbound = InboundMessage {
        conversation_id: room.room_id().to_string(),
        user_id: Some(event.sender.to_string()),
        text: cleaned_text,
        thread_id,
        is_group,
        bot_mentioned,
        from_bot,
    };

    let runtime = ctx.runtime.clone();
    let config = ctx.config.clone();

    // Dispatch blocks on the shared runtime mutex; park it on a blocking
    // worker so we don't starve the tokio reactor.
    let outcome = tokio::task::spawn_blocking(move || {
        handle_command(&runtime, &inbound, &config)
    })
    .await;

    let reply = match outcome {
        Ok(Ok(CommandOutcome::Ignored)) => return,
        Ok(Ok(CommandOutcome::Reply(text))) => text,
        Ok(Ok(CommandOutcome::AgentReply { text, .. })) => text,
        Ok(Err(error)) => format!("Puffer error: {error}"),
        Err(join_error) => format!("Puffer error: {join_error}"),
    };

    let _ = send_reply_chunks(&room, &reply).await;
}

async fn send_reply_chunks(room: &Room, body: &str) -> Result<(), matrix_sdk::Error> {
    for chunk in MessageSplitter::MATRIX.split(body) {
        send_with_retry(room, &chunk).await?;
    }
    Ok(())
}

/// Sends a single chunk with exponential backoff. Retries up to
/// `MAX_ATTEMPTS` times on transient errors before giving up. Matches
/// Hermes's `_send_with_retry` behavior in spirit.
async fn send_with_retry(room: &Room, chunk: &str) -> Result<(), matrix_sdk::Error> {
    const MAX_ATTEMPTS: u32 = 3;
    let mut attempt = 0u32;
    loop {
        let content = RoomMessageEventContent::text_plain(chunk);
        match room.send(content).await {
            Ok(_) => return Ok(()),
            Err(error) => {
                attempt += 1;
                if attempt >= MAX_ATTEMPTS {
                    return Err(error);
                }
                // Exponential backoff with a tiny jitter budget.
                let delay_ms = 250u64 * (1u64 << (attempt - 1));
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
        }
    }
}

/// Strips the bot's MXID and bare localpart from `body`, case-insensitive.
///
/// Matrix clients produce mentions either as the full `@bot:example.org`
/// form or as the bare `@bot` localpart. Both must be removed so the
/// agent sees clean prose instead of "@bot please fix the bug".
///
/// The search is case-insensitive but we preserve the original text
/// casing for anything that survives stripping.
pub(crate) fn strip_matrix_mentions(body: &str, mxid: &str, localpart: &str) -> String {
    // Build patterns once, in decreasing specificity so we never strip a
    // substring that belongs to something else.
    let patterns = [
        mxid.to_string(),           // "@bot:example.org"
        format!("@{localpart}"),    // "@bot"
    ];

    let mut out = body.to_string();
    for pattern in patterns {
        if pattern.is_empty() {
            continue;
        }
        out = case_insensitive_replace(&out, &pattern, "");
    }
    // Collapse any doubled whitespace the replacement introduced.
    let collapsed = out
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    collapsed.trim().to_string()
}

fn case_insensitive_replace(haystack: &str, needle: &str, replacement: &str) -> String {
    if needle.is_empty() {
        return haystack.to_string();
    }
    let lower_haystack = haystack.to_ascii_lowercase();
    let lower_needle = needle.to_ascii_lowercase();
    let mut out = String::with_capacity(haystack.len());
    let mut cursor = 0usize;
    while let Some(relative) = lower_haystack[cursor..].find(&lower_needle) {
        let absolute = cursor + relative;
        out.push_str(&haystack[cursor..absolute]);
        out.push_str(replacement);
        cursor = absolute + needle.len();
    }
    out.push_str(&haystack[cursor..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_removes_full_mxid() {
        let cleaned = strip_matrix_mentions(
            "@bot:example.org fix the bug",
            "@bot:example.org",
            "bot",
        );
        assert_eq!(cleaned, "fix the bug");
    }

    #[test]
    fn strip_removes_bare_localpart_mention() {
        let cleaned = strip_matrix_mentions("@bot please help", "@bot:example.org", "bot");
        assert_eq!(cleaned, "please help");
    }

    #[test]
    fn strip_handles_mixed_case() {
        let cleaned = strip_matrix_mentions("@Bot please HELP", "@bot:example.org", "bot");
        assert_eq!(cleaned, "please HELP");
    }

    #[test]
    fn strip_is_noop_when_no_mention() {
        let cleaned = strip_matrix_mentions("fix the bug", "@bot:example.org", "bot");
        assert_eq!(cleaned, "fix the bug");
    }

    #[test]
    fn strip_collapses_doubled_whitespace() {
        let cleaned = strip_matrix_mentions(
            "hey   @bot   please help",
            "@bot:example.org",
            "bot",
        );
        assert_eq!(cleaned, "hey please help");
    }

    #[test]
    fn strip_removes_multiple_mentions() {
        let cleaned = strip_matrix_mentions(
            "@bot @bot:example.org please @Bot help",
            "@bot:example.org",
            "bot",
        );
        assert_eq!(cleaned, "please help");
    }
}

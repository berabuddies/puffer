//! Shared slash-command handling for platform connectors.
//!
//! Every connector exposes the same small set of built-in commands so
//! users get a consistent experience across platforms. Each platform
//! calls [`handle_builtin_command`] first; anything unhandled falls
//! through to [`ConnectorRuntime::dispatch`].

use crate::{ConnectorRuntime, ConversationKey};
use puffer_session_store::TranscriptEvent;
use uuid::Uuid;

/// Outcome of a single inbound message, suitable for passing directly
/// to the platform's send path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandOutcome {
    /// Reply with this static text; the agent was not consulted.
    Reply(String),
    /// The agent produced this reply text.
    AgentReply {
        session_id: Uuid,
        created: bool,
        text: String,
    },
    /// The inbound user is not permitted, message is a bot echo, or the
    /// bot was not mentioned in a mention-required group. Silently ignore.
    Ignored,
}

/// Configuration shared by every built-in command handler.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BuiltinCommandConfig {
    /// Override for the `/start` greeting. `None` uses a built-in default.
    pub welcome_message: Option<String>,
}

/// Handles the cross-platform slash commands. Returns `Some(outcome)`
/// when the command was handled locally; `None` when the message
/// should be forwarded to the agent.
///
/// Recognized commands (case-insensitive):
/// * `/start` — show the welcome banner
/// * `/new`, `/reset` — detach the stored session so the next message
///   creates a fresh one
/// * `/help` — list available commands
/// * `/status` — summarize the active session (event count, created at)
/// * `/usage` — placeholder stub; returns a brief note. Real token
///   usage tracking is deferred to v2 (requires plumbing per-turn
///   usage reports into the session store).
///
/// For unknown slash commands this helper returns `None`, matching the
/// CLI convention where unknown `/foo` input is forwarded to the agent
/// verbatim.
pub fn handle_builtin_command(
    runtime: &ConnectorRuntime,
    key: &ConversationKey,
    raw: &str,
    config: &BuiltinCommandConfig,
) -> anyhow::Result<Option<CommandOutcome>> {
    let trimmed = raw.trim();
    let Some(rest) = trimmed.strip_prefix('/') else {
        return Ok(None);
    };
    let (name, _args) = rest
        .split_once(char::is_whitespace)
        .map(|(n, a)| (n, a))
        .unwrap_or((rest, ""));

    match name.to_ascii_lowercase().as_str() {
        "start" => Ok(Some(CommandOutcome::Reply(
            config
                .welcome_message
                .clone()
                .unwrap_or_else(default_welcome),
        ))),
        "new" | "reset" => {
            runtime.reset_conversation(key)?;
            Ok(Some(CommandOutcome::Reply(
                "Started a fresh Puffer session.".to_string(),
            )))
        }
        "help" => Ok(Some(CommandOutcome::Reply(help_text()))),
        "status" => Ok(Some(CommandOutcome::Reply(status_text(runtime, key)?))),
        "usage" => Ok(Some(CommandOutcome::Reply(usage_text(runtime, key)?))),
        _ => Ok(None),
    }
}

fn default_welcome() -> String {
    "Puffer is online. Send any message to talk to the agent, or /help \
     for commands."
        .to_string()
}

fn help_text() -> String {
    "/start  — greeting\n\
     /new    — start a fresh session for this chat\n\
     /reset  — alias for /new\n\
     /status — show the active session id and turn count\n\
     /usage  — usage summary (placeholder — full token usage is TBD)\n\
     /help   — show this message\n\
     any other text — forwarded to the Puffer agent"
        .to_string()
}

fn status_text(runtime: &ConnectorRuntime, key: &ConversationKey) -> anyhow::Result<String> {
    let Some(record) = runtime.session_record(key)? else {
        return Ok(
            "No active session yet. Send any message to create one."
                .to_string(),
        );
    };
    let counts = count_turns(&record.events);
    Ok(format!(
        "Session `{session}`\n\
         Created: {created}\n\
         Turns — user: {user}, assistant: {assistant}, tool: {tool}\n\
         Working directory: {cwd}",
        session = short_uuid(record.metadata.id),
        created = format_unix_ms(record.metadata.created_at_ms),
        user = counts.user,
        assistant = counts.assistant,
        tool = counts.tool,
        cwd = record.metadata.cwd.display(),
    ))
}

fn usage_text(runtime: &ConnectorRuntime, key: &ConversationKey) -> anyhow::Result<String> {
    // TODO(connector-v2): plumb TurnUsageReport through the session
    // store so we can surface real token counts here. For now we report
    // the event-count proxy and be honest about the gap.
    let Some(record) = runtime.session_record(key)? else {
        return Ok("No active session yet.".to_string());
    };
    let counts = count_turns(&record.events);
    Ok(format!(
        "Usage for session `{session}` (token counts coming in v2):\n\
         — user turns: {user}\n\
         — assistant turns: {assistant}\n\
         — tool invocations: {tool}",
        session = short_uuid(record.metadata.id),
        user = counts.user,
        assistant = counts.assistant,
        tool = counts.tool,
    ))
}

#[derive(Default)]
struct TurnCounts {
    user: usize,
    assistant: usize,
    tool: usize,
}

fn count_turns(events: &[TranscriptEvent]) -> TurnCounts {
    let mut counts = TurnCounts::default();
    for event in events {
        match event {
            TranscriptEvent::UserMessage { .. } => counts.user += 1,
            TranscriptEvent::AssistantMessage { .. } => counts.assistant += 1,
            TranscriptEvent::ToolInvocation { .. } => counts.tool += 1,
            _ => {}
        }
    }
    counts
}

fn short_uuid(id: Uuid) -> String {
    id.as_simple().to_string()[..8].to_string()
}

fn format_unix_ms(ms: u64) -> String {
    if ms == 0 {
        return "unknown".to_string();
    }
    let secs = ms / 1000;
    // Cheap ISO-8601-ish formatting without a chrono dep — good enough
    // for /status output. Users wanting precise timezones will read
    // `puffer sessions list`.
    let days_since_epoch = secs / 86_400;
    let remainder = secs % 86_400;
    let hours = remainder / 3600;
    let minutes = (remainder % 3600) / 60;
    let seconds = remainder % 60;
    format!(
        "epoch+{days_since_epoch}d {hours:02}:{minutes:02}:{seconds:02}",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ConnectorRuntimeConfig, ConversationSessionMap,
    };
    use puffer_config::{ConfigPaths, PufferConfig};
    use puffer_provider_registry::{AuthStore, ProviderRegistry};
    use puffer_resources::LoadedResources;
    use puffer_session_store::SessionStore;
    use tempfile::tempdir;

    fn test_runtime(root: std::path::PathBuf) -> ConnectorRuntime {
        let paths = ConfigPaths {
            workspace_root: root.clone(),
            workspace_config_dir: root.join(".puffer"),
            user_config_dir: root.join(".puffer-user"),
            builtin_resources_dir: root.join("resources"),
        };
        std::fs::create_dir_all(&paths.user_config_dir).unwrap();
        ConnectorRuntime::new(ConnectorRuntimeConfig {
            config: PufferConfig::default(),
            resources: LoadedResources::default(),
            providers: ProviderRegistry::default(),
            auth_store: AuthStore::default(),
            auth_path: root.join("auth.json"),
            session_store: SessionStore::from_paths(&paths).unwrap(),
            session_map: ConversationSessionMap::in_memory(),
            default_cwd: root,
        })
    }

    #[test]
    fn non_command_returns_none_so_caller_dispatches_to_agent() {
        let runtime = test_runtime(tempdir().unwrap().path().to_path_buf());
        let key = ConversationKey::new("test", "c1");
        let outcome = handle_builtin_command(
            &runtime,
            &key,
            "hello world",
            &BuiltinCommandConfig::default(),
        )
        .unwrap();
        assert!(outcome.is_none());
    }

    #[test]
    fn unknown_slash_command_returns_none_for_agent_fallback() {
        let runtime = test_runtime(tempdir().unwrap().path().to_path_buf());
        let key = ConversationKey::new("test", "c1");
        let outcome = handle_builtin_command(
            &runtime,
            &key,
            "/mystery arg",
            &BuiltinCommandConfig::default(),
        )
        .unwrap();
        assert!(outcome.is_none());
    }

    #[test]
    fn start_returns_welcome_using_config_override() {
        let runtime = test_runtime(tempdir().unwrap().path().to_path_buf());
        let key = ConversationKey::new("test", "c1");
        let outcome = handle_builtin_command(
            &runtime,
            &key,
            "/start",
            &BuiltinCommandConfig {
                welcome_message: Some("custom welcome".to_string()),
            },
        )
        .unwrap();
        assert_eq!(
            outcome,
            Some(CommandOutcome::Reply("custom welcome".to_string()))
        );
    }

    #[test]
    fn help_lists_the_core_commands() {
        let runtime = test_runtime(tempdir().unwrap().path().to_path_buf());
        let key = ConversationKey::new("test", "c1");
        let Some(CommandOutcome::Reply(text)) = handle_builtin_command(
            &runtime,
            &key,
            "/help",
            &BuiltinCommandConfig::default(),
        )
        .unwrap() else {
            panic!("help should return a reply");
        };
        for command in ["/start", "/new", "/help", "/status", "/usage"] {
            assert!(text.contains(command), "help missing `{command}` in:\n{text}");
        }
    }

    #[test]
    fn status_reports_no_session_before_dispatch() {
        let runtime = test_runtime(tempdir().unwrap().path().to_path_buf());
        let key = ConversationKey::new("test", "c1");
        let Some(CommandOutcome::Reply(text)) = handle_builtin_command(
            &runtime,
            &key,
            "/status",
            &BuiltinCommandConfig::default(),
        )
        .unwrap() else {
            panic!("status should return a reply");
        };
        assert!(text.contains("No active session"));
    }

    #[test]
    fn new_resets_the_stored_session_for_the_key() {
        let runtime = test_runtime(tempdir().unwrap().path().to_path_buf());
        let key = ConversationKey::new("test", "c1");
        runtime.reset_conversation(&key).unwrap(); // seed as no-op
        let outcome = handle_builtin_command(
            &runtime,
            &key,
            "/new some args",
            &BuiltinCommandConfig::default(),
        )
        .unwrap();
        assert!(matches!(outcome, Some(CommandOutcome::Reply(_))));
        assert!(runtime.session_for(&key).unwrap().is_none());
    }

    #[test]
    fn reset_is_an_alias_for_new() {
        let runtime = test_runtime(tempdir().unwrap().path().to_path_buf());
        let key = ConversationKey::new("test", "c1");
        let outcome = handle_builtin_command(
            &runtime,
            &key,
            "/reset",
            &BuiltinCommandConfig::default(),
        )
        .unwrap();
        assert!(matches!(outcome, Some(CommandOutcome::Reply(_))));
    }

    #[test]
    fn usage_reports_no_session_before_dispatch() {
        let runtime = test_runtime(tempdir().unwrap().path().to_path_buf());
        let key = ConversationKey::new("test", "c1");
        let Some(CommandOutcome::Reply(text)) = handle_builtin_command(
            &runtime,
            &key,
            "/usage",
            &BuiltinCommandConfig::default(),
        )
        .unwrap() else {
            panic!("usage should return a reply");
        };
        assert!(text.contains("No active session"));
    }
}

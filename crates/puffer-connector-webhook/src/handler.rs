use puffer_connector_core::{
    handle_builtin_command, BuiltinCommandConfig, CommandOutcome, ConnectorRuntime,
    ConversationKey, GroupKeyPolicy, InboundMessage,
};
use std::sync::Arc;

/// Stable platform id stored in the conversation map.
pub const PLATFORM_ID: &str = "webhook";

/// Dispatches one inbound webhook message. Pure logic — no axum types
/// here — so the full decision tree is unit-testable against an
/// in-memory runtime.
///
/// Webhook is a single-session-per-conversation platform with no group
/// semantics: every call is treated as a DM where the bot was mentioned.
/// Auth is enforced by the router before this runs.
pub fn handle_command(
    runtime: &Arc<ConnectorRuntime>,
    message: &InboundMessage,
    config: &crate::WebhookConfig,
) -> anyhow::Result<CommandOutcome> {
    if message.from_bot {
        return Ok(CommandOutcome::Ignored);
    }

    let trimmed_id = message.conversation_id.trim();
    if trimmed_id.is_empty() {
        return Ok(CommandOutcome::Ignored);
    }

    let trimmed = message.text.trim();
    if trimmed.is_empty() {
        return Ok(CommandOutcome::Ignored);
    }

    // Webhook is always single-session-per-conversation regardless of
    // user — `PerChat` with `is_group=false` collapses cleanly.
    let key = ConversationKey::for_policy(
        PLATFORM_ID,
        trimmed_id,
        message.user_id.as_deref(),
        GroupKeyPolicy::PerChat,
        false,
    );

    let builtin_config = BuiltinCommandConfig {
        welcome_message: config.welcome_message.clone(),
    };
    if let Some(outcome) = handle_builtin_command(runtime, &key, trimmed, &builtin_config)? {
        return Ok(outcome);
    }

    let outcome = runtime.dispatch(&key, trimmed)?;
    Ok(CommandOutcome::AgentReply {
        session_id: outcome.session_id,
        created: outcome.created,
        text: outcome.assistant_text,
    })
}

/// Parses `"Bearer <token>"` (case-insensitive on the scheme) into its
/// token component. Returns `None` for any other shape.
pub(crate) fn extract_bearer(header: &str) -> Option<String> {
    let trimmed = header.trim();
    let (scheme, rest) = trimmed.split_once(char::is_whitespace)?;
    if !scheme.eq_ignore_ascii_case("Bearer") {
        return None;
    }
    let token = rest.trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_config::{ConfigPaths, PufferConfig};
    use puffer_connector_core::{
        ConnectorRuntime, ConnectorRuntimeConfig, ConversationSessionMap,
    };
    use puffer_provider_registry::{AuthStore, ProviderRegistry};
    use puffer_resources::LoadedResources;
    use puffer_session_store::SessionStore;
    use std::sync::Arc;
    use tempfile::tempdir;

    fn test_runtime(root: std::path::PathBuf) -> Arc<ConnectorRuntime> {
        let paths = ConfigPaths {
            workspace_root: root.clone(),
            workspace_config_dir: root.join(".puffer"),
            user_config_dir: root.join(".puffer-user"),
            builtin_resources_dir: root.join("resources"),
        };
        std::fs::create_dir_all(&paths.user_config_dir).unwrap();
        Arc::new(ConnectorRuntime::new(ConnectorRuntimeConfig {
            config: PufferConfig::default(),
            resources: LoadedResources::default(),
            providers: ProviderRegistry::default(),
            auth_store: AuthStore::default(),
            auth_path: root.join("auth.json"),
            session_store: SessionStore::from_paths(&paths).unwrap(),
            session_map: ConversationSessionMap::in_memory(),
            default_cwd: root,
        }))
    }

    fn open_config() -> crate::WebhookConfig {
        crate::WebhookConfig {
            bind_address: "127.0.0.1:0".to_string(),
            path: None,
            auth_token: None,
            allowed_origins: Vec::new(),
            welcome_message: None,
            split_long_responses: true,
        }
    }

    fn msg(text: &str, conversation_id: &str, user: Option<&str>) -> InboundMessage {
        InboundMessage {
            conversation_id: conversation_id.to_string(),
            user_id: user.map(String::from),
            text: text.to_string(),
            thread_id: None,
            is_group: false,
            bot_mentioned: true,
            from_bot: false,
        }
    }

    #[test]
    fn empty_message_is_ignored() {
        let runtime = test_runtime(tempdir().unwrap().path().to_path_buf());
        let outcome =
            handle_command(&runtime, &msg("   ", "conv-1", None), &open_config()).unwrap();
        assert_eq!(outcome, CommandOutcome::Ignored);
    }

    #[test]
    fn empty_conversation_id_is_ignored() {
        let runtime = test_runtime(tempdir().unwrap().path().to_path_buf());
        let outcome =
            handle_command(&runtime, &msg("hi", "   ", None), &open_config()).unwrap();
        assert_eq!(outcome, CommandOutcome::Ignored);
    }

    #[test]
    fn start_returns_welcome_without_touching_the_agent() {
        let runtime = test_runtime(tempdir().unwrap().path().to_path_buf());
        let mut config = open_config();
        config.welcome_message = Some("welcome!".to_string());
        let outcome =
            handle_command(&runtime, &msg("/start", "conv-1", None), &config).unwrap();
        assert_eq!(outcome, CommandOutcome::Reply("welcome!".to_string()));
    }

    #[test]
    fn help_command_is_handled_locally() {
        let runtime = test_runtime(tempdir().unwrap().path().to_path_buf());
        let outcome =
            handle_command(&runtime, &msg("/help", "conv-1", None), &open_config()).unwrap();
        match outcome {
            CommandOutcome::Reply(text) => {
                assert!(text.contains("/new"));
                assert!(text.contains("/help"));
            }
            other => panic!("expected Reply, got {other:?}"),
        }
    }

    #[test]
    fn new_command_detaches_the_existing_session() {
        let runtime = test_runtime(tempdir().unwrap().path().to_path_buf());
        let key = ConversationKey::new(PLATFORM_ID, "caller-xyz");
        runtime.reset_conversation(&key).unwrap();
        let outcome = handle_command(
            &runtime,
            &msg("/new", "caller-xyz", None),
            &open_config(),
        )
        .unwrap();
        assert!(matches!(outcome, CommandOutcome::Reply(_)));
        assert!(runtime.session_for(&key).unwrap().is_none());
    }

    #[test]
    fn status_command_is_handled_locally() {
        let runtime = test_runtime(tempdir().unwrap().path().to_path_buf());
        let outcome =
            handle_command(&runtime, &msg("/status", "conv-1", None), &open_config()).unwrap();
        match outcome {
            CommandOutcome::Reply(text) => assert!(text.contains("No active session")),
            other => panic!("expected Reply, got {other:?}"),
        }
    }

    #[test]
    fn bot_self_messages_are_ignored_to_prevent_loops() {
        let runtime = test_runtime(tempdir().unwrap().path().to_path_buf());
        let mut message = msg("hello", "conv-1", None);
        message.from_bot = true;
        let outcome = handle_command(&runtime, &message, &open_config()).unwrap();
        assert_eq!(outcome, CommandOutcome::Ignored);
    }

    #[test]
    fn unknown_command_falls_through_to_agent_dispatch() {
        // `/mystery` is not a builtin command. The builtin handler must
        // return `None` so the message is forwarded to the agent
        // runtime. There's no real agent wired up in the test runtime,
        // so the best we can do is assert the handler doesn't short-
        // circuit into a `Reply` — it must attempt dispatch, which
        // either succeeds or surfaces a dispatch error.
        let runtime = test_runtime(tempdir().unwrap().path().to_path_buf());
        let result = handle_command(
            &runtime,
            &msg("/mystery arg", "conv-1", None),
            &open_config(),
        );
        match result {
            Ok(CommandOutcome::AgentReply { .. }) => { /* agent was reached */ }
            Ok(CommandOutcome::Reply(text)) => {
                panic!("unknown command must not produce a local Reply, got: {text}")
            }
            Ok(CommandOutcome::Ignored) => {
                panic!("unknown command must not be silently ignored")
            }
            Err(_) => {
                // Expected in the test runtime: dispatch fails because
                // no agent provider is configured. The important bit is
                // that builtin dispatch did NOT intercept the command.
            }
        }
    }

    #[test]
    fn extract_bearer_parses_well_formed_header() {
        assert_eq!(extract_bearer("Bearer abc"), Some("abc".to_string()));
        assert_eq!(extract_bearer("bearer abc"), Some("abc".to_string()));
        assert_eq!(extract_bearer("BEARER  token  "), Some("token".to_string()));
        assert_eq!(extract_bearer("Basic abc"), None);
        assert_eq!(extract_bearer("Bearer "), None);
    }
}

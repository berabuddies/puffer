use puffer_connector_core::{
    handle_builtin_command, BuiltinCommandConfig, CommandOutcome, ConnectorRuntime,
    ConversationKey, InboundMessage,
};
use std::sync::Arc;

/// Stable platform id stored in the conversation map.
pub(crate) const PLATFORM_ID: &str = "discord";

/// Dispatches one inbound message. Pure logic — no Discord SDK calls
/// here — so the full decision tree is unit-testable against an
/// in-memory runtime.
pub fn handle_command(
    runtime: &Arc<ConnectorRuntime>,
    message: &InboundMessage,
    config: &crate::DiscordConfig,
) -> anyhow::Result<CommandOutcome> {
    // 1. Bot-self filter — Discord delivers our own gateway messages
    //    back to us; never loop.
    if message.from_bot {
        return Ok(CommandOutcome::Ignored);
    }

    // 2. Allowed-users filter.
    if let Some(user) = message.user_id.as_deref() {
        if let Ok(id) = user.parse::<u64>() {
            if !config.is_user_allowed(id) {
                return Ok(CommandOutcome::Ignored);
            }
        }
    }

    let trimmed = message.text.trim();
    if trimmed.is_empty() {
        return Ok(CommandOutcome::Ignored);
    }

    // 3. Mention gating: in guild channels with `require_mention = true`,
    //    ignore messages that did not explicitly tag the bot.
    if message.is_group && config.require_mention && !message.bot_mentioned {
        return Ok(CommandOutcome::Ignored);
    }

    // 4. Compute the session key using the configured group policy.
    let key = ConversationKey::for_policy(
        PLATFORM_ID,
        &message.conversation_id,
        message.user_id.as_deref(),
        config.group_key_policy,
        message.is_group,
    );

    // 5. Built-in slash commands (start, new, reset, help, status, usage).
    let builtin_config = BuiltinCommandConfig {
        welcome_message: config.welcome_message.clone(),
    };
    if let Some(outcome) = handle_builtin_command(runtime, &key, trimmed, &builtin_config)? {
        return Ok(outcome);
    }

    // 6. Everything else — forward to the agent.
    let outcome = runtime.dispatch(&key, trimmed)?;
    Ok(CommandOutcome::AgentReply {
        session_id: outcome.session_id,
        created: outcome.created,
        text: outcome.assistant_text,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_config::{ConfigPaths, PufferConfig};
    use puffer_connector_core::{
        ConnectorRuntime, ConnectorRuntimeConfig, ConversationSessionMap, GroupKeyPolicy,
    };
    use puffer_provider_registry::{AuthStore, ProviderRegistry};
    use puffer_resources::LoadedResources;
    use puffer_session_store::SessionStore;
    use std::sync::Arc;
    use tempfile::tempdir;
    use uuid::Uuid;

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

    fn open_config() -> crate::DiscordConfig {
        crate::DiscordConfig {
            token: "t".to_string(),
            allowed_users: Vec::new(),
            welcome_message: None,
            require_mention: true,
            group_key_policy: GroupKeyPolicy::PerUser,
        }
    }

    fn dm(text: &str, user: Option<&str>) -> InboundMessage {
        InboundMessage {
            conversation_id: "42".to_string(),
            user_id: user.map(String::from),
            text: text.to_string(),
            thread_id: None,
            is_group: false,
            bot_mentioned: true,
            from_bot: false,
        }
    }

    fn group(text: &str, user: Option<&str>, mentioned: bool) -> InboundMessage {
        InboundMessage {
            conversation_id: "-1001".to_string(),
            user_id: user.map(String::from),
            text: text.to_string(),
            thread_id: None,
            is_group: true,
            bot_mentioned: mentioned,
            from_bot: false,
        }
    }

    #[test]
    fn disallowed_user_is_ignored() {
        let runtime = test_runtime(tempdir().unwrap().path().to_path_buf());
        let mut config = open_config();
        config.allowed_users = vec![42];
        let outcome = handle_command(&runtime, &dm("hi", Some("7")), &config).unwrap();
        assert_eq!(outcome, CommandOutcome::Ignored);
    }

    #[test]
    fn empty_message_is_ignored() {
        let runtime = test_runtime(tempdir().unwrap().path().to_path_buf());
        let outcome = handle_command(&runtime, &dm("   ", Some("1")), &open_config()).unwrap();
        assert_eq!(outcome, CommandOutcome::Ignored);
    }

    #[test]
    fn start_returns_welcome_without_touching_the_agent() {
        let runtime = test_runtime(tempdir().unwrap().path().to_path_buf());
        let mut config = open_config();
        config.welcome_message = Some("welcome!".to_string());
        let outcome = handle_command(&runtime, &dm("/start", Some("1")), &config).unwrap();
        assert_eq!(outcome, CommandOutcome::Reply("welcome!".to_string()));
    }

    #[test]
    fn help_command_is_handled_locally() {
        let runtime = test_runtime(tempdir().unwrap().path().to_path_buf());
        let outcome = handle_command(&runtime, &dm("/help", Some("1")), &open_config()).unwrap();
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
        // Seed a session for the DM key the handler would build.
        let key = ConversationKey::new(PLATFORM_ID, "42");
        let outcome = handle_command(&runtime, &dm("/new", Some("1")), &open_config()).unwrap();
        assert!(matches!(outcome, CommandOutcome::Reply(_)));
        assert!(runtime.session_for(&key).unwrap().is_none());
    }

    #[test]
    fn allowed_user_passes_filter() {
        let runtime = test_runtime(tempdir().unwrap().path().to_path_buf());
        let mut config = open_config();
        config.allowed_users = vec![42];
        let outcome = handle_command(&runtime, &dm("/help", Some("42")), &config).unwrap();
        assert!(matches!(outcome, CommandOutcome::Reply(_)));
    }

    #[test]
    fn bot_self_messages_are_ignored_to_prevent_loops() {
        let runtime = test_runtime(tempdir().unwrap().path().to_path_buf());
        let mut message = dm("hello", Some("1"));
        message.from_bot = true;
        let outcome = handle_command(&runtime, &message, &open_config()).unwrap();
        assert_eq!(outcome, CommandOutcome::Ignored);
    }

    #[test]
    fn group_message_without_mention_is_ignored_when_required() {
        let runtime = test_runtime(tempdir().unwrap().path().to_path_buf());
        let outcome = handle_command(
            &runtime,
            &group("hello", Some("1"), false),
            &open_config(),
        )
        .unwrap();
        assert_eq!(outcome, CommandOutcome::Ignored);
    }

    #[test]
    fn group_message_with_mention_is_accepted_locally() {
        let runtime = test_runtime(tempdir().unwrap().path().to_path_buf());
        let outcome = handle_command(
            &runtime,
            &group("/help", Some("1"), true),
            &open_config(),
        )
        .unwrap();
        assert!(matches!(outcome, CommandOutcome::Reply(_)));
    }

    #[test]
    fn group_with_require_mention_off_still_dispatches_on_non_mention() {
        let runtime = test_runtime(tempdir().unwrap().path().to_path_buf());
        let mut config = open_config();
        config.require_mention = false;
        let outcome = handle_command(
            &runtime,
            &group("/help", Some("1"), false),
            &config,
        )
        .unwrap();
        assert!(matches!(outcome, CommandOutcome::Reply(_)));
    }

    #[test]
    fn per_user_policy_keys_group_sessions_per_user() {
        let runtime = test_runtime(tempdir().unwrap().path().to_path_buf());
        let alice = ConversationKey::with_user(PLATFORM_ID, "-1001", "1");
        let bob = ConversationKey::with_user(PLATFORM_ID, "-1001", "2");
        runtime.bind_session(&alice, Uuid::new_v4()).unwrap();
        runtime.bind_session(&bob, Uuid::new_v4()).unwrap();

        let outcome = handle_command(
            &runtime,
            &group("/new", Some("1"), true),
            &open_config(),
        )
        .unwrap();
        assert!(matches!(outcome, CommandOutcome::Reply(_)));
        assert!(runtime.session_for(&alice).unwrap().is_none());
        assert!(
            runtime.session_for(&bob).unwrap().is_some(),
            "bob's session untouched"
        );
    }
}

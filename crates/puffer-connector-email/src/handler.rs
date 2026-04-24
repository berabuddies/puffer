use puffer_connector_core::{
    handle_builtin_command, BuiltinCommandConfig, CommandOutcome, ConnectorRuntime,
    ConversationKey, InboundMessage,
};
use std::sync::Arc;

/// Stable platform id stored in the conversation map.
pub(crate) const PLATFORM_ID: &str = "email";

/// Dispatches one inbound email. Pure logic — no IMAP or SMTP calls here
/// — so the full decision tree is unit-testable against an in-memory
/// runtime.
///
/// Filter order matches the other connectors: bot-self, allowed-senders,
/// empty text, built-in slash commands, agent dispatch.
pub fn handle_command(
    runtime: &Arc<ConnectorRuntime>,
    message: &InboundMessage,
    config: &crate::EmailConfig,
) -> anyhow::Result<CommandOutcome> {
    // 1. Bot-self filter — shouldn't happen on email since we never
    //    receive our own outbound mail, but cheap to check so we don't
    //    accidentally loop on edge cases like mail-list echoes.
    if message.from_bot {
        return Ok(CommandOutcome::Ignored);
    }

    // 2. Allowed-senders filter (case-insensitive on the raw sender
    //    address carried in `InboundMessage.user_id`).
    if let Some(sender) = message.user_id.as_deref() {
        if !config.is_sender_allowed(sender) {
            return Ok(CommandOutcome::Ignored);
        }
    }

    let trimmed = message.text.trim();
    if trimmed.is_empty() {
        return Ok(CommandOutcome::Ignored);
    }

    // 3. Compute the session key. Email is always DM-like so
    //    `is_group = false`; the policy therefore collapses to keying
    //    by `(platform, conversation_id)`.
    let key = ConversationKey::for_policy(
        PLATFORM_ID,
        &message.conversation_id,
        message.user_id.as_deref(),
        config.group_key_policy,
        false,
    );

    // 4. Built-in slash commands (start, new, reset, help, status, usage).
    let builtin_config = BuiltinCommandConfig {
        welcome_message: config.welcome_message.clone(),
    };
    if let Some(outcome) = handle_builtin_command(runtime, &key, trimmed, &builtin_config)? {
        return Ok(outcome);
    }

    // 5. Everything else — forward to the agent.
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

    fn open_config() -> crate::EmailConfig {
        crate::EmailConfig {
            imap_host: "imap.example.com".to_string(),
            imap_port: 993,
            smtp_host: "smtp.example.com".to_string(),
            smtp_port: 465,
            username: "bot@example.com".to_string(),
            password: "secret".to_string(),
            from_address: "bot@example.com".to_string(),
            allowed_senders: Vec::new(),
            welcome_message: None,
            poll_interval_secs: None,
            require_mention: false,
            group_key_policy: GroupKeyPolicy::PerUser,
        }
    }

    fn email(
        thread: &str,
        sender: Option<&str>,
        text: &str,
    ) -> InboundMessage {
        InboundMessage {
            conversation_id: thread.to_string(),
            user_id: sender.map(String::from),
            text: text.to_string(),
            thread_id: None,
            is_group: false,
            bot_mentioned: true,
            from_bot: false,
        }
    }

    #[test]
    fn disallowed_sender_is_ignored() {
        let runtime = test_runtime(tempdir().unwrap().path().to_path_buf());
        let mut config = open_config();
        config.allowed_senders = vec!["allowed@example.com".to_string()];
        let outcome = handle_command(
            &runtime,
            &email("thread-1", Some("stranger@example.com"), "body"),
            &config,
        )
        .unwrap();
        assert_eq!(outcome, CommandOutcome::Ignored);
    }

    #[test]
    fn empty_body_and_subject_is_ignored() {
        let runtime = test_runtime(tempdir().unwrap().path().to_path_buf());
        let outcome = handle_command(
            &runtime,
            &email("thread-1", Some("user@example.com"), "   "),
            &open_config(),
        )
        .unwrap();
        assert_eq!(outcome, CommandOutcome::Ignored);
    }

    #[test]
    fn start_returns_welcome_without_touching_the_agent() {
        let runtime = test_runtime(tempdir().unwrap().path().to_path_buf());
        let mut config = open_config();
        config.welcome_message = Some("welcome!".to_string());
        let outcome = handle_command(
            &runtime,
            &email("thread-1", Some("user@example.com"), "/start"),
            &config,
        )
        .unwrap();
        assert_eq!(outcome, CommandOutcome::Reply("welcome!".to_string()));
    }

    #[test]
    fn help_command_is_handled_locally() {
        let runtime = test_runtime(tempdir().unwrap().path().to_path_buf());
        let outcome = handle_command(
            &runtime,
            &email("thread-1", Some("user@example.com"), "/help"),
            &open_config(),
        )
        .unwrap();
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
        let thread_id = "msg-id-555";
        let key = ConversationKey::new(PLATFORM_ID, thread_id);
        // Seed the map so /new has something to remove.
        runtime.reset_conversation(&key).unwrap();
        let outcome = handle_command(
            &runtime,
            &email(thread_id, Some("user@example.com"), "/new"),
            &open_config(),
        )
        .unwrap();
        assert!(matches!(outcome, CommandOutcome::Reply(_)));
        assert!(runtime.session_for(&key).unwrap().is_none());
    }

    #[test]
    fn allowed_sender_passes_filter() {
        let runtime = test_runtime(tempdir().unwrap().path().to_path_buf());
        let mut config = open_config();
        config.allowed_senders = vec!["Alice@Example.com".to_string()];
        // /help avoids needing a real provider.
        let outcome = handle_command(
            &runtime,
            &email("thread-1", Some("alice@example.com"), "/help"),
            &config,
        )
        .unwrap();
        assert!(matches!(outcome, CommandOutcome::Reply(_)));
    }

    #[test]
    fn bot_self_messages_are_ignored_to_prevent_loops() {
        let runtime = test_runtime(tempdir().unwrap().path().to_path_buf());
        let mut message = email("thread-1", Some("user@example.com"), "hello");
        message.from_bot = true;
        let outcome = handle_command(&runtime, &message, &open_config()).unwrap();
        assert_eq!(outcome, CommandOutcome::Ignored);
    }
}

use crate::{ConversationKey, ConversationSessionMap};
use anyhow::{Context, Result};
use puffer_config::PufferConfig;
use puffer_core::{execute_user_turn, AppState};
use puffer_provider_registry::{AuthStore, ProviderRegistry};
use puffer_resources::LoadedResources;
use puffer_session_store::{SessionRecord, SessionStore, TranscriptEvent};
use std::path::PathBuf;
use std::sync::Mutex;
use uuid::Uuid;

/// Outcome of a single [`ConnectorRuntime::dispatch`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchOutcome {
    /// The Puffer session that ran the turn (created on first contact).
    pub session_id: Uuid,
    /// Whether this call created a fresh session (`true`) or resumed an
    /// existing one (`false`).
    pub created: bool,
    /// Assistant text to send back to the platform user.
    pub assistant_text: String,
}

/// Shared bridge that every connector calls when an inbound message
/// arrives on its platform.
///
/// Currently single-threaded: a single `Mutex` serializes turns. That
/// matches puffer's `execute_user_prompt` signature (it mutates
/// `AppState`/`AuthStore`) and is sufficient for v1 connector traffic.
/// Platform crates stay responsive by wrapping the blocking call in
/// `tokio::task::spawn_blocking`.
pub struct ConnectorRuntime {
    inner: Mutex<ConnectorRuntimeInner>,
}

struct ConnectorRuntimeInner {
    config: PufferConfig,
    resources: LoadedResources,
    providers: ProviderRegistry,
    auth_store: AuthStore,
    auth_path: PathBuf,
    session_store: SessionStore,
    session_map: ConversationSessionMap,
    default_cwd: PathBuf,
}

/// Builder-style arguments for [`ConnectorRuntime::new`].
///
/// Held together in one struct so the runtime keeps a single owned copy of
/// every piece of state and platform crates don't have to thread a dozen
/// arguments through their startup code.
pub struct ConnectorRuntimeConfig {
    pub config: PufferConfig,
    pub resources: LoadedResources,
    pub providers: ProviderRegistry,
    pub auth_store: AuthStore,
    pub auth_path: PathBuf,
    pub session_store: SessionStore,
    pub session_map: ConversationSessionMap,
    pub default_cwd: PathBuf,
}

impl ConnectorRuntime {
    pub fn new(config: ConnectorRuntimeConfig) -> Self {
        Self {
            inner: Mutex::new(ConnectorRuntimeInner {
                config: config.config,
                resources: config.resources,
                providers: config.providers,
                auth_store: config.auth_store,
                auth_path: config.auth_path,
                session_store: config.session_store,
                session_map: config.session_map,
                default_cwd: config.default_cwd,
            }),
        }
    }

    /// Binds an existing `session_id` to `key`. Useful when an outer
    /// process wants to pre-attach a connector conversation to a
    /// session that was created some other way (tests, seeding from
    /// a migration, etc).
    pub fn bind_session(&self, key: &ConversationKey, session_id: Uuid) -> Result<()> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("connector runtime mutex poisoned"))?;
        inner.session_map.insert(key, session_id)
    }

    /// Drops the stored session mapping for `key`. The next message on
    /// that conversation will create a brand-new session.
    ///
    /// Intended for `/new`-style user commands surfaced by platform
    /// connectors.
    pub fn reset_conversation(&self, key: &ConversationKey) -> Result<Option<Uuid>> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("connector runtime mutex poisoned"))?;
        inner.session_map.remove(key)
    }

    /// Returns the session id currently bound to `key`, if any. Does not
    /// create a new session.
    pub fn session_for(&self, key: &ConversationKey) -> Result<Option<Uuid>> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("connector runtime mutex poisoned"))?;
        Ok(inner.session_map.get(key))
    }

    /// Loads the full session record bound to `key`. Useful for slash
    /// commands like `/status` that want to surface event counts or
    /// timestamps.
    pub fn session_record(&self, key: &ConversationKey) -> Result<Option<SessionRecord>> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("connector runtime mutex poisoned"))?;
        match inner.session_map.get(key) {
            Some(session_id) => match inner.session_store.load_session(session_id) {
                Ok(record) => Ok(Some(record)),
                Err(_) => Ok(None),
            },
            None => Ok(None),
        }
    }

    /// Runs one Puffer turn for `input` on the session bound to `key`,
    /// creating a new session on first contact.
    ///
    /// This is the only entry point platform connectors need. It:
    /// 1. Resolves (or creates) the session.
    /// 2. Rehydrates `AppState` from the stored transcript.
    /// 3. Appends the inbound user message to the session log.
    /// 4. Calls `execute_user_turn`.
    /// 5. Appends the assistant reply and a state snapshot.
    /// 6. Returns the assistant text to be sent back on the platform.
    pub fn dispatch(&self, key: &ConversationKey, input: &str) -> Result<DispatchOutcome> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("connector runtime mutex poisoned"))?;
        inner.dispatch(key, input)
    }
}

impl ConnectorRuntimeInner {
    fn dispatch(&mut self, key: &ConversationKey, input: &str) -> Result<DispatchOutcome> {
        let (session_id, created) = match self.session_map.get(key) {
            Some(existing) if self.session_exists(existing) => (existing, false),
            _ => {
                let metadata = self
                    .session_store
                    .create_session(self.default_cwd.clone())
                    .context("failed to create connector session")?;
                self.session_map.insert(key, metadata.id)?;
                (metadata.id, true)
            }
        };

        let record = self
            .session_store
            .load_session(session_id)
            .with_context(|| format!("failed to load connector session {session_id}"))?;
        let mut state = AppState::from_session_record(self.config.clone(), record);
        if state.cwd.as_os_str().is_empty() {
            state.cwd = self.default_cwd.clone();
            state.session.cwd = self.default_cwd.clone();
        }

        // Record the inbound user message before running the turn so the
        // transcript on disk reflects what the agent was asked even if
        // the turn fails mid-way.
        state.push_message(puffer_core::MessageRole::User, input.to_string());
        self.session_store.append_event(
            session_id,
            TranscriptEvent::UserMessage {
                text: input.to_string(),
                actor: Some(state.user_actor()),
            },
        )?;

        let previous_auth_store = self.auth_store.clone();
        let turn = execute_user_turn(
            &mut state,
            &self.resources,
            &self.providers,
            &mut self.auth_store,
            input,
        )?;

        if self.auth_store != previous_auth_store {
            self.auth_store.save(&self.auth_path)?;
        }

        for invocation in &turn.tool_invocations {
            state.push_tool_invocation(
                &invocation.call_id,
                &invocation.tool_id,
                &invocation.input,
                &invocation.output,
                invocation.success,
            );
            self.session_store.append_event(
                session_id,
                TranscriptEvent::ToolInvocation {
                    call_id: invocation.call_id.clone(),
                    tool_id: invocation.tool_id.clone(),
                    input: invocation.input.clone(),
                    output: invocation.output.clone(),
                    success: invocation.success,
                    actor: Some(state.assistant_actor()),
                    subject: state.tool_subject_actor(&invocation.tool_id, &invocation.output),
                },
            )?;
        }
        state.push_message(
            puffer_core::MessageRole::Assistant,
            turn.assistant_text.clone(),
        );
        self.session_store.append_event(
            session_id,
            TranscriptEvent::AssistantMessage {
                text: turn.assistant_text.clone(),
                actor: Some(state.assistant_actor()),
            },
        )?;
        puffer_core::append_trace_events(&self.session_store, session_id, &turn.reflection_traces);
        self.session_store
            .append_event(session_id, state.snapshot_event())?;

        Ok(DispatchOutcome {
            session_id,
            created,
            assistant_text: turn.assistant_text,
        })
    }

    fn session_exists(&self, session_id: Uuid) -> bool {
        // `load_session` reads the `.session.json` stub; if it's missing
        // (e.g. the user deleted sessions manually) we want to fall back
        // to creating a fresh one rather than returning an error.
        self.session_store.load_session(session_id).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_config::ConfigPaths;
    use tempfile::tempdir;

    fn test_runtime(default_cwd: PathBuf, map_path: PathBuf) -> ConnectorRuntime {
        let paths = ConfigPaths {
            workspace_root: default_cwd.clone(),
            workspace_config_dir: default_cwd.join(".puffer"),
            user_config_dir: default_cwd.join(".puffer-user"),
            builtin_resources_dir: default_cwd.join("resources"),
        };
        std::fs::create_dir_all(&paths.user_config_dir).unwrap();
        let session_store = SessionStore::from_paths(&paths).unwrap();
        let session_map = ConversationSessionMap::load(&map_path).unwrap();

        ConnectorRuntime::new(ConnectorRuntimeConfig {
            config: PufferConfig::default(),
            resources: LoadedResources::default(),
            providers: ProviderRegistry::default(),
            auth_store: AuthStore::default(),
            auth_path: default_cwd.join("auth.json"),
            session_store,
            session_map,
            default_cwd,
        })
    }

    #[test]
    fn reset_conversation_detaches_the_stored_session() {
        let dir = tempdir().unwrap();
        let map_path = dir.path().join("map.json");
        let runtime = test_runtime(dir.path().to_path_buf(), map_path.clone());
        let key = ConversationKey::new("test", "conv-1");

        // Manually seed the map so we exercise reset without needing a
        // provider-backed turn.
        {
            let mut inner = runtime.inner.lock().unwrap();
            inner.session_map.insert(&key, Uuid::new_v4()).unwrap();
        }
        assert!(runtime.session_for(&key).unwrap().is_some());

        let removed = runtime.reset_conversation(&key).unwrap();
        assert!(removed.is_some());
        assert!(runtime.session_for(&key).unwrap().is_none());

        // Persisted to disk too.
        let reloaded = ConversationSessionMap::load(&map_path).unwrap();
        assert!(reloaded.get(&key).is_none());
    }

    #[test]
    fn session_for_returns_none_before_first_dispatch() {
        let dir = tempdir().unwrap();
        let runtime = test_runtime(dir.path().to_path_buf(), dir.path().join("map.json"));
        let key = ConversationKey::new("test", "unknown");
        assert!(runtime.session_for(&key).unwrap().is_none());
    }
}

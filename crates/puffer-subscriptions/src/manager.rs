//! Process-wide manager that owns the bus, store, supervised subscribers
//! and the router. Workflow tools reach into the manager via a global
//! `OnceLock` set up at puffer startup.

use crate::action::{ActionDispatcher, BuiltinActionDispatcher};
use crate::catalog_store::ConnectorCatalogStore;
use crate::classify::{Classifier, NullClassifier};
use crate::command_match::command_matches_terminal_event;
use crate::connection::{ConnectionState, ConnectionStore};
use crate::connector_process;
use crate::connector_stream::{ConnectorEventProcessor, ConnectorStreamHandle};
use crate::history::WorkflowHistoryStore;
use crate::protocol::{ConnectorActionRequest, ConnectorActionResponse};
use crate::proxy::{
    builtin_agent_proxy, handle_agent_proxy_event as decide_agent_proxy_event, AgentProxyDecision,
    AgentProxyStore,
};
use crate::router::{process_envelope_result, SubscriptionRouter};
use crate::spec::ActionSpec;
use crate::store::SubscriptionStore;
use anyhow::Result;
use puffer_subscriber_runtime::{
    EventBus, EventEnvelope, Manifest, SubscriberCommand, SubscriberHandle, SubscriberSupervisor,
    SupervisorConfig,
};
use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::runtime::Handle;

const CONNECTOR_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// Builder for [`SubscriptionManager`]. Lets callers swap in custom
/// dispatcher / classifier implementations (e.g. a real LLM-backed
/// classifier) before construction.
pub struct SubscriptionManagerBuilder {
    bus: EventBus,
    store_path: PathBuf,
    connector_store_path: PathBuf,
    connection_store_path: PathBuf,
    history_store_path: PathBuf,
    proxy_store_path: PathBuf,
    dispatcher: Arc<dyn ActionDispatcher>,
    classifier: Arc<dyn Classifier>,
}

impl SubscriptionManagerBuilder {
    /// Starts a builder with the default dispatcher and classifier and
    /// the supplied store path. The bus is freshly constructed.
    pub fn new(store_path: impl Into<PathBuf>) -> Self {
        let store_path = store_path.into();
        let connection_store_path = store_path
            .parent()
            .map(|parent| parent.join("connections.json"))
            .unwrap_or_else(|| PathBuf::from("connections.json"));
        let connector_store_path = store_path
            .parent()
            .map(|parent| parent.join("connectors.json"))
            .unwrap_or_else(|| PathBuf::from("connectors.json"));
        let history_store_path = store_path
            .parent()
            .map(|parent| parent.join("workflow_history.json"))
            .unwrap_or_else(|| PathBuf::from("workflow_history.json"));
        let proxy_store_path = store_path
            .parent()
            .map(|parent| parent.join("agent_proxy_bindings.json"))
            .unwrap_or_else(|| PathBuf::from("agent_proxy_bindings.json"));
        Self {
            bus: EventBus::new(),
            store_path,
            connector_store_path,
            connection_store_path,
            history_store_path,
            proxy_store_path,
            dispatcher: Arc::new(BuiltinActionDispatcher::new()),
            classifier: Arc::new(NullClassifier),
        }
    }

    /// Override the bus (useful for tests).
    pub fn with_bus(mut self, bus: EventBus) -> Self {
        self.bus = bus;
        self
    }

    /// Override the action dispatcher.
    pub fn with_dispatcher(mut self, dispatcher: Arc<dyn ActionDispatcher>) -> Self {
        self.dispatcher = dispatcher;
        self
    }

    /// Override the connection store path.
    pub fn with_connection_store_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.connection_store_path = path.into();
        self
    }

    /// Override the connector catalog store path.
    pub fn with_connector_store_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.connector_store_path = path.into();
        self
    }

    /// Override the workflow history store path.
    pub fn with_history_store_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.history_store_path = path.into();
        self
    }

    /// Override the agent proxy binding store path.
    pub fn with_proxy_store_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.proxy_store_path = path.into();
        self
    }

    /// Override the classifier.
    pub fn with_classifier(mut self, classifier: Arc<dyn Classifier>) -> Self {
        self.classifier = classifier;
        self
    }

    /// Loads the store and spawns the router on the supplied Tokio runtime.
    pub fn build(self, handle: Handle) -> Result<SubscriptionManager> {
        let store = Arc::new(SubscriptionStore::load(&self.store_path)?);
        let connector_store = Arc::new(ConnectorCatalogStore::load(&self.connector_store_path)?);
        let connection_store = Arc::new(ConnectionStore::load(&self.connection_store_path)?);
        let history_store = Arc::new(WorkflowHistoryStore::load(&self.history_store_path)?);
        let proxy_store = Arc::new(AgentProxyStore::load(&self.proxy_store_path)?);
        let dispatcher = self.dispatcher.clone();
        let classifier = self.classifier.clone();
        let bus = self.bus.clone();
        let store_for_router = store.clone();
        let history_for_router = history_store.clone();
        let dispatcher_for_router = dispatcher.clone();
        let classifier_for_router = classifier.clone();
        let router = {
            let _runtime_guard = handle.enter();
            SubscriptionRouter::spawn(
                bus,
                store_for_router,
                Some(history_for_router),
                dispatcher_for_router,
                classifier_for_router,
            )
        };
        let manager = SubscriptionManager {
            handle,
            bus: self.bus,
            store,
            connector_store,
            connection_store,
            history_store,
            proxy_store,
            dispatcher,
            classifier,
            router: Mutex::new(Some(router)),
            subscribers: Mutex::new(HashMap::new()),
            connector_streams: Mutex::new(HashMap::new()),
            command_wait_locks: Mutex::new(HashMap::new()),
        };
        manager.refresh_connection_consumers()?;
        Ok(manager)
    }
}

/// Process-wide subscription manager. Owns the bus, store, router task,
/// and the set of supervised subscriber children.
pub struct SubscriptionManager {
    handle: Handle,
    bus: EventBus,
    store: Arc<SubscriptionStore>,
    connector_store: Arc<ConnectorCatalogStore>,
    connection_store: Arc<ConnectionStore>,
    history_store: Arc<WorkflowHistoryStore>,
    proxy_store: Arc<AgentProxyStore>,
    dispatcher: Arc<dyn ActionDispatcher>,
    classifier: Arc<dyn Classifier>,
    router: Mutex<Option<SubscriptionRouter>>,
    subscribers: Mutex<HashMap<String, SubscriberHandle>>,
    connector_streams: Mutex<HashMap<String, ConnectorStreamHandle>>,
    command_wait_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl SubscriptionManager {
    /// Returns the underlying spec store.
    pub fn store(&self) -> Arc<SubscriptionStore> {
        self.store.clone()
    }

    /// Returns the underlying connector catalog store.
    pub fn connector_store(&self) -> Arc<ConnectorCatalogStore> {
        self.connector_store.clone()
    }

    /// Returns the underlying connection store.
    pub fn connection_store(&self) -> Arc<ConnectionStore> {
        self.connection_store.clone()
    }

    /// Returns the underlying direct workflow history store.
    pub fn history_store(&self) -> Arc<WorkflowHistoryStore> {
        self.history_store.clone()
    }

    /// Returns the underlying agent proxy binding store.
    pub fn proxy_store(&self) -> Arc<AgentProxyStore> {
        self.proxy_store.clone()
    }

    /// Recomputes whether each known connection has an enabled workflow
    /// binding consumer and updates its active/authenticated state.
    pub fn refresh_connection_consumers(&self) -> Result<()> {
        let bindings = self.store.list();
        for connection in self.connection_store.list() {
            let has_workflow_consumer = bindings.iter().any(|binding| {
                binding.connection_slug == connection.slug
                    && binding.status == crate::spec::WorkflowBindingStatus::Enabled
            });
            let has_proxy_consumer = self.proxy_store.has_enabled_consumer(&connection.slug);
            let has_consumer = has_workflow_consumer || has_proxy_consumer;
            if connection.has_consumer != has_consumer
                || matches!(
                    connection.state,
                    ConnectionState::Active | ConnectionState::Authenticated
                )
            {
                self.connection_store.update(&connection.slug, |record| {
                    record.set_has_consumer(has_consumer)
                })?;
            }
        }
        self.refresh_connector_streams()?;
        Ok(())
    }

    fn refresh_connector_streams(&self) -> Result<()> {
        let finished = self
            .connector_streams
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(slug, handle)| handle.is_finished().then_some(slug.clone()))
            .collect::<Vec<_>>();
        for slug in finished {
            if let Some(handle) = self.connector_streams.lock().unwrap().remove(&slug) {
                block_on_manager_handle(&self.handle, async move {
                    handle.shutdown().await;
                    Ok(())
                })?;
            }
        }
        for connection in self.connection_store.list() {
            let should_stream = connection.should_stream();
            let is_running = self
                .connector_streams
                .lock()
                .unwrap()
                .contains_key(&connection.slug);
            let template = self.connector_store.get(&connection.connector_slug);
            let stream_supported = template.as_ref().is_some_and(|template| {
                template.command_argv().is_some() && template.can_subscribe
            });
            if should_stream && !stream_supported {
                if is_running {
                    if let Some(handle) = self
                        .connector_streams
                        .lock()
                        .unwrap()
                        .remove(&connection.slug)
                    {
                        block_on_manager_handle(&self.handle, async move {
                            handle.shutdown().await;
                            Ok(())
                        })?;
                    }
                }
                continue;
            }
            if should_stream && !is_running {
                let Some(template) = template else {
                    continue;
                };
                let bus = self.bus.clone();
                let store = self.connection_store.clone();
                let slug = connection.slug.clone();
                let cursor = connection.cursor.clone();
                let processor: Arc<dyn ConnectorEventProcessor> =
                    Arc::new(ManagerConnectorEventProcessor {
                        store: self.store.clone(),
                        connection_store: self.connection_store.clone(),
                        history_store: self.history_store.clone(),
                        proxy_store: self.proxy_store.clone(),
                        dispatcher: self.dispatcher.clone(),
                        classifier: self.classifier.clone(),
                    });
                if let Some(handle) = block_on_manager_handle(
                    &self.handle,
                    ConnectorStreamHandle::spawn(
                        template,
                        slug,
                        cursor,
                        bus,
                        store,
                        Some(processor),
                    ),
                )? {
                    self.connector_streams
                        .lock()
                        .unwrap()
                        .insert(handle.connection_slug.clone(), handle);
                }
            } else if !should_stream && is_running {
                if let Some(handle) = self
                    .connector_streams
                    .lock()
                    .unwrap()
                    .remove(&connection.slug)
                {
                    block_on_manager_handle(&self.handle, async move {
                        handle.shutdown().await;
                        Ok(())
                    })?;
                }
            }
        }
        let known = self
            .connection_store
            .list()
            .into_iter()
            .map(|connection| connection.slug)
            .collect::<std::collections::BTreeSet<_>>();
        let stale = self
            .connector_streams
            .lock()
            .unwrap()
            .keys()
            .filter(|slug| !known.contains(*slug))
            .cloned()
            .collect::<Vec<_>>();
        for slug in stale {
            if let Some(handle) = self.connector_streams.lock().unwrap().remove(&slug) {
                block_on_manager_handle(&self.handle, async move {
                    handle.shutdown().await;
                    Ok(())
                })?;
            }
        }
        Ok(())
    }

    /// Runs auth checks for connections whose connector template exposes a
    /// typed `auth-ok` command. Returns connections newly marked degraded so
    /// callers can surface one-time user-visible notices.
    pub fn refresh_connection_auth(&self) -> Result<Vec<crate::connection::ConnectionRecord>> {
        let mut newly_degraded = Vec::new();
        for connection in self.connection_store.list() {
            if matches!(
                connection.state,
                ConnectionState::Created
                    | ConnectionState::Authenticating
                    | ConnectionState::Disabled
            ) {
                continue;
            }
            let Some(template) = self.connector_store.get(&connection.connector_slug) else {
                continue;
            };
            if !template.requires_auth {
                continue;
            }
            let auth_ok = match self.check_connection_auth(&template, &connection.slug) {
                Ok(Some(auth_ok)) => auth_ok,
                Ok(None) if connection.has_consumer => {
                    tracing::warn!(
                        connection = %connection.slug,
                        connector = %connection.connector_slug,
                        "active connector requires auth but exposes no auth check"
                    );
                    false
                }
                Ok(None) => continue,
                Err(error) => {
                    tracing::warn!(
                        connection = %connection.slug,
                        connector = %connection.connector_slug,
                        %error,
                        "connector auth check failed"
                    );
                    false
                }
            };
            if auth_ok {
                self.connection_store.update(&connection.slug, |record| {
                    record.auth_failure_notified = false;
                    if record.state == ConnectionState::Degraded {
                        record.state = ConnectionState::Authenticated;
                        record.set_has_consumer(record.has_consumer);
                    }
                })?;
            } else {
                let was_notified = connection.auth_failure_notified;
                let updated = self.connection_store.update(&connection.slug, |record| {
                    record.state = ConnectionState::Degraded;
                    record.auth_failure_notified = true;
                })?;
                if !was_notified {
                    newly_degraded.push(updated);
                }
            }
        }
        Ok(newly_degraded)
    }

    /// Runs a connector template's `auth-ok` command when one is available.
    pub fn check_connection_auth(
        &self,
        template: &crate::catalog::ConnectorTemplate,
        connection_slug: &str,
    ) -> Result<Option<bool>> {
        let template = template.clone();
        let connection_slug = connection_slug.to_string();
        let process_template = template.clone();
        let process_connection_slug = connection_slug.clone();
        let checked = block_on_manager_handle(&self.handle, async move {
            connector_process::run_auth_ok(
                &process_template,
                &process_connection_slug,
                CONNECTOR_COMMAND_TIMEOUT,
            )
            .await
        })?;
        if checked.is_some() {
            return Ok(checked);
        }
        self.check_builtin_connection_auth(&template, &connection_slug)
    }

    /// Runs a connector template's `act` command when one is available.
    pub fn run_connector_action(
        &self,
        template: &crate::catalog::ConnectorTemplate,
        request: &ConnectorActionRequest,
    ) -> Result<Option<ConnectorActionResponse>> {
        let template = template.clone();
        let request = request.clone();
        block_on_manager_handle(&self.handle, async move {
            connector_process::run_action(&template, &request, CONNECTOR_COMMAND_TIMEOUT).await
        })
    }

    /// Handles one connector event for a built-in agent proxy.
    pub fn handle_agent_proxy_event(
        &self,
        connector_slug: &str,
        connection_slug: &str,
        event: &serde_json::Value,
    ) -> Result<AgentProxyDecision> {
        let decision =
            decide_agent_proxy_event(connector_slug, connection_slug, event, &self.proxy_store)?;
        if matches!(decision, AgentProxyDecision::BindAgent { .. }) {
            self.refresh_connection_consumers()?;
        }
        Ok(decision)
    }

    /// Returns a handle on the event bus (used by tests and by future
    /// "live tail" UI).
    pub fn bus(&self) -> EventBus {
        self.bus.clone()
    }

    /// Spawns a subscriber from a manifest directory. Returns the
    /// subscriber's id. If a subscriber with that id is already running,
    /// returns `Ok(id)` without spawning a duplicate.
    pub fn start_subscriber(&self, manifest: Manifest) -> Result<String> {
        let id = manifest.spec.id.clone();
        let mut guard = self.subscribers.lock().unwrap();
        if guard.contains_key(&id) {
            return Ok(id);
        }
        let bus = self.bus.clone();
        let handle = block_on_manager_handle(&self.handle, async move {
            SubscriberSupervisor::spawn(manifest, bus, SupervisorConfig::default()).await
        })?;
        guard.insert(id.clone(), handle);
        Ok(id)
    }

    /// Sends a control command to the named subscriber. Returns an error
    /// when the subscriber is unknown or not running.
    pub fn send_command(&self, subscriber_id: &str, command: &SubscriberCommand) -> Result<()> {
        let command = command.clone();
        let sender = {
            let guard = self.subscribers.lock().unwrap();
            guard
                .get(subscriber_id)
                .ok_or_else(|| anyhow::anyhow!("subscriber `{subscriber_id}` is not running"))?
                .commands
                .clone()
        };
        block_on_manager_handle(&self.handle, async move { sender.send(&command).await })
    }

    /// Sends a control command and waits for the next matching event kind
    /// on `topic`.
    pub fn send_command_and_wait(
        &self,
        subscriber_id: &str,
        topic: &str,
        command: &SubscriberCommand,
        terminal_kinds: &[&str],
        timeout: std::time::Duration,
    ) -> Result<EventEnvelope> {
        let command = command.clone();
        let wait_lock = self.command_wait_lock(subscriber_id, topic);
        let _wait_guard = wait_lock.lock().unwrap();
        let sender = {
            let guard = self.subscribers.lock().unwrap();
            guard
                .get(subscriber_id)
                .ok_or_else(|| anyhow::anyhow!("subscriber `{subscriber_id}` is not running"))?
                .commands
                .clone()
        };
        let mut rx = self.bus.subscribe_topic(topic);
        let terminal_kinds: Vec<String> =
            terminal_kinds.iter().map(|kind| kind.to_string()).collect();
        block_on_manager_handle(&self.handle, async move {
            sender.send(&command).await?;
            let deadline = tokio::time::Instant::now() + timeout;
            loop {
                let remaining = deadline
                    .checked_duration_since(tokio::time::Instant::now())
                    .ok_or_else(|| anyhow::anyhow!("timed out waiting for subscriber event"))?;
                let envelope = tokio::time::timeout(remaining, rx.recv())
                    .await
                    .map_err(|_| anyhow::anyhow!("timed out waiting for subscriber event"))?
                    .ok_or_else(|| anyhow::anyhow!("subscriber event bus closed"))?;
                if terminal_kinds
                    .iter()
                    .any(|kind| kind == &envelope.event.kind)
                    && command_matches_terminal_event(&command, &envelope)
                {
                    return Ok(envelope);
                }
            }
        })
    }

    /// Returns the ids of currently supervised subscribers.
    pub fn subscriber_ids(&self) -> Vec<String> {
        self.subscribers.lock().unwrap().keys().cloned().collect()
    }

    fn command_wait_lock(&self, subscriber_id: &str, topic: &str) -> Arc<Mutex<()>> {
        let key = format!("{subscriber_id}\0{topic}");
        let mut guard = self.command_wait_locks.lock().unwrap();
        guard
            .entry(key)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    fn check_builtin_connection_auth(
        &self,
        template: &crate::catalog::ConnectorTemplate,
        connection_slug: &str,
    ) -> Result<Option<bool>> {
        match template.slug.as_str() {
            "telegram-login" => self.check_telegram_login_auth(connection_slug),
            _ => Ok(None),
        }
    }

    fn check_telegram_login_auth(&self, connection_slug: &str) -> Result<Option<bool>> {
        if !self
            .subscriber_ids()
            .iter()
            .any(|subscriber_id| subscriber_id == connection_slug)
        {
            return Ok(None);
        }
        let command = SubscriberCommand::TelegramListPeers {
            query: Some("__puffer_auth_probe__".to_string()),
            peer_kind: None,
            limit: Some(1),
        };
        let envelope = self.send_command_and_wait(
            connection_slug,
            connection_slug,
            &command,
            &["peer_list", "peer_list_error", "login_error"],
            Duration::from_secs(15),
        )?;
        Ok(Some(envelope.event.kind == "peer_list"))
    }

    /// Shuts down router and every supervised subscriber. Best-effort.
    pub fn shutdown(&self) {
        if let Some(router) = self.router.lock().unwrap().take() {
            let _ =
                block_on_manager_handle(&self.handle, async move { Ok(router.shutdown().await) });
        }
        let handles: Vec<_> = self
            .subscribers
            .lock()
            .unwrap()
            .drain()
            .map(|(_, h)| h)
            .collect();
        for handle in handles {
            let _ =
                block_on_manager_handle(&self.handle, async move { Ok(handle.shutdown().await) });
        }
        let streams: Vec<_> = self
            .connector_streams
            .lock()
            .unwrap()
            .drain()
            .map(|(_, handle)| handle)
            .collect();
        for handle in streams {
            let _ =
                block_on_manager_handle(&self.handle, async move { Ok(handle.shutdown().await) });
        }
    }
}

fn block_on_manager_handle<T, F>(handle: &Handle, future: F) -> Result<T>
where
    T: Send + 'static,
    F: Future<Output = Result<T>> + Send + 'static,
{
    if Handle::try_current().is_err() {
        return handle.block_on(future);
    }
    tokio::task::block_in_place(|| handle.block_on(future))
}

struct ManagerConnectorEventProcessor {
    store: Arc<SubscriptionStore>,
    connection_store: Arc<ConnectionStore>,
    history_store: Arc<WorkflowHistoryStore>,
    proxy_store: Arc<AgentProxyStore>,
    dispatcher: Arc<dyn ActionDispatcher>,
    classifier: Arc<dyn Classifier>,
}

impl ConnectorEventProcessor for ManagerConnectorEventProcessor {
    fn process_connector_event(
        &self,
        connector_slug: &str,
        connection_slug: &str,
        envelope: &EventEnvelope,
    ) -> Result<()> {
        self.process_agent_proxy(connector_slug, connection_slug, envelope)?;
        let result = process_envelope_result(
            envelope,
            &self.store,
            Some(&self.history_store),
            &self.dispatcher,
            &self.classifier,
            None,
        );
        if result.failed > 0 {
            anyhow::bail!(
                "{} workflow action(s) failed while processing connector event",
                result.failed
            );
        }
        Ok(())
    }
}

impl ManagerConnectorEventProcessor {
    fn process_agent_proxy(
        &self,
        connector_slug: &str,
        connection_slug: &str,
        envelope: &EventEnvelope,
    ) -> Result<()> {
        match decide_agent_proxy_event(
            connector_slug,
            connection_slug,
            &envelope.event.payload,
            &self.proxy_store,
        )? {
            AgentProxyDecision::Ignore => Ok(()),
            AgentProxyDecision::ConnectorAction { action, input } => {
                self.dispatch_connector_action(connector_slug, &action, input, envelope)
            }
            AgentProxyDecision::BindAgent { reply, .. } => {
                let _ = self.connection_store.update(connection_slug, |record| {
                    record.set_has_consumer(true);
                });
                if let Some(input) = reply {
                    self.dispatch_connector_action(
                        connector_slug,
                        "send_message",
                        input,
                        envelope,
                    )?;
                }
                Ok(())
            }
            AgentProxyDecision::RouteToAgent {
                target,
                message,
                binding,
            } => {
                let prompt = format!(
                    "Route this external connector message to agent `{target}`.\n\n{message}"
                );
                let result = self.dispatcher.dispatch(
                    &ActionSpec::TriageAgent {
                        prompt,
                        model: None,
                    },
                    envelope,
                );
                if !result.success {
                    anyhow::bail!("{}", result.summary);
                }
                let Some(proxy) = builtin_agent_proxy(connector_slug) else {
                    return Ok(());
                };
                let input = proxy.render_agent_reply(&result.summary, &binding);
                self.dispatch_connector_action(connector_slug, "send_message", input, envelope)
            }
        }
    }

    fn dispatch_connector_action(
        &self,
        connector_slug: &str,
        action: &str,
        input: serde_json::Value,
        envelope: &EventEnvelope,
    ) -> Result<()> {
        let result = self.dispatcher.dispatch(
            &ActionSpec::ConnectorAct {
                connector_slug: connector_slug.to_string(),
                action: action.to_string(),
                input,
            },
            envelope,
        );
        if result.success {
            Ok(())
        } else {
            anyhow::bail!("{}", result.summary)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::ConnectionRecord;
    use puffer_subscriber_runtime::SubscriberCommand;
    use serde_json::Value;
    use tempfile::tempdir;

    #[test]
    fn agent_proxy_binding_counts_as_connection_consumer() {
        let temp = tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(1)
            .build()
            .unwrap();
        let manager = SubscriptionManagerBuilder::new(temp.path().join("subscriptions.json"))
            .build(runtime.handle().clone())
            .unwrap();
        manager
            .connection_store()
            .create(ConnectionRecord::authenticated(
                "my-bot",
                "telegram-bot",
                "demo",
            ))
            .unwrap();

        let decision = manager
            .handle_agent_proxy_event(
                "telegram-bot",
                "my-bot",
                &serde_json::json!({"message":"/connect agent-1","from":{"id":123}}),
            )
            .unwrap();

        assert!(matches!(decision, AgentProxyDecision::BindAgent { .. }));
        let connection = manager.connection_store().get("my-bot").unwrap();
        assert!(connection.has_consumer);
        assert_eq!(connection.state, ConnectionState::Active);

        let decision = manager
            .handle_agent_proxy_event(
                "telegram-bot",
                "my-bot",
                &serde_json::json!({"message":"status?","from":{"id":123}}),
            )
            .unwrap();
        assert_eq!(
            decision,
            AgentProxyDecision::RouteToAgent {
                target: "agent-1".into(),
                message: "status?".into(),
                binding: crate::proxy::AgentProxyBinding {
                    connection_slug: "my-bot".into(),
                    external_principal: "123".into(),
                    reply_target: Some("123".into()),
                    agent_target: "agent-1".into(),
                    enabled: true,
                },
            }
        );

        manager.shutdown();
    }

    #[test]
    fn start_subscriber_allows_immediate_control_command() {
        let temp = tempdir().unwrap();
        let subscriber_dir = temp.path().join("subscriber");
        std::fs::create_dir_all(&subscriber_dir).unwrap();
        std::fs::write(
            subscriber_dir.join("manifest.toml"),
            r#"manifest_version = 1
id = "test-subscriber"
kind = "subscriber"
topic = "test-topic"

[run]
cmd = ["sh", "run.sh"]
"#,
        )
        .unwrap();
        std::fs::write(
            subscriber_dir.join("run.sh"),
            r#"IFS= read -r _line || exit 0
printf '%s\n' '{"topic":"test-topic","kind":"message","text":"ready"}'
"#,
        )
        .unwrap();

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(1)
            .build()
            .unwrap();
        let manager = SubscriptionManagerBuilder::new(temp.path().join("subscriptions.json"))
            .build(runtime.handle().clone())
            .unwrap();
        let mut rx = manager.bus().subscribe_topic("test-topic");
        let manifest = Manifest::load(&subscriber_dir).unwrap();

        manager.start_subscriber(manifest).unwrap();
        manager
            .send_command(
                "test-subscriber",
                &SubscriberCommand::Custom {
                    op: "ping".into(),
                    args: Value::Null,
                },
            )
            .unwrap();

        let envelope = runtime
            .block_on(async {
                tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv()).await
            })
            .unwrap()
            .unwrap();
        assert_eq!(envelope.subscriber_id, "test-subscriber");
        assert_eq!(envelope.event.text, "ready");

        manager.shutdown();
    }

    #[test]
    fn start_subscriber_passes_absolute_state_dir() {
        let temp = tempdir().unwrap();
        let subscriber_dir = temp.path().join("subscriber");
        std::fs::create_dir_all(&subscriber_dir).unwrap();
        std::fs::write(
            subscriber_dir.join("manifest.toml"),
            r#"manifest_version = 1
id = "state-subscriber"
kind = "subscriber"
topic = "state-topic"

[run]
cmd = ["sh", "run.sh"]

[state]
dir = "state"
"#,
        )
        .unwrap();
        std::fs::write(
            subscriber_dir.join("run.sh"),
            r#"printf '{"topic":"state-topic","kind":"state","text":"%s"}\n' "$PUFFER_SKILL_STATE_DIR"
"#,
        )
        .unwrap();

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(1)
            .build()
            .unwrap();
        let manager = SubscriptionManagerBuilder::new(temp.path().join("subscriptions.json"))
            .build(runtime.handle().clone())
            .unwrap();
        let mut rx = manager.bus().subscribe_topic("state-topic");
        let original_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(temp.path()).unwrap();
        let manifest = Manifest::load("subscriber").unwrap();

        manager.start_subscriber(manifest).unwrap();
        std::env::set_current_dir(original_cwd).unwrap();

        let envelope = runtime
            .block_on(async {
                tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv()).await
            })
            .unwrap()
            .unwrap();
        assert!(
            std::path::Path::new(&envelope.event.text).is_absolute(),
            "state dir should be absolute, got {}",
            envelope.event.text
        );

        manager.shutdown();
    }

    #[test]
    fn send_command_and_wait_returns_terminal_event() {
        let temp = tempdir().unwrap();
        let subscriber_dir = temp.path().join("subscriber");
        std::fs::create_dir_all(&subscriber_dir).unwrap();
        std::fs::write(
            subscriber_dir.join("manifest.toml"),
            r#"manifest_version = 1
id = "wait-subscriber"
kind = "subscriber"
topic = "wait-topic"

[run]
cmd = ["sh", "run.sh"]
"#,
        )
        .unwrap();
        std::fs::write(
            subscriber_dir.join("run.sh"),
            r#"IFS= read -r _line || exit 0
printf '%s\n' '{"topic":"wait-topic","kind":"ignored","text":"first"}'
printf '%s\n' '{"topic":"wait-topic","kind":"login_error","text":"terminal","payload":{"error":"boom"}}'
"#,
        )
        .unwrap();

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(1)
            .build()
            .unwrap();
        let manager = SubscriptionManagerBuilder::new(temp.path().join("subscriptions.json"))
            .build(runtime.handle().clone())
            .unwrap();
        let manifest = Manifest::load(&subscriber_dir).unwrap();

        manager.start_subscriber(manifest).unwrap();
        let envelope = manager
            .send_command_and_wait(
                "wait-subscriber",
                "wait-topic",
                &SubscriberCommand::Custom {
                    op: "ping".into(),
                    args: Value::Null,
                },
                &["login_awaiting_code", "login_error"],
                std::time::Duration::from_secs(2),
            )
            .unwrap();
        assert_eq!(envelope.event.kind, "login_error");
        assert_eq!(envelope.event.payload["error"], "boom");

        manager.shutdown();
    }
}

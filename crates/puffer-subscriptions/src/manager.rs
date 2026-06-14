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
use crate::contacts::contact_ids_for_connector;
use crate::history::WorkflowHistoryStore;
use crate::protocol::{ConnectorActionRequest, ConnectorActionResponse};
use crate::proxy::{
    builtin_agent_proxy, handle_agent_proxy_event as decide_agent_proxy_event, AgentProxyDecision,
    AgentProxyStore,
};
use crate::router::{process_envelope_batch_result, process_envelope_result, SubscriptionRouter};
use crate::spec::ActionSpec;
use crate::store::SubscriptionStore;
use anyhow::Result;
use puffer_subscriber_runtime::{
    CommandSender, EventBus, EventEnvelope, Manifest, SubscriberCommand, SubscriberHandle,
    SubscriberSupervisor, SupervisorConfig,
};
use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::runtime::Handle;

const CONNECTOR_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
/// WeChat's `act` path drives the LIVE client with deliberate human-like pacing
/// (think-time, a length-scaled compose pause, simulated mouse paths, navigate +
/// verify + delivery check), so a single real send legitimately takes ~40-60s —
/// far longer than an API connector's call. Give it a roomier ceiling so a real
/// send is not killed mid-flight. Other connectors keep the tight 30s; this
/// ceiling only bounds a stuck call, never a fast API one.
const WECHAT_ACTION_TIMEOUT: Duration = Duration::from_secs(150);
const COMMAND_RESTART_POLL_INTERVAL: Duration = Duration::from_millis(100);
const COMMAND_RESTART_RESENDS: usize = 2;

mod contact_methods;

enum ConnectionContactScope {
    All,
    Filter(Vec<String>),
    None,
}

/// Host-provided auth checker for built-in connectors whose credentials are
/// owned by the embedding process instead of a connector subprocess.
pub trait ConnectionAuthChecker: Send + Sync {
    /// Returns `Some(true)` when auth is healthy, `Some(false)` when auth is
    /// known broken, and `None` when this checker does not handle the
    /// connector.
    fn check(
        &self,
        manager: &SubscriptionManager,
        template: &crate::catalog::ConnectorTemplate,
        connection_slug: &str,
    ) -> Result<Option<bool>>;
}

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
    auth_checker: Option<Arc<dyn ConnectionAuthChecker>>,
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
            auth_checker: None,
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

    /// Override the process-provided connection auth checker.
    pub fn with_connection_auth_checker(mut self, checker: Arc<dyn ConnectionAuthChecker>) -> Self {
        self.auth_checker = Some(checker);
        self
    }

    /// Loads the store and spawns the router on the supplied Tokio runtime.
    pub fn build(self, handle: Handle) -> Result<SubscriptionManager> {
        let store = Arc::new(SubscriptionStore::load(&self.store_path)?);
        let connector_store = Arc::new(ConnectorCatalogStore::load(&self.connector_store_path)?);
        let connection_store = Arc::new(ConnectionStore::load(&self.connection_store_path)?);
        let history_store = Arc::new(WorkflowHistoryStore::load(&self.history_store_path)?);
        // Root cause D: reclaim runs left `Running` by a previous process (crash/kill
        // before completion) so their messages are no longer dedup-blocked forever.
        // Must run before the router starts consuming live events.
        let reclaimed = history_store.reclaim_orphaned_running_on_boot();
        if reclaimed > 0 {
            tracing::info!(
                count = reclaimed,
                "reclaimed orphaned Running workflow runs on startup"
            );
        }
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
            auth_checker: self.auth_checker,
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
    auth_checker: Option<Arc<dyn ConnectionAuthChecker>>,
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
                tracing::info!(
                    connection = %connection.slug,
                    connector = %connection.connector_slug,
                    has_workflow_consumer,
                    has_proxy_consumer,
                    has_consumer,
                    previous_has_consumer = connection.has_consumer,
                    previous_state = ?connection.state,
                    "refreshing connection consumer state"
                );
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
            let mut is_running = self
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
            let contact_ids = if should_stream && stream_supported {
                match self.contact_scope_for_connection(&connection.slug) {
                    ConnectionContactScope::All => Vec::new(),
                    ConnectionContactScope::Filter(contact_ids) => contact_ids,
                    ConnectionContactScope::None => {
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
                }
            } else {
                Vec::new()
            };
            if should_stream && stream_supported && is_running {
                let filter_changed = self
                    .connector_streams
                    .lock()
                    .unwrap()
                    .get(&connection.slug)
                    .is_some_and(|handle| handle.contact_ids() != contact_ids.as_slice());
                if filter_changed {
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
                    is_running = false;
                }
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
                        contact_ids,
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

    fn contact_scope_for_connection(&self, connection_slug: &str) -> ConnectionContactScope {
        if self.proxy_store.has_enabled_consumer(connection_slug) {
            return ConnectionContactScope::All;
        }
        let Some(connection) = self.connection_store.get(connection_slug) else {
            return ConnectionContactScope::None;
        };
        let mut contact_ids = std::collections::BTreeSet::new();
        let mut saw_workflow_consumer = false;
        for binding in self.store.list().into_iter().filter(|binding| {
            binding.status == crate::spec::WorkflowBindingStatus::Enabled
                && binding.connection_slug == connection_slug
        }) {
            saw_workflow_consumer = true;
            if binding.contact_ids.is_empty() {
                return ConnectionContactScope::All;
            }
            contact_ids.extend(contact_ids_for_connector(
                &connection.connector_slug,
                &binding.contact_ids,
            ));
        }
        if !saw_workflow_consumer || contact_ids.is_empty() {
            return ConnectionContactScope::None;
        }
        ConnectionContactScope::Filter(contact_ids.into_iter().collect())
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
                    continue;
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
        if let Some(checker) = &self.auth_checker {
            return checker.check(self, &template, &connection_slug);
        }
        Ok(None)
    }

    /// Runs a connector template's `act` command when one is available.
    pub fn run_connector_action(
        &self,
        template: &crate::catalog::ConnectorTemplate,
        request: &ConnectorActionRequest,
    ) -> Result<Option<ConnectorActionResponse>> {
        // WeChat's human-paced act path needs a roomier ceiling than fast API
        // connectors (see WECHAT_ACTION_TIMEOUT); pick per connector.
        let timeout = if template.slug.starts_with("wechat") {
            WECHAT_ACTION_TIMEOUT
        } else {
            CONNECTOR_COMMAND_TIMEOUT
        };
        let template = template.clone();
        let request = request.clone();
        block_on_manager_handle(&self.handle, async move {
            connector_process::run_action(&template, &request, timeout).await
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
            tracing::info!(
                subscriber = %id,
                topic = %manifest.topic(),
                "subscriber already running"
            );
            return Ok(id);
        }
        tracing::info!(
            subscriber = %id,
            topic = %manifest.topic(),
            manifest_dir = %manifest.dir.display(),
            state_dir = ?manifest.spec.state.as_ref().map(|state| state.dir.as_str()),
            "starting subscriber"
        );
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
            let deadline = tokio::time::Instant::now() + timeout;
            let mut sent_generation =
                send_command_before_deadline(&sender, &command, deadline).await?;
            let mut resend_count = 0usize;
            loop {
                let remaining = deadline
                    .checked_duration_since(tokio::time::Instant::now())
                    .ok_or_else(|| anyhow::anyhow!("timed out waiting for subscriber event"))?;
                let poll_interval = remaining.min(COMMAND_RESTART_POLL_INTERVAL);
                match tokio::time::timeout(poll_interval, rx.recv()).await {
                    Ok(Some(envelope)) => {
                        if terminal_kinds
                            .iter()
                            .any(|kind| kind == &envelope.event.kind)
                            && command_matches_terminal_event(&command, &envelope)
                        {
                            return Ok(envelope);
                        }
                    }
                    Ok(None) => anyhow::bail!("subscriber event bus closed"),
                    Err(_) => {}
                }
                let current_generation = sender.generation();
                if current_generation != sent_generation {
                    if resend_count >= COMMAND_RESTART_RESENDS {
                        anyhow::bail!("subscriber restarted while waiting for subscriber event");
                    }
                    resend_count += 1;
                    sent_generation =
                        send_command_before_deadline(&sender, &command, deadline).await?;
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

async fn send_command_before_deadline(
    sender: &CommandSender,
    command: &SubscriberCommand,
    deadline: tokio::time::Instant,
) -> Result<u64> {
    loop {
        let remaining = deadline
            .checked_duration_since(tokio::time::Instant::now())
            .ok_or_else(|| anyhow::anyhow!("timed out waiting for subscriber event"))?;
        let attempt_generation = sender.generation();
        match sender.send(command).await {
            Ok(()) => return Ok(attempt_generation),
            Err(_) if !sender.is_connected().await => {
                let sleep_for = remaining.min(COMMAND_RESTART_POLL_INTERVAL);
                tokio::time::sleep(sleep_for).await;
            }
            Err(error) => {
                let sleep_for = remaining.min(COMMAND_RESTART_POLL_INTERVAL);
                tokio::time::sleep(sleep_for).await;
                if !sender.is_connected().await || sender.generation() != attempt_generation {
                    continue;
                }
                return Err(error);
            }
        }
    }
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

    fn process_connector_events(
        &self,
        connector_slug: &str,
        connection_slug: &str,
        envelopes: &[EventEnvelope],
    ) -> Result<()> {
        for envelope in envelopes {
            self.process_agent_proxy(connector_slug, connection_slug, envelope)?;
        }
        let result = process_envelope_batch_result(
            envelopes,
            &self.store,
            Some(&self.history_store),
            &self.dispatcher,
            &self.classifier,
            None,
        );
        if result.failed > 0 {
            anyhow::bail!(
                "{} workflow action(s) failed while processing connector event batch",
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
                let Some(proxy) = builtin_agent_proxy(connector_slug) else {
                    return Ok(());
                };
                let prompt = proxy.route_prompt(&target, &message);
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
#[path = "manager_tests.rs"]
mod tests;

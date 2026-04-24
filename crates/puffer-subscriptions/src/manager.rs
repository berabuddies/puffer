//! Process-wide manager that owns the bus, store, supervised subscribers
//! and the router. Workflow tools reach into the manager via a global
//! `OnceLock` set up at puffer startup.

use crate::action::{ActionDispatcher, BuiltinActionDispatcher};
use crate::classify::{Classifier, NullClassifier};
use crate::router::SubscriptionRouter;
use crate::store::SubscriptionStore;
use anyhow::Result;
use puffer_subscriber_runtime::{
    EventBus, Manifest, SubscriberHandle, SubscriberSupervisor, SupervisorConfig,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::runtime::Handle;

/// Builder for [`SubscriptionManager`]. Lets callers swap in custom
/// dispatcher / classifier implementations (e.g. a real LLM-backed
/// classifier) before construction.
pub struct SubscriptionManagerBuilder {
    bus: EventBus,
    store_path: PathBuf,
    dispatcher: Arc<dyn ActionDispatcher>,
    classifier: Arc<dyn Classifier>,
}

impl SubscriptionManagerBuilder {
    /// Starts a builder with the default dispatcher and classifier and
    /// the supplied store path. The bus is freshly constructed.
    pub fn new(store_path: impl Into<PathBuf>) -> Self {
        Self {
            bus: EventBus::new(),
            store_path: store_path.into(),
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

    /// Override the classifier.
    pub fn with_classifier(mut self, classifier: Arc<dyn Classifier>) -> Self {
        self.classifier = classifier;
        self
    }

    /// Loads the store and spawns the router on the supplied Tokio runtime.
    pub fn build(self, handle: Handle) -> Result<SubscriptionManager> {
        let store = Arc::new(SubscriptionStore::load(&self.store_path)?);
        let dispatcher = self.dispatcher.clone();
        let classifier = self.classifier.clone();
        let bus = self.bus.clone();
        let store_for_router = store.clone();
        let router = handle.block_on(async move {
            SubscriptionRouter::spawn(bus, store_for_router, dispatcher, classifier)
        });
        Ok(SubscriptionManager {
            handle,
            bus: self.bus,
            store,
            router: Mutex::new(Some(router)),
            subscribers: Mutex::new(HashMap::new()),
        })
    }
}

/// Process-wide subscription manager. Owns the bus, store, router task,
/// and the set of supervised subscriber children.
pub struct SubscriptionManager {
    handle: Handle,
    bus: EventBus,
    store: Arc<SubscriptionStore>,
    router: Mutex<Option<SubscriptionRouter>>,
    subscribers: Mutex<HashMap<String, SubscriberHandle>>,
}

impl SubscriptionManager {
    /// Returns the underlying spec store.
    pub fn store(&self) -> Arc<SubscriptionStore> {
        self.store.clone()
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
        let handle = self
            .handle
            .block_on(async move { SubscriberSupervisor::spawn(manifest, bus, SupervisorConfig::default()) })?;
        guard.insert(id.clone(), handle);
        Ok(id)
    }

    /// Sends a control command to the named subscriber. Returns an error
    /// when the subscriber is unknown or not running.
    pub fn send_command(
        &self,
        subscriber_id: &str,
        command: &puffer_subscriber_runtime::SubscriberCommand,
    ) -> Result<()> {
        let sender = {
            let guard = self.subscribers.lock().unwrap();
            guard
                .get(subscriber_id)
                .ok_or_else(|| {
                    anyhow::anyhow!("subscriber `{subscriber_id}` is not running")
                })?
                .commands
                .clone()
        };
        self.handle.block_on(async move { sender.send(command).await })
    }

    /// Returns the ids of currently supervised subscribers.
    pub fn subscriber_ids(&self) -> Vec<String> {
        self.subscribers.lock().unwrap().keys().cloned().collect()
    }

    /// Shuts down router and every supervised subscriber. Best-effort.
    pub fn shutdown(&self) {
        if let Some(router) = self.router.lock().unwrap().take() {
            self.handle.block_on(async move { router.shutdown().await });
        }
        let handles: Vec<_> = self
            .subscribers
            .lock()
            .unwrap()
            .drain()
            .map(|(_, h)| h)
            .collect();
        for handle in handles {
            self.handle.block_on(async move { handle.shutdown().await });
        }
    }
}

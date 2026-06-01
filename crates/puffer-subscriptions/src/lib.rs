//! Routes connector events through user-defined workflow trigger bindings
//! (Puffer-side filter, optional LLM classify, one action) and persists
//! binding specs to disk.
//!
//! The unit of configuration is [`WorkflowBindingSpec`]. Legacy names such as
//! `SubscriptionSpec` remain as type aliases while the runtime migrates toward
//! connection-driven workflow terminology.
//!
//! Bindings live as a JSON document at `~/.puffer/workflow_bindings.json`. The
//! [`WorkflowBindingStore`] guards reads/writes with a mutex; mutations are
//! atomic via temp-file rename.
//!
//! The router consumes events from a [`puffer_subscriber_runtime::EventBus`]
//! and dispatches matched events to an [`ActionDispatcher`].

mod action;
mod catalog;
mod catalog_store;
#[cfg(test)]
mod catalog_tests;
mod classify;
mod command_match;
mod connection;
mod connector_process;
mod connector_stream;
mod history;
mod manager;
mod protocol;
mod proxy;
mod router;
mod spec;
mod store;
mod subscriber_manifest;

#[cfg(test)]
mod telegram_e2e_tests;

pub use action::{
    install_connector_action_executor, install_outbound, install_workflow_runner,
    installed_workflow_runner, ActionDispatcher, ActionResult, ActionUsage,
    BuiltinActionDispatcher, ConnectorActionExecutor, Outbound, WorkflowActionOutput,
    WorkflowActionRunner,
};
pub use catalog::{
    builtin_connector_template, builtin_connector_templates, suggested_connection_slug,
    ConnectorActionDefinition, ConnectorPermissionDefinition, ConnectorSlug,
    ConnectorSubscriberTemplate, ConnectorTemplate,
};
pub use catalog_store::{ConnectorCatalogStore, ConnectorCatalogStoreError};
pub use classify::{Classifier, ClassifyDecision, NullClassifier, RemoteClassifier};
pub use connection::{
    ConnectionRecord, ConnectionSlug, ConnectionState, ConnectionStore, ConnectionStoreError,
};
pub use history::{
    now_ms, WorkflowActionLog, WorkflowBindingRun, WorkflowBindingRunStatus, WorkflowHistoryStore,
    WorkflowHistoryStoreError,
};
pub use manager::{ConnectionAuthChecker, SubscriptionManager, SubscriptionManagerBuilder};
pub use protocol::{
    ConnectorActionRequest, ConnectorActionResponse, ConnectorSubscribeCommand,
    ConnectorSubscribeFrame,
};
pub use proxy::{
    builtin_agent_proxy, handle_agent_proxy_event, AgentProxy, AgentProxyBinding,
    AgentProxyDecision, AgentProxyStore, AgentProxyStoreError, TelegramBotAgentProxy,
};
pub use router::{
    prefilter_passes, process_envelope, process_envelope_batch_result, process_envelope_result,
    EnvelopeProcessResult, RouterStats, SubscriptionRouter,
};
pub use spec::{
    filter_matches, render_value_templates, validate_action_spec, validate_spec, ActionGraphNode,
    ActionSpec, FilterSpec, PrefilterSpec, SubscriptionSpec, SubscriptionStatus, TaggedFilterSpec,
    WorkflowBindingSpec, WorkflowBindingStatus,
};
pub use store::{
    SubscriptionStore, SubscriptionStoreError, WorkflowBindingStore, WorkflowBindingStoreError,
};
pub use subscriber_manifest::{
    connection_subscriber_manifest, connection_subscriber_manifest_exists,
    connection_workflow_trigger_supported, connector_runtime_hints,
    connector_workflow_trigger_supported, direct_subscriber_manifest, find_subscriber_manifest,
    SubscriberManifestRoots,
};

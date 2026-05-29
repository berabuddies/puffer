//! Core runtime glue for external "connectors" (Telegram, Slack, …) that
//! bridge an outside platform into a running Puffer process.
//!
//! A connector is a background service that:
//! 1. Listens for inbound messages on its platform.
//! 2. Maps each external conversation (channel/DM/thread) to a Puffer session,
//!    creating one on first contact and resuming it on subsequent messages.
//! 3. Forwards the message text through [`ConnectorRuntime::dispatch`],
//!    which runs one Puffer turn against the stored session and returns
//!    the assistant's final reply.
//! 4. Sends that reply back on the platform.
//!
//! This crate is deliberately framework-agnostic: it exposes a plain
//! blocking [`ConnectorRuntime::dispatch`] function plus the persistent
//! conversation→session map. Platform-specific crates own their own
//! runtime (e.g. Tokio for HTTP-based bots) and call `dispatch` via
//! `spawn_blocking` when they need to keep the async event loop free.
//!
//! ## Feature parity with Hermes
//!
//! The v1 implementation covers:
//! * session creation / resume per conversation
//! * per-user session segmentation inside group chats
//!   (`GroupKeyPolicy::PerUser`)
//! * built-in `/start`, `/new`/`/reset`, `/help`, `/status`, `/usage`
//!   commands shared by every platform
//! * mention-gating in group chats (`require_mention`)
//! * bot-self message filtering
//! * per-platform message splitting with code-fence-safe chunking
//! * bounded exponential-backoff retry for outbound sends
//! * Slack thread-ts preservation in the conversation key
//!
//! ## Deferred to v2 (tracked as `TODO(connector-v2)` in code)
//!
//! * **Media / voice / file ingestion** — would require
//!   `execute_user_prompt` to accept attachments; a core change, not a
//!   connector change.
//! * **Interruption queue** — `/stop` mid-turn + pending-message queue
//!   while the agent is working; requires turning [`ConnectorRuntime`]
//!   into an actor with per-session queues.
//! * **Home channels for cron delivery** — would let scheduled jobs
//!   route output to a specific connector channel; needs the Puffer
//!   cron store to know about the connector runtime.
//! * **Expanded commands** (`/approve`, `/deny`, `/model`, `/provider`,
//!   `/sethome`) — depend on the interruption queue and on threading
//!   session-scoped `AppState` mutations through the connector bridge.
//! * **Typing indicators** — one SDK call per platform; deferred in
//!   favor of the retry + splitter work, which fixes actual correctness
//!   bugs first.

mod catalog;
mod commands;
mod config;
mod connection;
mod protocol;
mod proxy;
mod runtime;
mod session_map;
mod support;
mod traits;

pub use catalog::{
    builtin_connector_template, builtin_connector_templates, ConnectorActionDefinition,
    ConnectorPermissionDefinition, ConnectorSlug, ConnectorTemplate,
};
pub use commands::{handle_builtin_command, BuiltinCommandConfig, CommandOutcome};
pub use config::{ConnectorConfig, ConnectorsConfig};
pub use connection::{
    ConnectionRecord, ConnectionSlug, ConnectionState, ConnectionStore, ConnectionStoreError,
};
pub use protocol::{
    ConnectorActionRequest, ConnectorActionResponse, ConnectorSubscribeCommand,
    ConnectorSubscribeFrame,
};
pub use proxy::{
    handle_agent_proxy_event, AgentProxy, AgentProxyBinding, AgentProxyDecision, AgentProxyStore,
    AgentProxyStoreError,
};
pub use runtime::{ConnectorRuntime, ConnectorRuntimeConfig, DispatchOutcome};
pub use session_map::{ConversationKey, ConversationSessionMap, GroupKeyPolicy};
pub use support::{retry_with_backoff, InboundMessage, MessageSplitter};
pub use traits::{Connector, ConnectorHandle, ConnectorStartError};

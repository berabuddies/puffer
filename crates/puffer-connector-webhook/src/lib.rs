//! HTTP webhook connector for Puffer. Exposes a small axum-backed HTTP
//! surface so any upstream system that can `POST` JSON (n8n, Zapier,
//! cURL, a frontend, another service, …) can talk to a running Puffer
//! process.
//!
//! The connector:
//! * binds to a caller-configured `bind_address`
//! * accepts `POST {path}` (default `/puffer`) with a JSON body of
//!   `{"conversation_id": "...", "message": "...", "user_id": "..."}`
//!   (the `user_id` field is optional)
//! * optionally validates an `Authorization: Bearer <token>` header
//!   against a configured `auth_token`
//! * maps each `conversation_id` to a Puffer session (created on first
//!   message) — the mapping is platform-scoped to `"webhook"` and is
//!   always single-session-per-conversation (no group/per-user
//!   segmentation, since webhook callers don't model group chats)
//! * handles the shared `/start`, `/new`, `/reset`, `/help`, `/status`,
//!   `/usage` slash commands via
//!   [`puffer_connector_core::handle_builtin_command`]
//! * exposes `GET /health` returning `200 OK "puffer"`
//!
//! The axum layer is deliberately thin — auth is enforced in the router
//! and the rest lives in [`handle_command`], which takes an
//! [`InboundMessage`] and can be exercised by unit tests without
//! standing up a TCP listener.

pub mod config;
pub mod connector;
pub mod handler;
pub mod router;

pub use config::WebhookConfig;
pub use connector::WebhookConnector;
pub use handler::{handle_command, PLATFORM_ID};
pub use puffer_connector_core::{CommandOutcome, InboundMessage};
pub use router::{build_router, AppState, WebhookRequest, WebhookResponse};

//! Shared Slack account support for Puffer.
//!
//! This crate owns Slack credential serialization, best-effort local Slack app
//! import, and the blocking Web API client used by runtime workflow tools and
//! connector actions.

pub mod auth;
pub mod client;
pub mod local_import;

pub use auth::{
    connection_description, connector_slug_for_auth, credential_dir, credential_path,
    load_credential, save_credential, SlackAuthKind, SlackCredential,
};
pub use client::{SlackAuthTest, SlackClient};
pub use local_import::{
    import_local_slack_session, normalize_workspace_url, SlackLocalImport, SlackLocalImportOptions,
};

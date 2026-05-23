//! Shared Lark and Feishu connection support for Puffer.
//!
//! This crate owns credential serialization and the blocking OpenAPI client
//! used by runtime workflow tools and direct connector actions.

pub mod auth;
pub mod client;

pub use auth::{
    connection_description, connector_slug_for_auth, credential_dir, credential_path,
    load_credential, save_credential, LarkAuthKind, LarkBrand, LarkCredential,
};
pub use client::{LarkAuthTest, LarkClient, LarkMediaKind};

//! Auth Station OAuth helpers for the `worldagent` provider.
//!
//! Auth Station's `/login` flow returns the final `token` and
//! `refresh_token` directly in the callback URL. There is no PKCE,
//! no code exchange. This crate owns URL building, callback
//! parsing, JWT-payload decoding, and refresh.

#![allow(dead_code)]

mod auth;

//! Auth Station OAuth helpers for the `worldagent` provider.
//!
//! Auth Station's `/login` flow returns the final `token` and
//! `refresh_token` directly in the callback URL. There is no PKCE,
//! no code exchange. This crate owns URL building, callback
//! parsing, JWT-payload decoding, and refresh.

mod auth;

pub use auth::{
    build_login_url, decode_jwt_profile, generate_client_state,
    parse_callback_input, WorldAgentCallback, WorldAgentJwtProfile,
    WorldAgentLoginConfig, WORLDAGENT_AUTH_BASE_URL,
    WORLDAGENT_AUTH_URL_OVERRIDE_ENV, WORLDAGENT_CALLBACK_PATH,
    WORLDAGENT_CALLBACK_PORT, WORLDAGENT_DEFAULT_REDIRECT_URI,
};

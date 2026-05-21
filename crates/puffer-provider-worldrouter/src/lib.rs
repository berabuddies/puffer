//! Auth Station OAuth helpers for the `worldrouter` provider.
//!
//! Auth Station's `/login` flow returns the final `token` and
//! `refresh_token` directly in the callback URL. There is no PKCE,
//! no code exchange. This crate owns URL building, callback
//! parsing, JWT-payload decoding, and refresh.

mod auth;

pub use auth::{
    build_login_url, decode_jwt_profile, exchange_jwt_for_api_key,
    generate_client_state, parse_callback_input, refresh_oauth_token,
    worldrouter_access_token_expires_at_ms,
    ExchangedApiKey, WorldRouterCallback, WorldRouterJwtProfile, WorldRouterLoginConfig,
    WorldRouterOAuthCredentials, WORLDROUTER_AUTH_BASE_URL,
    WORLDROUTER_AUTH_URL_OVERRIDE_ENV, WORLDROUTER_CALLBACK_PATH,
    WORLDROUTER_CALLBACK_PORT, WORLDROUTER_CONTROL_BASE_URL,
    WORLDROUTER_CONTROL_URL_OVERRIDE_ENV, WORLDROUTER_DEFAULT_REDIRECT_URI,
    WORLDROUTER_KEY_ALIAS_PREFIX,
};

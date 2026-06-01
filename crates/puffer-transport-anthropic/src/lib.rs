//! Canonical public surface for Anthropic-compatible transport behavior.
//!
//! This crate keeps the Claude-compatible request builder and OAuth helpers in
//! one stable public API while preserving internal module boundaries for auth,
//! fingerprint generation, and request shaping.

mod auth;
mod cch;
mod fingerprint;
mod request;
mod response;
mod usage;

pub use auth::{
    build_authorization_url, create_api_key, exchange_authorization_code,
    exchange_authorization_code_with_client, fetch_user_roles, generate_pkce,
    get_session_ingress_auth, parse_authorization_input, refresh_oauth_token,
    refresh_oauth_token_with_client, should_use_claude_ai_auth, AnthropicAuth,
    AnthropicOAuthConfig, AnthropicOAuthCredentials, AnthropicPkce, AnthropicUserRoles,
    ANTHROPIC_ALL_SCOPES, ANTHROPIC_API_BASE_URL, ANTHROPIC_CLAUDE_AI_INFERENCE_SCOPE,
    ANTHROPIC_CLAUDE_AI_SCOPES, ANTHROPIC_MANUAL_REDIRECT_URL, ANTHROPIC_TOKEN_URL,
    CLAUDE_AI_AUTHORIZE_URL, CONSOLE_AUTHORIZE_URL, OAUTH_BETA_HEADER,
};
pub use cch::finalize_cch_body;
pub use fingerprint::compute_fingerprint;
pub use request::{
    anthropic_user_agent, attribution_header, build_messages_request,
    build_messages_request_with_tools, AnthropicMessage, AnthropicModelRequest,
    AnthropicRequestConfig, AnthropicToolChoice, AnthropicToolDefinition, BuiltAnthropicRequest,
};
pub use response::{
    AnthropicContentBlock, AnthropicMessageResponse, AnthropicTextBlock, AnthropicToolUseBlock,
    AnthropicUnknownBlock,
};
pub use usage::{fetch_oauth_usage, AnthropicExtraUsage, AnthropicRateLimit, AnthropicUtilization};

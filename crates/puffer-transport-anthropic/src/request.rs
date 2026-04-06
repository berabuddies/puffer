use crate::{compute_fingerprint, AnthropicAuth, OAUTH_BETA_HEADER};
use anyhow::{anyhow, Result};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::env;

const ANTHROPIC_VERSION_HEADER: &str = "2023-06-01";
const CLAUDE_CODE_BETA_HEADER: &str = "claude-code-20250219";
const CONTEXT_MANAGEMENT_BETA_HEADER: &str = "context-management-2025-06-27";
const INTERLEAVED_THINKING_BETA_HEADER: &str = "interleaved-thinking-2025-05-14";
const PROMPT_CACHING_SCOPE_BETA_HEADER: &str = "prompt-caching-scope-2026-01-05";
const STAINLESS_LANG: &str = "js";
const STAINLESS_RUNTIME: &str = "node";
const STAINLESS_PACKAGE_VERSION: &str = "0.80.0";

/// Represents a minimal Anthropic messages request body.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AnthropicModelRequest {
    pub model: String,
    pub max_tokens: u32,
    pub messages: Vec<AnthropicMessage>,
}

/// Represents one Anthropic message block used in the request body.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AnthropicMessage {
    pub role: String,
    pub content: String,
}

/// Describes a tool exposed to Anthropic's messages API.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AnthropicToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// Selects how Anthropic should choose from the declared tool list.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnthropicToolChoice {
    Auto,
    Any,
    Tool { name: String },
}

/// Carries runtime configuration for building an Anthropic request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnthropicRequestConfig {
    pub base_url: String,
    pub session_id: String,
    pub custom_headers: IndexMap<String, String>,
    pub remote_container_id: Option<String>,
    pub remote_session_id: Option<String>,
    pub client_app: Option<String>,
    pub entrypoint: String,
    pub user_type: String,
    pub version: String,
    pub workload: Option<String>,
    pub additional_protection: bool,
    pub cch_enabled: bool,
    pub auth: AnthropicAuth,
    pub beta_header: Option<String>,
    pub client_request_id: Option<String>,
}

/// Stores the ordered request shape used by tests and higher-level HTTP executors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltAnthropicRequest {
    pub method: &'static str,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub attribution_prefix_block: String,
}

/// Builds an Anthropic `/v1/messages` request while preserving Claude-like header order.
pub fn build_messages_request(
    config: &AnthropicRequestConfig,
    payload: &AnthropicModelRequest,
) -> Result<BuiltAnthropicRequest> {
    build_messages_request_with_tools(config, payload, &[], None)
}

/// Builds an Anthropic `/v1/messages` request with tool metadata and optional tool choice.
pub fn build_messages_request_with_tools(
    config: &AnthropicRequestConfig,
    payload: &AnthropicModelRequest,
    tools: &[AnthropicToolDefinition],
    tool_choice: Option<&AnthropicToolChoice>,
) -> Result<BuiltAnthropicRequest> {
    let first_user_text = payload
        .messages
        .iter()
        .find(|message| message.role == "user")
        .map(|message| message.content.as_str())
        .unwrap_or("");
    let fingerprint = compute_fingerprint(first_user_text, &config.version);

    let mut headers = vec![
        ("Accept".to_string(), "application/json".to_string()),
        ("Content-Type".to_string(), "application/json".to_string()),
        ("User-Agent".to_string(), anthropic_user_agent(config)),
        (
            "X-Claude-Code-Session-Id".to_string(),
            config.session_id.clone(),
        ),
        ("X-Stainless-Arch".to_string(), stainless_arch().to_string()),
        ("X-Stainless-Lang".to_string(), STAINLESS_LANG.to_string()),
        ("X-Stainless-OS".to_string(), stainless_os().to_string()),
        (
            "X-Stainless-Package-Version".to_string(),
            STAINLESS_PACKAGE_VERSION.to_string(),
        ),
        ("X-Stainless-Retry-Count".to_string(), "0".to_string()),
        (
            "X-Stainless-Runtime".to_string(),
            STAINLESS_RUNTIME.to_string(),
        ),
        (
            "X-Stainless-Runtime-Version".to_string(),
            stainless_runtime_version(),
        ),
        ("X-Stainless-Timeout".to_string(), stainless_timeout()),
    ];
    headers.extend(
        config
            .custom_headers
            .iter()
            .map(|(key, value)| (key.clone(), value.clone())),
    );
    if let Some(container_id) = &config.remote_container_id {
        headers.push((
            "x-claude-remote-container-id".to_string(),
            container_id.clone(),
        ));
    }
    if let Some(remote_session_id) = &config.remote_session_id {
        headers.push((
            "x-claude-remote-session-id".to_string(),
            remote_session_id.clone(),
        ));
    }
    if let Some(client_app) = &config.client_app {
        headers.push(("x-client-app".to_string(), client_app.clone()));
    }
    if config.additional_protection {
        headers.push((
            "x-anthropic-additional-protection".to_string(),
            "true".to_string(),
        ));
    }
    headers.push((
        "anthropic-beta".to_string(),
        default_beta_header(config, &payload.model),
    ));
    headers.push((
        "anthropic-dangerous-direct-browser-access".to_string(),
        "true".to_string(),
    ));
    headers.push((
        "anthropic-version".to_string(),
        ANTHROPIC_VERSION_HEADER.to_string(),
    ));
    append_auth_headers(&mut headers, config);
    headers.push(("x-app".to_string(), "cli".to_string()));
    if let Some(client_request_id) = &config.client_request_id {
        headers.push(("x-client-request-id".to_string(), client_request_id.clone()));
    }

    Ok(BuiltAnthropicRequest {
        method: "POST",
        url: format!(
            "{}/v1/messages?beta=true",
            config.base_url.trim_end_matches('/')
        ),
        headers,
        body: build_request_body(payload, tools, tool_choice)?,
        attribution_prefix_block: attribution_header(config, &fingerprint),
    })
}

/// Returns the Claude-style Anthropic user-agent string.
pub fn anthropic_user_agent(config: &AnthropicRequestConfig) -> String {
    let mut suffix_parts = vec![config.user_type.clone(), config.entrypoint.clone()];
    if let Some(client_app) = &config.client_app {
        suffix_parts.push(format!("client-app/{client_app}"));
    }
    if let Some(workload) = &config.workload {
        suffix_parts.push(format!("workload/{workload}"));
    }
    format!(
        "claude-cli/{} ({})",
        config.version,
        suffix_parts.join(", ")
    )
}

/// Builds the Anthropic attribution system block, including the optional CCH placeholder.
pub fn attribution_header(config: &AnthropicRequestConfig, fingerprint: &str) -> String {
    let cch = if config.cch_enabled {
        " cch=00000;"
    } else {
        ""
    };
    let workload = config
        .workload
        .as_ref()
        .map(|value| format!(" cc_workload={value};"))
        .unwrap_or_default();
    format!(
        "x-anthropic-billing-header: cc_version={}.{}; cc_entrypoint={};{}{}",
        config.version, fingerprint, config.entrypoint, cch, workload
    )
}

fn append_auth_headers(headers: &mut Vec<(String, String)>, config: &AnthropicRequestConfig) {
    match &config.auth {
        AnthropicAuth::None => {}
        AnthropicAuth::ApiKey(key) => {
            headers.push(("x-api-key".to_string(), key.clone()));
        }
        AnthropicAuth::OAuthBearer(token) => {
            headers.push(("Authorization".to_string(), format!("Bearer {token}")));
        }
        AnthropicAuth::SessionIngress {
            token,
            organization_uuid,
        } => {
            if token.starts_with("sk-ant-sid") {
                headers.push(("Cookie".to_string(), format!("sessionKey={token}")));
                if let Some(org_uuid) = organization_uuid {
                    headers.push(("X-Organization-Uuid".to_string(), org_uuid.clone()));
                }
            } else {
                headers.push(("Authorization".to_string(), format!("Bearer {token}")));
            }
        }
    }
}

fn default_beta_header(config: &AnthropicRequestConfig, model: &str) -> String {
    if let Some(header) = &config.beta_header {
        return header.clone();
    }
    match &config.auth {
        AnthropicAuth::OAuthBearer(_) => OAUTH_BETA_HEADER.to_string(),
        _ => default_agentic_beta_header(model),
    }
}

fn default_agentic_beta_header(model: &str) -> String {
    let model_name = model.to_ascii_lowercase();
    let mut betas = vec![
        INTERLEAVED_THINKING_BETA_HEADER.to_string(),
        CONTEXT_MANAGEMENT_BETA_HEADER.to_string(),
        PROMPT_CACHING_SCOPE_BETA_HEADER.to_string(),
    ];
    if !model_name.contains("claude-3-") {
        betas.push(CLAUDE_CODE_BETA_HEADER.to_string());
    }
    betas.join(",")
}

fn stainless_arch() -> &'static str {
    match env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        "x86" => "x86",
        other => other,
    }
}

fn stainless_os() -> &'static str {
    match env::consts::OS {
        "macos" => "MacOS",
        "linux" => "Linux",
        "windows" => "Windows",
        other => other,
    }
}

fn stainless_runtime_version() -> String {
    env::var("PUFFER_STAINLESS_RUNTIME_VERSION").unwrap_or_else(|_| "v24.3.0".to_string())
}

fn stainless_timeout() -> String {
    let timeout_ms = env::var("API_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(600_000);
    (timeout_ms / 1000).to_string()
}

fn build_request_body(
    payload: &AnthropicModelRequest,
    tools: &[AnthropicToolDefinition],
    tool_choice: Option<&AnthropicToolChoice>,
) -> Result<String> {
    let mut body = serde_json::to_value(payload)?;
    let object = body
        .as_object_mut()
        .ok_or_else(|| anyhow!("Anthropic request payload must serialize to an object"))?;
    if !tools.is_empty() {
        object.insert("tools".to_string(), serde_json::to_value(tools)?);
    }
    if let Some(choice) = tool_choice {
        object.insert("tool_choice".to_string(), serde_json::to_value(choice)?);
    }
    Ok(serde_json::to_string(&body)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn base_config(auth: AnthropicAuth) -> AnthropicRequestConfig {
        AnthropicRequestConfig {
            base_url: "https://api.anthropic.com".to_string(),
            session_id: "session-1".to_string(),
            custom_headers: IndexMap::new(),
            remote_container_id: None,
            remote_session_id: None,
            client_app: None,
            entrypoint: "cli".to_string(),
            user_type: "external".to_string(),
            version: "1.2.3".to_string(),
            workload: None,
            additional_protection: false,
            cch_enabled: true,
            auth,
            beta_header: None,
            client_request_id: Some("req-1".to_string()),
        }
    }

    #[test]
    fn oauth_request_preserves_expected_header_order() {
        let request = build_messages_request(
            &base_config(AnthropicAuth::OAuthBearer("token-1".to_string())),
            &AnthropicModelRequest {
                model: "claude-sonnet-4-5".to_string(),
                max_tokens: 1024,
                messages: vec![AnthropicMessage {
                    role: "user".to_string(),
                    content: "hello".to_string(),
                }],
            },
        )
        .unwrap();

        let keys: Vec<&str> = request
            .headers
            .iter()
            .map(|(key, _)| key.as_str())
            .collect();
        assert_eq!(
            keys,
            vec![
                "Accept",
                "Content-Type",
                "User-Agent",
                "X-Claude-Code-Session-Id",
                "X-Stainless-Arch",
                "X-Stainless-Lang",
                "X-Stainless-OS",
                "X-Stainless-Package-Version",
                "X-Stainless-Retry-Count",
                "X-Stainless-Runtime",
                "X-Stainless-Runtime-Version",
                "X-Stainless-Timeout",
                "anthropic-beta",
                "anthropic-dangerous-direct-browser-access",
                "anthropic-version",
                "Authorization",
                "x-app",
                "x-client-request-id",
            ]
        );
        assert!(request
            .attribution_prefix_block
            .starts_with("x-anthropic-billing-header: cc_version=1.2.3."));
    }

    #[test]
    fn session_ingress_sid_uses_cookie_auth() {
        let request = build_messages_request(
            &AnthropicRequestConfig {
                auth: AnthropicAuth::SessionIngress {
                    token: "sk-ant-sid-123".to_string(),
                    organization_uuid: Some("org-1".to_string()),
                },
                ..base_config(AnthropicAuth::ApiKey("unused".to_string()))
            },
            &AnthropicModelRequest {
                model: "claude-sonnet-4-5".to_string(),
                max_tokens: 1,
                messages: vec![],
            },
        )
        .unwrap();
        assert!(request
            .headers
            .iter()
            .any(|(key, value)| key == "Cookie" && value == "sessionKey=sk-ant-sid-123"));
        assert!(request
            .headers
            .iter()
            .any(|(key, value)| key == "X-Organization-Uuid" && value == "org-1"));
    }

    #[test]
    fn request_with_tools_serializes_tool_payload() {
        let request = build_messages_request_with_tools(
            &base_config(AnthropicAuth::ApiKey("key-1".to_string())),
            &AnthropicModelRequest {
                model: "claude-sonnet-4-5".to_string(),
                max_tokens: 256,
                messages: vec![AnthropicMessage {
                    role: "user".to_string(),
                    content: "inspect the repo".to_string(),
                }],
            },
            &[AnthropicToolDefinition {
                name: "bash".to_string(),
                description: "Runs a shell command".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "command": {"type": "string"}
                    },
                    "required": ["command"]
                }),
            }],
            Some(&AnthropicToolChoice::Tool {
                name: "bash".to_string(),
            }),
        )
        .unwrap();

        let body: Value = serde_json::from_str(&request.body).unwrap();
        assert_eq!(body["tools"][0]["name"], "bash");
        assert_eq!(body["tool_choice"]["type"], "tool");
        assert_eq!(body["tool_choice"]["name"], "bash");
    }

    #[test]
    fn none_auth_omits_auth_headers() {
        let request = build_messages_request(
            &base_config(AnthropicAuth::None),
            &AnthropicModelRequest {
                model: "claude-sonnet-4-5".to_string(),
                max_tokens: 256,
                messages: vec![AnthropicMessage {
                    role: "user".to_string(),
                    content: "inspect the repo".to_string(),
                }],
            },
        )
        .unwrap();
        assert!(!request
            .headers
            .iter()
            .any(|(key, _)| key.eq_ignore_ascii_case("authorization")));
        assert!(!request
            .headers
            .iter()
            .any(|(key, _)| key.eq_ignore_ascii_case("x-api-key")));
    }

    #[test]
    fn request_with_tools_preserves_rich_schema_and_expanded_name() {
        let request = build_messages_request_with_tools(
            &base_config(AnthropicAuth::ApiKey("key-1".to_string())),
            &AnthropicModelRequest {
                model: "claude-sonnet-4-5".to_string(),
                max_tokens: 256,
                messages: vec![AnthropicMessage {
                    role: "user".to_string(),
                    content: "inspect the repo".to_string(),
                }],
            },
            &[AnthropicToolDefinition {
                name: "workspace/search:text.v2".to_string(),
                description: "Searches workspace text".to_string(),
                input_schema: json!({
                    "oneOf": [
                        {
                            "type": "object",
                            "properties": {
                                "query": {"type": "string"},
                                "path": {"type": ["string", "null"]}
                            },
                            "required": ["query"]
                        },
                        {"type": "boolean"}
                    ]
                }),
            }],
            Some(&AnthropicToolChoice::Auto),
        )
        .unwrap();

        let body: Value = serde_json::from_str(&request.body).unwrap();
        assert_eq!(body["tools"][0]["name"], "workspace/search:text.v2");
        assert_eq!(
            body["tools"][0]["input_schema"]["oneOf"][0]["properties"]["path"]["type"],
            json!(["string", "null"])
        );
        assert_eq!(
            body["tools"][0]["input_schema"]["oneOf"][1]["type"],
            "boolean"
        );
        assert_eq!(body["tool_choice"]["type"], "auto");
    }

    #[test]
    fn remote_and_custom_headers_preserve_expected_order() {
        let mut custom_headers = IndexMap::new();
        custom_headers.insert("x-custom-a".to_string(), "one".to_string());
        custom_headers.insert("x-custom-b".to_string(), "two".to_string());
        let request = build_messages_request(
            &AnthropicRequestConfig {
                custom_headers,
                remote_container_id: Some("container-1".to_string()),
                remote_session_id: Some("remote-1".to_string()),
                client_app: Some("sdk-app/1.0".to_string()),
                additional_protection: true,
                auth: AnthropicAuth::ApiKey("sk-ant".to_string()),
                ..base_config(AnthropicAuth::ApiKey("unused".to_string()))
            },
            &AnthropicModelRequest {
                model: "claude-sonnet-4-5".to_string(),
                max_tokens: 16,
                messages: vec![AnthropicMessage {
                    role: "user".to_string(),
                    content: "hello".to_string(),
                }],
            },
        )
        .unwrap();

        let keys = request
            .headers
            .iter()
            .map(|(key, _)| key.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            keys,
            vec![
                "Accept",
                "Content-Type",
                "User-Agent",
                "X-Claude-Code-Session-Id",
                "X-Stainless-Arch",
                "X-Stainless-Lang",
                "X-Stainless-OS",
                "X-Stainless-Package-Version",
                "X-Stainless-Retry-Count",
                "X-Stainless-Runtime",
                "X-Stainless-Runtime-Version",
                "X-Stainless-Timeout",
                "x-custom-a",
                "x-custom-b",
                "x-claude-remote-container-id",
                "x-claude-remote-session-id",
                "x-client-app",
                "x-anthropic-additional-protection",
                "anthropic-beta",
                "anthropic-dangerous-direct-browser-access",
                "anthropic-version",
                "x-api-key",
                "x-app",
                "x-client-request-id",
            ]
        );
    }

    #[test]
    fn non_sid_session_ingress_uses_bearer_auth() {
        let request = build_messages_request(
            &AnthropicRequestConfig {
                auth: AnthropicAuth::SessionIngress {
                    token: "session-token".to_string(),
                    organization_uuid: Some("org-1".to_string()),
                },
                ..base_config(AnthropicAuth::ApiKey("unused".to_string()))
            },
            &AnthropicModelRequest {
                model: "claude-sonnet-4-5".to_string(),
                max_tokens: 1,
                messages: vec![],
            },
        )
        .unwrap();
        assert!(request
            .headers
            .iter()
            .any(|(key, value)| key == "Authorization" && value == "Bearer session-token"));
        assert!(!request.headers.iter().any(|(key, _)| key == "Cookie"));
    }

    #[test]
    fn attribution_header_omits_cch_when_disabled_and_includes_workload() {
        let config = AnthropicRequestConfig {
            cch_enabled: false,
            workload: Some("batch".to_string()),
            ..base_config(AnthropicAuth::ApiKey("sk-ant".to_string()))
        };
        let header = attribution_header(&config, "abc");
        assert!(header.contains("cc_workload=batch"));
        assert!(!header.contains("cch=00000"));
    }

    #[test]
    fn oauth_beta_header_can_be_overridden() {
        let request = build_messages_request(
            &AnthropicRequestConfig {
                beta_header: Some("custom-beta".to_string()),
                auth: AnthropicAuth::OAuthBearer("token-1".to_string()),
                ..base_config(AnthropicAuth::ApiKey("unused".to_string()))
            },
            &AnthropicModelRequest {
                model: "claude-sonnet-4-5".to_string(),
                max_tokens: 8,
                messages: vec![AnthropicMessage {
                    role: "user".to_string(),
                    content: "hello".to_string(),
                }],
            },
        )
        .unwrap();
        assert!(request
            .headers
            .iter()
            .any(|(key, value)| key == "anthropic-beta" && value == "custom-beta"));
    }

    #[test]
    fn api_key_request_uses_agentic_beta_header_bundle() {
        let request = build_messages_request(
            &base_config(AnthropicAuth::ApiKey("sk-ant".to_string())),
            &AnthropicModelRequest {
                model: "claude-haiku-4-5-20251001".to_string(),
                max_tokens: 8,
                messages: vec![AnthropicMessage {
                    role: "user".to_string(),
                    content: "hello".to_string(),
                }],
            },
        )
        .unwrap();
        let beta = request
            .headers
            .iter()
            .find(|(key, _)| key == "anthropic-beta")
            .map(|(_, value)| value.as_str())
            .expect("beta header");
        assert_eq!(
            beta,
            "interleaved-thinking-2025-05-14,context-management-2025-06-27,prompt-caching-scope-2026-01-05,claude-code-20250219"
        );
        assert_eq!(
            request.url,
            "https://api.anthropic.com/v1/messages?beta=true"
        );
    }

    #[test]
    fn user_agent_includes_client_app_and_workload() {
        let user_agent = anthropic_user_agent(&AnthropicRequestConfig {
            client_app: Some("sdk-app/1.0".to_string()),
            workload: Some("cron".to_string()),
            ..base_config(AnthropicAuth::ApiKey("sk-ant".to_string()))
        });
        assert!(user_agent.contains("client-app/sdk-app/1.0"));
        assert!(user_agent.contains("workload/cron"));
    }
}

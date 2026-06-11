//! OpenAI-backed contact inference helpers.

use super::daemon_contacts_trace::ContactInferTrace;
use crate::auth_credentials::to_registry_oauth_credential_openai;
use crate::daemon::DaemonState;
use anyhow::{anyhow, Context, Result};
use puffer_config::{ProxyConfig, PufferConfig};
use puffer_provider_openai::{
    build_json_post_request, extract_responses_tool_calls, parse_responses_response,
    refresh_oauth_token, refresh_oauth_token_with_client, BuiltOpenAIRequest, OpenAIAuth,
    OpenAIRequestConfig, OpenAIResponseToolCall, OpenAIResponsesResponse, OpenAIResponsesTool,
    OPENAI_TOKEN_URL,
};
use puffer_provider_registry::{AuthStore, ProviderDescriptor, StoredCredential};
use puffer_subscriptions::{normalize_contact_ids, ConnectorContact, ContactProposal};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

const INFER_SYSTEM_PROMPT: &str = r#"You infer the user's important contacts from connector candidates. You may only call CreateContact JSON tool calls.

Rules:

Propose as many reasonable contacts as possible, up to 30.
Favor inclusion when a candidate has plausible evidence of a real person or useful relationship, even if the evidence is incomplete.
Do not create contacts for spam, automated senders, mailing lists, newsletters, no-reply addresses, support bots, sales outreach, transactional notifications, or one-off low-value interactions.
Prefer candidates with evidence of a real relationship, such as repeated interaction, two-way conversation, collaboration, planning, personal connection, work coordination, business relevance, or trusted communication.
Do not hallucinate facts. Only use evidence from the candidate's name and chat history.
Deduplicate contacts that clearly refer to the same person.
Each description must be exactly two sentences.
Write each description as a natural summary of the user's relationship with the contact.
Describe who the contact appears to be, how they interact with the user, and what topics or work connect them.
Do not use formulaic justification openers about importance or spam status.
Keep descriptions specific, concise, and grounded in the chat history."#;
const TRACE_PROMPT_LIMIT: usize = 16_000;
const MODEL_CONTEXT_SNIPPET_LIMIT: usize = 30;
const OPENAI_PROVIDER_ID: &str = "openai";
const OPENAI_CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
const OPENAI_CODEX_COMPAT_VERSION: &str = "0.125.0";
const OPENAI_CODEX_ORIGINATOR: &str = "codex_cli_rs";

struct ContactOpenAiExecution {
    provider_id: String,
    default_model: Option<String>,
    auth_store: AuthStore,
    auth_path: PathBuf,
    request_config: OpenAIRequestConfig,
    refresh_token: Option<String>,
    proxy: ProxyConfig,
}

struct ContactHttpResponse {
    status: reqwest::StatusCode,
    text: String,
}

/// Returns the system prompt used for model-backed contact inference.
pub(crate) fn contact_infer_system_prompt() -> &'static str {
    INFER_SYSTEM_PROMPT
}

/// Infers contact proposals from ranked connector candidates.
pub(crate) fn infer_proposals(
    state: &DaemonState,
    candidates: &[ConnectorContact],
    limit: usize,
    model: Option<&str>,
    trace: &ContactInferTrace<'_>,
) -> Result<Vec<ContactProposal>> {
    match openai_execution(state)? {
        Some(mut execution) => {
            let model = contact_inference_model(&execution, model);
            infer_with_openai(&mut execution, &model, candidates, limit, trace)
        }
        None => {
            trace.message(
                "assistant",
                "Local inference fallback",
                "No OpenAI provider credential was available for contact inference, so Puffer used local heuristic proposals.",
            );
            Ok(trace_heuristic_proposals(candidates, limit, trace))
        }
    }
}

/// Returns a compact candidate sample suitable for trace output.
pub(crate) fn candidate_trace_sample(candidates: &[ConnectorContact]) -> Value {
    json!(candidates
        .iter()
        .take(12)
        .map(|candidate| json!({
            "id": candidate.id,
            "name": candidate.name,
            "score": candidate.score,
            "context_count": candidate.context.len(),
        }))
        .collect::<Vec<_>>())
}

fn infer_with_openai(
    execution: &mut ContactOpenAiExecution,
    model: &str,
    candidates: &[ConnectorContact],
    limit: usize,
    trace: &ContactInferTrace<'_>,
) -> Result<Vec<ContactProposal>> {
    let compact = compact_candidates(candidates);
    let user_prompt = candidate_prompt_json(&compact)?;
    trace.message(
        "user",
        "Candidate prompt",
        format!(
            "```json\n{}\n```",
            truncate_text(&user_prompt, TRACE_PROMPT_LIMIT)
        ),
    );
    let body = contact_inference_request_body(model, &user_prompt);
    let call_id = trace.tool_id("OpenAIContactInference");
    trace.tool_event(
        &call_id,
        "OpenAIContactInference",
        "running",
        "Requesting optional CreateContact tool calls.",
        json!({
            "model": model,
            "candidate_count": compact.len(),
            "limit": limit,
            "tool_choice": "auto",
            "base_url": execution.request_config.base_url,
            "responses_path": execution.request_config.responses_path,
        }),
        Value::Null,
    );
    let response = match send_contact_openai_request(execution, &body) {
        Ok(response) => response,
        Err(err) => {
            trace.tool_event(
                &call_id,
                "OpenAIContactInference",
                "failed",
                "OpenAI contact inference failed.",
                json!({
                    "model": model,
                    "candidate_count": compact.len(),
                    "limit": limit,
                    "tool_choice": "auto",
                    "base_url": execution.request_config.base_url,
                    "responses_path": execution.request_config.responses_path,
                }),
                json!({ "error": err.to_string() }),
            );
            return Err(err);
        }
    };
    let tool_calls = extract_responses_tool_calls(&response)?;
    let proposals =
        attach_candidate_avatars(parse_openai_proposals(&tool_calls, limit), candidates);
    trace.tool_event(
        &call_id,
        "OpenAIContactInference",
        "completed",
        "OpenAI returned contact inference output.",
        json!({
            "model": model,
            "candidate_count": compact.len(),
            "limit": limit,
            "tool_choice": "auto",
            "base_url": execution.request_config.base_url,
            "responses_path": execution.request_config.responses_path,
        }),
        json!({
            "proposal_count": proposals.len(),
            "tool_call_count": tool_calls.len(),
        }),
    );
    for proposal in &proposals {
        trace.tool_event(
            &trace.tool_id("CreateContact"),
            "CreateContact",
            "completed",
            &format!("Proposed contact {}", proposal.name),
            json!(proposal),
            json!({ "status": "proposed" }),
        );
    }
    Ok(proposals)
}

fn contact_inference_request_body(model: &str, user_prompt: &str) -> Value {
    json!({
        "model": model,
        "instructions": INFER_SYSTEM_PROMPT,
        "input": user_prompt,
        "tools": [create_contact_tool()],
        "tool_choice": "auto",
        "store": false
    })
}

fn create_contact_tool() -> OpenAIResponsesTool {
    OpenAIResponsesTool {
        kind: "function".to_string(),
        name: "CreateContact".to_string(),
        description: "Create one grouped contact proposal.".to_string(),
        strict: false,
        parameters: json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "name": {"type": "string"},
                "description": {"type": "string"},
                "avatar": {"type": ["string", "null"]},
                "contact_ids": {
                    "type": "array",
                    "items": {"type": "string"}
                }
            },
            "required": ["name", "description", "contact_ids"]
        }),
        filters: None,
        user_location: None,
        external_web_access: None,
    }
}

fn compact_candidates(candidates: &[ConnectorContact]) -> Vec<Value> {
    candidates
        .iter()
        .map(|candidate| {
            json!({
                "id": candidate.id,
                "name": candidate.name,
                "score": candidate.score,
                "context": candidate.context.iter().take(MODEL_CONTEXT_SNIPPET_LIMIT).map(|ctx| json!({
                    "kind": ctx.kind,
                    "text": ctx.text,
                    "timestamp_ms": ctx.timestamp_ms,
                })).collect::<Vec<_>>(),
            })
        })
        .collect()
}

fn candidate_prompt_json(candidates: &[Value]) -> Result<String> {
    serde_json::to_string_pretty(candidates).context("format contact inference candidate prompt")
}

fn send_contact_openai_request(
    execution: &mut ContactOpenAiExecution,
    body: &Value,
) -> Result<OpenAIResponsesResponse> {
    let request = build_contact_openai_request(&execution.request_config, body)?;
    let response = send_built_openai_request(&request, &execution.proxy)?;
    if response.status != reqwest::StatusCode::UNAUTHORIZED || execution.refresh_token.is_none() {
        return parse_contact_openai_response(response, &request.url);
    }

    refresh_contact_openai_oauth(execution)
        .context("failed to refresh OpenAI OAuth credentials after 401")?;
    let retry = build_contact_openai_request(&execution.request_config, body)?;
    let retry_response = send_built_openai_request(&retry, &execution.proxy)?;
    parse_contact_openai_response(retry_response, &retry.url)
}

fn build_contact_openai_request(
    config: &OpenAIRequestConfig,
    body: &Value,
) -> Result<BuiltOpenAIRequest> {
    let path = config.responses_path.as_deref().unwrap_or("/v1/responses");
    build_json_post_request(config, path, body).context("build OpenAI contact inference request")
}

fn send_built_openai_request(
    request: &BuiltOpenAIRequest,
    proxy: &ProxyConfig,
) -> Result<ContactHttpResponse> {
    let client = puffer_core::blocking_client_for_url(
        proxy,
        puffer_core::HttpPurpose::Model,
        &request.url,
        Duration::from_secs(300),
    )
    .unwrap_or_else(|_| reqwest::blocking::Client::new());
    let mut builder = client.post(&request.url);
    for (key, value) in &request.headers {
        builder = builder.header(key, value);
    }
    let response = builder
        .body(request.body.clone())
        .send()
        .with_context(|| format!("send OpenAI contact inference request to {}", request.url))?;
    let status = response.status();
    let text = response.text().with_context(|| {
        format!(
            "read OpenAI contact inference response from {}",
            request.url
        )
    })?;
    Ok(ContactHttpResponse { status, text })
}

fn parse_contact_openai_response(
    response: ContactHttpResponse,
    url: &str,
) -> Result<OpenAIResponsesResponse> {
    if !response.status.is_success() {
        return Err(openai_status_error(response.status, url));
    }
    parse_responses_response(&response.text).context("parse OpenAI contact inference response")
}

fn openai_status_error(status: reqwest::StatusCode, url: &str) -> anyhow::Error {
    let class = if status.is_client_error() {
        "client error"
    } else if status.is_server_error() {
        "server error"
    } else {
        "error"
    };
    anyhow!(
        "OpenAI contact inference request returned an error status: HTTP status {class} ({status}) for url ({url})"
    )
}

fn refresh_contact_openai_oauth(execution: &mut ContactOpenAiExecution) -> Result<()> {
    let refresh_token = execution
        .refresh_token
        .clone()
        .context("missing refresh token for OpenAI OAuth retry")?;
    let refreshed = match puffer_core::blocking_client_for_url(
        &execution.proxy,
        puffer_core::HttpPurpose::OAuth,
        OPENAI_TOKEN_URL,
        Duration::from_secs(60),
    ) {
        Ok(client) => refresh_oauth_token_with_client(&client, &refresh_token),
        Err(_) => refresh_oauth_token(&refresh_token),
    }?;
    let stored = to_registry_oauth_credential_openai(refreshed);
    execution.request_config.auth = OpenAIAuth::OAuthBearer(stored.access_token.clone());
    execution.request_config.account_id = stored.account_id.clone();
    execution.refresh_token = Some(stored.refresh_token.clone());
    execution
        .auth_store
        .set_oauth(execution.provider_id.clone(), stored);
    execution
        .auth_store
        .save(&execution.auth_path)
        .context("save refreshed OpenAI OAuth credentials")
}

fn parse_openai_proposals(calls: &[OpenAIResponseToolCall], limit: usize) -> Vec<ContactProposal> {
    let mut proposals = Vec::new();
    for call in calls {
        if call.name != "CreateContact" {
            continue;
        }
        if let Some(proposal) = proposal_from_value(call.arguments.clone()) {
            proposals.push(proposal);
        }
    }
    proposals.truncate(limit);
    proposals
}

fn proposal_from_value(value: Value) -> Option<ContactProposal> {
    let mut proposal = serde_json::from_value::<ContactProposal>(value).ok()?;
    proposal.contact_ids = normalize_contact_ids(proposal.contact_ids);
    (!proposal.name.trim().is_empty() && !proposal.contact_ids.is_empty()).then_some(proposal)
}

fn trace_heuristic_proposals(
    candidates: &[ConnectorContact],
    limit: usize,
    trace: &ContactInferTrace<'_>,
) -> Vec<ContactProposal> {
    let proposals = heuristic_proposals(candidates, limit);
    for proposal in &proposals {
        trace.tool_event(
            &trace.tool_id("CreateContact"),
            "CreateContact",
            "completed",
            &format!("Proposed contact {}", proposal.name),
            json!(proposal),
            json!({ "status": "heuristic" }),
        );
    }
    proposals
}

fn attach_candidate_avatars(
    mut proposals: Vec<ContactProposal>,
    candidates: &[ConnectorContact],
) -> Vec<ContactProposal> {
    let avatars = candidates
        .iter()
        .filter_map(|candidate| {
            let avatar = candidate.avatar.as_deref()?.trim();
            if avatar.is_empty() {
                return None;
            }
            Some((candidate.id.as_str(), avatar.to_string()))
        })
        .collect::<HashMap<_, _>>();
    if avatars.is_empty() {
        return proposals;
    }
    for proposal in &mut proposals {
        if proposal
            .avatar
            .as_deref()
            .map(str::trim)
            .is_some_and(|value| !value.is_empty())
        {
            continue;
        }
        proposal.avatar = proposal
            .contact_ids
            .iter()
            .find_map(|id| avatars.get(id.as_str()).cloned());
    }
    proposals
}

fn heuristic_proposals(candidates: &[ConnectorContact], limit: usize) -> Vec<ContactProposal> {
    candidates
        .iter()
        .take(limit)
        .map(|candidate| {
            let name = candidate
                .name
                .as_deref()
                .unwrap_or(candidate.id.as_str())
                .to_string();
            ContactProposal {
                name,
                description: format!(
                    "{} appears repeatedly in recent connector context with direct conversation history. The available messages suggest an ongoing relationship with practical coordination or task-relevant exchanges.",
                    candidate.id
                ),
                avatar: candidate.avatar.clone(),
                contact_ids: vec![candidate.id.clone()],
            }
        })
        .collect()
}

fn openai_execution(state: &DaemonState) -> Result<Option<ContactOpenAiExecution>> {
    let inputs = state.build_runtime_inputs_without_discovery()?;
    let Some(provider) = inputs.providers.provider(OPENAI_PROVIDER_ID).cloned() else {
        return Ok(None);
    };
    let Some(credential) = inputs.auth_store.get(OPENAI_PROVIDER_ID).cloned() else {
        return Ok(None);
    };
    let (request_config, refresh_token) = contact_openai_request_config(&provider, &credential);
    let default_model = configured_contact_openai_model(&state.config_snapshot(), &provider);
    Ok(Some(ContactOpenAiExecution {
        provider_id: provider.id,
        default_model,
        auth_store: inputs.auth_store,
        auth_path: state.config_paths().user_config_dir.join("auth.json"),
        request_config,
        refresh_token,
        proxy: state.config_snapshot().network.proxy,
    }))
}

fn contact_inference_model(execution: &ContactOpenAiExecution, requested: Option<&str>) -> String {
    requested
        .and_then(|model| normalized_contact_openai_model(&execution.provider_id, model))
        .or(execution.default_model.as_deref())
        .unwrap_or("gpt-5.4-mini")
        .to_string()
}

fn configured_contact_openai_model(
    config: &PufferConfig,
    provider: &ProviderDescriptor,
) -> Option<String> {
    let model = config.default_model.as_deref()?;
    let model_has_provider = model.split_once('/').is_some();
    if model_has_provider {
        return normalized_contact_openai_model(&provider.id, model).map(ToOwned::to_owned);
    }
    if config
        .default_provider
        .as_deref()
        .is_some_and(|provider_id| contact_openai_provider_ids_match(provider_id, &provider.id))
    {
        return normalized_contact_openai_model(&provider.id, model).map(ToOwned::to_owned);
    }
    None
}

fn normalized_contact_openai_model<'a>(provider_id: &str, model: &'a str) -> Option<&'a str> {
    let model = model.trim();
    if model.is_empty() {
        return None;
    }
    let Some((prefix, unscoped)) = model.split_once('/') else {
        return Some(model);
    };
    let unscoped = unscoped.trim();
    if unscoped.is_empty() {
        return None;
    }
    contact_openai_provider_ids_match(prefix, provider_id).then_some(unscoped)
}

fn contact_openai_provider_ids_match(left: &str, right: &str) -> bool {
    canonical_contact_openai_provider_id(left) == canonical_contact_openai_provider_id(right)
}

fn canonical_contact_openai_provider_id(provider_id: &str) -> String {
    match provider_id.trim().to_ascii_lowercase().as_str() {
        "codex" => OPENAI_PROVIDER_ID.to_string(),
        provider_id => provider_id.to_string(),
    }
}

fn contact_openai_request_config(
    provider: &ProviderDescriptor,
    credential: &StoredCredential,
) -> (OpenAIRequestConfig, Option<String>) {
    let oauth = matches!(credential, StoredCredential::OAuth(_));
    let (auth, refresh_token, account_id) = match credential {
        StoredCredential::ApiKey { key } => (OpenAIAuth::ApiKey(key.clone()), None, None),
        StoredCredential::OAuth(credential) => (
            OpenAIAuth::OAuthBearer(credential.access_token.clone()),
            Some(credential.refresh_token.clone()),
            credential.account_id.clone(),
        ),
    };
    let base_url = contact_openai_base_url(provider, oauth);
    let mut custom_headers = provider
        .headers
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<Vec<_>>();
    append_contact_openai_headers(&mut custom_headers, provider, oauth);
    (
        OpenAIRequestConfig {
            base_url: base_url.clone(),
            version: contact_openai_request_version(provider, oauth),
            auth,
            originator: OPENAI_CODEX_ORIGINATOR.to_string(),
            session_id: None,
            account_id,
            custom_headers,
            query_params: provider
                .query_params
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            chat_completions_path: provider.chat_completions_path.clone(),
            responses_path: Some(contact_openai_responses_path(&base_url).to_string()),
        },
        refresh_token,
    )
}

fn contact_openai_base_url(provider: &ProviderDescriptor, oauth: bool) -> String {
    if !oauth || provider.id != OPENAI_PROVIDER_ID {
        return provider.base_url.clone();
    }
    let trimmed = provider.base_url.trim_end_matches('/');
    if provider_is_codex_style(provider) {
        trimmed.to_string()
    } else {
        OPENAI_CODEX_BASE_URL.to_string()
    }
}

fn contact_openai_request_version(provider: &ProviderDescriptor, oauth: bool) -> String {
    if provider_is_codex_style(provider) || (oauth && provider.id == OPENAI_PROVIDER_ID) {
        OPENAI_CODEX_COMPAT_VERSION.to_string()
    } else {
        env!("CARGO_PKG_VERSION").to_string()
    }
}

fn append_contact_openai_headers(
    headers: &mut Vec<(String, String)>,
    provider: &ProviderDescriptor,
    oauth: bool,
) {
    if (provider.id == OPENAI_PROVIDER_ID || provider_is_codex_style(provider) || oauth)
        && !has_header(headers, "version")
    {
        headers.push((
            "version".to_string(),
            contact_openai_request_version(provider, oauth),
        ));
    }
    append_env_header(headers, "OpenAI-Organization", "OPENAI_ORGANIZATION");
    append_env_header(headers, "OpenAI-Project", "OPENAI_PROJECT");
}

fn append_env_header(headers: &mut Vec<(String, String)>, header: &str, env_var: &str) {
    if has_header(headers, header) {
        return;
    }
    if let Ok(value) = std::env::var(env_var) {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            headers.push((header.to_string(), trimmed.to_string()));
        }
    }
}

fn has_header(headers: &[(String, String)], name: &str) -> bool {
    headers
        .iter()
        .any(|(header, _)| header.eq_ignore_ascii_case(name))
}

fn provider_is_codex_style(provider: &ProviderDescriptor) -> bool {
    provider.default_api == "openai-codex-responses"
        || provider
            .base_url
            .trim_end_matches('/')
            .contains("/backend-api")
        || provider
            .base_url
            .trim_end_matches('/')
            .contains("/api/codex")
}

fn contact_openai_responses_path(base_url: &str) -> &'static str {
    let trimmed = base_url.trim_end_matches('/');
    if trimmed.contains("/backend-api") || trimmed.contains("/api/codex") {
        "/responses"
    } else {
        "/v1/responses"
    }
}

fn truncate_text(value: &str, max_chars: usize) -> String {
    let mut truncated = value.chars().take(max_chars).collect::<String>();
    if truncated.len() < value.len() {
        truncated.push_str("\n... truncated ...");
    }
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;

    #[test]
    fn system_prompt_encourages_reasonable_inclusion_without_selection_guidance() {
        let prompt = contact_infer_system_prompt();

        assert!(prompt.contains("Propose as many reasonable contacts as possible"));
        assert!(prompt.contains("Favor inclusion"));
        assert!(prompt.contains("Do not create contacts for spam"));
        assert!(prompt.contains("natural summary of the user's relationship"));
        assert!(!prompt.contains("The first sentence must explain why this contact matters"));
        assert!(!prompt.contains("The second sentence must explain why this contact is not spam"));
        assert!(!prompt.contains("This contact matters because"));
        assert!(!prompt.contains("This is not spam"));
        assert!(!prompt.contains("When uncertain, skip the candidate."));
        assert!(!prompt.contains("Selection guidance:"));
        assert!(!prompt.contains("Return zero tool calls if no candidate is clearly worth saving."));
    }

    #[test]
    fn parse_openai_proposals_accepts_zero_tool_calls() {
        let response = parse_responses_response(r#"{"output":[]}"#).unwrap();
        let calls = extract_responses_tool_calls(&response).unwrap();

        assert!(parse_openai_proposals(&calls, 30).is_empty());
    }

    #[test]
    fn parse_openai_proposals_accepts_responses_tool_calls() {
        let calls = vec![OpenAIResponseToolCall {
            item_id: None,
            status: None,
            call_id: "call_1".to_string(),
            name: "CreateContact".to_string(),
            arguments: json!({
                "name": "Alice",
                "description": "Alice coordinates launch work with the user. The recent context is a direct planning exchange rather than automated spam.",
                "avatar": null,
                "contact_ids": ["telegram@alice"]
            }),
        }];

        let proposals = parse_openai_proposals(&calls, 30);

        assert_eq!(proposals.len(), 1);
        assert_eq!(proposals[0].name, "Alice");
        assert_eq!(proposals[0].contact_ids, vec!["telegram@alice"]);
    }

    #[test]
    fn attach_candidate_avatars_fills_missing_proposal_avatar() {
        let avatar = "data:image/jpeg;base64,ZmFrZQ==".to_string();
        let proposals = vec![ContactProposal {
            name: "Alice".to_string(),
            description: "Alice coordinates launch work with the user. They discuss checklists and release timing.".to_string(),
            avatar: None,
            contact_ids: vec!["telegram@alice".to_string()],
        }];
        let candidates = vec![ConnectorContact {
            id: "telegram@alice".to_string(),
            avatar: Some(avatar.clone()),
            name: Some("Alice".to_string()),
            context: Vec::new(),
            score: 1.0,
            last_message_at_ms: None,
        }];

        let enriched = attach_candidate_avatars(proposals, &candidates);

        assert_eq!(enriched[0].avatar.as_deref(), Some(avatar.as_str()));
    }

    #[test]
    fn candidate_prompt_json_is_pretty_and_auditable() {
        let candidates = [ConnectorContact {
            id: "telegram@alice".to_string(),
            avatar: None,
            name: Some("Alice Example".to_string()),
            context: vec![puffer_subscriptions::ContactContext {
                kind: "message".to_string(),
                text: "Alice asked for the launch checklist.".to_string(),
                timestamp_ms: None,
                payload: Value::Null,
            }],
            score: 42.5,
            last_message_at_ms: None,
        }];

        let prompt = candidate_prompt_json(&compact_candidates(&candidates)).unwrap();

        assert!(prompt.starts_with("[\n  {"));
        assert!(prompt.contains("\n    \"id\": \"telegram@alice\","));
        assert!(prompt.contains(
            "\n    \"context\": [\n      {\n        \"kind\": \"message\",\n        \"text\": \"Alice asked for the launch checklist.\",\n        \"timestamp_ms\": null\n      }\n    ]"
        ));
        assert!(!prompt.contains("[{\"id\""));
    }

    #[test]
    fn contact_inference_request_uses_provider_base_url_and_responses_path() {
        let provider = test_openai_provider("http://45.77.128.10:8317/v1");
        let (config, refresh_token) = contact_openai_request_config(
            &provider,
            &StoredCredential::ApiKey {
                key: "sk-relay".to_string(),
            },
        );
        let request = build_contact_openai_request(
            &config,
            &contact_inference_request_body("gpt-5.4-mini", "[]"),
        )
        .unwrap();

        assert_eq!(refresh_token, None);
        assert_eq!(request.url, "http://45.77.128.10:8317/v1/responses");
        assert!(!request.url.contains("api.openai.com"));
        assert!(!request.url.contains("chat/completions"));
        let body: Value = serde_json::from_str(&request.body).unwrap();
        assert_eq!(body["instructions"], INFER_SYSTEM_PROMPT);
        assert_eq!(body["tools"][0]["name"], "CreateContact");
    }

    #[test]
    fn contact_inference_uses_configured_openai_model() {
        let provider = test_openai_provider("http://45.77.128.10:8317/v1");
        let mut config = PufferConfig::default();
        config.default_provider = Some("codex".to_string());
        config.default_model = Some("codex/gpt-relay".to_string());

        assert_eq!(
            configured_contact_openai_model(&config, &provider).as_deref(),
            Some("gpt-relay")
        );
    }

    #[test]
    fn contact_inference_ignores_other_provider_default_model() {
        let provider = test_openai_provider("http://45.77.128.10:8317/v1");
        let mut config = PufferConfig::default();
        config.default_provider = Some("anthropic".to_string());
        config.default_model = Some("claude-sonnet-4-5".to_string());

        assert_eq!(configured_contact_openai_model(&config, &provider), None);
    }

    #[test]
    fn contact_inference_model_strips_openai_and_codex_prefixes() {
        assert_eq!(
            normalized_contact_openai_model("openai", "openai/gpt-relay"),
            Some("gpt-relay")
        );
        assert_eq!(
            normalized_contact_openai_model("openai", "codex/gpt-relay"),
            Some("gpt-relay")
        );
        assert_eq!(
            normalized_contact_openai_model("openai", "anthropic/claude-sonnet-4-5"),
            None
        );
    }

    #[test]
    fn contact_openai_response_rejects_error_status_json() {
        let err = parse_contact_openai_response(
            ContactHttpResponse {
                status: reqwest::StatusCode::TOO_MANY_REQUESTS,
                text: r#"{"error":{"message":"rate limit","type":"rate_limit_error"}}"#.to_string(),
            },
            "https://example.test/v1/responses",
        )
        .unwrap_err();

        assert!(err
            .to_string()
            .contains("OpenAI contact inference request returned an error status"));
    }

    fn test_openai_provider(base_url: &str) -> ProviderDescriptor {
        ProviderDescriptor {
            id: OPENAI_PROVIDER_ID.to_string(),
            display_name: "OpenAI".to_string(),
            base_url: base_url.to_string(),
            default_api: "openai-responses".to_string(),
            auth_modes: Vec::new(),
            headers: IndexMap::new(),
            query_params: IndexMap::new(),
            chat_completions_path: None,
            discovery: None,
            media: None,
            models: Vec::new(),
        }
    }
}

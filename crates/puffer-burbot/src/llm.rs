mod chat;
mod openai_error;
mod parse;
mod schema;

use crate::contract::CapabilityContract;
use crate::planner::CompletionRole;
use anyhow::{anyhow, Context, Result};
use chat::{
    build_chat_tool_call_request, send_and_parse_chat_candidate_list,
    send_and_parse_chat_goal_verification, send_and_parse_chat_tool_call,
};
pub(crate) use openai_error::retryable_openai_error;
use openai_error::{
    assistant_content_missing, assistant_content_stopped_by_length, openai_error_status,
    response_error_field_equals, OpenAINoStructuralAssistantContent, OpenAIStatusError,
    OpenAIWallTimeoutError,
};
pub(crate) use parse::model_proposal_violations;
#[cfg(test)]
use parse::parse_model_candidate;
pub(crate) use parse::validate_puffer_tool_call;
use parse::{parse_candidate_list_json, parse_goal_verification_json, parse_tool_call_json};
use puffer_config::{load_config, ConfigPaths};
use puffer_provider_openai::{
    build_json_post_request, build_responses_request, extract_responses_text,
    parse_responses_response, refresh_oauth_token, BuiltOpenAIRequest, OpenAIAuth,
    OpenAIRequestConfig, OpenAIResponsesRequest,
};
use puffer_provider_registry::{
    detect_import_candidates, AuthStore, ExternalImportFamily, ProviderRegistry, StoredCredential,
};
use puffer_resources::load_resources;
use reqwest::blocking::Client;
use schema::{
    apply_chat_compatibility_overrides, build_artifact_review_chat_tool_request,
    build_artifact_review_plain_request, build_artifact_review_request,
    build_goal_verification_chat_tool_request, build_goal_verification_plain_request,
    build_goal_verification_request, build_observe_act_chat_tool_request,
    build_observe_act_plain_request, build_observe_act_request, build_tool_call_plain_request,
    build_tool_call_request, generated_artifact_review_prompt, goal_verification_prompt,
    observe_act_prompt, CHAT_ARTIFACT_REVIEW_TOOL_NAME, CHAT_GOAL_VERIFICATION_TOOL_NAME,
    CHAT_OBSERVE_ACT_TOOL_NAME,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;

const CODEX_COMPAT_VERSION: &str = "0.125.0";
const CODEX_BACKEND_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
const MAX_PROMPT_IMAGES: usize = 4;

#[derive(Debug, Clone)]
pub(crate) struct LlmProbeOptions {
    pub(crate) model: String,
    pub(crate) prompt: String,
    pub(crate) dry_run: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct LlmProbeResult {
    pub(crate) model: String,
    pub(crate) auth_source: String,
    pub(crate) url: String,
    pub(crate) sent: bool,
    pub(crate) text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct PufferToolCallProposal {
    pub(crate) tool_id: String,
    pub(crate) args: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct ModelCandidateProposal {
    pub(crate) id: Option<String>,
    pub(crate) tool_id: String,
    pub(crate) args: Value,
    pub(crate) completion_role: CompletionRole,
    pub(crate) depends_on: Vec<String>,
    pub(crate) rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct GoalVerificationResult {
    pub(crate) satisfied: bool,
    pub(crate) confidence: f64,
    pub(crate) missing_evidence: Vec<String>,
    pub(crate) suggested_candidates: Vec<ModelCandidateProposal>,
    pub(crate) rejected_suggested_candidates: Vec<String>,
}

struct OpenAiCredential {
    auth: OpenAIAuth,
    auth_source: String,
    base_url: String,
    account_id: Option<String>,
    refresh_token: Option<String>,
    custom_headers: Vec<(String, String)>,
    query_params: Vec<(String, String)>,
}

pub(crate) fn probe_openai(
    workspace_root: &Path,
    options: LlmProbeOptions,
) -> Result<LlmProbeResult> {
    let mut credential = resolve_openai_credential(workspace_root)?;
    let config = request_config(&credential);
    let request = build_probe_request(&config, &options.model, &options.prompt)?;
    if options.dry_run {
        return Ok(LlmProbeResult {
            model: options.model,
            auth_source: credential.auth_source,
            url: request.url,
            sent: false,
            text: None,
        });
    }
    match send_and_parse(&request) {
        Ok(text) => Ok(LlmProbeResult {
            model: options.model,
            auth_source: credential.auth_source,
            url: request.url,
            sent: true,
            text: Some(text),
        }),
        Err(error) if is_unauthorized(&error) && credential.refresh_token.is_some() => {
            refresh_credential(&mut credential)?;
            let config = request_config(&credential);
            let retry = build_probe_request(&config, &options.model, &options.prompt)?;
            let text = send_and_parse(&retry)?;
            Ok(LlmProbeResult {
                model: options.model,
                auth_source: credential.auth_source,
                url: retry.url,
                sent: true,
                text: Some(text),
            })
        }
        Err(error) => Err(error),
    }
}

pub(crate) fn propose_puffer_tool_call(
    workspace_root: &Path,
    model: &str,
    goal: &str,
    contract: &CapabilityContract,
    image_context: Option<&Value>,
) -> Result<PufferToolCallProposal> {
    let credential = resolve_openai_credential(workspace_root)?;
    let config = request_config(&credential);
    let available_tools = contract
        .actions
        .iter()
        .map(|action| {
            json!({
                "tool_id": action.name,
                "description": action.description,
                "input_schema": crate::model_policy::model_proposal_args_schema(action),
                "side_effect_class": action.side_effect_class,
                "risk_level": action.risk_level,
            })
        })
        .collect::<Vec<_>>();
    let tool_prompt = format!(
        "You are Burbot's bounded Puffer-tool proposal operator.\n\
         Choose exactly one attached contract-declared tool call. Prefer the lowest-risk tool whose schema can satisfy the goal. \
         The tool call will be executed once through Puffer's existing tool runtime in the current workspace. \
         Do not use prose as an action. If the next step requires substantial analysis, search, compilation, symbolic work, \
         or computation, call a workspace tool that performs that work against local artifacts instead of trying to finish \
         the analysis in this response. The proposal is invalid if it only prints, echoes, comments on, or restates future work \
         instead of actually inspecting evidence, creating or changing the required artifact, starting a required service, \
         running a concrete check, or repairing a concrete failure. If creating or replacing a file, use a file-writing tool \
         with object args instead of shell heredocs, redirects, or inline generated file payloads. Do not submit placeholder commands, TODO scaffolds, \
         fake success markers, or commands whose only effect is describing what should be implemented.\n\n\
         Goal:\n{goal}"
    );
    let prompt = format!(
        "You are Burbot's bounded Puffer-tool proposal operator.\n\
         Return exactly one JSON object with this schema: {{\"tool_id\":\"<available tool id>\",\"args\":{{...}}}}.\n\
         The tool call will be executed once through Puffer's existing tool runtime in the current workspace.\n\
         Choose only one of the available contract-declared tools below. Prefer the lowest-risk tool whose schema can satisfy the goal. \
         If the next step requires substantial analysis, search, compilation, symbolic work, or computation, choose a workspace \
         tool that performs that work against local artifacts instead of trying to finish the analysis in this response. \
         Do not include markdown. The proposal is invalid if it only prints, echoes, comments on, or restates future work instead of \
         actually inspecting evidence, creating or changing the required artifact, starting a required service, running a concrete check, \
         or repairing a concrete failure. If creating or replacing a file, use a file-writing tool with object args instead of shell heredocs, \
         redirects, or inline generated file payloads. Do not submit placeholder commands, TODO scaffolds, fake success markers, or commands whose \
         only effect is describing what should be implemented.\n\n\
         Available tools:\n{}\n\n\
         Goal:\n{goal}",
        serde_json::to_string_pretty(&available_tools)?
    );
    let images = image_context
        .map(image_data_urls_from_context)
        .unwrap_or_default();
    let normalized_model = normalize_openai_model(model);
    if use_chat_tool_calls(&config.base_url) {
        let request =
            build_chat_tool_call_request(&config, &normalized_model, &tool_prompt, contract)?;
        match send_and_parse_chat_tool_call(&request, contract) {
            Ok(proposal) => return Ok(proposal),
            Err(error)
                if !retryable_openai_error(&error) || chat_tool_call_request_rejected(&error) => {}
            Err(error) => return Err(error),
        }
    }
    let request = build_tool_call_request(&config, &normalized_model, &prompt, contract, &images)?;
    let text = match send_and_parse(&request) {
        Ok(text) => text,
        Err(error) if should_retry_without_response_format(&error) => {
            let fallback =
                build_tool_call_plain_request(&config, &normalized_model, &prompt, &images)?;
            send_and_parse(&fallback)?
        }
        Err(error) => return Err(error),
    };
    parse_tool_call_json(&text, contract)
}

pub(crate) fn propose_observe_act_candidates(
    workspace_root: &Path,
    model: &str,
    goal: &str,
    contract: &CapabilityContract,
    observation_context: &Value,
    image_context: Option<&Value>,
) -> Result<Vec<ModelCandidateProposal>> {
    let credential = resolve_openai_credential(workspace_root)?;
    let config = request_config(&credential);
    let prompt = observe_act_prompt(goal, contract, observation_context)?;
    let images = image_data_urls_from_context(image_context.unwrap_or(observation_context));
    let normalized_model = normalize_openai_model(model);
    if use_chat_tool_calls(&config.base_url) {
        let request = build_observe_act_chat_tool_request(
            &config,
            &normalized_model,
            &prompt,
            contract,
            &images,
        )?;
        match send_and_parse_chat_candidate_list(&request, contract, CHAT_OBSERVE_ACT_TOOL_NAME) {
            Ok(candidates) => return Ok(candidates),
            Err(error) if chat_tool_call_fallback_allowed(&error) => {}
            Err(error) => return Err(error),
        }
    }
    let request =
        build_observe_act_request(&config, &normalized_model, &prompt, contract, &images)?;
    let text = match send_and_parse(&request) {
        Ok(text) => text,
        Err(error) if should_retry_without_response_format(&error) => {
            let fallback =
                build_observe_act_plain_request(&config, &normalized_model, &prompt, &images)?;
            send_and_parse(&fallback)?
        }
        Err(error) => return Err(error),
    };
    parse_candidate_list_json(&text, contract)
}

pub(crate) fn verify_goal_satisfied(
    workspace_root: &Path,
    model: &str,
    goal: &str,
    contract: &CapabilityContract,
    verification_context: &Value,
    image_context: Option<&Value>,
) -> Result<GoalVerificationResult> {
    let credential = resolve_openai_credential(workspace_root)?;
    let config = request_config(&credential);
    let prompt = goal_verification_prompt(goal, contract, verification_context)?;
    let images = image_data_urls_from_context(image_context.unwrap_or(verification_context));
    let normalized_model = normalize_openai_model(model);
    if use_chat_tool_calls(&config.base_url) {
        let request = build_goal_verification_chat_tool_request(
            &config,
            &normalized_model,
            &prompt,
            contract,
            &images,
        )?;
        match send_and_parse_chat_goal_verification(
            &request,
            contract,
            CHAT_GOAL_VERIFICATION_TOOL_NAME,
        ) {
            Ok(verification) => return Ok(verification),
            Err(error) if chat_tool_call_fallback_allowed(&error) => {}
            Err(error) => return Err(error),
        }
    }
    let request =
        build_goal_verification_request(&config, &normalized_model, &prompt, contract, &images)?;
    let text = match send_and_parse(&request) {
        Ok(text) => text,
        Err(error) if should_retry_without_response_format(&error) => {
            let fallback = build_goal_verification_plain_request(
                &config,
                &normalized_model,
                &prompt,
                &images,
            )?;
            send_and_parse(&fallback)?
        }
        Err(error) => return Err(error),
    };
    parse_goal_verification_json(&text, contract)
}

pub(crate) fn review_generated_artifact(
    workspace_root: &Path,
    model: &str,
    goal: &str,
    contract: &CapabilityContract,
    review_context: &Value,
    image_context: Option<&Value>,
) -> Result<GoalVerificationResult> {
    let credential = resolve_openai_credential(workspace_root)?;
    let config = request_config(&credential);
    let prompt = generated_artifact_review_prompt(goal, contract, review_context)?;
    let images = image_data_urls_from_context(image_context.unwrap_or(review_context));
    let normalized_model = normalize_openai_model(model);
    if use_chat_tool_calls(&config.base_url) {
        let request = build_artifact_review_chat_tool_request(
            &config,
            &normalized_model,
            &prompt,
            contract,
            &images,
        )?;
        match send_and_parse_chat_goal_verification(
            &request,
            contract,
            CHAT_ARTIFACT_REVIEW_TOOL_NAME,
        ) {
            Ok(verification) => return Ok(verification),
            Err(error) if chat_tool_call_fallback_allowed(&error) => {}
            Err(error) => return Err(error),
        }
    }
    let request =
        build_artifact_review_request(&config, &normalized_model, &prompt, contract, &images)?;
    let text = match send_and_parse(&request) {
        Ok(text) => text,
        Err(error) if should_retry_without_response_format(&error) => {
            let fallback =
                build_artifact_review_plain_request(&config, &normalized_model, &prompt, &images)?;
            send_and_parse(&fallback)?
        }
        Err(error) => return Err(error),
    };
    parse_goal_verification_json(&text, contract)
}

fn resolve_openai_credential(workspace_root: &Path) -> Result<OpenAiCredential> {
    let paths = ConfigPaths::discover(workspace_root);
    let config = load_config(&paths).unwrap_or_default();
    let base_url = configured_openai_base_url(config.openai_base_url.clone());
    if let Some(oauth_only) = forced_codex_credential_mode() {
        return resolve_forced_codex_credential(oauth_only);
    }
    if let Ok(key) = std::env::var("OPENAI_API_KEY") {
        let key = key.trim().to_string();
        if !key.is_empty() {
            return Ok(OpenAiCredential {
                auth: OpenAIAuth::ApiKey(key),
                auth_source: "OPENAI_API_KEY".to_string(),
                base_url,
                account_id: None,
                refresh_token: None,
                custom_headers: env_openai_headers(),
                query_params: Vec::new(),
            });
        }
    }

    let auth_path = paths.user_config_dir.join("auth.json");
    let auth_store = AuthStore::load(&auth_path).unwrap_or_default();
    if let Some(credential) = auth_store.get("openai") {
        return credential_from_stored(
            credential,
            "puffer openai auth",
            Some(CODEX_BACKEND_BASE_URL.to_string()),
            Vec::new(),
            Vec::new(),
        );
    }

    let mut candidates = detect_import_candidates(ExternalImportFamily::OpenAi)?;
    if let Some(candidate) = candidates.pop() {
        let headers = candidate.openai_headers.into_iter().collect::<Vec<_>>();
        let query_params = candidate
            .openai_query_params
            .into_iter()
            .collect::<Vec<_>>();
        return credential_from_stored(
            &candidate.credential,
            &format!(
                "local Codex credential at {}",
                candidate.source_path.display()
            ),
            candidate
                .openai_base_url
                .or_else(|| Some(CODEX_BACKEND_BASE_URL.to_string())),
            headers,
            query_params,
        );
    }

    let discovered_base = resource_openai_base_url(workspace_root).unwrap_or(base_url);
    Err(anyhow!(
        "no OpenAI credential found; checked OPENAI_API_KEY, {}, and local Codex auth (base URL would be {})",
        auth_path.display(),
        discovered_base
    ))
}

fn forced_codex_credential_mode() -> Option<bool> {
    std::env::var("BURBOT_OPENAI_AUTH_SOURCE")
        .ok()
        .and_then(|value| match value.trim() {
            "codex" | "local-codex" => Some(false),
            "codex-oauth" | "local-codex-oauth" => Some(true),
            _ => None,
        })
}

fn resolve_forced_codex_credential(oauth_only: bool) -> Result<OpenAiCredential> {
    let candidates = detect_import_candidates(ExternalImportFamily::OpenAi)?;
    let mut api_key_candidate = None;
    for candidate in candidates {
        match &candidate.credential {
            StoredCredential::OAuth(_) => {
                return credential_from_stored(
                    &candidate.credential,
                    &format!(
                        "local Codex OAuth credential at {}",
                        candidate.source_path.display()
                    ),
                    Some(CODEX_BACKEND_BASE_URL.to_string()),
                    Vec::new(),
                    Vec::new(),
                );
            }
            StoredCredential::ApiKey { .. } if api_key_candidate.is_none() => {
                api_key_candidate = Some(candidate);
            }
            _ => {}
        }
    }
    if !oauth_only {
        if let Some(candidate) = api_key_candidate {
            let headers = candidate.openai_headers.into_iter().collect::<Vec<_>>();
            let query_params = candidate
                .openai_query_params
                .into_iter()
                .collect::<Vec<_>>();
            return credential_from_stored(
                &candidate.credential,
                &format!(
                    "local Codex API key credential at {}",
                    candidate.source_path.display()
                ),
                candidate
                    .openai_base_url
                    .or_else(|| Some("https://api.openai.com".to_string())),
                headers,
                query_params,
            );
        }
    }
    Err(anyhow!(
        "BURBOT_OPENAI_AUTH_SOURCE requested local Codex credentials, but none were found"
    ))
}

fn normalize_openai_model(model: &str) -> String {
    model
        .split_once('/')
        .map(|(_, value)| value)
        .unwrap_or(model)
        .to_string()
}

fn image_data_urls_from_context(value: &Value) -> Vec<String> {
    let mut urls = Vec::new();
    collect_image_data_urls(value, &mut urls);
    urls
}

fn collect_image_data_urls(value: &Value, urls: &mut Vec<String>) {
    if urls.len() >= MAX_PROMPT_IMAGES {
        return;
    }
    match value {
        Value::Object(object) => {
            if let Some(data_url) = image_data_url_from_object(object) {
                if !urls.iter().any(|existing| existing == &data_url) {
                    urls.push(data_url);
                }
            }
            for nested in object.values() {
                collect_image_data_urls(nested, urls);
                if urls.len() >= MAX_PROMPT_IMAGES {
                    break;
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_image_data_urls(item, urls);
                if urls.len() >= MAX_PROMPT_IMAGES {
                    break;
                }
            }
        }
        Value::String(text) if text.starts_with("data:image/") => {
            if !urls.iter().any(|existing| existing == text) {
                urls.push(text.clone());
            }
        }
        _ => {}
    }
}

fn image_data_url_from_object(object: &serde_json::Map<String, Value>) -> Option<String> {
    if let Some(data_url) = string_field(object, &["image_url", "url", "data_url"]) {
        if data_url.starts_with("data:image/") {
            return Some(data_url.to_string());
        }
    }
    let mime = string_field(object, &["type", "mime_type", "media_type"])?;
    if !mime.starts_with("image/") {
        return None;
    }
    let base64 = string_field(object, &["base64", "data"])?;
    Some(format!("data:{mime};base64,{base64}"))
}

fn string_field<'a>(object: &'a serde_json::Map<String, Value>, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| object.get(*key).and_then(Value::as_str))
}

fn credential_from_stored(
    credential: &StoredCredential,
    auth_source: &str,
    base_url: Option<String>,
    custom_headers: Vec<(String, String)>,
    query_params: Vec<(String, String)>,
) -> Result<OpenAiCredential> {
    match credential {
        StoredCredential::ApiKey { key } => Ok(OpenAiCredential {
            auth: OpenAIAuth::ApiKey(key.clone()),
            auth_source: auth_source.to_string(),
            base_url: base_url.unwrap_or_else(|| "https://api.openai.com".to_string()),
            account_id: None,
            refresh_token: None,
            custom_headers,
            query_params,
        }),
        StoredCredential::OAuth(oauth) => Ok(OpenAiCredential {
            auth: OpenAIAuth::OAuthBearer(oauth.access_token.clone()),
            auth_source: auth_source.to_string(),
            base_url: base_url.unwrap_or_else(|| CODEX_BACKEND_BASE_URL.to_string()),
            account_id: oauth.account_id.clone(),
            refresh_token: Some(oauth.refresh_token.clone()).filter(|value| !value.is_empty()),
            custom_headers,
            query_params,
        }),
    }
}

fn request_config(credential: &OpenAiCredential) -> OpenAIRequestConfig {
    let mut custom_headers = credential.custom_headers.clone();
    if is_codex_backend(&credential.base_url)
        && !custom_headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("version"))
    {
        custom_headers.push(("version".to_string(), CODEX_COMPAT_VERSION.to_string()));
    }
    OpenAIRequestConfig {
        base_url: credential.base_url.clone(),
        version: if is_codex_backend(&credential.base_url) {
            CODEX_COMPAT_VERSION.to_string()
        } else {
            env!("CARGO_PKG_VERSION").to_string()
        },
        auth: credential.auth.clone(),
        originator: "codex_cli_rs".to_string(),
        session_id: Some(uuid::Uuid::new_v4().to_string()),
        account_id: credential.account_id.clone(),
        custom_headers,
        query_params: credential.query_params.clone(),
    }
}

fn build_probe_request(
    config: &OpenAIRequestConfig,
    model: &str,
    prompt: &str,
) -> Result<BuiltOpenAIRequest> {
    if is_codex_backend(&config.base_url) {
        let body = json!({
            "model": model,
            "instructions": "You are Burbot's smoke-test operator. Reply with a concise JSON object.",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": prompt}]
            }],
            "tools": [],
            "tool_choice": "auto",
            "parallel_tool_calls": false,
            "store": false,
            "stream": false,
            "include": [],
            "prompt_cache_key": "burbot-smoke-test"
        });
        build_json_post_request(config, "/responses", &body)
    } else if supports_responses_api(&config.base_url) {
        build_responses_request(
            config,
            &OpenAIResponsesRequest {
                model: model.to_string(),
                input: prompt.to_string(),
                text: None,
            },
        )
    } else {
        let mut body = json!({
            "model": model,
            "messages": [
                {
                    "role": "system",
                    "content": "You are Burbot's smoke-test operator. Reply with a concise JSON object."
                },
                {
                    "role": "user",
                    "content": prompt
                }
            ],
            "response_format": {"type": "json_object"},
            "stream": false
        });
        apply_chat_compatibility_overrides(config, &mut body);
        build_json_post_request(config, "/v1/chat/completions", &body)
    }
}

fn send_and_parse(request: &BuiltOpenAIRequest) -> Result<String> {
    let attempts = openai_retry_attempts();
    let mut last_error = None;
    for attempt in 0..attempts {
        match send_and_extract_once(request) {
            Ok(output) => return Ok(output),
            Err(error)
                if attempt + 1 < attempts
                    && retryable_openai_error(&error)
                    && !assistant_content_stopped_by_length(&error) =>
            {
                last_error = Some(error);
                std::thread::sleep(std::time::Duration::from_millis(
                    500 * u64::from(attempt + 1),
                ));
            }
            Err(error) => return Err(error),
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow!("OpenAI request failed without an error")))
}

fn send_and_extract_once(request: &BuiltOpenAIRequest) -> Result<String> {
    let text = send_and_read_success(request)?;
    let output = parse_llm_response_text(&request.url, &text)?;
    if !output.trim().is_empty() {
        return Ok(output);
    }
    Err(no_structural_content_error(
        endpoint_kind(&request.url),
        None,
        Vec::new(),
    ))
}

fn send_and_read_success(request: &BuiltOpenAIRequest) -> Result<String> {
    let timeout_secs = openai_timeout_secs();
    let timeout = std::time::Duration::from_secs(timeout_secs);
    let connect_timeout = timeout.min(std::time::Duration::from_secs(30));
    let client = Client::builder()
        .connect_timeout(connect_timeout)
        .timeout(timeout)
        .build()
        .context("failed to build OpenAI HTTP client")?;
    let attempts = openai_retry_attempts();
    let mut last_error = None;
    for attempt in 0..attempts {
        match send_and_parse_once_with_wall_timeout(&client, request, timeout_secs) {
            Ok(text) => return Ok(text),
            Err(error) if attempt + 1 < attempts && retryable_openai_error(&error) => {
                last_error = Some(error);
                std::thread::sleep(std::time::Duration::from_millis(
                    500 * u64::from(attempt + 1),
                ));
            }
            Err(error) => return Err(error),
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow!("OpenAI request failed without an error")))
}

fn send_and_parse_once_with_wall_timeout(
    client: &Client,
    request: &BuiltOpenAIRequest,
    timeout_secs: u64,
) -> Result<String> {
    let (sender, receiver) = std::sync::mpsc::channel();
    let client = client.clone();
    let request = request.clone();
    std::thread::Builder::new()
        .name("burbot-openai-request".to_string())
        .spawn(move || {
            let _ = sender.send(send_and_parse_once(&client, &request));
        })
        .context("failed to spawn OpenAI request worker")?;
    receiver
        .recv_timeout(std::time::Duration::from_secs(timeout_secs))
        .map_err(|error| match error {
            std::sync::mpsc::RecvTimeoutError::Timeout => {
                anyhow!(OpenAIWallTimeoutError { timeout_secs })
            }
            std::sync::mpsc::RecvTimeoutError::Disconnected => {
                anyhow!("OpenAI request worker exited without returning a result")
            }
        })?
}

fn send_and_parse_once(client: &Client, request: &BuiltOpenAIRequest) -> Result<String> {
    let mut builder = client.post(&request.url).body(request.body.clone());
    for (name, value) in &request.headers {
        builder = builder.header(name, value);
    }
    let response = builder
        .send()
        .with_context(|| format!("failed to send OpenAI request to {}", request.url))?;
    let status = response.status();
    let text = response.text().context("failed to read OpenAI response")?;
    if !status.is_success() {
        return Err(OpenAIStatusError { status, body: text }.into());
    }
    Ok(text)
}

fn parse_llm_response_text(url: &str, text: &str) -> Result<String> {
    if is_chat_completions_url(url) {
        extract_chat_completions_payload_text(text)
    } else {
        let parsed = parse_responses_response(text)?;
        Ok(extract_responses_text(&parsed))
    }
}

fn extract_chat_completions_payload_text(text: &str) -> Result<String> {
    let value: Value =
        serde_json::from_str(text).context("failed to parse OpenAI Chat Completions payload")?;
    let Some(message) = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(Value::as_object)
    else {
        return Err(no_structural_content_error(
            "chat_completions",
            chat_finish_reason(&value),
            Vec::new(),
        ));
    };
    let finish_reason = chat_finish_reason(&value);
    if finish_reason.as_deref() == Some("length") {
        return Err(no_structural_content_error(
            "chat_completions",
            finish_reason,
            message.keys().cloned().collect::<Vec<_>>(),
        ));
    }
    let Some(content) = message.get("content") else {
        return Err(missing_chat_completion_content_error(&value, message));
    };
    if let Some(content) = content.as_str() {
        if !content.trim().is_empty() {
            return Ok(content.to_string());
        }
        return Err(missing_chat_completion_content_error(&value, message));
    }
    if let Some(items) = content.as_array() {
        let text = items
            .iter()
            .filter_map(|item| {
                item.get("text")
                    .and_then(Value::as_str)
                    .or_else(|| item.get("content").and_then(Value::as_str))
            })
            .collect::<Vec<_>>()
            .join("");
        if !text.trim().is_empty() {
            return Ok(text);
        }
        return Err(missing_chat_completion_content_error(&value, message));
    }
    if content.is_object() || content.is_array() {
        return Ok(content.to_string());
    }
    Err(missing_chat_completion_content_error(&value, message))
}

fn missing_chat_completion_content_error(
    response: &Value,
    message: &serde_json::Map<String, Value>,
) -> anyhow::Error {
    no_structural_content_error(
        "chat_completions",
        chat_finish_reason(response),
        message.keys().cloned().collect::<Vec<_>>(),
    )
}

fn chat_finish_reason(response: &Value) -> Option<String> {
    response
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("finish_reason"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn no_structural_content_error(
    endpoint_kind: &'static str,
    finish_reason: Option<String>,
    message_keys: Vec<String>,
) -> anyhow::Error {
    OpenAINoStructuralAssistantContent {
        endpoint_kind,
        finish_reason,
        message_keys,
    }
    .into()
}

fn endpoint_kind(url: &str) -> &'static str {
    if is_chat_completions_url(url) {
        "chat_completions"
    } else {
        "responses"
    }
}

fn should_retry_without_response_format(error: &anyhow::Error) -> bool {
    response_format_schema_rejected(error) || assistant_content_missing(error)
}

fn chat_tool_call_fallback_allowed(error: &anyhow::Error) -> bool {
    chat_tool_call_request_rejected(error)
        || assistant_content_missing(error)
        || non_retryable_structural_chat_tool_error(error)
}

fn response_format_schema_rejected(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<OpenAIStatusError>()
            .is_some_and(|error| {
                error.status == reqwest::StatusCode::BAD_REQUEST
                    && response_error_field_equals(&error.body, "param", "response_format")
            })
    })
}

fn refresh_credential(credential: &mut OpenAiCredential) -> Result<()> {
    let refresh_token = credential
        .refresh_token
        .clone()
        .ok_or_else(|| anyhow!("missing OpenAI refresh token"))?;
    let refreshed = refresh_oauth_token(&refresh_token)?;
    credential.auth = OpenAIAuth::OAuthBearer(refreshed.access_token);
    credential.refresh_token = Some(refreshed.refresh_token);
    credential.account_id = refreshed.account_id;
    credential.auth_source.push_str(" (refreshed)");
    Ok(())
}

fn is_unauthorized(error: &anyhow::Error) -> bool {
    openai_error_status(error) == Some(reqwest::StatusCode::UNAUTHORIZED)
}

fn chat_tool_call_request_rejected(error: &anyhow::Error) -> bool {
    openai_error_status(error) == Some(reqwest::StatusCode::BAD_REQUEST)
}

fn non_retryable_structural_chat_tool_error(error: &anyhow::Error) -> bool {
    openai_error_status(error).is_none() && !retryable_openai_error(error)
}

fn is_codex_backend(base_url: &str) -> bool {
    let trimmed = base_url.trim_end_matches('/');
    trimmed.contains("/backend-api") || trimmed.contains("/api/codex")
}

fn use_chat_tool_calls(base_url: &str) -> bool {
    if supports_responses_api(base_url) {
        return false;
    }
    match std::env::var("BURBOT_OPENAI_USE_CHAT_TOOL_CALLS")
        .ok()
        .map(|value| value.to_ascii_lowercase())
    {
        Some(value) if matches!(value.as_str(), "0" | "false" | "no" | "off") => false,
        Some(value) if matches!(value.as_str(), "1" | "true" | "yes" | "on") => true,
        Some(_) => true,
        None => !is_deepseek_base_url(base_url),
    }
}

fn supports_responses_api(base_url: &str) -> bool {
    is_codex_backend(base_url) || base_url.contains("api.openai.com")
}

fn is_deepseek_base_url(base_url: &str) -> bool {
    base_url.contains("api.deepseek.com")
}

fn is_chat_completions_url(url: &str) -> bool {
    url.split('?')
        .next()
        .unwrap_or(url)
        .trim_end_matches('/')
        .ends_with("/chat/completions")
}

fn configured_openai_base_url(config_base_url: Option<String>) -> String {
    resolve_openai_base_url(config_base_url, env_openai_base_url())
}

fn resolve_openai_base_url(
    config_base_url: Option<String>,
    env_base_url: Option<String>,
) -> String {
    env_base_url
        .or(config_base_url)
        .unwrap_or_else(|| "https://api.openai.com".to_string())
}

fn env_openai_base_url() -> Option<String> {
    std::env::var("OPENAI_BASE_URL").ok().and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn resource_openai_base_url(workspace_root: &Path) -> Option<String> {
    let paths = ConfigPaths::discover(workspace_root);
    let resources = load_resources(&paths).ok()?;
    let mut providers = ProviderRegistry::new();
    for provider in &resources.providers {
        providers.register_with_source(
            provider.value.clone().into_descriptor(),
            provider.source_info.as_provider_source(),
        );
    }
    providers
        .provider("openai")
        .map(|provider| provider.base_url.clone())
}

fn env_openai_headers() -> Vec<(String, String)> {
    let mut headers = Vec::new();
    for (header, env_var) in [
        ("OpenAI-Organization", "OPENAI_ORGANIZATION"),
        ("OpenAI-Project", "OPENAI_PROJECT"),
    ] {
        if let Ok(value) = std::env::var(env_var) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                headers.push((header.to_string(), trimmed.to_string()));
            }
        }
    }
    headers
}

fn openai_timeout_secs() -> u64 {
    std::env::var("BURBOT_OPENAI_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(300)
}

fn openai_retry_attempts() -> u32 {
    std::env::var("BURBOT_OPENAI_RETRY_ATTEMPTS")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(3)
}

#[allow(clippy::unused_async)]
async fn _llm_layer_is_future_async_capable(_: Value) -> Result<Value> {
    Ok(json!({}))
}

#[cfg(test)]
mod policy_tests;
#[cfg(test)]
mod tests;

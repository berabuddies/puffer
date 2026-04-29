mod schema;

use crate::contract::{ActionContract, CapabilityContract};
use crate::planner::CompletionRole;
use anyhow::{anyhow, Context, Result};
use puffer_config::{load_config, ConfigPaths};
use puffer_provider_openai::{
    build_json_post_request, build_responses_request, extract_chat_completions_text,
    extract_responses_text, parse_chat_completions_response, parse_responses_response,
    refresh_oauth_token, BuiltOpenAIRequest, OpenAIAuth, OpenAIRequestConfig,
    OpenAIResponsesRequest,
};
use puffer_provider_registry::{
    detect_import_candidates, AuthStore, ExternalImportFamily, ProviderRegistry, StoredCredential,
};
use puffer_resources::load_resources;
use reqwest::blocking::Client;
use schema::{
    build_goal_verification_plain_request, build_goal_verification_request,
    build_observe_act_plain_request, build_observe_act_request, build_tool_call_plain_request,
    build_tool_call_request, goal_verification_prompt, observe_act_prompt,
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
    pub(crate) tool_id: String,
    pub(crate) args: Value,
    pub(crate) completion_role: CompletionRole,
    pub(crate) rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct GoalVerificationResult {
    pub(crate) satisfied: bool,
    pub(crate) confidence: f64,
    pub(crate) missing_evidence: Vec<String>,
    pub(crate) suggested_candidates: Vec<ModelCandidateProposal>,
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
                "input_schema": action.input_schema,
                "side_effect_class": action.side_effect_class,
                "risk_level": action.risk_level,
            })
        })
        .collect::<Vec<_>>();
    let prompt = format!(
        "You are Burbot's bounded Puffer-tool proposal operator.\n\
         Return exactly one JSON object with this schema: {{\"tool_id\":\"<available tool id>\",\"args\":{{...}}}}.\n\
         The tool call will be executed once through Puffer's existing tool runtime in the current workspace.\n\
         Choose only one of the available contract-declared tools below. Prefer the lowest-risk tool whose schema can satisfy the goal. \
         Do not include markdown. The proposal is invalid if it only prints, echoes, comments on, or restates future work instead of \
         actually inspecting evidence, creating or changing the required artifact, starting a required service, running a concrete check, \
         or repairing a concrete failure. Do not submit placeholder commands, TODO scaffolds, fake success markers, or commands whose \
         only effect is describing what should be implemented.\n\n\
         Available tools:\n{}\n\n\
         Goal:\n{goal}",
        serde_json::to_string_pretty(&available_tools)?
    );
    let images = image_context
        .map(image_data_urls_from_context)
        .unwrap_or_default();
    let normalized_model = normalize_openai_model(model);
    let request = build_tool_call_request(&config, &normalized_model, &prompt, contract, &images)?;
    let text = match send_and_parse(&request) {
        Ok(text) => text,
        Err(error) if response_format_schema_rejected(&error) => {
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
    let request =
        build_observe_act_request(&config, &normalized_model, &prompt, contract, &images)?;
    let text = match send_and_parse(&request) {
        Ok(text) => text,
        Err(error) if response_format_schema_rejected(&error) => {
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
    let request =
        build_goal_verification_request(&config, &normalized_model, &prompt, contract, &images)?;
    let text = match send_and_parse(&request) {
        Ok(text) => text,
        Err(error) if response_format_schema_rejected(&error) => {
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

/// Validates a structural Puffer tool call against the imported tool contract.
pub(crate) fn validate_puffer_tool_call(
    tool_id: &str,
    args: Value,
    contract: &CapabilityContract,
) -> Result<PufferToolCallProposal> {
    if tool_id.is_empty() {
        return Err(anyhow!("Puffer tool-call proposal missing `tool_id`"));
    }
    let action = action_for_tool_id(contract, tool_id)?;
    validate_tool_args_object(tool_id, &args)?;
    validate_tool_args_schema(action, &args)?;
    Ok(PufferToolCallProposal {
        tool_id: tool_id.to_string(),
        args,
    })
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

fn parse_tool_call_json(
    text: &str,
    contract: &CapabilityContract,
) -> Result<PufferToolCallProposal> {
    let value: Value = serde_json::from_str(text.trim()).with_context(|| {
        format!("failed to parse LLM tool-call proposal as strict JSON object: {text}")
    })?;
    validate_tool_call_value(value, contract)
}

fn parse_candidate_list_json(
    text: &str,
    contract: &CapabilityContract,
) -> Result<Vec<ModelCandidateProposal>> {
    let value: Value = serde_json::from_str(text.trim()).with_context(|| {
        format!("failed to parse LLM candidate proposal as strict JSON object: {text}")
    })?;
    let candidates = value
        .get("candidates")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("LLM candidate proposal missing array `candidates`"))?;
    if candidates.is_empty() {
        return Err(anyhow!("LLM candidate proposal contained no candidates"));
    }
    parse_model_candidate_array(candidates, contract, "candidate proposal")
}

fn parse_goal_verification_json(
    text: &str,
    contract: &CapabilityContract,
) -> Result<GoalVerificationResult> {
    let value: Value = serde_json::from_str(text.trim()).with_context(|| {
        format!("failed to parse LLM goal verification as strict JSON object: {text}")
    })?;
    let satisfied = value
        .get("satisfied")
        .and_then(Value::as_bool)
        .ok_or_else(|| anyhow!("LLM goal verification missing bool `satisfied`"))?;
    let confidence = value
        .get("confidence")
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
        .clamp(0.0, 1.0);
    let missing_evidence = value
        .get("missing_evidence")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let suggested_candidate_values = value
        .get("suggested_candidates")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let suggested_candidates = parse_model_candidate_array(
        &suggested_candidate_values,
        contract,
        "goal verification suggested_candidates",
    )?;
    if (!satisfied || !missing_evidence.is_empty()) && suggested_candidates.is_empty() {
        return Err(anyhow!(
            "LLM goal verification marked the goal unsatisfied without structural follow-up candidates"
        ));
    }
    Ok(GoalVerificationResult {
        satisfied,
        confidence,
        missing_evidence,
        suggested_candidates,
    })
}

fn parse_model_candidate_array(
    candidates: &[Value],
    contract: &CapabilityContract,
    source: &str,
) -> Result<Vec<ModelCandidateProposal>> {
    let mut parsed = Vec::new();
    let mut errors = Vec::new();
    for (index, candidate) in candidates.iter().enumerate() {
        match parse_model_candidate(candidate, contract) {
            Ok(candidate) => parsed.push(candidate),
            Err(error) => errors.push(format!("{index}: {error}")),
        }
    }
    if errors.is_empty() {
        Ok(parsed)
    } else {
        Err(anyhow!("invalid {source} entries: {}", errors.join("; ")))
    }
}

fn parse_model_candidate(
    value: &Value,
    contract: &CapabilityContract,
) -> Result<ModelCandidateProposal> {
    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("model candidate must be a JSON object"))?;
    let tool_id = object
        .get("tool_id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("model candidate missing `tool_id`"))?;
    let args = object
        .get("args")
        .cloned()
        .ok_or_else(|| anyhow!("model candidate missing object `args`"))?;
    let role = object
        .get("completion_role")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("model candidate missing `completion_role`"))
        .and_then(parse_completion_role)?;
    let rationale = object
        .get("rationale")
        .and_then(Value::as_str)
        .unwrap_or("model proposed this contract-gated action")
        .to_string();
    let proposal = validate_puffer_tool_call(tool_id, args, contract)?;
    Ok(ModelCandidateProposal {
        tool_id: proposal.tool_id,
        args: proposal.args,
        completion_role: role,
        rationale,
    })
}

fn parse_completion_role(value: &str) -> Result<CompletionRole> {
    match value {
        "terminal" => Ok(CompletionRole::Terminal),
        "support" => Ok(CompletionRole::Support),
        "verification" => Ok(CompletionRole::Verification),
        "repair" => Ok(CompletionRole::Repair),
        _ => Err(anyhow!("unknown model candidate completion_role `{value}`")),
    }
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

fn validate_tool_call_value(
    value: Value,
    contract: &CapabilityContract,
) -> Result<PufferToolCallProposal> {
    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("LLM tool-call proposal must be a JSON object"))?;
    for key in object.keys() {
        if key != "tool_id" && key != "args" {
            return Err(anyhow!(
                "LLM tool-call proposal contained unsupported field `{key}`"
            ));
        }
    }
    let tool_id = object
        .get("tool_id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("LLM tool-call proposal missing `tool_id`"))?;
    let args = object
        .get("args")
        .cloned()
        .ok_or_else(|| anyhow!("LLM tool-call proposal missing object `args`"))?;
    validate_puffer_tool_call(tool_id, args, contract)
}

fn action_for_tool_id<'a>(
    contract: &'a CapabilityContract,
    tool_id: &str,
) -> Result<&'a ActionContract> {
    contract
        .actions
        .iter()
        .find(|action| action.name == tool_id)
        .ok_or_else(|| {
            anyhow!("Puffer tool-call proposal selected unavailable Puffer tool `{tool_id}`")
        })
}

fn validate_tool_args_object(tool_id: &str, args: &Value) -> Result<()> {
    if args.is_object() {
        Ok(())
    } else {
        Err(anyhow!(
            "Puffer tool-call proposal args for `{tool_id}` must be a JSON object"
        ))
    }
}

fn validate_tool_args_schema(action: &ActionContract, args: &Value) -> Result<()> {
    let validator = jsonschema::validator_for(&action.input_schema)
        .with_context(|| format!("invalid input_schema for Puffer tool {}", action.name))?;
    let errors = validator
        .iter_errors(args)
        .take(5)
        .map(|error| error.to_string())
        .collect::<Vec<_>>();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(anyhow!(
            "Puffer tool-call proposal args for `{}` do not match input_schema: {}",
            action.name,
            errors.join(", ")
        ))
    }
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
        let body = json!({
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
        build_json_post_request(config, "/v1/chat/completions", &body)
    }
}

fn send_and_parse(request: &BuiltOpenAIRequest) -> Result<String> {
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(openai_timeout_secs()))
        .build()
        .context("failed to build OpenAI HTTP client")?;
    let attempts = openai_retry_attempts();
    let mut last_error = None;
    for attempt in 0..attempts {
        match send_and_parse_once(&client, request) {
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
        return Err(anyhow!(
            "OpenAI request failed with status {status}: {text}"
        ));
    }
    let output = parse_llm_response_text(&request.url, &text)?;
    if output.trim().is_empty() {
        Ok(text)
    } else {
        Ok(output)
    }
}

fn parse_llm_response_text(url: &str, text: &str) -> Result<String> {
    if is_chat_completions_url(url) {
        let parsed = parse_chat_completions_response(text)?;
        Ok(extract_chat_completions_text(&parsed))
    } else {
        let parsed = parse_responses_response(text)?;
        Ok(extract_responses_text(&parsed))
    }
}

fn retryable_openai_error(error: &anyhow::Error) -> bool {
    retryable_openai_error_message(&error.to_string())
}

/// Returns whether an OpenAI error message represents a retryable transport failure.
pub(crate) fn retryable_openai_error_message(message: &str) -> bool {
    message.contains("timed out")
        || message.contains("connection error")
        || message.contains("status 408")
        || message.contains("status 429")
        || message.contains("status 500")
        || message.contains("status 502")
        || message.contains("status 503")
        || message.contains("status 504")
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
    error.to_string().contains("status 401")
}

fn response_format_schema_rejected(error: &anyhow::Error) -> bool {
    let message = error.to_string();
    message.contains("Invalid schema for response_format")
        || message.contains("unsupported response_format")
        || message.contains("response_format type is unavailable")
}

fn is_codex_backend(base_url: &str) -> bool {
    let trimmed = base_url.trim_end_matches('/');
    trimmed.contains("/backend-api") || trimmed.contains("/api/codex")
}

fn supports_responses_api(base_url: &str) -> bool {
    is_codex_backend(base_url) || base_url.contains("api.openai.com")
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
mod tests;

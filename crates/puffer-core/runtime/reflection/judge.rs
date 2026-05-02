use super::llm::{build_llm_judge_prompt, render_llm_judge_context, render_relevant_paths};
use super::{
    BatchAssessment, JudgeSignal, LlmJudgeConfig, LlmJudgePromptCacheMode, ReflectionLanguage,
};
use crate::runtime::openai::conversation::{items_to_responses_input, ConversationItem};
use crate::runtime::openai::{
    build_codex_openai_request_body, parse_openai_assistant_text, resolve_openai_execution_config,
    send_openai_request_with_refresh,
};
use crate::runtime::system_prompt::render_runtime_system_prompt;
use crate::runtime::{execute_user_prompt_with_options, TurnRequestOptions};
// `resolve_provider_and_model` / `resolve_model_api` are file-private in
// `runtime.rs`; pulling them through `super::super` at the import site keeps
// the reaching-up-two-levels smell confined to one place.
use super::super::{resolve_model_api, resolve_provider_and_model};
use crate::AppState;
use anyhow::Context;
use puffer_provider_openai::{build_json_post_request, OpenAIResponsesResponse};
use puffer_provider_registry::{AuthStore, ProviderDescriptor, ProviderRegistry};
use puffer_resources::LoadedResources;
use serde_json::Value;
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::hash::{Hash, Hasher};

#[derive(Debug, Clone)]
pub(super) struct LlmJudgeAttempt {
    pub(super) provider: Option<String>,
    pub(super) model: Option<String>,
    pub(super) prompt: String,
    pub(super) request_url: Option<String>,
    pub(super) prompt_cache_key: Option<String>,
    pub(super) request_body: Option<String>,
    pub(super) raw_response_text: Option<String>,
    pub(super) raw_response_body: Option<String>,
    pub(super) response_id: Option<String>,
    pub(super) input_tokens: Option<u64>,
    pub(super) output_tokens: Option<u64>,
    pub(super) cached_input_tokens: Option<u64>,
    pub(super) cache_hit_ratio: Option<String>,
    pub(super) error: Option<String>,
}

impl LlmJudgeAttempt {
    pub(super) fn with_raw_response_text(&self, raw_response_text: Option<String>) -> Self {
        let mut clone = self.clone();
        clone.raw_response_text = raw_response_text;
        clone
    }
}

pub(super) fn run_llm_judge(
    goal: &str,
    relevant_paths: &BTreeSet<String>,
    language: ReflectionLanguage,
    config: &LlmJudgeConfig,
    assessment: &BatchAssessment,
    code_signal: Option<&JudgeSignal>,
    items: &[ConversationItem],
    state: &AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
) -> LlmJudgeAttempt {
    let prompt = build_llm_judge_prompt(
        language,
        goal,
        assessment,
        code_signal,
        &render_llm_judge_context(
            items,
            config.context_scope,
            config.recent_item_count,
            config.max_context_chars,
            config.max_tool_output_chars,
        ),
        &render_relevant_paths(relevant_paths),
    );
    let mut side_state = build_llm_judge_side_state(state, config, &prompt);
    let mut side_resources = resources.clone();
    side_resources.tools.clear();
    let provider = side_state.current_provider.clone();
    let model = side_state.current_model.clone();

    let resolved = match resolve_provider_and_model(&side_state, providers) {
        Ok(value) => value,
        Err(error) => {
            return LlmJudgeAttempt {
                provider,
                model,
                prompt,
                request_url: None,
                prompt_cache_key: side_state.prompt_cache_key_override.clone(),
                request_body: None,
                raw_response_text: None,
                raw_response_body: None,
                response_id: None,
                input_tokens: None,
                output_tokens: None,
                cached_input_tokens: None,
                cache_hit_ratio: None,
                error: Some(error.to_string()),
            };
        }
    };
    let (provider_descriptor, model_id) = resolved;
    let api = resolve_model_api(&side_state, providers, provider_descriptor, &model_id);

    if matches!(
        api.as_str(),
        "openai-responses" | "azure-openai-responses" | "openai-codex-responses"
    ) {
        return run_openai_responses_judge(
            &mut side_state,
            &side_resources,
            provider_descriptor,
            &model_id,
            auth_store,
            &prompt,
        );
    }

    match execute_user_prompt_with_options(
        &mut side_state,
        &side_resources,
        providers,
        auth_store,
        &prompt,
        TurnRequestOptions {
            structured_output: None,
            tool_filter: None,
            reflection: None,
            cancel: None,
        },
    ) {
        Ok(response) => LlmJudgeAttempt {
            provider,
            model,
            prompt,
            request_url: None,
            prompt_cache_key: side_state.prompt_cache_key_override.clone(),
            request_body: None,
            raw_response_text: Some(response.assistant_text),
            raw_response_body: None,
            response_id: None,
            input_tokens: side_state.last_input_tokens.map(u64::from),
            output_tokens: None,
            cached_input_tokens: None,
            cache_hit_ratio: side_state
                .last_cache_hit_ratio
                .map(|value| format!("{value:.2}")),
            error: None,
        },
        Err(error) => LlmJudgeAttempt {
            provider,
            model,
            prompt,
            request_url: None,
            prompt_cache_key: side_state.prompt_cache_key_override.clone(),
            request_body: None,
            raw_response_text: None,
            raw_response_body: None,
            response_id: None,
            input_tokens: None,
            output_tokens: None,
            cached_input_tokens: None,
            cache_hit_ratio: None,
            error: Some(error.to_string()),
        },
    }
}

pub(super) fn build_llm_judge_side_state(
    state: &AppState,
    config: &LlmJudgeConfig,
    prompt: &str,
) -> AppState {
    let mut side_state = state.clone();
    side_state.transcript.clear();
    side_state.plan_mode = false;
    side_state.plan_mode_attachment_turns = 0;
    side_state.plan_mode_attachment_count = 0;
    side_state.plan_mode_has_exited = false;
    side_state.plan_mode_needs_reentry_attachment = false;
    side_state.plan_mode_needs_exit_attachment = false;
    side_state.last_input_tokens = None;
    side_state.last_cache_hit_ratio = None;
    side_state.session_cache_hit_ratio = None;
    if let Some(selector) = &config.model_selector {
        side_state.current_model = Some(selector.clone());
        if let Some((provider, _)) = selector.split_once('/') {
            side_state.current_provider = Some(provider.to_string());
        }
    }
    if let Some(effort) = &config.effort_level {
        side_state.effort_level = effort.clone();
    }
    side_state.prompt_cache_key_override = match config.prompt_cache_mode {
        LlmJudgePromptCacheMode::InheritMainAgent => side_state.prompt_cache_key_override.clone(),
        LlmJudgePromptCacheMode::Dedicated => Some(build_llm_judge_prompt_cache_key(
            state,
            side_state.current_model.as_deref(),
            prompt,
        )),
    };
    side_state
}

fn run_openai_responses_judge(
    state: &mut AppState,
    resources: &LoadedResources,
    provider: &ProviderDescriptor,
    model_id: &str,
    auth_store: &mut AuthStore,
    prompt: &str,
) -> LlmJudgeAttempt {
    let mut attempt = LlmJudgeAttempt {
        provider: Some(provider.id.clone()),
        model: Some(format!("{}/{}", provider.id, model_id)),
        prompt: prompt.to_string(),
        request_url: None,
        prompt_cache_key: state.prompt_cache_key_override.clone(),
        request_body: None,
        raw_response_text: None,
        raw_response_body: None,
        response_id: None,
        input_tokens: None,
        output_tokens: None,
        cached_input_tokens: None,
        cache_hit_ratio: None,
        error: None,
    };
    let mut execution = match resolve_openai_execution_config(state, auth_store, provider) {
        Ok(value) => value,
        Err(error) => {
            attempt.error = Some(error.to_string());
            return attempt;
        }
    };
    let supports_reasoning = model_supports_reasoning(provider, model_id);
    let instructions =
        match render_runtime_system_prompt(state, resources, model_id, &BTreeSet::new()) {
            Ok(value) => value,
            Err(error) => {
                attempt.error = Some(error.to_string());
                return attempt;
            }
        };
    let wire_input = items_to_responses_input(&[ConversationItem::user_message(prompt)]);
    let body = build_codex_openai_request_body(
        state,
        model_id,
        &instructions,
        wire_input,
        &[],
        supports_reasoning,
        None,
        false,
    );
    attempt.prompt_cache_key = body
        .get("prompt_cache_key")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let captured_request = RefCell::new(None);
    let response =
        match send_openai_request_with_refresh(auth_store, &mut execution, |request_config| {
            let request = build_json_post_request(
                request_config,
                responses_path(&request_config.base_url),
                &body,
            )?;
            let _ = captured_request.replace(Some(request.clone()));
            Ok(request)
        }) {
            Ok(value) => value,
            Err(error) => {
                if let Some(request) = captured_request.into_inner() {
                    attempt.request_url = Some(request.url);
                    attempt.request_body = Some(request.body);
                }
                attempt.error = Some(error.to_string());
                return attempt;
            }
        };
    let request = match captured_request
        .into_inner()
        .context("llm judge request was not captured before dispatch")
    {
        Ok(value) => value,
        Err(error) => {
            attempt.error = Some(error.to_string());
            return attempt;
        }
    };
    attempt.request_url = Some(request.url);
    attempt.request_body = Some(request.body);
    let raw_response_body = serde_json::to_string_pretty(&response)
        .or_else(|_| serde_json::to_string(&response))
        .unwrap_or_else(|_| response.to_string());
    attempt.raw_response_body = Some(raw_response_body);
    attempt.response_id = response
        .get("id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let input_tokens = response
        .pointer("/usage/input_tokens")
        .and_then(Value::as_u64);
    let output_tokens = response
        .pointer("/usage/output_tokens")
        .and_then(Value::as_u64);
    let cached_input_tokens = response
        .pointer("/usage/input_tokens_details/cached_tokens")
        .and_then(Value::as_u64);
    attempt.input_tokens = input_tokens;
    attempt.output_tokens = output_tokens;
    attempt.cached_input_tokens = cached_input_tokens;
    if let Some(input) = input_tokens {
        state.last_input_tokens = Some(input as u32);
        state.update_cache_stats(input, cached_input_tokens.unwrap_or(0));
    }
    let parsed: OpenAIResponsesResponse = match serde_json::from_value(response.clone())
        .context("failed to parse OpenAI Responses payload")
    {
        Ok(value) => value,
        Err(error) => {
            attempt.error = Some(error.to_string());
            attempt.cache_hit_ratio = state
                .last_cache_hit_ratio
                .map(|value| format!("{value:.2}"));
            return attempt;
        }
    };
    attempt.raw_response_text = match parse_openai_assistant_text(&parsed, &response, state)
        .context("failed to read judge text")
    {
        Ok(value) => Some(value),
        Err(error) => {
            attempt.error = Some(error.to_string());
            attempt.cache_hit_ratio = state
                .last_cache_hit_ratio
                .map(|value| format!("{value:.2}"));
            return attempt;
        }
    };
    attempt.cache_hit_ratio = state
        .last_cache_hit_ratio
        .map(|value| format!("{value:.2}"));
    attempt
}

fn build_llm_judge_prompt_cache_key(
    state: &AppState,
    model_selector: Option<&str>,
    prompt: &str,
) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (
        "reflection-judge",
        state.session.id,
        model_selector.unwrap_or_default(),
        prompt.trim(),
    )
        .hash(&mut hasher);
    format!("reflection-judge-{:016x}", hasher.finish())
}

fn model_supports_reasoning(provider: &ProviderDescriptor, model_id: &str) -> bool {
    provider
        .models
        .iter()
        .find(|model| model.id == model_id)
        .map(|model| model.supports_reasoning)
        .unwrap_or(false)
}

fn responses_path(base_url: &str) -> &'static str {
    let trimmed = base_url.trim_end_matches('/');
    if trimmed.contains("/backend-api") || trimmed.contains("/api/codex") {
        "/responses"
    } else {
        "/v1/responses"
    }
}

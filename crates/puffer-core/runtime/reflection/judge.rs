use super::llm::{
    build_llm_judge_prompt, build_llm_judge_subagent_prompt, render_llm_judge_context,
    render_relevant_paths,
};
use super::{
    BatchAssessment, JudgeSignal, LlmJudgeConfig, LlmJudgePromptCacheMode, LlmJudgeStrategy,
    ReflectionLanguage,
};
use crate::runtime::openai::conversation::{items_to_responses_input, ConversationItem};
use crate::runtime::openai::{
    build_codex_openai_request_body, parse_openai_assistant_text, resolve_openai_execution_config,
    send_openai_request_with_refresh,
};
use crate::runtime::request_tool_filter::build_request_tool_filter;
use crate::runtime::system_prompt::render_runtime_system_prompt;
use crate::runtime::{execute_user_prompt_with_options, CancelToken, TurnRequestOptions};
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

#[allow(clippy::too_many_arguments)]
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
    // Parent agent loop's cancel token. Threaded into the sub-agent path
    // so user Ctrl+C interrupts an in-flight judge instead of burning
    // more provider calls; ignored by the single-call path (one
    // round-trip, nothing to interrupt mid-stream).
    cancel: Option<&CancelToken>,
) -> LlmJudgeAttempt {
    // Dispatch on strategy. The two paths share `build_llm_judge_side_state`
    // (= same parent_session_id linkage, fresh trace, etc.) and the same
    // return type — only the prompt + tool catalog differ.
    let context = render_llm_judge_context(
        items,
        config.context_scope,
        config.recent_item_count,
        config.max_context_chars,
        config.max_tool_output_chars,
    );
    let relevant = render_relevant_paths(relevant_paths);
    match config.strategy {
        LlmJudgeStrategy::SingleCall => run_llm_judge_single_call(
            language,
            goal,
            assessment,
            code_signal,
            &context,
            &relevant,
            config,
            state,
            resources,
            providers,
            auth_store,
            cancel,
        ),
        LlmJudgeStrategy::SubAgent { max_iterations } => run_llm_judge_subagent(
            language,
            goal,
            assessment,
            code_signal,
            &context,
            &relevant,
            max_iterations,
            config,
            state,
            resources,
            providers,
            auth_store,
            cancel,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn run_llm_judge_single_call(
    language: ReflectionLanguage,
    goal: &str,
    assessment: &BatchAssessment,
    code_signal: Option<&JudgeSignal>,
    context: &str,
    relevant: &str,
    config: &LlmJudgeConfig,
    state: &AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    cancel: Option<&CancelToken>,
) -> LlmJudgeAttempt {
    let prompt = build_llm_judge_prompt(language, goal, assessment, code_signal, context, relevant);
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
            cancel,
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
            // Inherit parent's cancel so user Ctrl+C interrupts an
            // in-flight single-call judge — mirrors CC's `M85`
            // reactive-compact (`maxTurns:1`) which checks
            // `abortController.signal.aborted` after the fork returns
            // (claude-2.1.133 bundle line 2212).
            cancel,
            max_turns: None,
            lightweight_context: false,
            observability: None,
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

/// Sub-agent reflection judge. Same side-state plumbing as the
/// single-call path, but:
/// - Tool catalog narrowed to `Read`/`Grep`/`Glob` (read-only
///   verification) via the request-scoped tool filter.
/// - Prompt instructs the judge to *use* those tools to ground its
///   verdict in filesystem evidence, then return the same JSON shape.
/// - `reflection_config` is cleared on the cloned side state and
///   `TurnRequestOptions.reflection` is `None` (recursion guard —
///   without both, `apply_session_reflection_default` re-injects the
///   parent's policy and the judge spawns its own grader).
/// - Parent `cancel` is forwarded so user Ctrl+C interrupts an
///   in-flight judge instead of burning more provider calls.
/// - `max_iterations` becomes `TurnRequestOptions.max_turns`, a hard
///   cap on inner-loop turns. The prompt also advertises the limit;
///   the cap is the safety net for a model that ignores the hint.
#[allow(clippy::too_many_arguments)]
fn run_llm_judge_subagent(
    language: ReflectionLanguage,
    goal: &str,
    assessment: &BatchAssessment,
    code_signal: Option<&JudgeSignal>,
    context: &str,
    relevant: &str,
    max_iterations: u32,
    config: &LlmJudgeConfig,
    state: &AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    cancel: Option<&CancelToken>,
) -> LlmJudgeAttempt {
    let prompt = build_llm_judge_subagent_prompt(
        language,
        goal,
        assessment,
        code_signal,
        context,
        relevant,
        max_iterations,
    );
    let mut side_state = build_llm_judge_side_state(state, config, &prompt);
    let provider = side_state.current_provider.clone();
    let model = side_state.current_model.clone();

    // Verification toolset: Read / Grep / Glob only. The
    // `request_tool_filter` is canonical-name aware so callers don't
    // need to enumerate aliases. Write/Edit/Bash are deliberately
    // absent — the judge judges, it does not repair.
    let allowed_tools: Vec<String> = ["Read", "Grep", "Glob"]
        .into_iter()
        .map(String::from)
        .collect();
    let tool_filter = match build_request_tool_filter(&allowed_tools) {
        Ok(filter) => filter,
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
                error: Some(format!("failed to build judge tool filter: {error}")),
            };
        }
    };

    match execute_user_prompt_with_options(
        &mut side_state,
        resources,
        providers,
        auth_store,
        &prompt,
        TurnRequestOptions {
            structured_output: None,
            tool_filter: tool_filter.as_ref(),
            // Recursive reflection inside a reflection sub-agent would
            // burn tokens for negligible signal — disable. Note: we
            // also clear `side_state.reflection_config` in
            // `build_llm_judge_side_state` because
            // `apply_session_reflection_default` would otherwise
            // re-inject it from the cloned parent state.
            reflection: None,
            // Inherit the parent's cancel token so user Ctrl+C
            // interrupts a long judge run mid-iteration. The agent
            // loop checks the token at every turn boundary.
            cancel,
            // Hard cap so even a misbehaving model that keeps
            // requesting Read/Grep/Glob calls forever stops at the
            // configured budget. The prompt also advertises this
            // limit; this is the belt to that suspenders.
            max_turns: Some(max_iterations),
            observability: None,
            lightweight_context: false,
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
    // Reflection judge is a subagent. Mint a fresh session id so its
    // trace lives in its own Langfuse Session view, and link back to
    // the parent via `SessionMetadata::parent_session_id` so the root
    // agent_loop span emits `puffer.parent.session_id` +
    // `puffer.subagent.kind=agent_tool`. The trace pipeline uses the
    // same field to select `PromptWithEmbeddedToolIo` for the rendered
    // judge prompt (which embeds tool calls / outputs), so under
    // INCLUDE_PROMPTS=1, INCLUDE_TOOL_IO=0 the prompt is redacted.
    // Review v6 BLOCK #1.
    side_state.session.parent_session_id = Some(state.session.id);
    side_state.session.id = uuid::Uuid::new_v4();
    side_state.transcript.clear();
    // Recursion guard. Without this, `apply_session_reflection_default`
    // (runtime.rs) would re-inject the parent's `reflection_config`
    // into the sub-agent's `TurnRequestOptions` because we cloned the
    // parent state — and the sub-agent would then spawn its own
    // grader sub-agent, which would spawn another, etc. Setting
    // `options.reflection = None` at the call site is necessary but
    // not sufficient; the cloned state has to be cleared too. Mirrors
    // CC's `runForkedAgent`, which deliberately strips
    // recursion-prone session knobs before forking.
    side_state.reflection_config = None;
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
    cancel: Option<&CancelToken>,
) -> LlmJudgeAttempt {
    // Pre-flight cancel check: if the user already pressed Esc before
    // this judge call started (parent loop hadn't reached its next
    // turn boundary yet), skip the HTTP round-trip entirely. Mirrors
    // CC's `M85` post-fork `abortController.signal.aborted` check
    // (claude-2.1.133 bundle line 2212) — same granularity, just
    // applied at the only sync boundary this path has.
    if let Some(token) = cancel {
        if token.is_cancelled() {
            return LlmJudgeAttempt {
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
                error: Some("judge cancelled by user".to_string()),
            };
        }
    }
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
        &execution.request_config.base_url,
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
    let response = match send_openai_request_with_refresh(
        auth_store,
        &mut execution,
        &state.config.network.proxy,
        |request_config| {
            let request = build_json_post_request(
                request_config,
                responses_path(&request_config.base_url),
                &body,
            )?;
            let _ = captured_request.replace(Some(request.clone()));
            Ok(request)
        },
    ) {
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

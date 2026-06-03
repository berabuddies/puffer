use super::support::{
    is_openai_structured_output_error, prefer_native_structured_output,
    structured_output_endpoint_id, OPENAI_STRUCTURED_OUTPUT_FAMILY,
};
use super::websocket::{execute_openai_websocket_streaming, openai_websocket_enabled};
use super::websocket_state::openai_websocket_http_fallback_active;
use crate::runtime::provider_adapter::ProviderAdapter;
use crate::runtime::{agent_loop, blocking_loop, mcp_discovery};
use crate::runtime::{TurnExecution, TurnRequestOptions, TurnStreamEvent};
use crate::AppState;
use anyhow::Result;
use puffer_provider_registry::{AuthStore, ProviderDescriptor, ProviderRegistry};
use puffer_resources::LoadedResources;

/// Adapter for the OpenAI Responses API family — covers `openai-responses`,
/// `azure-openai-responses`, and `openai-codex-responses`. The streaming
/// implementation switches between the websocket and SSE transports based
/// on `openai_websocket_enabled()`, so the loop never branches on transport.
pub(crate) struct OpenAIResponsesAdapter;

impl ProviderAdapter for OpenAIResponsesAdapter {
    fn api_id(&self) -> &'static str {
        // The adapter handles three aliases; the canonical id used in
        // error messages is the most common one.
        "openai-responses"
    }

    fn execute_turn(
        &self,
        state: &mut AppState,
        resources: &LoadedResources,
        providers: &ProviderRegistry,
        provider: &ProviderDescriptor,
        model_id: String,
        auth_store: &mut AuthStore,
        input: &str,
        options: TurnRequestOptions<'_>,
    ) -> Result<TurnExecution> {
        let use_native =
            prefer_native_structured_output(state, provider, &model_id, options.structured_output);
        match run_responses_attempt(
            state, resources, providers, provider, &model_id, auth_store, input, &options,
            use_native, None,
        ) {
            Ok(turn) => Ok(turn),
            Err(error) if use_native && is_openai_structured_output_error(&error) => {
                state.mark_native_structured_output_unsupported(
                    OPENAI_STRUCTURED_OUTPUT_FAMILY,
                    provider.id.as_str(),
                    &model_id,
                    structured_output_endpoint_id(provider),
                );
                run_responses_attempt(
                    state, resources, providers, provider, &model_id, auth_store, input, &options,
                    false, None,
                )
            }
            Err(error) => Err(error),
        }
    }

    fn execute_turn_streaming(
        &self,
        state: &mut AppState,
        resources: &LoadedResources,
        providers: &ProviderRegistry,
        provider: &ProviderDescriptor,
        model_id: String,
        auth_store: &mut AuthStore,
        input: &str,
        options: TurnRequestOptions<'_>,
        on_event: &mut dyn FnMut(TurnStreamEvent),
    ) -> Result<TurnExecution> {
        // WebSocket transport keeps the legacy (non-agent_loop) path —
        // it has fundamentally different framing and per-event flow
        // control. SSE path uses agent_loop + responses_session.
        if openai_websocket_enabled() && !openai_websocket_http_fallback_active(state) {
            let mut wrapped = |event: TurnStreamEvent| on_event(event);
            return execute_openai_websocket_streaming(
                state,
                resources,
                providers,
                provider,
                model_id,
                auth_store,
                input,
                options,
                &mut wrapped,
            );
        }
        let use_native =
            prefer_native_structured_output(state, provider, &model_id, options.structured_output);
        match run_responses_attempt(
            state,
            resources,
            providers,
            provider,
            &model_id,
            auth_store,
            input,
            &options,
            use_native,
            Some(on_event as &mut dyn FnMut(TurnStreamEvent)),
        ) {
            Ok(turn) => Ok(turn),
            Err(error) if use_native && is_openai_structured_output_error(&error) => {
                state.mark_native_structured_output_unsupported(
                    OPENAI_STRUCTURED_OUTPUT_FAMILY,
                    provider.id.as_str(),
                    &model_id,
                    structured_output_endpoint_id(provider),
                );
                run_responses_attempt(
                    state,
                    resources,
                    providers,
                    provider,
                    &model_id,
                    auth_store,
                    input,
                    &options,
                    false,
                    Some(on_event),
                )
            }
            Err(error) => Err(error),
        }
    }
}

/// One attempt of OpenAI Responses execution: build a session with the
/// requested `use_native` flag, hand it off to `agent_loop`. The
/// adapter retries with `use_native=false` if the first attempt fails
/// with a native structured-output unsupported error.
fn run_responses_attempt(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    provider: &ProviderDescriptor,
    model_id: &str,
    auth_store: &mut AuthStore,
    input: &str,
    options: &TurnRequestOptions<'_>,
    use_native: bool,
    on_event: Option<&mut dyn FnMut(TurnStreamEvent)>,
) -> Result<TurnExecution> {
    let registry = mcp_discovery::registry_with_mcp_tools(resources, state.tool_runner.as_ref());
    let mut session = super::responses_session::setup_responses_session(
        state,
        resources,
        provider,
        model_id.to_string(),
        auth_store,
        options,
        use_native,
    )?;
    let mut inputs = agent_loop::LoopInputs {
        state,
        resources,
        providers,
        provider,
        model_id,
        auth_store,
        input,
        reflection_config: options.reflection.clone(),
        tool_filter: options.tool_filter,
        registry: &registry,
        cancel: options.cancel,
        max_turns: options.max_turns,
        observability: options.observability.clone(),
    };
    match on_event {
        Some(sink) => agent_loop::run_streaming_loop(&mut inputs, &mut session, sink),
        None => blocking_loop::run_blocking_loop(&mut inputs, &mut session),
    }
}

/// Adapter for OpenAI Chat Completions (`openai-completions`).
pub(crate) struct OpenAICompletionsAdapter;

impl ProviderAdapter for OpenAICompletionsAdapter {
    fn api_id(&self) -> &'static str {
        "openai-completions"
    }

    fn execute_turn(
        &self,
        state: &mut AppState,
        resources: &LoadedResources,
        providers: &ProviderRegistry,
        provider: &ProviderDescriptor,
        model_id: String,
        auth_store: &mut AuthStore,
        input: &str,
        options: TurnRequestOptions<'_>,
    ) -> Result<TurnExecution> {
        let use_native =
            prefer_native_structured_output(state, provider, &model_id, options.structured_output);
        match run_completions_attempt(
            state, resources, providers, provider, &model_id, auth_store, input, &options,
            use_native, None,
        ) {
            Ok(turn) => Ok(turn),
            Err(error) if use_native && is_openai_structured_output_error(&error) => {
                state.mark_native_structured_output_unsupported(
                    OPENAI_STRUCTURED_OUTPUT_FAMILY,
                    provider.id.as_str(),
                    &model_id,
                    structured_output_endpoint_id(provider),
                );
                run_completions_attempt(
                    state, resources, providers, provider, &model_id, auth_store, input, &options,
                    false, None,
                )
            }
            Err(error) => Err(error),
        }
    }

    fn execute_turn_streaming(
        &self,
        state: &mut AppState,
        resources: &LoadedResources,
        providers: &ProviderRegistry,
        provider: &ProviderDescriptor,
        model_id: String,
        auth_store: &mut AuthStore,
        input: &str,
        options: TurnRequestOptions<'_>,
        on_event: &mut dyn FnMut(TurnStreamEvent),
    ) -> Result<TurnExecution> {
        let use_native =
            prefer_native_structured_output(state, provider, &model_id, options.structured_output);
        // Without an explicit streaming attempt the session's
        // `one_turn_streaming` (which synthesizes ThinkingDelta /
        // TextDelta from `reasoning_content`) is never invoked — the
        // default trait impl routes to `execute_turn` and therefore
        // `run_blocking_loop`, dropping the live thinking signal that
        // reasoning-capable Chat Completions providers (Moonshot Kimi
        // `k2p5`, Deepseek, OpenRouter) emit. Wire it explicitly.
        match run_completions_attempt(
            state,
            resources,
            providers,
            provider,
            &model_id,
            auth_store,
            input,
            &options,
            use_native,
            Some(on_event),
        ) {
            Ok(turn) => Ok(turn),
            Err(error) if use_native && is_openai_structured_output_error(&error) => {
                state.mark_native_structured_output_unsupported(
                    OPENAI_STRUCTURED_OUTPUT_FAMILY,
                    provider.id.as_str(),
                    &model_id,
                    structured_output_endpoint_id(provider),
                );
                run_completions_attempt(
                    state,
                    resources,
                    providers,
                    provider,
                    &model_id,
                    auth_store,
                    input,
                    &options,
                    false,
                    Some(on_event),
                )
            }
            Err(error) => Err(error),
        }
    }
}

/// One attempt of OpenAI Chat Completions execution.
///
/// `on_event` distinguishes blocking from streaming dispatch: when
/// `Some(...)`, route to `run_streaming_loop` so the session's
/// `one_turn_streaming` fires `ThinkingDelta` + `TextDelta` events;
/// otherwise route to `run_blocking_loop` (no events). Without this
/// branch, the streaming path never reaches the session's event-emit
/// site and reasoning-capable providers' thinking blocks stay silent.
fn run_completions_attempt(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    provider: &ProviderDescriptor,
    model_id: &str,
    auth_store: &mut AuthStore,
    input: &str,
    options: &TurnRequestOptions<'_>,
    use_native: bool,
    on_event: Option<&mut dyn FnMut(TurnStreamEvent)>,
) -> Result<TurnExecution> {
    let registry = mcp_discovery::registry_with_mcp_tools(resources, state.tool_runner.as_ref());
    let mut session = super::completions_session::setup_completions_session(
        state,
        resources,
        provider,
        model_id.to_string(),
        auth_store,
        options,
        use_native,
    )?;
    let mut inputs = agent_loop::LoopInputs {
        state,
        resources,
        providers,
        provider,
        model_id,
        auth_store,
        input,
        reflection_config: options.reflection.clone(),
        tool_filter: options.tool_filter,
        registry: &registry,
        cancel: options.cancel,
        max_turns: options.max_turns,
        observability: options.observability.clone(),
    };
    match on_event {
        Some(sink) => agent_loop::run_streaming_loop(&mut inputs, &mut session, sink),
        None => blocking_loop::run_blocking_loop(&mut inputs, &mut session),
    }
}

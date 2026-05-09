//! [`TurnSession`] impl for the OpenAI Responses API.
//!
//! Captures execution config + serialized tools + system instructions
//! once per user prompt; threads `previous_response_id` and
//! `continuation_start` across iterations of the agent_loop so that
//! providers supporting server-side response threading send only
//! continuation items per turn instead of the full transcript.
//!
//! Vendor types (`OpenAIResponsesResponse`, `OpenAIResponsesTool`,
//! `OpenAIRequestConfig`) stay confined to this submodule + the parent
//! `runtime/openai.rs` — agent_loop only sees neutral `ConversationItem`
//! / `ToolCallRequest` / `TurnStreamEvent`.

use anyhow::Result;
use puffer_provider_openai::{
    build_json_post_request, extract_responses_tool_calls, OpenAIRequestConfig,
    OpenAIResponsesResponse, OpenAIResponsesTextConfig, OpenAIResponsesTool,
};
use puffer_provider_registry::{AuthStore, ProviderDescriptor};
use puffer_resources::LoadedResources;
use puffer_tools::ToolRegistry;
use serde_json::Value;
use std::collections::HashSet;

use super::conversation::{
    append_reasoning_items, generate_openai_summary,
    insert_context_reminder_preserving_legacy_leading_system, insert_managed_system_prompt_1,
    items_to_responses_input, managed_system_prompt_1_from_env, ConversationItem,
};
use super::support::{
    apply_previous_response_id, build_codex_openai_request_body, openai_responses_path,
};
use super::support::{openai_model_supports_reasoning, openai_supports_response_threading};
use super::{
    parse_openai_assistant_text, resolve_openai_execution_config, send_openai_request_with_refresh,
    send_openai_request_with_refresh_streaming, OpenAIExecutionConfig,
};
use crate::permissions::load_runtime_permission_context;
use crate::runtime::agent_loop::{AssistantTurn, TurnSession};
use crate::runtime::structured_output_support::StructuredOutputConfig;
use crate::runtime::structured_output_support::{
    openai_responses_text_config, openai_tool_definitions_for_request,
};
use crate::runtime::system_prompt::render_runtime_system_prompt;
use crate::runtime::tool_executor::ToolExecutionBackend;
use crate::runtime::TurnRequestOptions;
use crate::runtime::{ToolCallRequest, TurnStreamEvent, TurnUsageReport};
use crate::AppState;

/// Per-prompt session for the OpenAI Responses API. All static-per-prompt
/// data (execution config, tools, system instructions, capability flags)
/// is captured by [`setup_responses_session`]; the threading state
/// (`previous_response_id`, `continuation_start`) is mutated turn by
/// turn inside the session itself.
pub(super) struct OpenAIResponsesTurnSession {
    pub execution: OpenAIExecutionConfig,
    pub instructions: String,
    pub managed_system_prompt_1: Option<String>,
    pub tools: Vec<OpenAIResponsesTool>,
    pub text: Option<OpenAIResponsesTextConfig>,
    pub structured_output: Option<StructuredOutputConfig>,
    pub model_id: String,
    pub supports_reasoning: bool,
    pub supports_response_threading: bool,
    /// Server-side response identifier from the most recent turn. When
    /// set, the next request omits already-known prefix items.
    pub previous_response_id: Option<String>,
    /// First index of items NOT yet known to the API. When threading is
    /// active, the next request only sends `items[continuation_start..]`.
    pub continuation_start: Option<usize>,
}

impl OpenAIResponsesTurnSession {
    /// Slices the transcript to whatever the API still needs to see.
    fn build_wire_input(&self, items: &[ConversationItem]) -> Value {
        match (
            self.supports_response_threading,
            self.previous_response_id.as_ref(),
            self.continuation_start,
        ) {
            (true, Some(_), Some(start)) if start <= items.len() => {
                items_to_responses_input(&items[start..])
            }
            _ => items_to_responses_input(items),
        }
    }

    /// Constructs the JSON request body for one turn (streaming flag
    /// flips the `stream: true` field plus internal SSE expectations).
    fn build_request_body(&self, wire_input: Value, state: &AppState, stream: bool) -> Value {
        let mut body = build_codex_openai_request_body(
            state,
            &self.model_id,
            &self.instructions,
            wire_input,
            &self.tools,
            self.supports_reasoning,
            self.text.clone(),
            stream,
        );
        let prev_resp_id = if self.supports_response_threading {
            self.previous_response_id.as_deref()
        } else {
            None
        };
        apply_previous_response_id(&mut body, prev_resp_id);
        body
    }

    /// Reads typed token-usage fields from a Responses payload, updates
    /// `state.last_input_tokens` + cache stats. Caller is responsible
    /// for emitting any `TurnStreamEvent::Usage` event (streaming path
    /// only).
    fn record_usage(
        state: &mut AppState,
        input_tokens: Option<usize>,
        cached_tokens: u64,
    ) -> Option<usize> {
        if let Some(tokens) = input_tokens {
            state.last_input_tokens = Some(tokens as u32);
        }
        if let Some(input) = input_tokens {
            state.update_cache_stats(input as u64, cached_tokens);
        }
        input_tokens
    }
}

impl TurnSession for OpenAIResponsesTurnSession {
    fn one_turn_streaming(
        &mut self,
        state: &mut AppState,
        auth_store: &mut AuthStore,
        items: &mut Vec<ConversationItem>,
        on_event: &mut dyn FnMut(TurnStreamEvent),
    ) -> Result<AssistantTurn> {
        let items_len_at_request = items.len();
        let wire_input = self.build_wire_input(items);

        // Pre-render the body so the per-attempt closure stays cheap
        // and avoids re-borrowing `state` mutably from inside a nested
        // closure (which the borrow checker rejects).
        let body_value = self.build_request_body(wire_input, state, true);
        let body_str = body_value.to_string();
        let body_for_each_attempt = move |request_config: &OpenAIRequestConfig| {
            let body: Value = serde_json::from_str(&body_str)
                .map_err(|e| anyhow::anyhow!("body re-parse failed: {e}"))?;
            build_json_post_request(
                request_config,
                openai_responses_path(&request_config.base_url),
                &body,
            )
        };
        let mut sized = |event: TurnStreamEvent| on_event(event);
        let response = send_openai_request_with_refresh_streaming(
            auth_store,
            &mut self.execution,
            body_for_each_attempt,
            &mut sized,
        )?;

        // Update threading state.
        if self.supports_response_threading {
            self.previous_response_id = response.response_id;
        } else {
            self.previous_response_id = None;
        }

        // Token-usage bookkeeping + Usage stream event.
        let input_tokens = response.input_tokens;
        Self::record_usage(
            state,
            input_tokens,
            response.cached_tokens.unwrap_or(0) as u64,
        );
        if let Some(input) = response.input_tokens {
            on_event(TurnStreamEvent::Usage(TurnUsageReport {
                input_tokens: input as u64,
                output_tokens: response.output_tokens.unwrap_or(0) as u64,
                cache_read_tokens: response.cached_tokens.unwrap_or(0) as u64,
                cache_creation_tokens: 0,
            }));
        }

        // Build the neutral AssistantTurn.
        let assistant_text_for_items = response.assistant_text.clone();
        let mut pre_tool_items: Vec<ConversationItem> = Vec::new();
        if !assistant_text_for_items.trim().is_empty() {
            pre_tool_items.push(ConversationItem::assistant_message(
                &assistant_text_for_items,
            ));
        }
        let mut tmp = Vec::new();
        append_reasoning_items(&mut tmp, &response.reasoning_items);
        pre_tool_items.extend(tmp);

        // Compute continuation_start *now* — points to the position of the
        // first FunctionCall item (i.e. after asst_msg + reasoning, before
        // FunctionCall items the loop is about to append).
        let established_count = pre_tool_items.len();
        if self.supports_response_threading && self.previous_response_id.is_some() {
            self.continuation_start = Some(items_len_at_request + established_count);
        } else {
            self.continuation_start = None;
        }

        // Append FunctionCall items to the pre-tool batch.
        for tc in &response.tool_calls {
            pre_tool_items.push(ConversationItem::FunctionCall {
                call_id: tc.call_id.clone(),
                name: tc.name.clone(),
                arguments: serde_json::to_string(&tc.arguments).unwrap_or_default(),
            });
        }

        let tool_calls: Vec<ToolCallRequest> = response
            .tool_calls
            .iter()
            .map(|tc| ToolCallRequest {
                call_id: tc.call_id.clone(),
                tool_id: tc.name.clone(),
                input: serde_json::to_string(&tc.arguments).unwrap_or_default(),
            })
            .collect();

        let final_assistant_text = if tool_calls.is_empty() {
            if response.assistant_text.trim().is_empty() {
                super::parse_openai_text(&response.raw_response)
                    .or_else(|_| super::parse_openai_text_fallback(&response.raw_response, state))
                    .unwrap_or_default()
            } else {
                response.assistant_text.clone()
            }
        } else {
            String::new()
        };

        Ok(AssistantTurn {
            pre_tool_items,
            tool_calls,
            assistant_text: final_assistant_text,
            input_tokens_hint: input_tokens,
            emitted_tool_call_ids: response.emitted_tool_call_ids,
            // Streaming path emits usage via `TurnStreamEvent::Usage`
            // from `openai.rs:290`, so the blocking-loop fallback isn't
            // used here.
            usage_report: None,
        })
    }

    fn one_turn_blocking(
        &mut self,
        state: &mut AppState,
        auth_store: &mut AuthStore,
        items: &mut Vec<ConversationItem>,
    ) -> Result<AssistantTurn> {
        let items_len_at_request = items.len();
        let wire_input = self.build_wire_input(items);

        let body_value = self.build_request_body(wire_input, state, false);
        let body_str = body_value.to_string();
        let body_for_each_attempt = move |request_config: &OpenAIRequestConfig| {
            let body: Value = serde_json::from_str(&body_str)
                .map_err(|e| anyhow::anyhow!("body re-parse failed: {e}"))?;
            build_json_post_request(
                request_config,
                openai_responses_path(&request_config.base_url),
                &body,
            )
        };
        let response_value = send_openai_request_with_refresh(
            auth_store,
            &mut self.execution,
            body_for_each_attempt,
        )?;

        // Typed parsing (re-uses paths from the legacy non-streaming code).
        let input_tokens = response_value
            .pointer("/usage/input_tokens")
            .and_then(Value::as_u64)
            .map(|v| v as usize);
        let cached_tokens = response_value
            .pointer("/usage/input_tokens_details/cached_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        Self::record_usage(state, input_tokens, cached_tokens);

        let parsed: OpenAIResponsesResponse = serde_json::from_value(response_value.clone())
            .map_err(|e| anyhow::anyhow!("failed to parse OpenAI Responses payload: {e}"))?;

        if self.supports_response_threading {
            self.previous_response_id = parsed.id.clone();
        } else {
            self.previous_response_id = None;
        }

        let tool_calls_vendor = extract_responses_tool_calls(&parsed)?;
        let tool_calls: Vec<ToolCallRequest> = tool_calls_vendor
            .iter()
            .map(|tc| ToolCallRequest {
                call_id: tc.call_id.clone(),
                tool_id: tc.name.clone(),
                input: serde_json::to_string(&tc.arguments).unwrap_or_default(),
            })
            .collect();

        // Build pre_tool_items: assistant text + reasoning + FunctionCall.
        let mut pre_tool_items: Vec<ConversationItem> = Vec::new();
        let assistant_text_inline = super::extract_responses_text(&parsed);
        if !assistant_text_inline.trim().is_empty() {
            pre_tool_items.push(ConversationItem::assistant_message(&assistant_text_inline));
        }
        // Reasoning items live under /output as type=reasoning entries.
        let reasoning_raw: Vec<Value> = response_value
            .pointer("/output")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter(|item| item.get("type").and_then(Value::as_str) == Some("reasoning"))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let mut tmp = Vec::new();
        append_reasoning_items(&mut tmp, &reasoning_raw);
        pre_tool_items.extend(tmp);

        let established_count = pre_tool_items.len();
        if self.supports_response_threading && self.previous_response_id.is_some() {
            self.continuation_start = Some(items_len_at_request + established_count);
        } else {
            self.continuation_start = None;
        }

        for tc in &tool_calls_vendor {
            pre_tool_items.push(ConversationItem::FunctionCall {
                call_id: tc.call_id.clone(),
                name: tc.name.clone(),
                arguments: serde_json::to_string(&tc.arguments).unwrap_or_default(),
            });
        }

        let final_assistant_text = if tool_calls.is_empty() {
            parse_openai_assistant_text(&parsed, &response_value, state)?
        } else {
            String::new()
        };

        Ok(AssistantTurn {
            pre_tool_items,
            tool_calls,
            assistant_text: final_assistant_text,
            input_tokens_hint: input_tokens,
            emitted_tool_call_ids: HashSet::new(),
            usage_report: None,
        })
    }

    fn generate_summary(&self, old_context: &str, model_id: &str) -> Option<String> {
        // Phase 2 of compaction: ask the same OpenAI endpoint we use
        // for the turn loop to summarize the prefix into a compact
        // context block. Falls through to Phase 3 (drop oldest items)
        // when the summary call fails / errors / times out.
        generate_openai_summary(old_context, model_id, &self.execution.request_config)
    }

    fn tool_execution_backend(&self) -> ToolExecutionBackend<'_> {
        ToolExecutionBackend::OpenAi {
            request_config: &self.execution.request_config,
            structured_output: self.structured_output.as_ref(),
        }
    }

    fn pre_loop_inject(&mut self, items: &mut Vec<ConversationItem>) {
        // Pin per-turn dynamic context (currentDate + gitStatus) at
        // the front so every Responses request includes it. Static
        // instructions stay in `instructions` — only this dynamic part
        // belongs in `input`.
        insert_managed_system_prompt_1(items, self.managed_system_prompt_1.as_deref());
        let context_reminder = super::build_context_reminder_message();
        insert_context_reminder_preserving_legacy_leading_system(items, &context_reminder);
    }

    fn notify_compacted(&mut self) {
        // Compaction invalidates server-side cached state, so the next
        // request must replay the full surviving transcript without
        // referencing the prior response_id.
        self.previous_response_id = None;
        self.continuation_start = None;
    }
}

/// Builds an `OpenAIResponsesTurnSession` from agent-loop inputs.
/// Captures execution config + serialized tools + system instructions
/// once per user prompt; threading state starts empty.
pub(super) fn setup_responses_session(
    state: &mut AppState,
    resources: &LoadedResources,
    provider: &ProviderDescriptor,
    model_id: String,
    auth_store: &mut AuthStore,
    options: &TurnRequestOptions<'_>,
    use_native: bool,
) -> Result<OpenAIResponsesTurnSession> {
    let execution = resolve_openai_execution_config(state, auth_store, provider)?;
    let registry =
        super::super::mcp_discovery::registry_with_mcp_tools(resources, state.tool_runner.as_ref());
    let permission_context = load_runtime_permission_context(&state.cwd, resources, state)?;
    let text = openai_responses_text_config(options.structured_output, use_native);
    let tools = openai_tool_definitions_for_request(
        &registry,
        options.structured_output,
        use_native,
        Some(&permission_context),
        options.tool_filter,
    )?;
    // Native server-side tools (e.g. `web_search`) serialize without a name,
    // so filter empty entries out of the system-prompt tool set.
    let enabled_tool_names = tools
        .iter()
        .map(|tool| tool.name.clone())
        .filter(|name| !name.is_empty())
        .collect::<std::collections::BTreeSet<_>>();
    let system_prompt =
        render_runtime_system_prompt(state, resources, &model_id, &enabled_tool_names)?;
    let instructions = super::openai_request_instructions(state, resources, Some(&system_prompt))?;
    let model = provider.models.iter().find(|m| m.id == model_id);
    let supports_reasoning = openai_model_supports_reasoning(provider, &model_id);
    let supports_response_threading =
        openai_supports_response_threading(provider, &execution.request_config.base_url, model);

    Ok(OpenAIResponsesTurnSession {
        execution,
        instructions,
        managed_system_prompt_1: managed_system_prompt_1_from_env(),
        tools,
        text,
        structured_output: options.structured_output.cloned(),
        model_id,
        supports_reasoning,
        supports_response_threading,
        previous_response_id: None,
        continuation_start: None,
    })
}

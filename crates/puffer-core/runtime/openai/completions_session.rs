//! [`TurnSession`] impl for the OpenAI Chat Completions API.
//!
//! No live SSE parser yet — the response comes back as one JSON
//! payload via `send_openai_request_with_refresh`. Streaming and
//! non-streaming `one_turn_*` variants both go through the same
//! request path; the streaming path additionally fires
//! `ThinkingDelta` and `TextDelta` events synthesized from the
//! parsed response so reasoning-capable Chat Completions providers
//! (Moonshot Kimi, Deepseek, OpenRouter relays, …) keep their
//! thinking blocks visible in the TUI. Real per-token streaming is
//! a follow-up (would need `stream: true` on the request body and a
//! Chat Completions SSE parser).

use anyhow::Result;
use puffer_provider_openai::{
    build_chat_completions_request, extract_chat_completions_reasoning,
    extract_chat_completions_tool_calls, extract_chat_completions_visible_text,
    parse_chat_completions_response, OpenAIChatCompletionsRequest, OpenAIChatCompletionTool,
    OpenAIChatResponseFormat, OpenAIRequestConfig, OpenAIResponsesToolChoiceMode,
};
use puffer_provider_registry::{AuthStore, ModelCompat, ProviderDescriptor, ThinkingFormat};
use puffer_resources::LoadedResources;
use puffer_tools::ToolRegistry;
use serde_json::{json, Value};
use std::collections::HashSet;

use super::conversation::{
    build_system_reminder, generate_openai_summary, items_to_chat_messages, ConversationItem,
};
use super::{
    parse_openai_text, parse_openai_text_fallback, send_openai_request_with_refresh,
    OpenAIExecutionConfig,
};
use crate::permissions::load_runtime_permission_context;
use crate::runtime::agent_loop::{AssistantTurn, TurnSession};
use crate::runtime::structured_output_support::{
    openai_chat_completion_tools_for_request, openai_chat_response_format, StructuredOutputConfig,
};
use crate::runtime::system_prompt::render_runtime_system_prompt;
use crate::runtime::tool_executor::ToolExecutionBackend;
use crate::runtime::{ToolCallRequest, TurnRequestOptions, TurnStreamEvent};
use crate::AppState;

pub(super) struct OpenAICompletionsTurnSession {
    pub execution: OpenAIExecutionConfig,
    pub tools: Vec<OpenAIChatCompletionTool>,
    pub response_format: Option<OpenAIChatResponseFormat>,
    pub system_prompt: String,
    pub plan_mode_context: Option<String>,
    pub system_reminder: String,
    pub structured_output: Option<StructuredOutputConfig>,
    pub model_id: String,
    /// Resolved compat from `Model.compat` (when `kind = openai-completions`)
    /// — controls reasoning-effort wire format, requires-reasoning-content
    /// flag, and per-effort name remapping. `None` means "use canonical
    /// OpenAI shape with auto-detected defaults".
    pub compat: Option<puffer_provider_registry::OpenAiCompletionsCompat>,
    /// Whether the *model itself* supports reasoning. Gates emission of
    /// any reasoning_effort field even when the relay declares support.
    pub model_supports_reasoning: bool,
}

impl TurnSession for OpenAICompletionsTurnSession {
    fn one_turn_streaming(
        &mut self,
        state: &mut AppState,
        auth_store: &mut AuthStore,
        items: &mut Vec<ConversationItem>,
        on_event: &mut dyn FnMut(TurnStreamEvent),
    ) -> Result<AssistantTurn> {
        // Use the rich `send_and_parse` (not `one_turn_blocking`) so
        // we keep `reasoning_chain` after parsing. Synthesize streaming
        // events from the (already-final) response so the TUI's
        // thinking + assistant cards stay populated. Real per-token
        // streaming is a follow-up — needs `stream: true` on the wire
        // body and a Chat Completions SSE parser. For reasoning-capable
        // providers this is the difference between "thinking block
        // visible" and "thinking block missing" (issue raised against
        // `kimi-coding/k2p5` with `effort: xhigh`).
        let result = self.send_and_parse(state, auth_store, items)?;
        if let Some(reasoning) = result.reasoning_chain.as_deref() {
            if !reasoning.is_empty() {
                on_event(TurnStreamEvent::ThinkingDelta(reasoning.to_string()));
            }
        }
        if !result.assistant_text.is_empty() {
            on_event(TurnStreamEvent::TextDelta(result.assistant_text.clone()));
        }
        Ok(result.into_assistant_turn())
    }

    fn one_turn_blocking(
        &mut self,
        state: &mut AppState,
        auth_store: &mut AuthStore,
        items: &mut Vec<ConversationItem>,
    ) -> Result<AssistantTurn> {
        Ok(self.send_and_parse(state, auth_store, items)?.into_assistant_turn())
    }

    fn generate_summary(&self, old_context: &str, model_id: &str) -> Option<String> {
        // Same Phase 2 helper Responses uses — issues a single
        // non-streaming summarization request via the OpenAI
        // /responses endpoint. Falls through to Phase 3 (drop oldest)
        // on any failure.
        generate_openai_summary(old_context, model_id, &self.execution.request_config)
    }

    fn tool_execution_backend(&self) -> ToolExecutionBackend<'_> {
        ToolExecutionBackend::OpenAi {
            request_config: &self.execution.request_config,
            structured_output: self.structured_output.as_ref(),
        }
    }
}

/// Internal "rich" result from a Chat Completions round-trip — carries
/// everything `AssistantTurn` carries PLUS the optional reasoning chain
/// so `one_turn_streaming` can synthesize a `ThinkingDelta` event for
/// reasoning-capable providers (Moonshot Kimi, Deepseek, OpenRouter).
struct CompletionsTurnResult {
    pre_tool_items: Vec<ConversationItem>,
    tool_calls: Vec<ToolCallRequest>,
    assistant_text: String,
    reasoning_chain: Option<String>,
}

impl CompletionsTurnResult {
    fn into_assistant_turn(self) -> AssistantTurn {
        AssistantTurn {
            pre_tool_items: self.pre_tool_items,
            tool_calls: self.tool_calls,
            assistant_text: self.assistant_text,
            input_tokens_hint: None,
            emitted_tool_call_ids: HashSet::new(),
        }
    }
}

impl OpenAICompletionsTurnSession {
    /// Builds the wire body, sends the (non-streaming) request, parses
    /// the response, and pulls out the bits both `one_turn_streaming`
    /// and `one_turn_blocking` need. Stays a private method on the
    /// session so it has access to `&mut self` for execution config
    /// state mutation under OAuth refresh.
    fn send_and_parse(
        &mut self,
        state: &mut AppState,
        auth_store: &mut AuthStore,
        items: &mut Vec<ConversationItem>,
    ) -> Result<CompletionsTurnResult> {
        let messages = items_to_chat_messages(
            items,
            Some(&self.system_prompt),
            self.plan_mode_context.as_deref(),
            Some(&self.system_reminder),
        );

        let model_id = self.model_id.clone();
        let tools = self.tools.clone();
        let response_format = self.response_format.clone();

        // Resolve effort + thinking params per the model's compat. When
        // `requires_reasoning_content_on_assistant_messages` is set, also
        // patch every prior assistant message to carry an empty
        // `reasoning_content` so DeepSeek-style relays don't reject the
        // replay.
        let reasoning_fields =
            resolve_reasoning_fields(self.compat.as_ref(), self.model_supports_reasoning, &state.effort_level);
        let mut messages = messages;
        if reasoning_fields.requires_reasoning_content_on_assistant_messages
            && self.model_supports_reasoning
        {
            for msg in &mut messages {
                if msg.role == "assistant" && msg.reasoning_content.is_none() {
                    msg.reasoning_content = Some(String::new());
                }
            }
        }
        let messages_for_attempt = messages.clone();

        let body_for_each_attempt = move |request_config: &OpenAIRequestConfig| {
            build_chat_completions_request(
                request_config,
                &OpenAIChatCompletionsRequest {
                    model: model_id.clone(),
                    messages: messages_for_attempt.clone(),
                    tools: tools.clone(),
                    tool_choice: if tools.is_empty() {
                        None
                    } else {
                        Some(OpenAIResponsesToolChoiceMode::Auto)
                    },
                    response_format: response_format.clone(),
                    reasoning_effort: reasoning_fields.reasoning_effort.clone(),
                    reasoning: reasoning_fields.reasoning.clone(),
                    thinking: reasoning_fields.thinking.clone(),
                    enable_thinking: reasoning_fields.enable_thinking,
                    chat_template_kwargs: reasoning_fields.chat_template_kwargs.clone(),
                },
            )
        };

        let response: Value =
            send_openai_request_with_refresh(auth_store, &mut self.execution, body_for_each_attempt)?;

        let parsed = parse_chat_completions_response(&serde_json::to_string(&response)?)?;
        let tool_calls_vendor = extract_chat_completions_tool_calls(&parsed)?;
        let tool_calls: Vec<ToolCallRequest> = tool_calls_vendor
            .iter()
            .map(|tc| ToolCallRequest {
                call_id: tc.call_id.clone(),
                tool_id: tc.name.clone(),
                input: serde_json::to_string(&tc.arguments).unwrap_or_default(),
            })
            .collect();

        // Strip any inline `<think>…</think>` block from the visible
        // text so it doesn't double-render alongside the thinking card
        // emitted from `extract_chat_completions_reasoning`.
        let assistant_text_from_msg = extract_chat_completions_visible_text(&parsed);
        let reasoning_chain = extract_chat_completions_reasoning(&parsed);

        let mut pre_tool_items: Vec<ConversationItem> = Vec::new();
        if !assistant_text_from_msg.trim().is_empty() {
            pre_tool_items.push(ConversationItem::assistant_message(&assistant_text_from_msg));
        }
        for tc in &tool_calls_vendor {
            pre_tool_items.push(ConversationItem::FunctionCall {
                call_id: tc.call_id.clone(),
                name: tc.name.clone(),
                arguments: serde_json::to_string(&tc.arguments).unwrap_or_default(),
            });
        }

        let final_assistant_text = if tool_calls.is_empty() {
            if assistant_text_from_msg.trim().is_empty() {
                parse_openai_text(&response)
                    .or_else(|_| parse_openai_text_fallback(&response, state))
                    .unwrap_or_default()
            } else {
                assistant_text_from_msg
            }
        } else {
            String::new()
        };

        Ok(CompletionsTurnResult {
            pre_tool_items,
            tool_calls,
            assistant_text: final_assistant_text,
            reasoning_chain,
        })
    }
}

pub(super) fn setup_completions_session(
    state: &mut AppState,
    resources: &LoadedResources,
    provider: &ProviderDescriptor,
    model_id: String,
    auth_store: &mut AuthStore,
    options: &TurnRequestOptions<'_>,
    use_native: bool,
) -> Result<OpenAICompletionsTurnSession> {
    let execution = super::resolve_openai_execution_config(state, auth_store, provider)?;
    let registry = super::super::mcp_discovery::registry_with_mcp_tools(
        resources,
        state.tool_runner.as_ref(),
    );
    let permission_context = load_runtime_permission_context(&state.cwd, resources, state)?;
    let response_format = openai_chat_response_format(options.structured_output, use_native);
    let tools = openai_chat_completion_tools_for_request(
        &registry,
        options.structured_output,
        use_native,
        Some(&permission_context),
        options.tool_filter,
    )?;
    let system_prompt = render_runtime_system_prompt(
        state,
        resources,
        &model_id,
        &tools
            .iter()
            .map(|tool| tool.function.name.clone())
            .collect::<std::collections::BTreeSet<_>>(),
    )?;
    let plan_mode_context = crate::plan_mode::take_plan_mode_context_message(state, resources)?;
    let system_reminder = build_system_reminder(&crate::runtime::git_status_context());

    let model_descriptor = provider.models.iter().find(|m| m.id == model_id);
    let model_supports_reasoning = model_descriptor.map(|m| m.supports_reasoning).unwrap_or(false);
    let compat = model_descriptor
        .and_then(|m| m.compat.as_ref())
        .and_then(|c| c.as_openai_completions())
        .cloned();

    Ok(OpenAICompletionsTurnSession {
        execution,
        tools,
        response_format,
        system_prompt,
        plan_mode_context,
        system_reminder,
        structured_output: options.structured_output.cloned(),
        model_id,
        compat,
        model_supports_reasoning,
    })
}

/// Resolved reasoning fields for one Chat Completions request.
struct ReasoningFields {
    reasoning_effort: Option<String>,
    reasoning: Option<Value>,
    thinking: Option<Value>,
    enable_thinking: Option<bool>,
    chat_template_kwargs: Option<Value>,
    requires_reasoning_content_on_assistant_messages: bool,
}

/// Maps puffer's effort_level + the model's compat → the wire-format
/// fields used by the active thinking_format. Pi-mono parity:
/// `pi-mono/packages/ai/src/providers/openai-completions.ts:1071`.
///
/// When `model.supports_reasoning` is false OR
/// `compat.supports_reasoning_effort = Some(false)`, returns "no
/// reasoning fields" so non-reasoning models keep their cheap path.
fn resolve_reasoning_fields(
    compat: Option<&puffer_provider_registry::OpenAiCompletionsCompat>,
    model_supports_reasoning: bool,
    effort_level: &str,
) -> ReasoningFields {
    let mut fields = ReasoningFields {
        reasoning_effort: None,
        reasoning: None,
        thinking: None,
        enable_thinking: None,
        chat_template_kwargs: None,
        requires_reasoning_content_on_assistant_messages: compat
            .and_then(|c| c.requires_reasoning_content_on_assistant_messages)
            .unwrap_or(false),
    };

    if !model_supports_reasoning {
        return fields;
    }
    if compat
        .and_then(|c| c.supports_reasoning_effort)
        .map(|v| !v)
        .unwrap_or(false)
    {
        return fields;
    }

    // Skip reasoning fields when the user explicitly opted out via /effort low+.
    if matches!(effort_level, "off" | "none") {
        return fields;
    }

    // Resolve the puffer-effort → vendor-string name. The `auto`
    // synonyms collapse to "medium" so we always send a known value.
    let resolved_level = match effort_level {
        "auto" | "unset" | "default" | "" => "medium",
        "max" => "high",
        other => other,
    };
    let vendor_level = compat
        .and_then(|c| c.reasoning_effort_map.as_ref())
        .and_then(|map| map.get(resolved_level))
        .cloned()
        .unwrap_or_else(|| resolved_level.to_string());

    let format = compat
        .and_then(|c| c.thinking_format)
        .unwrap_or(ThinkingFormat::Openai);

    match format {
        ThinkingFormat::Openai => {
            fields.reasoning_effort = Some(vendor_level);
        }
        ThinkingFormat::Openrouter => {
            fields.reasoning = Some(json!({ "effort": vendor_level }));
        }
        ThinkingFormat::Deepseek => {
            fields.thinking = Some(json!({ "type": "enabled" }));
            fields.reasoning_effort = Some(vendor_level);
        }
        ThinkingFormat::Zai | ThinkingFormat::Qwen => {
            fields.enable_thinking = Some(true);
        }
        ThinkingFormat::QwenChatTemplate => {
            fields.chat_template_kwargs = Some(json!({ "enable_thinking": true }));
        }
    }

    fields
}

#[cfg(test)]
mod reasoning_fields_tests {
    use super::*;
    use puffer_provider_registry::OpenAiCompletionsCompat;
    use indexmap::IndexMap;

    #[test]
    fn non_reasoning_model_emits_no_fields() {
        let f = resolve_reasoning_fields(None, false, "high");
        assert!(f.reasoning_effort.is_none());
        assert!(f.thinking.is_none());
        assert!(f.reasoning.is_none());
        assert!(f.enable_thinking.is_none());
    }

    #[test]
    fn default_format_uses_top_level_reasoning_effort() {
        let f = resolve_reasoning_fields(None, true, "high");
        assert_eq!(f.reasoning_effort.as_deref(), Some("high"));
        assert!(f.reasoning.is_none());
    }

    #[test]
    fn openrouter_uses_nested_reasoning_object() {
        let compat = OpenAiCompletionsCompat {
            thinking_format: Some(ThinkingFormat::Openrouter),
            ..Default::default()
        };
        let f = resolve_reasoning_fields(Some(&compat), true, "high");
        assert!(f.reasoning_effort.is_none());
        assert_eq!(f.reasoning, Some(json!({ "effort": "high" })));
    }

    #[test]
    fn deepseek_emits_both_thinking_and_reasoning_effort() {
        let mut map = IndexMap::new();
        map.insert("xhigh".to_string(), "max".to_string());
        let compat = OpenAiCompletionsCompat {
            thinking_format: Some(ThinkingFormat::Deepseek),
            reasoning_effort_map: Some(map),
            ..Default::default()
        };
        let f = resolve_reasoning_fields(Some(&compat), true, "xhigh");
        assert_eq!(f.thinking, Some(json!({ "type": "enabled" })));
        assert_eq!(f.reasoning_effort.as_deref(), Some("max"));
    }

    #[test]
    fn zai_uses_top_level_enable_thinking() {
        let compat = OpenAiCompletionsCompat {
            thinking_format: Some(ThinkingFormat::Zai),
            ..Default::default()
        };
        let f = resolve_reasoning_fields(Some(&compat), true, "high");
        assert_eq!(f.enable_thinking, Some(true));
        assert!(f.reasoning_effort.is_none());
    }

    #[test]
    fn qwen_chat_template_uses_chat_template_kwargs() {
        let compat = OpenAiCompletionsCompat {
            thinking_format: Some(ThinkingFormat::QwenChatTemplate),
            ..Default::default()
        };
        let f = resolve_reasoning_fields(Some(&compat), true, "high");
        assert_eq!(
            f.chat_template_kwargs,
            Some(json!({ "enable_thinking": true }))
        );
    }

    #[test]
    fn explicit_supports_reasoning_effort_false_disables_field() {
        let compat = OpenAiCompletionsCompat {
            supports_reasoning_effort: Some(false),
            ..Default::default()
        };
        let f = resolve_reasoning_fields(Some(&compat), true, "high");
        assert!(f.reasoning_effort.is_none());
        assert!(f.thinking.is_none());
    }

    #[test]
    fn requires_reasoning_content_flag_propagates() {
        let compat = OpenAiCompletionsCompat {
            requires_reasoning_content_on_assistant_messages: Some(true),
            ..Default::default()
        };
        let f = resolve_reasoning_fields(Some(&compat), true, "high");
        assert!(f.requires_reasoning_content_on_assistant_messages);
    }

    #[test]
    fn auto_effort_collapses_to_medium() {
        let f = resolve_reasoning_fields(None, true, "auto");
        assert_eq!(f.reasoning_effort.as_deref(), Some("medium"));
    }

    #[test]
    fn off_effort_emits_no_reasoning_fields() {
        let f = resolve_reasoning_fields(None, true, "off");
        assert!(f.reasoning_effort.is_none());
    }
}

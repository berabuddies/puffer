//! Unified conversation item type — Puffer's equivalent of Codex's `ResponseItem`.
//!
//! All internal logic operates on `Vec<ConversationItem>` using Responses API
//! semantics as the canonical format. Provider-specific wire formats are produced
//! by boundary converters:
//! - `items_to_responses_input()` — OpenAI Responses API
//! - `items_to_chat_messages()` — OpenAI Chat Completions
//! - `items_to_anthropic_messages()` — Anthropic Messages API (Phase 3)

use crate::runtime::ToolInvocation;
use crate::AppState;
use puffer_provider_openai::{
    OpenAIChatFunctionCall, OpenAIChatMessage, OpenAIChatToolCall, OpenAIRequestConfig,
};
use puffer_provider_registry::ProviderDescriptor;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// ContentPart — message content block (aligned with Codex ContentItem)
// ---------------------------------------------------------------------------

/// A structural part of a message's content.
///
/// Aligned with Codex `ContentItem { InputText | InputImage | OutputText }`,
/// simplified to `Text | Image` since Puffer doesn't distinguish input/output text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    /// A text content block.
    Text { text: String },
    /// An image reference (URL or data URI).
    Image { url: String },
}

// ---------------------------------------------------------------------------
// ToolOutputPayload — tool output (aligned with Codex FunctionCallOutputPayload)
// ---------------------------------------------------------------------------

/// Payload for a tool execution result.
///
/// Aligned with Codex `FunctionCallOutputPayload { body, success }`:
/// - `text` serializes as the wire value (plain string).
/// - `is_error` is internal metadata, NOT sent on the wire.
///   Maps to Anthropic `tool_result.is_error` / inverse of Codex `success`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutputPayload {
    /// The tool output text. Serialized directly as a string on the wire.
    pub text: String,
    /// Whether the tool execution failed. Internal metadata — not serialized.
    pub is_error: bool,
}

impl ToolOutputPayload {
    pub fn success(text: String) -> Self {
        Self {
            text,
            is_error: false,
        }
    }

    pub fn error(text: String) -> Self {
        Self {
            text,
            is_error: true,
        }
    }
}

/// Serialize: only send the text string on the wire (matching Codex pattern).
impl Serialize for ToolOutputPayload {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.text)
    }
}

/// Deserialize: recover from a plain string, `is_error` defaults to false.
impl<'de> Deserialize<'de> for ToolOutputPayload {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Ok(Self {
            text,
            is_error: false,
        })
    }
}

// ---------------------------------------------------------------------------
// ConversationItem — the canonical conversation atom (aligned with Codex ResponseItem)
// ---------------------------------------------------------------------------

/// A single item in the conversation history — Puffer's sole source of truth.
///
/// Aligned with Codex `ResponseItem` (`#[serde(tag = "type")]`). Provider-specific
/// wire formats are produced by boundary converters, not by the type itself.
///
/// Each variant serializes to `{"type": "message"|"function_call"|...}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConversationItem {
    /// A user, assistant, or system message.
    Message {
        role: String,
        content: Vec<ContentPart>,
    },
    /// A tool call requested by the model.
    FunctionCall {
        call_id: String,
        name: String,
        arguments: String,
    },
    /// Output from a tool execution.
    FunctionCallOutput {
        call_id: String,
        output: ToolOutputPayload,
    },
    /// Assistant reasoning chain returned by the Responses API when `include`
    /// contains `reasoning.encrypted_content`, or by the Anthropic Messages
    /// API as a `thinking` / `redacted_thinking` content block. Aligned
    /// with Codex `ResponseItem::Reasoning`. Carrying this across turns
    /// lets the model resume its prior thought process instead of
    /// re-thinking from scratch, which is essential for multi-turn tool
    /// loops with high reasoning effort.
    Reasoning {
        /// Summary blocks echoed back verbatim to the server.
        #[serde(default)]
        summary: Vec<ReasoningSummary>,
        /// Opaque encrypted thinking chain — provider-specific blob.
        /// On the Anthropic path this stores the `signature` for
        /// regular `thinking` blocks, OR the `data` payload for
        /// `redacted_thinking` blocks (depending on `redacted`). On the
        /// OpenAI Responses path this is the `encrypted_content` field
        /// surfaced via `include: ["reasoning.encrypted_content"]`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        encrypted_content: Option<String>,
        /// Pi-mono parity: when true, the upstream returned this as a
        /// `redacted_thinking` content block (encrypted reasoning that
        /// was blocked from display by safety filters). The next-turn
        /// replay must echo `{type:"redacted_thinking", data}` instead
        /// of `{type:"thinking", thinking, signature}` — emitting a
        /// regular thinking block with the redacted opaque payload as
        /// `signature` would fail upstream signature verification. See
        /// `pi-mono/packages/ai/src/providers/anthropic.ts:511,1015`.
        #[serde(default, skip_serializing_if = "is_false")]
        redacted: bool,
    },
    /// Compaction marker — replaces summarized older messages.
    /// Aligned with Codex `Compaction { encrypted_content }`.
    Compaction { summary: String },
}

/// A reasoning summary block. Aligned with Codex `ReasoningItemReasoningSummary`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReasoningSummary {
    SummaryText { text: String },
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(value: &bool) -> bool {
    !*value
}

impl ConversationItem {
    pub fn user_message(content: impl Into<String>) -> Self {
        Self::Message {
            role: "user".to_string(),
            content: vec![ContentPart::Text {
                text: content.into(),
            }],
        }
    }

    pub fn assistant_message(content: impl Into<String>) -> Self {
        Self::Message {
            role: "assistant".to_string(),
            content: vec![ContentPart::Text {
                text: content.into(),
            }],
        }
    }

    pub fn system_message(content: impl Into<String>) -> Self {
        Self::Message {
            role: "system".to_string(),
            content: vec![ContentPart::Text {
                text: content.into(),
            }],
        }
    }

    /// Extract the plain-text content of this item (joining multiple Text blocks).
    pub fn text_content(&self) -> Option<String> {
        match self {
            Self::Message { content, .. } => {
                let texts: Vec<&str> = content
                    .iter()
                    .filter_map(|p| match p {
                        ContentPart::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect();
                if texts.is_empty() {
                    None
                } else {
                    Some(texts.join("\n"))
                }
            }
            Self::FunctionCallOutput { output, .. } => Some(output.text.clone()),
            _ => None,
        }
    }

    /// Approximate token count with CJK-aware heuristic.
    ///
    /// ASCII characters average ~0.25 tokens each (4 chars/token).
    /// CJK / non-ASCII characters average ~1.5 tokens each.
    pub fn estimated_tokens(&self) -> usize {
        match self {
            Self::Message { content, .. } => content
                .iter()
                .map(|p| match p {
                    ContentPart::Text { text } => estimate_text_tokens(text),
                    ContentPart::Image { .. } => 300,
                })
                .sum(),
            Self::FunctionCall {
                arguments, name, ..
            } => estimate_text_tokens(name) + estimate_text_tokens(arguments),
            Self::FunctionCallOutput { output, .. } => estimate_text_tokens(&output.text),
            Self::Reasoning {
                summary,
                encrypted_content,
                redacted: _,
            } => {
                let summary_tokens: usize = summary
                    .iter()
                    .map(|s| match s {
                        ReasoningSummary::SummaryText { text } => estimate_text_tokens(text),
                    })
                    .sum();
                let encrypted_tokens = encrypted_content
                    .as_ref()
                    .map(|s| estimate_text_tokens(s))
                    .unwrap_or(0);
                summary_tokens + encrypted_tokens
            }
            Self::Compaction { summary } => estimate_text_tokens(summary),
        }
    }
}

/// CJK-aware token estimation: ASCII chars count as 1 unit, non-ASCII as 6 units,
/// then divide by 4. This yields ~0.25 tokens/ASCII char and ~1.5 tokens/CJK char.
fn estimate_text_tokens(text: &str) -> usize {
    let mut units = 0usize;
    for ch in text.chars() {
        if ch.is_ascii() {
            units += 1;
        } else {
            units += 6;
        }
    }
    (units + 3) / 4
}

// ---------------------------------------------------------------------------
// normalize_items — shared pre-processing (aligned with Pi-Mono transformMessages)
// ---------------------------------------------------------------------------

/// Shared pre-processing before sending to any provider.
///
/// Aligned with Pi-Mono `transformMessages()`:
/// - Ensures every FunctionCall has a matching FunctionCallOutput
/// - Inserts synthetic error outputs for orphaned FunctionCalls
///
/// This guarantees tool_call/tool_result pairing regardless of provider.
pub fn normalize_items(items: &mut Vec<ConversationItem>) {
    let call_ids: std::collections::HashSet<String> = items
        .iter()
        .filter_map(|item| {
            if let ConversationItem::FunctionCall { call_id, .. } = item {
                Some(call_id.clone())
            } else {
                None
            }
        })
        .collect();

    let output_ids: std::collections::HashSet<String> = items
        .iter()
        .filter_map(|item| {
            if let ConversationItem::FunctionCallOutput { call_id, .. } = item {
                Some(call_id.clone())
            } else {
                None
            }
        })
        .collect();

    let orphans: Vec<String> = call_ids.difference(&output_ids).cloned().collect();

    for orphan_id in orphans {
        if let Some(pos) = items.iter().position(|item| {
            matches!(item, ConversationItem::FunctionCall { call_id, .. } if call_id == &orphan_id)
        }) {
            items.insert(
                pos + 1,
                ConversationItem::FunctionCallOutput {
                    call_id: orphan_id,
                    output: ToolOutputPayload::error(
                        "[Tool call was not completed — no result available]".into(),
                    ),
                },
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Transcript → ConversationItem
// ---------------------------------------------------------------------------

/// Converts the in-memory transcript + current user input into canonical
/// conversation items. This replaces both `transcript_to_openai_input` and
/// `transcript_to_openai_chat_messages`.
pub(crate) fn transcript_to_items(state: &AppState, input: &str) -> Vec<ConversationItem> {
    let mut items: Vec<ConversationItem> = state
        .transcript
        .iter()
        .flat_map(|message| match message.role {
            crate::MessageRole::User => vec![ConversationItem::user_message(&message.text)],
            crate::MessageRole::Assistant => {
                vec![ConversationItem::assistant_message(&message.text)]
            }
            crate::MessageRole::System => vec![ConversationItem::system_message(&message.text)],
            crate::MessageRole::ToolCall => {
                let call_id = message.call_id.clone().unwrap_or_default();
                let name = message.tool_id.clone().unwrap_or_default();
                let arguments = message.tool_input.clone().unwrap_or_else(|| "{}".into());
                vec![ConversationItem::FunctionCall {
                    call_id,
                    name,
                    arguments,
                }]
            }
            crate::MessageRole::ToolResult => {
                let call_id = message.call_id.clone().unwrap_or_default();
                let is_error = message.success.map(|s| !s).unwrap_or(false);
                vec![ConversationItem::FunctionCallOutput {
                    call_id,
                    output: ToolOutputPayload {
                        text: message.text.clone(),
                        is_error,
                    },
                }]
            }
        })
        .collect();

    // Append the current user input unless the transcript already ends with it
    // (callers like command.rs and flow.rs push the user message to the
    // transcript *before* calling execute, so it would be double-counted).
    if !input.trim().is_empty() {
        let already_present = matches!(
            items.last(),
            Some(ConversationItem::Message { role, content })
                if role == "user"
                    && content.len() == 1
                    && matches!(&content[0], ContentPart::Text { text } if text == input)
        );
        if !already_present {
            items.push(ConversationItem::user_message(input));
        }
    }
    items
}

/// Converts a raw reasoning output item (as returned by the Responses API
/// `response.output_item.done` event or the non-streaming response body)
/// into a `ConversationItem::Reasoning`. Returns `None` if the item is not
/// a reasoning item or has no useful payload.
///
/// Aligned with Codex: the server may include either `summary` items,
/// `encrypted_content`, or both. We preserve whichever is present so the
/// next turn's request can replay them verbatim.
pub(crate) fn reasoning_item_from_value(item: &Value) -> Option<ConversationItem> {
    if item.get("type").and_then(Value::as_str) != Some("reasoning") {
        return None;
    }
    let summary: Vec<ReasoningSummary> = item
        .get("summary")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|s| {
                    if s.get("type").and_then(Value::as_str) == Some("summary_text") {
                        s.get("text").and_then(Value::as_str).map(|text| {
                            ReasoningSummary::SummaryText {
                                text: text.to_string(),
                            }
                        })
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    let encrypted_content = item
        .get("encrypted_content")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    // Skip empty reasoning items (no summary, no encrypted content) to avoid
    // sending useless payloads.
    if summary.is_empty() && encrypted_content.is_none() {
        return None;
    }
    Some(ConversationItem::Reasoning {
        summary,
        encrypted_content,
        redacted: false,
    })
}

/// Appends reasoning items (captured from a response) to the conversation.
/// Call this BEFORE `append_tool_results` so reasoning items precede the
/// function calls they preceded on the wire — matching Codex's history order.
pub(crate) fn append_reasoning_items(items: &mut Vec<ConversationItem>, raw_items: &[Value]) {
    for raw in raw_items {
        if let Some(reasoning) = reasoning_item_from_value(raw) {
            items.push(reasoning);
        }
    }
}

/// Appends tool call items and their outputs to the conversation.
/// This is the shared path for both Responses and Chat Completions after
/// tool execution completes.
pub(crate) fn append_tool_results(
    items: &mut Vec<ConversationItem>,
    invocations: &[ToolInvocation],
) {
    for inv in invocations {
        items.push(ConversationItem::FunctionCall {
            call_id: inv.call_id.clone(),
            name: inv.tool_id.clone(),
            arguments: inv.input.clone(),
        });
    }
    for inv in invocations {
        items.push(ConversationItem::FunctionCallOutput {
            call_id: inv.call_id.clone(),
            output: if inv.success {
                ToolOutputPayload::success(inv.output.clone())
            } else {
                ToolOutputPayload::error(inv.output.clone())
            },
        });
    }
}

// ---------------------------------------------------------------------------
// ConversationItem → Responses API wire format
// ---------------------------------------------------------------------------

/// Serializes conversation items to the Responses API `input` array format.
pub(crate) fn items_to_responses_input(items: &[ConversationItem]) -> Value {
    if items.is_empty() {
        return Value::Array(Vec::new());
    }
    Value::Array(
        drop_transient_system_messages(items)
            .into_iter()
            .map(item_to_responses_value)
            .collect(),
    )
}

pub(crate) fn managed_system_prompt_1_from_env() -> Option<String> {
    std::env::var("PUFFER_SYSTEM_PROMPT_1")
        .ok()
        .map(|prompt| prompt.trim().to_string())
        .filter(|prompt| !prompt.is_empty())
}

pub(crate) fn insert_managed_system_prompt_1(
    items: &mut Vec<ConversationItem>,
    prompt: Option<&str>,
) {
    if let Some(prompt) = prompt.map(str::trim).filter(|prompt| !prompt.is_empty()) {
        items.insert(0, ConversationItem::system_message(prompt));
    }
}

/// Inserts the per-turn synthetic context reminder at the right position
/// in `items`: after any leading run of `role:"system"` items, so the
/// boundary filter's first-position exemption keeps preserving the
/// sub-agent identity prompt that `agents.rs:319` pushes at transcript[0].
///
/// **TODO(phase3): delete this helper and revert call sites to
/// `items.insert(0, …)` once sub-agent identity moves into top-level
/// `Prompt::base_instructions` (codex parity). At that point no
/// legitimate `role:"system"` exists in transcript, so no leading-system
/// run to navigate around.**
///
/// Naive `items.insert(0, …)` would push the identity to position 1,
/// past the exemption, and `drop_transient_system_messages()` would then
/// strip it. Inserting after the leading system run keeps both the
/// reminder and the identity in their right slots.
pub(crate) fn insert_context_reminder_preserving_legacy_leading_system(
    items: &mut Vec<ConversationItem>,
    reminder_text: &str,
) {
    let insert_pos = items
        .iter()
        .take_while(
            |item| matches!(item, ConversationItem::Message { role, .. } if role == "system"),
        )
        .count();
    items.insert(insert_pos, ConversationItem::user_message(reminder_text));
}

/// Strips transient `role:"system"` items that appear after the first
/// non-system item — these are local UI notifications (slash command
/// output, "Provider request failed: ...", "Interrupted by user.")
/// that leaked into the transcript via `emit_system_message` and have
/// no business reaching the model.
///
/// Items at the head of the list are preserved so legitimate system
/// content (e.g. a sub-agent identity prompt pushed by `agents.rs:321`)
/// still reaches the wire — until we move that to top-level
/// `instructions` in a follow-up.
///
/// Without this filter:
/// - ChatGPT Codex backend (`chatgpt.com/backend-api/codex/responses`)
///   rejects with `400` on any role:"system" inside `input`.
/// - Permissive proxies silently merge the leaked content into the
///   top-level `instructions`, corrupting the system prompt.
/// - On Chat Completions, the leaked entry shows up mid-conversation
///   which strict providers reject and lenient ones treat as a real
///   instruction switch.
fn drop_transient_system_messages(items: &[ConversationItem]) -> Vec<&ConversationItem> {
    let mut out = Vec::with_capacity(items.len());
    let mut seen_non_system = false;
    for item in items {
        let is_system = matches!(
            item,
            ConversationItem::Message { role, .. } if role == "system"
        );
        if is_system && seen_non_system {
            continue;
        }
        if !is_system {
            seen_non_system = true;
        }
        out.push(item);
    }
    out
}

fn item_to_responses_value(item: &ConversationItem) -> Value {
    match item {
        ConversationItem::Message { role, content } => {
            let text = content_parts_to_text(content);
            let content_block = if role == "assistant" {
                json!([{"type": "output_text", "text": text, "annotations": []}])
            } else if role == "system" {
                json!(text)
            } else {
                json!([{"type": "input_text", "text": text}])
            };

            let mut val = json!({
                "type": "message",
                "role": role,
                "content": content_block,
            });
            if role == "assistant" {
                val["status"] = json!("completed");
            }
            val
        }
        ConversationItem::FunctionCall {
            call_id,
            name,
            arguments,
        } => json!({
            "type": "function_call",
            "call_id": call_id,
            "name": name,
            "arguments": arguments,
        }),
        ConversationItem::FunctionCallOutput { call_id, output } => json!({
            "type": "function_call_output",
            "call_id": call_id,
            "output": output.text,
        }),
        ConversationItem::Reasoning {
            summary,
            encrypted_content,
            redacted: _,
        } => {
            let mut val = json!({
                "type": "reasoning",
                "summary": summary,
            });
            if let Some(encrypted) = encrypted_content {
                val["encrypted_content"] = Value::String(encrypted.clone());
            }
            val
        }
        ConversationItem::Compaction { summary } => {
            // Compaction markers are rendered as user messages on the wire.
            json!({
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": format!("[Conversation compacted — prior context summarized]\n\n{summary}")}],
            })
        }
    }
}

/// Extract plain text from ContentPart array (joining Text blocks with newline).
fn content_parts_to_text(parts: &[ContentPart]) -> String {
    let texts: Vec<&str> = parts
        .iter()
        .filter_map(|p| match p {
            ContentPart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    texts.join("\n")
}

// ---------------------------------------------------------------------------
// ConversationItem → Chat Completions wire format
// ---------------------------------------------------------------------------

/// Converts conversation items to Chat Completions messages.
///
/// This is the "translation layer" — the only Chat-Completions-specific code
/// needed. Key translations:
/// - System prompt prepended as system message
/// - Consecutive FunctionCall items grouped into one assistant message with
///   `tool_calls` array (Chat Completions requirement)
/// - FunctionCallOutput → tool role message
pub(crate) fn items_to_chat_messages(
    items: &[ConversationItem],
    system_prompt: Option<&str>,
    managed_system_prompt_1: Option<&str>,
    plan_mode_context: Option<&str>,
    system_reminder: Option<&str>,
) -> Vec<OpenAIChatMessage> {
    let mut messages = Vec::new();

    // System prompt as first message.
    if let Some(prompt) = system_prompt.filter(|p| !p.trim().is_empty()) {
        messages.push(chat_message("system", prompt));
    }
    if let Some(prompt) = managed_system_prompt_1.filter(|p| !p.trim().is_empty()) {
        messages.push(chat_message("system", prompt));
    }
    if let Some(ctx) = plan_mode_context.filter(|c| !c.trim().is_empty()) {
        messages.push(chat_message("system", ctx));
    }
    // System reminder (currentDate + gitStatus) — this was previously
    // MISSING from Chat Completions. Now both paths get it via the shared
    // ConversationItem pipeline.
    if let Some(reminder) = system_reminder.filter(|r| !r.trim().is_empty()) {
        messages.push(chat_message("system", reminder));
    }

    // Same boundary filter as items_to_responses_input — strip transient
    // role:"system" items emitted by `flow.rs::emit_system_message` so they
    // don't appear mid-conversation on the wire (which strict Chat
    // Completions providers reject).
    let filtered: Vec<&ConversationItem> = drop_transient_system_messages(items);
    let items_owned: Vec<ConversationItem> = filtered.into_iter().cloned().collect();
    let items: &[ConversationItem] = &items_owned;
    let mut i = 0;
    while i < items.len() {
        match &items[i] {
            ConversationItem::Message { role, content } => {
                let text = content_parts_to_text(content);
                messages.push(chat_message(role, &text));
                i += 1;
            }
            ConversationItem::FunctionCall { .. } => {
                // Group consecutive FunctionCall items into one assistant message
                // with tool_calls (Chat Completions requirement: tool_calls must
                // be in a single assistant message).
                let mut tool_calls = Vec::new();
                while i < items.len() {
                    if let ConversationItem::FunctionCall {
                        call_id,
                        name,
                        arguments,
                    } = &items[i]
                    {
                        tool_calls.push(OpenAIChatToolCall {
                            id: call_id.clone(),
                            kind: "function".to_string(),
                            function: OpenAIChatFunctionCall {
                                name: name.clone(),
                                arguments: arguments.clone(),
                            },
                        });
                        i += 1;
                    } else {
                        break;
                    }
                }
                messages.push(OpenAIChatMessage {
                    role: "assistant".to_string(),
                    content: None,
                    tool_call_id: None,
                    tool_calls,
                    reasoning_content: None,
                });
            }
            ConversationItem::FunctionCallOutput { call_id, output } => {
                messages.push(OpenAIChatMessage {
                    role: "tool".to_string(),
                    content: Some(json!(output.text)),
                    tool_call_id: Some(call_id.clone()),
                    tool_calls: Vec::new(),
                    reasoning_content: None,
                });
                i += 1;
            }
            ConversationItem::Reasoning { .. } => {
                // Chat Completions has no concept of reasoning items on the
                // wire — they exist only in the Responses API. Drop them here.
                i += 1;
            }
            ConversationItem::Compaction { summary } => {
                let text =
                    format!("[Conversation compacted — prior context summarized]\n\n{summary}");
                messages.push(chat_message("user", &text));
                i += 1;
            }
        }
    }

    // Ensure at least one user message exists.
    if messages.iter().all(|m| m.role == "system") {
        messages.push(chat_message("user", ""));
    }

    messages
}

fn chat_message(role: &str, content: &str) -> OpenAIChatMessage {
    OpenAIChatMessage {
        role: role.to_string(),
        content: Some(json!(content)),
        tool_call_id: None,
        tool_calls: Vec::new(),
        reasoning_content: None,
    }
}

// ---------------------------------------------------------------------------
// ConversationItem → Anthropic Messages API wire format
// ---------------------------------------------------------------------------

/// Converts conversation items to the Anthropic Messages API format.
///
/// Anthropic constraints enforced here:
/// - System messages are **skipped** (caller passes them via the top-level `system` param).
/// - `FunctionCall` items are grouped into assistant messages with `tool_use` content blocks.
/// - `FunctionCallOutput` items are grouped into user messages with `tool_result` content blocks.
/// - Strict role alternation: consecutive same-role messages are merged.
/// - The first message must have role `"user"`.
///
/// This is the Anthropic equivalent of `items_to_responses_input()` and
/// `items_to_chat_messages()` — a pure boundary converter from canonical
/// `ConversationItem` to Anthropic wire JSON.
pub(crate) fn items_to_anthropic_messages(items: &[ConversationItem]) -> Vec<Value> {
    let mut messages: Vec<Value> = Vec::new();

    let mut i = 0;
    while i < items.len() {
        match &items[i] {
            ConversationItem::Message { role, content } => {
                if role == "system" {
                    // Anthropic uses top-level `system` param — skip system messages.
                    i += 1;
                    continue;
                }
                let text = content_parts_to_text(content);
                push_or_merge(&mut messages, role, json!(text));
                i += 1;
            }
            ConversationItem::FunctionCall { .. } => {
                // Group consecutive FunctionCall items into one assistant message
                // with tool_use content blocks.
                let mut tool_uses = Vec::new();
                while i < items.len() {
                    if let ConversationItem::FunctionCall {
                        call_id,
                        name,
                        arguments,
                    } = &items[i]
                    {
                        let input_val: Value = serde_json::from_str(arguments).unwrap_or(json!({}));
                        tool_uses.push(json!({
                            "type": "tool_use",
                            "id": call_id,
                            "name": name,
                            "input": input_val,
                        }));
                        i += 1;
                    } else {
                        break;
                    }
                }
                push_or_merge(&mut messages, "assistant", Value::Array(tool_uses));
            }
            ConversationItem::FunctionCallOutput { .. } => {
                // Group consecutive FunctionCallOutput items into one user message
                // with tool_result content blocks.
                let mut tool_results = Vec::new();
                while i < items.len() {
                    if let ConversationItem::FunctionCallOutput { call_id, output } = &items[i] {
                        let mut result = json!({
                            "type": "tool_result",
                            "tool_use_id": call_id,
                            "content": output.text,
                        });
                        if output.is_error {
                            result["is_error"] = json!(true);
                        }
                        tool_results.push(result);
                        i += 1;
                    } else {
                        break;
                    }
                }
                push_or_merge(&mut messages, "user", Value::Array(tool_results));
            }
            ConversationItem::Reasoning {
                summary,
                encrypted_content,
                redacted,
            } => {
                // On the Anthropic path, Reasoning items carry the
                // upstream's thinking block: `summary` holds the prose
                // and `encrypted_content` holds the `signature` token
                // for normal thinking blocks, or the opaque `data`
                // payload for redacted thinking. With a signature,
                // emit a verifiable `thinking` block so providers like
                // `kimi-coding/k2p5` accept the next-turn replay
                // (otherwise they reject with "reasoning_content is
                // missing in assistant tool call message"). Without a
                // signature (aborted stream), fall back to a plain
                // `text` block so the model still sees its prior
                // reasoning. Pi-mono parity:
                // `packages/ai/src/providers/anthropic.ts:1015`.
                if *redacted {
                    // Redacted reasoning: re-emit the opaque data
                    // payload as `redacted_thinking`. The upstream
                    // expects this exact shape — emitting the data as
                    // a regular thinking signature would fail crypto
                    // verification.
                    let Some(data) = encrypted_content.as_deref() else {
                        i += 1;
                        continue;
                    };
                    let block = json!({
                        "type": "redacted_thinking",
                        "data": data,
                    });
                    push_or_merge(&mut messages, "assistant", Value::Array(vec![block]));
                    i += 1;
                    continue;
                }
                let thinking_text: String = summary
                    .iter()
                    .map(|s| match s {
                        ReasoningSummary::SummaryText { text } => text.as_str(),
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let signature_present = encrypted_content
                    .as_deref()
                    .map(|s| !s.trim().is_empty())
                    .unwrap_or(false);
                let block = if signature_present {
                    json!({
                        "type": "thinking",
                        "thinking": thinking_text,
                        "signature": encrypted_content.as_deref().unwrap(),
                    })
                } else if !thinking_text.trim().is_empty() {
                    // Aborted thinking — preserve content as plain text
                    // to avoid losing chain-of-thought. Anthropic accepts
                    // text blocks unconditionally.
                    json!({
                        "type": "text",
                        "text": thinking_text,
                    })
                } else {
                    i += 1;
                    continue;
                };
                push_or_merge(&mut messages, "assistant", Value::Array(vec![block]));
                i += 1;
            }
            ConversationItem::Compaction { summary } => {
                let text =
                    format!("[Conversation compacted — prior context summarized]\n\n{summary}");
                push_or_merge(&mut messages, "user", json!(text));
                i += 1;
            }
        }
    }

    // Anthropic requires the first message to be "user".
    if messages
        .first()
        .and_then(|m| m["role"].as_str())
        .is_some_and(|r| r != "user")
    {
        messages.insert(0, json!({"role": "user", "content": "Continue."}));
    }

    // Ensure non-empty.
    if messages.is_empty() {
        messages.push(json!({"role": "user", "content": ""}));
    }

    messages
}

/// Pushes a content value to the message list, merging with the last message
/// if it has the same role (to maintain strict Anthropic alternation).
///
/// `content_val` can be:
/// - A JSON string (plain text message)
/// - A JSON array (tool_use or tool_result blocks)
///
/// When merging, text is converted to a content array if needed, and arrays
/// are concatenated.
fn push_or_merge(messages: &mut Vec<Value>, role: &str, content_val: Value) {
    if let Some(last) = messages.last_mut() {
        if last["role"].as_str() == Some(role) {
            // Merge into existing message of the same role.
            let existing = &mut last["content"];
            // Normalize existing to array.
            let existing_arr = match existing.take() {
                Value::Array(arr) => arr,
                Value::String(s) => vec![json!({"type": "text", "text": s})],
                other => vec![other],
            };
            // Normalize new content to array.
            let new_arr = match content_val {
                Value::Array(arr) => arr,
                Value::String(s) => vec![json!({"type": "text", "text": s})],
                other => vec![other],
            };
            let mut merged = existing_arr;
            merged.extend(new_arr);
            *existing = Value::Array(merged);
            return;
        }
    }
    messages.push(json!({"role": role, "content": content_val}));
}

// ---------------------------------------------------------------------------
// Unified compaction (replaces both compact_openai_input and
// compact_openai_chat_messages)
// ---------------------------------------------------------------------------

/// Maximum consecutive auto-compact cycles before giving up.
const MAX_COMPACT_CYCLES: usize = 10;

/// Max output tokens for compact summary requests.
const COMPACT_SUMMARY_MAX_OUTPUT_TOKENS: usize = 16_384;

/// Compacts conversation items when context exceeds the threshold.
///
/// Three-phase strategy (shared by all provider paths):
/// 1. Snip old FunctionCallOutput items (keep last 6, truncate to 500 chars)
/// 2. Generate AI summary of old items via `summary_fn`
/// 3. Fallback: drop oldest items
///
/// Returns `true` when Phase 2 or 3 occurred (caller should reset
/// `previous_response_id` for Responses path and inject post-compact context).
///
/// `summary_fn` takes `(old_context_text, model_id)` and returns an optional
/// summary string. This abstraction allows both OpenAI and Anthropic callers
/// to supply their own summary generation logic.
pub(crate) fn compact_conversation_with(
    items: &mut Vec<ConversationItem>,
    provider: &ProviderDescriptor,
    model_id: &str,
    input_tokens_hint: Option<usize>,
    summary_fn: &dyn Fn(&str, &str) -> Option<String>,
) -> bool {
    if items.len() <= 3 {
        return false;
    }

    let model_info = provider.models.iter().find(|m| m.id == model_id);
    let context_window = model_info
        .map(|m| m.context_window as usize)
        .unwrap_or(200_000);
    let max_output = model_info
        .map(|m| m.max_output_tokens as usize)
        .filter(|&v| v > 0)
        .unwrap_or(16_384);
    let effective = context_window.saturating_sub(max_output);
    let threshold = effective * 90 / 100;

    let estimate = |arr: &[ConversationItem]| -> usize {
        arr.iter().map(ConversationItem::estimated_tokens).sum()
    };

    let current_tokens = input_tokens_hint.unwrap_or_else(|| estimate(items));
    if current_tokens <= threshold {
        return false;
    }

    // Phase 1: Snip old FunctionCallOutput items (keep last 6).
    let keep_recent = 6;
    if items.len() > keep_recent {
        let cutoff = items.len() - keep_recent;
        for item in &mut items[..cutoff] {
            if let ConversationItem::FunctionCallOutput { output, .. } = item {
                if output.text.len() > 500 {
                    let snipped: String = output.text.chars().take(500).collect();
                    output.text = format!("{snipped}\n[...output snipped...]");
                }
            }
        }
    }

    // Re-estimate after snipping.
    let after_snip = if input_tokens_hint.is_some() {
        estimate(items) // Re-estimate since hint was pre-snip.
    } else {
        estimate(items)
    };
    if after_snip <= threshold {
        return false;
    }

    // Phase 2: AI summary of old items, keep recent items intact.
    // Find a valid keep boundary: we want at least 4 items, but the kept tail
    // must start with a Message to avoid orphan FunctionCall/FunctionCallOutput
    // items that would produce invalid history.
    let keep_count = find_valid_keep_boundary(items, 4);
    let to_summarize = &items[..items.len() - keep_count];
    if !to_summarize.is_empty() {
        let old_context = build_summary_text(to_summarize);
        let summary = summary_fn(&old_context, model_id);

        if let Some(summary_text) = summary {
            let kept: Vec<ConversationItem> = items.split_off(items.len() - keep_count);
            items.clear();
            items.push(ConversationItem::user_message(format!(
                "[Conversation compacted — prior context summarized]\n\n{summary_text}"
            )));
            items.extend(kept);
            return true;
        }
    }

    // Phase 3 (fallback): Drop oldest items with circuit breaker.
    let mut cycles = 0;
    while items.len() > 3 && estimate(items) > threshold && cycles < MAX_COMPACT_CYCLES {
        items.remove(0);
        cycles += 1;
    }

    // Strip leading orphan FunctionCall/FunctionCallOutput items that lack a
    // preceding assistant message — these would produce invalid history.
    while !items.is_empty()
        && !matches!(
            items.first(),
            Some(ConversationItem::Message { .. } | ConversationItem::Compaction { .. })
        )
    {
        items.remove(0);
    }

    if items.is_empty()
        || !matches!(
            items.first(),
            Some(ConversationItem::Message { .. } | ConversationItem::Compaction { .. })
        )
    {
        items.insert(
            0,
            ConversationItem::user_message("[Earlier context compacted]"),
        );
    }
    true
}

/// Convenience wrapper: compacts using the OpenAI Responses API for summary generation.
pub(crate) fn compact_conversation(
    items: &mut Vec<ConversationItem>,
    provider: &ProviderDescriptor,
    model_id: &str,
    request_config: &OpenAIRequestConfig,
    input_tokens_hint: Option<usize>,
) -> bool {
    let summary_fn = |old_context: &str, mid: &str| {
        let _ = provider;
        generate_openai_summary(old_context, mid, request_config)
    };
    compact_conversation_with(items, provider, model_id, input_tokens_hint, &summary_fn)
}

/// Injects a context-restoration message after compaction.
/// Previously only applied to Responses path — now shared by both.
pub(crate) fn inject_post_compact_context(
    items: &mut Vec<ConversationItem>,
    cwd: &std::path::Path,
) {
    let reminder = format!(
        "[Context restored after compaction]\nCurrent working directory: {}\n\
         Continue the task from where you left off.",
        cwd.display()
    );
    let pos = 1.min(items.len());
    items.insert(pos, ConversationItem::user_message(reminder));
}

/// Finds the number of items to keep from the tail during compaction such that
/// the kept slice starts with a `Message` item. This prevents orphan
/// `FunctionCall`/`FunctionCallOutput` items that would produce invalid Chat
/// Completions history.
///
/// Starts from `min_keep` and extends backward until a `Message` boundary is
/// found, capped at the total item count.
fn find_valid_keep_boundary(items: &[ConversationItem], min_keep: usize) -> usize {
    let n = items.len();
    let mut keep = min_keep.min(n);
    // Walk backward from the candidate boundary until we find a Message.
    while keep < n {
        let start_index = n - keep;
        if matches!(&items[start_index], ConversationItem::Message { .. }) {
            break;
        }
        keep += 1;
    }
    keep
}

// ---------------------------------------------------------------------------
// System reminder (shared — previously Responses-only)
// ---------------------------------------------------------------------------

/// Builds the system reminder text (currentDate + gitStatus).
/// Previously only injected into Responses API `instructions` field.
/// Now available to both paths via ConversationItem pipeline.
pub(crate) fn build_system_reminder(git_status: &str) -> String {
    let now = time::OffsetDateTime::now_utc();
    let date_str = format!("{}-{:02}-{:02}", now.year(), now.month() as u8, now.day());
    let mut reminder = format!("# currentDate\nToday's date is {date_str}.");
    if !git_status.is_empty() {
        reminder.push_str(&format!("\n\n# gitStatus\n{git_status}"));
    }
    reminder
}

// ---------------------------------------------------------------------------
// Compaction helpers
// ---------------------------------------------------------------------------

/// Builds a text representation of items for AI summarization.
pub(crate) fn build_summary_text(items: &[ConversationItem]) -> String {
    let mut text = String::new();
    for item in items {
        match item {
            ConversationItem::Message { role, content } => {
                let full = content_parts_to_text(content);
                let preview: String = full.chars().take(500).collect();
                text.push_str(&format!("[{role}]: {preview}\n\n"));
            }
            ConversationItem::FunctionCall {
                name, arguments, ..
            } => {
                let preview: String = arguments.chars().take(200).collect();
                text.push_str(&format!("[tool_call {name}]: {preview}\n\n"));
            }
            ConversationItem::FunctionCallOutput { output, .. } => {
                let preview: String = output.text.chars().take(300).collect();
                text.push_str(&format!("[tool_result]: {preview}\n\n"));
            }
            ConversationItem::Reasoning { .. } => {
                // Opaque to summarization — skip.
            }
            ConversationItem::Compaction { summary } => {
                let preview: String = summary.chars().take(300).collect();
                text.push_str(&format!("[compaction]: {preview}\n\n"));
            }
        }
    }
    text
}

/// Generates an AI summary of old context via the Responses API.
pub(crate) fn generate_openai_summary(
    old_context: &str,
    model_id: &str,
    request_config: &OpenAIRequestConfig,
) -> Option<String> {
    use puffer_provider_openai::OpenAIAuth;
    use reqwest::blocking::Client;

    let base_url = &request_config.base_url;
    let prompt = format!(
        "Summarize this conversation fragment into a compact context block. \
         Preserve file paths, function names, errors, and key decisions verbatim. \
         Structure: 1) Intent 2) Key Concepts 3) Files & Code 4) Errors & Fixes \
         5) Pending Tasks 6) Current State. Be thorough but concise. \
         Do NOT use any tools.\n\n---\n\n{old_context}"
    );

    let body = json!({
        "model": model_id,
        "input": prompt,
        "max_output_tokens": COMPACT_SUMMARY_MAX_OUTPUT_TOKENS,
        "stream": false,
    });

    let path = super::support::openai_responses_path(base_url);
    let url = format!("{}{path}", base_url.trim_end_matches('/'));
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .ok()?;

    let mut headers = vec![("Content-Type".to_string(), "application/json".to_string())];
    match &request_config.auth {
        OpenAIAuth::ApiKey(key) => {
            headers.push(("Authorization".to_string(), format!("Bearer {key}")));
        }
        OpenAIAuth::OAuthBearer(token) => {
            headers.push(("Authorization".to_string(), format!("Bearer {token}")));
        }
        OpenAIAuth::None => {}
    }

    let body_str = body.to_string();
    super::support::trace_openai_http_request(&url, &headers, &body_str);

    let mut request = client.post(&url);
    for (key, value) in &headers {
        request = request.header(key, value);
    }
    let response = request.body(body_str).send().ok()?;

    let status = response.status();
    super::support::trace_openai_http_response_headers(
        &url,
        status.as_u16(),
        response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
    );

    if !status.is_success() {
        return None;
    }

    let json: Value = response.json().ok()?;
    json.get("output")?
        .as_array()?
        .iter()
        .find(|item| item.get("type").and_then(Value::as_str) == Some("message"))
        .and_then(|msg| msg.get("content"))
        .and_then(Value::as_array)
        .and_then(|arr| arr.first())
        .and_then(|block| block.get("text"))
        .and_then(Value::as_str)
        .map(String::from)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;
    use puffer_config::PufferConfig;
    use puffer_session_store::SessionMetadata;
    use std::path::PathBuf;

    fn test_state() -> AppState {
        let meta = SessionMetadata {
            id: uuid::Uuid::new_v4(),
            display_name: Some("test".to_string()),
            generated_title: None,
            cwd: PathBuf::from("/tmp"),
            created_at_ms: 0,
            updated_at_ms: 0,
            parent_session_id: None,
            slug: None,
            tags: Vec::new(),
            note: None,
        };
        AppState::new(PufferConfig::default(), PathBuf::from("/tmp"), meta)
    }

    #[test]
    fn transcript_to_items_empty_state() {
        let state = test_state();
        let items = transcript_to_items(&state, "hello");
        assert_eq!(items.len(), 1);
        assert!(matches!(&items[0], ConversationItem::Message { role, .. } if role == "user"));
        assert_eq!(items[0].text_content().unwrap(), "hello");
    }

    #[test]
    fn transcript_to_items_with_history() {
        let mut state = test_state();
        state.push_message(crate::MessageRole::User, "first");
        state.push_message(crate::MessageRole::Assistant, "reply");
        let items = transcript_to_items(&state, "second");
        assert_eq!(items.len(), 3);
        assert!(matches!(&items[0], ConversationItem::Message { role, .. } if role == "user"));
        assert!(matches!(&items[1], ConversationItem::Message { role, .. } if role == "assistant"));
        assert!(matches!(&items[2], ConversationItem::Message { role, .. } if role == "user"));
        assert_eq!(items[2].text_content().unwrap(), "second");
    }

    #[test]
    fn transcript_to_items_deduplicates_when_already_in_transcript() {
        let mut state = test_state();
        // Simulate what command.rs / flow.rs does: push the user message
        // to the transcript *before* calling execute with the same text.
        state.push_message(crate::MessageRole::User, "hello world");
        let items = transcript_to_items(&state, "hello world");
        // Should appear only once, not twice.
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].text_content().unwrap(), "hello world");
    }

    #[test]
    fn transcript_to_items_appends_new_input_after_assistant() {
        let mut state = test_state();
        state.push_message(crate::MessageRole::User, "first");
        state.push_message(crate::MessageRole::Assistant, "reply");
        // Different input than last transcript entry — should be appended.
        let items = transcript_to_items(&state, "second");
        assert_eq!(items.len(), 3);
        assert_eq!(items[2].text_content().unwrap(), "second");
    }

    #[test]
    fn items_to_responses_input_round_trip() {
        let items = vec![
            ConversationItem::user_message("hello"),
            ConversationItem::assistant_message("hi"),
            ConversationItem::FunctionCall {
                call_id: "c1".into(),
                name: "Bash".into(),
                arguments: r#"{"command":"ls"}"#.into(),
            },
            ConversationItem::FunctionCallOutput {
                call_id: "c1".into(),
                output: ToolOutputPayload::success("file.txt".into()),
            },
        ];
        let value = items_to_responses_input(&items);
        let arr = value.as_array().unwrap();
        assert_eq!(arr.len(), 4);
        assert_eq!(arr[0]["role"], "user");
        assert_eq!(arr[0]["content"][0]["type"], "input_text");
        assert_eq!(arr[1]["role"], "assistant");
        assert_eq!(arr[1]["content"][0]["type"], "output_text");
        assert_eq!(arr[2]["type"], "function_call");
        assert_eq!(arr[2]["name"], "Bash");
        assert_eq!(arr[3]["type"], "function_call_output");
        assert_eq!(arr[3]["output"], "file.txt");
    }

    /// Regression: a `MessageRole::System` item pushed by
    /// `flow.rs::emit_system_message` after the conversation has already
    /// started (e.g. "Interrupted by user.", "Provider request failed: ...")
    /// must be stripped before reaching the Responses API `input` array,
    /// because (a) ChatGPT Codex backend rejects it with 400, and
    /// (b) permissive proxies silently merge it into the top-level
    /// `instructions`, polluting the system prompt.
    #[test]
    fn responses_input_drops_transient_system_messages() {
        let items = vec![
            ConversationItem::user_message("first user message"),
            ConversationItem::assistant_message("first reply"),
            ConversationItem::system_message("Interrupted by user."),
            ConversationItem::user_message("second user message"),
        ];
        let value = items_to_responses_input(&items);
        let arr = value.as_array().unwrap();
        assert_eq!(arr.len(), 3, "transient system entry must be dropped");
        assert!(
            !arr.iter().any(|m| m["role"] == "system"),
            "no system role should appear after the first non-system item: {arr:?}"
        );
        assert_eq!(arr[0]["role"], "user");
        assert_eq!(arr[1]["role"], "assistant");
        assert_eq!(arr[2]["role"], "user");
        assert_eq!(arr[2]["content"][0]["text"], "second user message");
    }

    /// Regression: same scenario for Chat Completions wire format. Strict
    /// providers (and some Anthropic-via-OpenAI shims like GLM coding plan
    /// when routed through chat completions) reject mid-conversation
    /// `role:"system"`; lenient ones treat it as an instruction switch
    /// and behave unpredictably.
    #[test]
    fn chat_messages_drops_transient_system_messages() {
        let items = vec![
            ConversationItem::user_message("first user message"),
            ConversationItem::assistant_message("first reply"),
            ConversationItem::system_message("Interrupted by user."),
            ConversationItem::user_message("second user message"),
        ];
        let msgs = items_to_chat_messages(&items, Some("real system prompt"), None, None, None);
        // Position 0 is the explicit top-level system_prompt; positions
        // 1..N must contain no further `system` role.
        assert_eq!(msgs[0].role, "system");
        for (i, m) in msgs.iter().enumerate().skip(1) {
            assert_ne!(
                m.role, "system",
                "transient system leaked into chat messages at position {i}: {m:?}"
            );
        }
        // Sanity: message order is user, assistant, user.
        assert_eq!(msgs[1].role, "user");
        assert_eq!(msgs[2].role, "assistant");
        assert_eq!(msgs[3].role, "user");
    }

    /// First-position exemption: a `system_message` at the head of the
    /// items list represents legitimate model-facing system content
    /// (e.g. a sub-agent identity prompt pushed by `agents.rs:321`
    /// after `nested_state.transcript.clear()`). It must NOT be dropped.
    #[test]
    fn responses_input_preserves_leading_system_message() {
        let items = vec![
            ConversationItem::system_message("You are the bug-bounty subagent."),
            ConversationItem::user_message("audit this"),
        ];
        let value = items_to_responses_input(&items);
        let arr = value.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["role"], "system");
        assert_eq!(arr[0]["content"], "You are the bug-bounty subagent.");
        assert_eq!(arr[1]["role"], "user");
    }

    /// Anthropic boundary already strips role:"system" entirely (the
    /// protocol disallows it in messages) — keep that contract under
    /// the same scenario the Responses-side regression covers.
    #[test]
    fn anthropic_messages_never_contain_system_role() {
        let items = vec![
            ConversationItem::user_message("first user message"),
            ConversationItem::assistant_message("first reply"),
            ConversationItem::system_message("Interrupted by user."),
            ConversationItem::user_message("second user message"),
        ];
        let msgs = items_to_anthropic_messages(&items);
        for m in &msgs {
            assert_ne!(
                m["role"], "system",
                "system role leaked into Anthropic messages: {m:?}"
            );
        }
    }

    /// Regression for the gap that codex-review R2 surfaced: the runtime
    /// path used to do `items.insert(0, user_message(context_reminder))`
    /// before the wire boundary, which pushed the sub-agent identity
    /// prompt that `agents.rs:319` puts at transcript[0] to items[1] —
    /// past `drop_transient_system_messages`'s first-position exemption,
    /// where it then got stripped.
    ///
    /// The fix lives in `insert_context_reminder_preserving_legacy_leading_system()` (this file): it
    /// inserts the reminder *after* any leading run of `role:"system"`
    /// items, preserving the sub-agent identity at the head where the
    /// boundary expects it.
    #[test]
    fn responses_input_keeps_subagent_identity_after_runtime_prepend() {
        let mut items = vec![
            ConversationItem::system_message("You are the bug-bounty subagent."),
            ConversationItem::user_message("audit this"),
        ];
        // Mimic the runtime call site (openai.rs / websocket.rs).
        insert_context_reminder_preserving_legacy_leading_system(
            &mut items,
            "[context: cwd=/foo, ts=...]",
        );
        let value = items_to_responses_input(&items);
        let arr = value.as_array().unwrap();
        let identity_count = arr
            .iter()
            .filter(|m| {
                m["role"] == "system"
                    && m["content"]
                        .as_str()
                        .map(|s| s.contains("bug-bounty subagent"))
                        .unwrap_or(false)
            })
            .count();
        assert_eq!(
            identity_count, 1,
            "sub-agent identity prompt must survive the runtime context-reminder prepend: {arr:?}"
        );
        // And the reminder itself must still be present, immediately after
        // the system identity.
        assert_eq!(arr[0]["role"], "system");
        assert_eq!(arr[1]["role"], "user");
        assert!(
            arr[1]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("context"),
            "reminder text expected at position 1"
        );
    }

    /// Mirror of `responses_input_preserves_leading_system_message` for the
    /// Chat Completions boundary — confirms the first-position exemption
    /// holds on both paths.
    #[test]
    fn chat_messages_preserves_leading_system_message() {
        let items = vec![
            ConversationItem::system_message("You are the bug-bounty subagent."),
            ConversationItem::user_message("audit this"),
        ];
        let msgs = items_to_chat_messages(&items, Some("real system prompt"), None, None, None);
        // Position 0 = explicit top-level system prompt; position 1 = the
        // sub-agent identity from items[0]; position 2 = the user message.
        assert_eq!(msgs[0].role, "system");
        assert_eq!(msgs[1].role, "system");
        assert_eq!(
            msgs[1].content.as_ref().and_then(|v| v.as_str()),
            Some("You are the bug-bounty subagent.")
        );
        assert_eq!(msgs[2].role, "user");
    }

    #[test]
    fn managed_system_prompt_1_is_second_chat_system_message() {
        let items = vec![ConversationItem::user_message("audit this")];
        let msgs = items_to_chat_messages(
            &items,
            Some("base system prompt"),
            Some("managed system prompt"),
            None,
            None,
        );
        assert_eq!(msgs[0].role, "system");
        assert_eq!(
            msgs[0].content.as_ref().and_then(|v| v.as_str()),
            Some("base system prompt")
        );
        assert_eq!(msgs[1].role, "system");
        assert_eq!(
            msgs[1].content.as_ref().and_then(|v| v.as_str()),
            Some("managed system prompt")
        );
        assert_eq!(msgs[2].role, "user");
    }

    #[test]
    fn managed_system_prompt_1_is_leading_responses_system_item() {
        let mut items = vec![ConversationItem::user_message("audit this")];
        insert_managed_system_prompt_1(&mut items, Some("managed system prompt"));
        insert_context_reminder_preserving_legacy_leading_system(
            &mut items,
            "[context: cwd=/foo, ts=...]",
        );

        let value = items_to_responses_input(&items);
        let arr = value.as_array().unwrap();
        assert_eq!(arr[0]["role"], "system");
        assert_eq!(arr[0]["content"], "managed system prompt");
        assert_eq!(arr[1]["role"], "user");
        assert!(arr[1]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("context"));
        assert_eq!(arr[2]["role"], "user");
    }

    /// Mid-conversation transient system items get dropped even when a
    /// stale UI `MessageRole::System` was reloaded from session_store at
    /// startup. The reload path is `state.rs:197`; when followed by any
    /// user/assistant content the new entry lands at items[1+] where the
    /// boundary filter strips it.
    ///
    /// This is the core safety net for the "session reload after a
    /// crash that left a `Provider request failed: ...` system in the
    /// transcript" scenario that motivated PR#46.
    #[test]
    fn responses_input_drops_replayed_ui_system_when_followed_by_user() {
        let items = vec![
            ConversationItem::user_message("first user message"),
            ConversationItem::system_message("Provider request failed: 400 ..."),
            ConversationItem::user_message("retry"),
        ];
        let value = items_to_responses_input(&items);
        let arr = value.as_array().unwrap();
        assert_eq!(arr.len(), 2, "stale UI system must be dropped");
        assert_eq!(arr[0]["role"], "user");
        assert_eq!(arr[1]["role"], "user");
    }

    #[test]
    fn items_to_chat_messages_groups_tool_calls() {
        let items = vec![
            ConversationItem::user_message("do stuff"),
            ConversationItem::FunctionCall {
                call_id: "c1".into(),
                name: "Read".into(),
                arguments: "{}".into(),
            },
            ConversationItem::FunctionCall {
                call_id: "c2".into(),
                name: "Grep".into(),
                arguments: "{}".into(),
            },
            ConversationItem::FunctionCallOutput {
                call_id: "c1".into(),
                output: ToolOutputPayload::success("content1".into()),
            },
            ConversationItem::FunctionCallOutput {
                call_id: "c2".into(),
                output: ToolOutputPayload::success("content2".into()),
            },
        ];
        let msgs = items_to_chat_messages(&items, Some("sys prompt"), None, None, None);
        // system + user + assistant(tool_calls) + tool + tool = 5
        assert_eq!(msgs.len(), 5);
        assert_eq!(msgs[0].role, "system");
        assert_eq!(msgs[1].role, "user");
        assert_eq!(msgs[2].role, "assistant");
        assert_eq!(msgs[2].tool_calls.len(), 2); // c1 + c2 grouped
        assert_eq!(msgs[3].role, "tool");
        assert_eq!(msgs[4].role, "tool");
    }

    #[test]
    fn items_to_chat_messages_includes_system_reminder() {
        let items = vec![ConversationItem::user_message("test")];
        let msgs = items_to_chat_messages(
            &items,
            Some("base prompt"),
            None,
            None,
            Some("# currentDate\nToday is 2026-04-12."),
        );
        // system(prompt) + system(reminder) + user = 3
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0].role, "system");
        assert_eq!(msgs[1].role, "system");
        let reminder_text = msgs[1].content.as_ref().unwrap().as_str().unwrap();
        assert!(reminder_text.contains("currentDate"));
    }

    #[test]
    fn append_tool_results_adds_items() {
        let mut items = vec![ConversationItem::user_message("go")];
        let invocations = vec![crate::runtime::ToolInvocation {
            call_id: "c1".into(),
            tool_id: "Bash".into(),
            input: r#"{"command":"ls"}"#.into(),
            output: "result".into(),
            success: true,
            terminate: false,
        }];
        append_tool_results(&mut items, &invocations);
        assert_eq!(items.len(), 3);
        assert!(matches!(&items[1], ConversationItem::FunctionCall { name, .. } if name == "Bash"));
        assert!(
            matches!(&items[2], ConversationItem::FunctionCallOutput { output, .. } if output.text == "result" && !output.is_error)
        );
    }

    #[test]
    fn append_tool_results_propagates_is_error() {
        let mut items = vec![ConversationItem::user_message("go")];
        let invocations = vec![crate::runtime::ToolInvocation {
            call_id: "c1".into(),
            tool_id: "Bash".into(),
            input: r#"{"command":"bad"}"#.into(),
            output: "command not found".into(),
            success: false,
            terminate: false,
        }];
        append_tool_results(&mut items, &invocations);
        assert!(
            matches!(&items[2], ConversationItem::FunctionCallOutput { output, .. } if output.is_error)
        );
    }

    #[test]
    fn inject_post_compact_context_inserts_at_position_1() {
        let mut items = vec![
            ConversationItem::user_message("summary"),
            ConversationItem::user_message("recent"),
        ];
        inject_post_compact_context(&mut items, std::path::Path::new("/work"));
        assert_eq!(items.len(), 3);
        let text = items[1].text_content().unwrap();
        assert!(text.contains("Context restored after compaction"));
        assert!(text.contains("/work"));
    }

    #[test]
    fn build_system_reminder_includes_date() {
        let reminder = build_system_reminder("");
        assert!(reminder.contains("currentDate"));
        assert!(reminder.contains("Today's date is"));
    }

    #[test]
    fn build_system_reminder_includes_git_status() {
        let reminder = build_system_reminder("On branch main");
        assert!(reminder.contains("gitStatus"));
        assert!(reminder.contains("On branch main"));
    }

    #[test]
    fn build_summary_text_formats_all_item_types() {
        let items = vec![
            ConversationItem::user_message("implement feature"),
            ConversationItem::FunctionCall {
                call_id: "c1".into(),
                name: "Bash".into(),
                arguments: r#"{"cmd":"ls"}"#.into(),
            },
            ConversationItem::FunctionCallOutput {
                call_id: "c1".into(),
                output: ToolOutputPayload::success("file.rs".into()),
            },
        ];
        let text = build_summary_text(&items);
        assert!(text.contains("[user]: implement feature"));
        assert!(text.contains("[tool_call Bash]:"));
        assert!(text.contains("[tool_result]: file.rs"));
    }

    #[test]
    fn compact_conversation_no_op_when_small() {
        let mut items = vec![
            ConversationItem::user_message("hello"),
            ConversationItem::assistant_message("hi"),
        ];
        // With small items and high threshold, no compaction should occur.
        let provider = test_provider(200_000, 16_384);
        let config = test_request_config();
        let compacted = compact_conversation(&mut items, &provider, "gpt-5", &config, None);
        assert!(!compacted);
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn compact_conversation_phase1_snips_old_outputs() {
        let mut items = Vec::new();
        // 10 large function call outputs + 6 recent items
        for i in 0..10 {
            items.push(ConversationItem::FunctionCallOutput {
                call_id: format!("c{i}"),
                output: ToolOutputPayload::success("x".repeat(2000)),
            });
        }
        for i in 0..6 {
            items.push(ConversationItem::user_message(format!("recent {i}")));
        }
        let provider = test_provider(200_000, 16_384);
        let config = test_request_config();
        // Force compaction by providing a high input_tokens_hint.
        let compacted = compact_conversation(
            &mut items,
            &provider,
            "gpt-5",
            &config,
            Some(180_000), // near threshold
        );
        // Phase 1 should have snipped old outputs.
        for item in &items[..items.len() - 6] {
            if let ConversationItem::FunctionCallOutput { output, .. } = item {
                assert!(output.text.len() <= 600, "output should be snipped");
            }
        }
        // May or may not have triggered Phase 2/3 depending on estimate.
        let _ = compacted;
    }

    fn test_provider(context_window: u32, max_output: u32) -> ProviderDescriptor {
        ProviderDescriptor {
            id: "test".into(),
            display_name: "test".into(),
            base_url: "https://api.openai.com".into(),
            default_api: "openai-responses".into(),
            auth_modes: Vec::new(),
            headers: indexmap::IndexMap::new(),
            query_params: indexmap::IndexMap::new(),
            discovery: None,
            models: vec![puffer_provider_registry::ModelDescriptor {
                id: "gpt-5".into(),
                display_name: "GPT-5".into(),
                provider: "test".into(),
                api: "openai-responses".into(),
                context_window,
                max_output_tokens: max_output,
                supports_reasoning: false,
                compat: None,
                input: vec![puffer_provider_registry::Modality::Text],
                cost: None,
            }],
        }
    }

    fn test_request_config() -> OpenAIRequestConfig {
        OpenAIRequestConfig {
            base_url: "https://api.openai.com".into(),
            version: "test".into(),
            auth: puffer_provider_openai::OpenAIAuth::None,
            originator: "test".into(),
            session_id: None,
            account_id: None,
            custom_headers: Vec::new(),
            query_params: Vec::new(),
        }
    }

    // =======================================================================
    // Serialization round-trip tests
    // =======================================================================

    #[test]
    fn serde_round_trip_message() {
        let item = ConversationItem::user_message("hello world");
        let json = serde_json::to_string(&item).unwrap();
        let parsed: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "message");
        assert_eq!(parsed["role"], "user");
        assert_eq!(parsed["content"][0]["type"], "text");
        assert_eq!(parsed["content"][0]["text"], "hello world");

        // Round-trip back to Rust
        let recovered: ConversationItem = serde_json::from_str(&json).unwrap();
        assert_eq!(recovered, item);
    }

    #[test]
    fn serde_round_trip_function_call() {
        let item = ConversationItem::FunctionCall {
            call_id: "call_abc".into(),
            name: "Bash".into(),
            arguments: r#"{"command":"ls -la"}"#.into(),
        };
        let json = serde_json::to_string(&item).unwrap();
        let parsed: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "function_call");
        assert_eq!(parsed["call_id"], "call_abc");
        assert_eq!(parsed["name"], "Bash");

        let recovered: ConversationItem = serde_json::from_str(&json).unwrap();
        assert_eq!(recovered, item);
    }

    #[test]
    fn serde_round_trip_function_call_output() {
        // ToolOutputPayload serializes as a plain string — is_error is NOT on the wire.
        let item = ConversationItem::FunctionCallOutput {
            call_id: "call_abc".into(),
            output: ToolOutputPayload::error("permission denied".into()),
        };
        let json = serde_json::to_string(&item).unwrap();
        let parsed: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "function_call_output");
        assert_eq!(parsed["output"], "permission denied");
        // is_error must NOT appear on wire (Codex FunctionCallOutputPayload pattern)
        assert!(parsed.get("is_error").is_none());

        // Round-trip: is_error resets to false (wire doesn't carry it)
        let recovered: ConversationItem = serde_json::from_str(&json).unwrap();
        if let ConversationItem::FunctionCallOutput { output, .. } = &recovered {
            assert_eq!(output.text, "permission denied");
            assert!(
                !output.is_error,
                "is_error should default to false after deserialization"
            );
        } else {
            panic!("expected FunctionCallOutput");
        }
    }

    #[test]
    fn serde_round_trip_compaction() {
        let item = ConversationItem::Compaction {
            summary: "User asked to refactor auth module.".into(),
        };
        let json = serde_json::to_string(&item).unwrap();
        let parsed: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "compaction");
        assert_eq!(parsed["summary"], "User asked to refactor auth module.");

        let recovered: ConversationItem = serde_json::from_str(&json).unwrap();
        assert_eq!(recovered, item);
    }

    #[test]
    fn serde_content_part_tagged_correctly() {
        let text = ContentPart::Text {
            text: "hello".into(),
        };
        let img = ContentPart::Image {
            url: "data:image/png;base64,abc".into(),
        };

        let text_json: Value = serde_json::to_value(&text).unwrap();
        assert_eq!(text_json["type"], "text");
        assert_eq!(text_json["text"], "hello");

        let img_json: Value = serde_json::to_value(&img).unwrap();
        assert_eq!(img_json["type"], "image");
        assert_eq!(img_json["url"], "data:image/png;base64,abc");
    }

    // =======================================================================
    // ToolOutputPayload tests
    // =======================================================================

    #[test]
    fn tool_output_payload_success_and_error() {
        let s = ToolOutputPayload::success("ok".into());
        assert_eq!(s.text, "ok");
        assert!(!s.is_error);

        let e = ToolOutputPayload::error("fail".into());
        assert_eq!(e.text, "fail");
        assert!(e.is_error);
    }

    #[test]
    fn tool_output_payload_serialize_is_plain_string() {
        let payload = ToolOutputPayload::error("some error output".into());
        let json = serde_json::to_value(&payload).unwrap();
        // Must serialize as a bare string, not an object
        assert!(json.is_string());
        assert_eq!(json.as_str().unwrap(), "some error output");
    }

    #[test]
    fn tool_output_payload_deserialize_from_string() {
        let json = serde_json::Value::String("tool output text".into());
        let payload: ToolOutputPayload = serde_json::from_value(json).unwrap();
        assert_eq!(payload.text, "tool output text");
        assert!(!payload.is_error); // default false
    }

    // =======================================================================
    // normalize_items tests
    // =======================================================================

    #[test]
    fn normalize_items_fills_orphan_function_call() {
        let mut items = vec![
            ConversationItem::user_message("run a command"),
            ConversationItem::FunctionCall {
                call_id: "c1".into(),
                name: "Bash".into(),
                arguments: r#"{"command":"ls"}"#.into(),
            },
            // No FunctionCallOutput for c1 — it's orphaned
            ConversationItem::user_message("continue"),
        ];
        normalize_items(&mut items);

        // Should have 4 items now (synthetic output inserted after the FunctionCall)
        assert_eq!(items.len(), 4);
        if let ConversationItem::FunctionCallOutput { call_id, output } = &items[2] {
            assert_eq!(call_id, "c1");
            assert!(output.is_error);
            assert!(output.text.contains("not completed"));
        } else {
            panic!("expected synthetic FunctionCallOutput at index 2");
        }
    }

    #[test]
    fn normalize_items_no_op_when_paired() {
        let mut items = vec![
            ConversationItem::FunctionCall {
                call_id: "c1".into(),
                name: "Read".into(),
                arguments: "{}".into(),
            },
            ConversationItem::FunctionCallOutput {
                call_id: "c1".into(),
                output: ToolOutputPayload::success("file content".into()),
            },
        ];
        let original_len = items.len();
        normalize_items(&mut items);
        assert_eq!(
            items.len(),
            original_len,
            "paired items should not be modified"
        );
    }

    #[test]
    fn normalize_items_multiple_orphans() {
        let mut items = vec![
            ConversationItem::FunctionCall {
                call_id: "c1".into(),
                name: "Read".into(),
                arguments: "{}".into(),
            },
            ConversationItem::FunctionCall {
                call_id: "c2".into(),
                name: "Grep".into(),
                arguments: "{}".into(),
            },
            // Both orphaned
        ];
        normalize_items(&mut items);
        // Each orphan gets a synthetic output
        assert_eq!(items.len(), 4);
        // Verify both outputs exist
        let output_ids: Vec<&str> = items
            .iter()
            .filter_map(|i| match i {
                ConversationItem::FunctionCallOutput { call_id, .. } => Some(call_id.as_str()),
                _ => None,
            })
            .collect();
        assert!(output_ids.contains(&"c1"));
        assert!(output_ids.contains(&"c2"));
    }

    // =======================================================================
    // Token estimation tests
    // =======================================================================

    #[test]
    fn token_estimation_ascii_text() {
        // "hello world" = 11 ASCII chars → (11 + 3) / 4 = 3 tokens
        let item = ConversationItem::user_message("hello world");
        assert_eq!(item.estimated_tokens(), 3);
    }

    #[test]
    fn token_estimation_cjk_text() {
        // "你好世界" = 4 CJK chars → 4 * 6 = 24 units → (24 + 3) / 4 = 6 tokens
        // This is ~1.5 tokens per CJK char, matching real tokenizer behavior.
        let item = ConversationItem::user_message("你好世界");
        assert_eq!(item.estimated_tokens(), 6);
    }

    #[test]
    fn token_estimation_mixed_text() {
        // "Hello 世界" = 6 ASCII + 2 CJK → 6 + 12 = 18 units → (18 + 3) / 4 = 5
        let item = ConversationItem::user_message("Hello 世界");
        assert_eq!(item.estimated_tokens(), 5);
    }

    #[test]
    fn token_estimation_function_call() {
        let item = ConversationItem::FunctionCall {
            call_id: "c1".into(),
            name: "Bash".into(),                 // 4 chars → 1 token
            arguments: r#"{"cmd":"ls"}"#.into(), // 12 chars → 3 tokens
        };
        // name(1) + arguments(3) = 4
        assert_eq!(item.estimated_tokens(), 4);
    }

    #[test]
    fn token_estimation_image_block() {
        let item = ConversationItem::Message {
            role: "user".into(),
            content: vec![
                ContentPart::Text {
                    text: "look at this".into(),
                },
                ContentPart::Image {
                    url: "data:image/png;base64,abc".into(),
                },
            ],
        };
        // text: (12 + 3) / 4 = 3, image: 300 → total 303
        assert_eq!(item.estimated_tokens(), 303);
    }

    // =======================================================================
    // text_content extraction tests
    // =======================================================================

    #[test]
    fn text_content_from_message() {
        let item = ConversationItem::user_message("hello");
        assert_eq!(item.text_content().unwrap(), "hello");
    }

    #[test]
    fn text_content_from_multi_part_message() {
        let item = ConversationItem::Message {
            role: "user".into(),
            content: vec![
                ContentPart::Text {
                    text: "first".into(),
                },
                ContentPart::Image {
                    url: "img.png".into(),
                },
                ContentPart::Text {
                    text: "second".into(),
                },
            ],
        };
        // Images are skipped, text blocks joined with \n
        assert_eq!(item.text_content().unwrap(), "first\nsecond");
    }

    #[test]
    fn text_content_from_image_only_message() {
        let item = ConversationItem::Message {
            role: "user".into(),
            content: vec![ContentPart::Image {
                url: "img.png".into(),
            }],
        };
        // No text blocks → None
        assert!(item.text_content().is_none());
    }

    #[test]
    fn text_content_from_function_call_output() {
        let item = ConversationItem::FunctionCallOutput {
            call_id: "c1".into(),
            output: ToolOutputPayload::success("file content".into()),
        };
        assert_eq!(item.text_content().unwrap(), "file content");
    }

    #[test]
    fn text_content_from_function_call_is_none() {
        let item = ConversationItem::FunctionCall {
            call_id: "c1".into(),
            name: "Bash".into(),
            arguments: "{}".into(),
        };
        assert!(item.text_content().is_none());
    }

    // =======================================================================
    // Compaction wire format tests
    // =======================================================================

    #[test]
    fn compaction_renders_as_user_message_in_responses_input() {
        let items = vec![ConversationItem::Compaction {
            summary: "Prior context summary".into(),
        }];
        let value = items_to_responses_input(&items);
        let arr = value.as_array().unwrap();
        assert_eq!(arr[0]["type"], "message");
        assert_eq!(arr[0]["role"], "user");
        let text = arr[0]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("Conversation compacted"));
        assert!(text.contains("Prior context summary"));
    }

    #[test]
    fn compaction_renders_as_user_message_in_chat_messages() {
        let items = vec![
            ConversationItem::Compaction {
                summary: "Prior context summary".into(),
            },
            ConversationItem::user_message("next question"),
        ];
        let msgs = items_to_chat_messages(&items, Some("sys"), None, None, None);
        // system + compaction(user) + user = 3
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[1].role, "user");
        let text = msgs[1].content.as_ref().unwrap().as_str().unwrap();
        assert!(text.contains("Prior context summary"));
    }

    // =======================================================================
    // find_valid_keep_boundary tests
    // =======================================================================

    #[test]
    fn find_valid_keep_boundary_skips_orphan_tool_items() {
        let items = vec![
            ConversationItem::user_message("old"),
            ConversationItem::assistant_message("reply"),
            ConversationItem::FunctionCall {
                call_id: "c1".into(),
                name: "Bash".into(),
                arguments: "{}".into(),
            },
            ConversationItem::FunctionCallOutput {
                call_id: "c1".into(),
                output: ToolOutputPayload::success("ok".into()),
            },
            ConversationItem::user_message("recent"),
        ];
        // min_keep=2 would start at index 3 (FunctionCallOutput) — not a Message.
        // Should extend to include the full tool pair + preceding assistant,
        // landing on index 1 (assistant Message), keep=4.
        let keep = find_valid_keep_boundary(&items, 2);
        assert!(keep >= 2);
        let start = items.len() - keep;
        assert!(
            matches!(&items[start], ConversationItem::Message { .. }),
            "boundary must start at a Message, got {:?}",
            &items[start]
        );
    }

    #[test]
    fn find_valid_keep_boundary_already_at_message() {
        let items = vec![
            ConversationItem::user_message("old"),
            ConversationItem::assistant_message("reply"),
            ConversationItem::user_message("recent"),
        ];
        // min_keep=1, index 2 is a Message — should return 1
        let keep = find_valid_keep_boundary(&items, 1);
        assert_eq!(keep, 1);
    }

    // =======================================================================
    // is_error propagation in transcript_to_items
    // =======================================================================

    #[test]
    fn transcript_to_items_propagates_is_error() {
        let mut state = test_state();
        // Push a tool call + failed tool result
        state.transcript.push(crate::state::RenderedMessage {
            text: "error output".into(),
            thinking: None,
            role: crate::MessageRole::ToolResult,
            call_id: Some("c1".into()),
            tool_id: None,
            tool_input: None,
            success: Some(false),
        });
        let items = transcript_to_items(&state, "");
        // Find the FunctionCallOutput
        let output_item = items
            .iter()
            .find(|i| matches!(i, ConversationItem::FunctionCallOutput { .. }));
        assert!(output_item.is_some(), "should have a FunctionCallOutput");
        if let ConversationItem::FunctionCallOutput { output, .. } = output_item.unwrap() {
            assert!(
                output.is_error,
                "is_error should be true for failed tool result"
            );
            assert_eq!(output.text, "error output");
        }
    }

    // =======================================================================
    // items_to_anthropic_messages tests
    // =======================================================================

    #[test]
    fn anthropic_basic_user_assistant_alternation() {
        let items = vec![
            ConversationItem::user_message("hello"),
            ConversationItem::assistant_message("hi there"),
            ConversationItem::user_message("how are you"),
        ];
        let msgs = items_to_anthropic_messages(&items);
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[0]["content"], "hello");
        assert_eq!(msgs[1]["role"], "assistant");
        assert_eq!(msgs[1]["content"], "hi there");
        assert_eq!(msgs[2]["role"], "user");
        assert_eq!(msgs[2]["content"], "how are you");
    }

    #[test]
    fn anthropic_system_messages_skipped() {
        let items = vec![
            ConversationItem::system_message("you are helpful"),
            ConversationItem::user_message("hello"),
        ];
        let msgs = items_to_anthropic_messages(&items);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[0]["content"], "hello");
    }

    #[test]
    fn anthropic_tool_use_in_assistant_content() {
        let items = vec![
            ConversationItem::user_message("list files"),
            ConversationItem::FunctionCall {
                call_id: "c1".into(),
                name: "Bash".into(),
                arguments: r#"{"command":"ls"}"#.into(),
            },
            ConversationItem::FunctionCallOutput {
                call_id: "c1".into(),
                output: ToolOutputPayload::success("file.rs".into()),
            },
        ];
        let msgs = items_to_anthropic_messages(&items);
        assert_eq!(
            msgs.len(),
            3,
            "user + assistant(tool_use) + user(tool_result)"
        );

        // Assistant message with tool_use
        assert_eq!(msgs[1]["role"], "assistant");
        let tool_use = &msgs[1]["content"][0];
        assert_eq!(tool_use["type"], "tool_use");
        assert_eq!(tool_use["id"], "c1");
        assert_eq!(tool_use["name"], "Bash");
        assert_eq!(tool_use["input"]["command"], "ls");

        // User message with tool_result
        assert_eq!(msgs[2]["role"], "user");
        let tool_result = &msgs[2]["content"][0];
        assert_eq!(tool_result["type"], "tool_result");
        assert_eq!(tool_result["tool_use_id"], "c1");
        assert_eq!(tool_result["content"], "file.rs");
        assert!(
            tool_result.get("is_error").is_none(),
            "is_error should be absent when false"
        );
    }

    #[test]
    fn anthropic_is_error_propagated_to_tool_result() {
        let items = vec![
            ConversationItem::user_message("run something"),
            ConversationItem::FunctionCall {
                call_id: "c1".into(),
                name: "Bash".into(),
                arguments: "{}".into(),
            },
            ConversationItem::FunctionCallOutput {
                call_id: "c1".into(),
                output: ToolOutputPayload::error("permission denied".into()),
            },
        ];
        let msgs = items_to_anthropic_messages(&items);
        let tool_result = &msgs[2]["content"][0];
        assert_eq!(tool_result["is_error"], true);
        assert_eq!(tool_result["content"], "permission denied");
    }

    #[test]
    fn anthropic_consecutive_function_calls_grouped() {
        // Parallel tool calls: two FunctionCalls in a row → one assistant message
        let items = vec![
            ConversationItem::user_message("do both"),
            ConversationItem::FunctionCall {
                call_id: "c1".into(),
                name: "Read".into(),
                arguments: r#"{"path":"a.rs"}"#.into(),
            },
            ConversationItem::FunctionCall {
                call_id: "c2".into(),
                name: "Grep".into(),
                arguments: r#"{"pattern":"foo"}"#.into(),
            },
            ConversationItem::FunctionCallOutput {
                call_id: "c1".into(),
                output: ToolOutputPayload::success("content a".into()),
            },
            ConversationItem::FunctionCallOutput {
                call_id: "c2".into(),
                output: ToolOutputPayload::success("content b".into()),
            },
        ];
        let msgs = items_to_anthropic_messages(&items);
        // user + assistant(2 tool_uses) + user(2 tool_results) = 3
        assert_eq!(msgs.len(), 3);

        // Assistant has 2 tool_use blocks
        assert_eq!(msgs[1]["role"], "assistant");
        let content = msgs[1]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["name"], "Read");
        assert_eq!(content[1]["name"], "Grep");

        // User has 2 tool_result blocks
        assert_eq!(msgs[2]["role"], "user");
        let results = msgs[2]["content"].as_array().unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0]["tool_use_id"], "c1");
        assert_eq!(results[1]["tool_use_id"], "c2");
    }

    #[test]
    fn anthropic_consecutive_same_role_merged() {
        // Two consecutive user messages should be merged
        let items = vec![
            ConversationItem::user_message("first"),
            ConversationItem::user_message("second"),
            ConversationItem::assistant_message("reply"),
        ];
        let msgs = items_to_anthropic_messages(&items);
        assert_eq!(msgs.len(), 2, "consecutive users should merge");
        assert_eq!(msgs[0]["role"], "user");
        // Merged content becomes array
        let content = msgs[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["text"], "first");
        assert_eq!(content[1]["text"], "second");
    }

    #[test]
    fn anthropic_first_message_must_be_user() {
        // If items start with assistant, a synthetic user message is prepended
        let items = vec![ConversationItem::assistant_message("I'm ready")];
        let msgs = items_to_anthropic_messages(&items);
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[1]["role"], "assistant");
    }

    #[test]
    fn anthropic_compaction_as_user_message() {
        let items = vec![
            ConversationItem::Compaction {
                summary: "Prior work summary.".into(),
            },
            ConversationItem::assistant_message("understood"),
            ConversationItem::user_message("continue"),
        ];
        let msgs = items_to_anthropic_messages(&items);
        assert_eq!(msgs[0]["role"], "user");
        let text = msgs[0]["content"].as_str().unwrap();
        assert!(text.contains("Prior work summary."));
        assert!(text.contains("Conversation compacted"));
    }

    #[test]
    fn anthropic_empty_items_produces_user_message() {
        let items: Vec<ConversationItem> = vec![];
        let msgs = items_to_anthropic_messages(&items);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["role"], "user");
    }

    #[test]
    fn anthropic_tool_call_with_text_response() {
        // Assistant sends text + tool call in same turn
        let items = vec![
            ConversationItem::user_message("help me"),
            ConversationItem::assistant_message("Let me check that."),
            ConversationItem::FunctionCall {
                call_id: "c1".into(),
                name: "Read".into(),
                arguments: r#"{"path":"foo.rs"}"#.into(),
            },
            ConversationItem::FunctionCallOutput {
                call_id: "c1".into(),
                output: ToolOutputPayload::success("file content".into()),
            },
        ];
        let msgs = items_to_anthropic_messages(&items);
        // user + assistant(text+tool_use merged) + user(tool_result) = 3
        assert_eq!(msgs.len(), 3);
        // The assistant message should have merged text + tool_use
        assert_eq!(msgs[1]["role"], "assistant");
        let content = msgs[1]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "Let me check that.");
        assert_eq!(content[1]["type"], "tool_use");
    }

    // =======================================================================
    // Multi-turn wire format verification (Codex/CC/Pi-Mono parity)
    // =======================================================================

    /// Simulates a realistic multi-turn conversation with tool calls and
    /// verifies all three wire formats match the Codex/CC/Pi-Mono patterns:
    /// - Responses API: flat array of {message, function_call, function_call_output}
    /// - Chat Completions: messages with role-based tool_calls / role:"tool"
    /// - Anthropic Messages: strict alternation, tool_use in assistant, tool_result in user
    #[test]
    fn multi_turn_wire_format_parity() {
        // Build a realistic 2-turn conversation:
        // Turn 1: user asks → assistant calls Bash → gets result → replies
        // Turn 2: user asks follow-up → assistant calls Read(failed) → gets error → replies
        let items = vec![
            // Turn 1
            ConversationItem::user_message("list files"),
            ConversationItem::FunctionCall {
                call_id: "call_1".into(),
                name: "Bash".into(),
                arguments: r#"{"command":"ls"}"#.into(),
            },
            ConversationItem::FunctionCallOutput {
                call_id: "call_1".into(),
                output: ToolOutputPayload::success("file1.rs\nfile2.rs".into()),
            },
            ConversationItem::assistant_message("I found 2 files."),
            // Turn 2 (with error)
            ConversationItem::user_message("read the missing file"),
            ConversationItem::FunctionCall {
                call_id: "call_2".into(),
                name: "Read".into(),
                arguments: r#"{"path":"missing.rs"}"#.into(),
            },
            ConversationItem::FunctionCallOutput {
                call_id: "call_2".into(),
                output: ToolOutputPayload::error("file not found".into()),
            },
            ConversationItem::assistant_message("The file doesn't exist."),
        ];

        // === Responses API (Codex pattern) ===
        let responses = items_to_responses_input(&items);
        let r = responses.as_array().unwrap();
        // Should be 8 flat items: message, function_call, function_call_output, message, ...
        assert_eq!(r.len(), 8, "Responses API: expected 8 flat items");
        assert_eq!(r[0]["type"], "message");
        assert_eq!(r[0]["role"], "user");
        assert_eq!(r[1]["type"], "function_call");
        assert_eq!(r[1]["name"], "Bash");
        assert_eq!(r[1]["call_id"], "call_1");
        assert_eq!(r[2]["type"], "function_call_output");
        assert_eq!(r[2]["call_id"], "call_1");
        assert_eq!(r[3]["type"], "message");
        assert_eq!(r[3]["role"], "assistant");
        // Turn 2
        assert_eq!(r[5]["type"], "function_call");
        assert_eq!(r[5]["name"], "Read");
        assert_eq!(r[6]["type"], "function_call_output");
        assert_eq!(r[6]["call_id"], "call_2");
        assert_eq!(r[7]["type"], "message");
        assert_eq!(r[7]["role"], "assistant");

        // === Chat Completions (OpenAI legacy pattern) ===
        let chat = items_to_chat_messages(&items, None, None, None, None);
        // user → assistant(tool_calls) → tool → assistant → user → assistant(tool_calls) → tool → assistant
        assert!(
            chat.len() >= 4,
            "Chat Completions: should have role-based messages"
        );
        assert_eq!(chat[0].role, "user");
        // Function calls become assistant with tool_calls
        assert_eq!(chat[1].role, "assistant");
        assert_eq!(chat[1].tool_calls.len(), 1);
        assert_eq!(chat[1].tool_calls[0].function.name, "Bash");
        // Function output becomes role:"tool"
        assert_eq!(chat[2].role, "tool");
        assert_eq!(chat[2].tool_call_id.as_deref().unwrap(), "call_1");

        // === Anthropic Messages (CC pattern) ===
        let anthropic = items_to_anthropic_messages(&items);
        // Strict alternation: user, assistant(tool_use), user(tool_result), assistant, user, ...
        // Verify alternation
        for i in 1..anthropic.len() {
            assert_ne!(
                anthropic[i]["role"],
                anthropic[i - 1]["role"],
                "Anthropic: strict alternation violated at index {i}"
            );
        }
        // First must be user
        assert_eq!(anthropic[0]["role"], "user");
        // Find tool_use in assistant
        let assistant_with_tool = anthropic.iter().find(|m| {
            m["role"] == "assistant"
                && m["content"]
                    .as_array()
                    .map(|c| c.iter().any(|b| b["type"] == "tool_use"))
                    .unwrap_or(false)
        });
        assert!(
            assistant_with_tool.is_some(),
            "Anthropic: should have assistant with tool_use"
        );
        let tool_use_block = assistant_with_tool.unwrap()["content"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["type"] == "tool_use")
            .unwrap();
        assert_eq!(tool_use_block["name"], "Bash");
        assert_eq!(tool_use_block["id"], "call_1");

        // Find tool_result in user
        let user_with_result = anthropic.iter().find(|m| {
            m["role"] == "user"
                && m["content"]
                    .as_array()
                    .map(|c| c.iter().any(|b| b["type"] == "tool_result"))
                    .unwrap_or(false)
        });
        assert!(
            user_with_result.is_some(),
            "Anthropic: should have user with tool_result"
        );
        let tool_result_block = user_with_result.unwrap()["content"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["type"] == "tool_result")
            .unwrap();
        assert_eq!(tool_result_block["tool_use_id"], "call_1");

        // Find is_error propagation in Turn 2
        let error_result = anthropic
            .iter()
            .flat_map(|m| {
                m["content"]
                    .as_array()
                    .unwrap_or(&vec![])
                    .iter()
                    .filter(|b| b["type"] == "tool_result" && b["tool_use_id"] == "call_2")
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .next();
        assert!(
            error_result.is_some(),
            "Anthropic: should have tool_result for call_2"
        );
        assert_eq!(
            error_result.unwrap()["is_error"],
            true,
            "Anthropic: is_error should propagate"
        );
    }

    /// Dumps the actual wire-format JSON for visual inspection.
    /// Run with `cargo test -- dump_wire_format --nocapture` to see output.
    #[test]
    fn dump_wire_format_for_review() {
        let items = vec![
            ConversationItem::user_message("list files"),
            ConversationItem::FunctionCall {
                call_id: "call_1".into(),
                name: "Bash".into(),
                arguments: r#"{"command":"ls"}"#.into(),
            },
            ConversationItem::FunctionCallOutput {
                call_id: "call_1".into(),
                output: ToolOutputPayload::success("file1.rs\nfile2.rs".into()),
            },
            ConversationItem::assistant_message("I found 2 files."),
            ConversationItem::user_message("read the missing file"),
            ConversationItem::FunctionCall {
                call_id: "call_2".into(),
                name: "Read".into(),
                arguments: r#"{"path":"missing.rs"}"#.into(),
            },
            ConversationItem::FunctionCallOutput {
                call_id: "call_2".into(),
                output: ToolOutputPayload::error("file not found".into()),
            },
            ConversationItem::assistant_message("The file doesn't exist."),
        ];

        // 1. Responses API
        let responses = items_to_responses_input(&items);
        eprintln!("\n========== OpenAI Responses API (Codex-style) ==========");
        eprintln!("{}", serde_json::to_string_pretty(&responses).unwrap());

        // 2. Chat Completions
        let chat = items_to_chat_messages(&items, None, None, None, None);
        let chat_json: Vec<Value> = chat
            .iter()
            .map(|m| {
                let mut obj = json!({"role": m.role, "content": m.content});
                if !m.tool_calls.is_empty() {
                    obj["tool_calls"] = json!(m.tool_calls.iter().map(|tc| {
                        json!({"id": tc.id, "type": "function", "function": {"name": tc.function.name, "arguments": tc.function.arguments}})
                    }).collect::<Vec<_>>());
                }
                if let Some(ref id) = m.tool_call_id {
                    obj["tool_call_id"] = json!(id);
                }
                obj
            })
            .collect();
        eprintln!("\n========== OpenAI Chat Completions ==========");
        eprintln!("{}", serde_json::to_string_pretty(&chat_json).unwrap());

        // 3. Anthropic Messages
        let anthropic = items_to_anthropic_messages(&items);
        eprintln!("\n========== Anthropic Messages API (CC-style) ==========");
        eprintln!("{}", serde_json::to_string_pretty(&anthropic).unwrap());
    }

    /// Verifies that a reasoning item from the Responses API is captured and
    /// re-emitted verbatim in the next turn's `input` array.
    ///
    /// Without this, high-effort models re-think from scratch every turn,
    /// causing minutes-long delays per request on proxies that don't support
    /// server-side `previous_response_id` threading.
    #[test]
    fn reasoning_items_round_trip_through_conversation_history() {
        let raw_reasoning = json!({
            "type": "reasoning",
            "id": "rs_abc123",
            "summary": [
                {"type": "summary_text", "text": "Analyzing the task..."},
                {"type": "summary_text", "text": "Plan: read file, write solution."}
            ],
            "encrypted_content": "OPAQUE_BLOB_XYZ"
        });

        let item = reasoning_item_from_value(&raw_reasoning).expect("reasoning item parsed");
        let ConversationItem::Reasoning {
            summary,
            encrypted_content,
            redacted: _,
        } = &item
        else {
            panic!("expected Reasoning variant, got {item:?}");
        };
        assert_eq!(summary.len(), 2);
        assert!(matches!(
            &summary[0],
            ReasoningSummary::SummaryText { text } if text == "Analyzing the task..."
        ));
        assert_eq!(encrypted_content.as_deref(), Some("OPAQUE_BLOB_XYZ"));

        // Serialize back to Responses API wire format.
        let items = vec![item];
        let wire = items_to_responses_input(&items);
        let arr = wire.as_array().expect("wire input is array");
        assert_eq!(arr.len(), 1);
        let out = &arr[0];
        assert_eq!(out.get("type").and_then(Value::as_str), Some("reasoning"));
        assert_eq!(
            out.get("encrypted_content").and_then(Value::as_str),
            Some("OPAQUE_BLOB_XYZ")
        );
        let summary_arr = out.get("summary").and_then(Value::as_array).unwrap();
        assert_eq!(summary_arr.len(), 2);
        assert_eq!(
            summary_arr[0].get("type").and_then(Value::as_str),
            Some("summary_text")
        );
        assert_eq!(
            summary_arr[0].get("text").and_then(Value::as_str),
            Some("Analyzing the task...")
        );
    }

    /// Non-reasoning items return None from the extractor.
    #[test]
    fn reasoning_item_from_value_ignores_non_reasoning_items() {
        assert!(reasoning_item_from_value(&json!({
            "type": "message",
            "role": "assistant",
            "content": [{"type": "output_text", "text": "hi"}]
        }))
        .is_none());
        assert!(reasoning_item_from_value(&json!({
            "type": "function_call",
            "call_id": "c1",
            "name": "Read",
            "arguments": "{}"
        }))
        .is_none());
    }

    /// Reasoning items with empty summary AND missing encrypted_content are
    /// filtered out — avoids wasting tokens replaying useless payloads.
    #[test]
    fn reasoning_item_from_value_rejects_empty_payloads() {
        assert!(reasoning_item_from_value(&json!({
            "type": "reasoning",
            "summary": []
        }))
        .is_none());
        assert!(reasoning_item_from_value(&json!({
            "type": "reasoning"
        }))
        .is_none());
    }

    /// `append_reasoning_items` preserves the order of reasoning items
    /// relative to each other. Order matters because a later reasoning item
    /// may depend on an earlier function_call_output.
    #[test]
    fn append_reasoning_items_preserves_order() {
        let mut items: Vec<ConversationItem> = vec![ConversationItem::user_message("task")];
        let raw = vec![
            json!({
                "type": "reasoning",
                "summary": [{"type": "summary_text", "text": "first"}],
            }),
            json!({
                "type": "reasoning",
                "summary": [{"type": "summary_text", "text": "second"}],
            }),
        ];
        append_reasoning_items(&mut items, &raw);

        assert_eq!(items.len(), 3);
        let ConversationItem::Reasoning { summary: s1, .. } = &items[1] else {
            panic!("item[1] should be Reasoning");
        };
        let ConversationItem::Reasoning { summary: s2, .. } = &items[2] else {
            panic!("item[2] should be Reasoning");
        };
        assert!(matches!(&s1[0], ReasoningSummary::SummaryText { text } if text == "first"));
        assert!(matches!(&s2[0], ReasoningSummary::SummaryText { text } if text == "second"));
    }

    /// End-to-end check on a realistic multi-turn history: once reasoning is
    /// in the items vector, the next turn's Responses wire body must include
    /// both `reasoning` items AND function_call / function_call_output items
    /// in the correct order — mirroring what Codex sends.
    #[test]
    fn multi_turn_history_wire_format_includes_reasoning() {
        let items = vec![
            ConversationItem::user_message("do the thing"),
            ConversationItem::Reasoning {
                summary: vec![ReasoningSummary::SummaryText {
                    text: "thinking step 1".to_string(),
                }],
                encrypted_content: Some("BLOB1".to_string()),
                redacted: false,
            },
            ConversationItem::FunctionCall {
                call_id: "c1".to_string(),
                name: "Read".to_string(),
                arguments: "{\"file_path\":\"/x\"}".to_string(),
            },
            ConversationItem::FunctionCallOutput {
                call_id: "c1".to_string(),
                output: ToolOutputPayload::success("file body".to_string()),
            },
            ConversationItem::Reasoning {
                summary: vec![ReasoningSummary::SummaryText {
                    text: "thinking step 2".to_string(),
                }],
                encrypted_content: Some("BLOB2".to_string()),
                redacted: false,
            },
        ];
        let wire = items_to_responses_input(&items);
        let arr = wire.as_array().unwrap();
        let types: Vec<&str> = arr
            .iter()
            .filter_map(|v| v.get("type").and_then(Value::as_str))
            .collect();
        assert_eq!(
            types,
            vec![
                "message",
                "reasoning",
                "function_call",
                "function_call_output",
                "reasoning",
            ]
        );
        // Encrypted blobs are preserved verbatim — that's the whole point.
        assert_eq!(
            arr[1].get("encrypted_content").and_then(Value::as_str),
            Some("BLOB1")
        );
        assert_eq!(
            arr[4].get("encrypted_content").and_then(Value::as_str),
            Some("BLOB2")
        );
    }
}

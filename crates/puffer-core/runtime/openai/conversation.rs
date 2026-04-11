//! Unified conversation item type — Puffer's equivalent of Codex's `ResponseItem`.
//!
//! All internal logic operates on `Vec<ConversationItem>` using Responses API
//! semantics as the canonical format. The Chat Completions path is a thin
//! translation layer that converts at wire boundaries.

use crate::AppState;
use puffer_provider_openai::{
    OpenAIChatFunctionCall, OpenAIChatMessage, OpenAIChatToolCall, OpenAIRequestConfig,
    OpenAIResponseToolCall, OpenAIResponsesFunctionCallOutput,
};
use puffer_provider_registry::ProviderDescriptor;
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Core type
// ---------------------------------------------------------------------------

/// A single item in the conversation history.
///
/// Uses Responses API semantics as the canonical format (matching Codex's
/// `ResponseItem`). The Chat Completions path converts to/from this type at
/// the wire boundary.
#[derive(Debug, Clone)]
pub(crate) enum ConversationItem {
    /// A user, assistant, or system message.
    Message {
        role: String,
        content: String,
        /// Optional message ID (Responses API only).
        id: Option<String>,
        /// Optional status (e.g. "completed" for assistant messages).
        status: Option<String>,
    },
    /// A tool call requested by the model.
    FunctionCall {
        call_id: String,
        name: String,
        arguments: String,
    },
    /// Output from a tool execution.
    FunctionCallOutput { call_id: String, output: String },
}

impl ConversationItem {
    pub(crate) fn user_message(content: impl Into<String>) -> Self {
        Self::Message {
            role: "user".to_string(),
            content: content.into(),
            id: None,
            status: None,
        }
    }

    pub(crate) fn assistant_message(content: impl Into<String>) -> Self {
        Self::Message {
            role: "assistant".to_string(),
            content: content.into(),
            id: None,
            status: Some("completed".to_string()),
        }
    }

    pub(crate) fn system_message(content: impl Into<String>) -> Self {
        Self::Message {
            role: "system".to_string(),
            content: content.into(),
            id: None,
            status: None,
        }
    }

    /// Approximate token count with CJK-aware heuristic.
    ///
    /// ASCII characters average ~0.25 tokens each (4 chars/token).
    /// CJK / non-ASCII characters average ~1.5 tokens each.
    fn estimated_tokens(&self) -> usize {
        match self {
            Self::Message { content, .. } => estimate_text_tokens(content),
            Self::FunctionCall {
                arguments, name, ..
            } => estimate_text_tokens(name) + estimate_text_tokens(arguments),
            Self::FunctionCallOutput { output, .. } => estimate_text_tokens(output),
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
// Transcript → ConversationItem
// ---------------------------------------------------------------------------

/// Converts the in-memory transcript + current user input into canonical
/// conversation items. This replaces both `transcript_to_openai_input` and
/// `transcript_to_openai_chat_messages`.
pub(crate) fn transcript_to_items(state: &AppState, input: &str) -> Vec<ConversationItem> {
    let mut items: Vec<ConversationItem> = state
        .transcript
        .iter()
        .enumerate()
        .flat_map(|(index, message)| match message.role {
            crate::MessageRole::User => vec![ConversationItem::user_message(&message.text)],
            crate::MessageRole::Assistant => vec![ConversationItem::Message {
                role: "assistant".to_string(),
                content: message.text.clone(),
                id: Some(format!("msg_{index}")),
                status: Some("completed".to_string()),
            }],
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
                vec![ConversationItem::FunctionCallOutput {
                    call_id,
                    output: message.text.clone(),
                }]
            }
        })
        .collect();

    // Always append the current user input.
    if items.is_empty() || !input.trim().is_empty() {
        items.push(ConversationItem::user_message(input));
    }
    items
}

/// Appends tool call items and their outputs to the conversation.
/// This is the shared path for both Responses and Chat Completions after
/// tool execution completes.
pub(crate) fn append_tool_results(
    items: &mut Vec<ConversationItem>,
    tool_calls: &[OpenAIResponseToolCall],
    outputs: &[OpenAIResponsesFunctionCallOutput],
) {
    for tc in tool_calls {
        items.push(ConversationItem::FunctionCall {
            call_id: tc.call_id.clone(),
            name: tc.name.clone(),
            arguments: serde_json::to_string(&tc.arguments).unwrap_or_else(|_| "{}".to_string()),
        });
    }
    for out in outputs {
        items.push(ConversationItem::FunctionCallOutput {
            call_id: out.call_id.clone(),
            output: out.output.clone(),
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
    Value::Array(items.iter().map(item_to_responses_value).collect())
}

fn item_to_responses_value(item: &ConversationItem) -> Value {
    match item {
        ConversationItem::Message {
            role,
            content,
            id,
            status,
        } => {
            let content_block = if role == "assistant" {
                json!([{"type": "output_text", "text": content, "annotations": []}])
            } else if role == "system" {
                // System messages use plain string content in Responses API.
                json!(content)
            } else {
                json!([{"type": "input_text", "text": content}])
            };

            let mut val = json!({
                "type": "message",
                "role": role,
                "content": content_block,
            });
            if let Some(id) = id {
                val["id"] = json!(id);
            }
            if let Some(status) = status {
                val["status"] = json!(status);
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
            "output": output,
        }),
    }
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
    plan_mode_context: Option<&str>,
    system_reminder: Option<&str>,
) -> Vec<OpenAIChatMessage> {
    let mut messages = Vec::new();

    // System prompt as first message.
    if let Some(prompt) = system_prompt.filter(|p| !p.trim().is_empty()) {
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

    let mut i = 0;
    while i < items.len() {
        match &items[i] {
            ConversationItem::Message { role, content, .. } => {
                messages.push(chat_message(role, content));
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
                });
            }
            ConversationItem::FunctionCallOutput { call_id, output } => {
                messages.push(OpenAIChatMessage {
                    role: "tool".to_string(),
                    content: Some(json!(output)),
                    tool_call_id: Some(call_id.clone()),
                    tool_calls: Vec::new(),
                });
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
    }
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
/// Three-phase strategy (shared by both Responses and Chat paths):
/// 1. Snip old FunctionCallOutput items (keep last 6, truncate to 500 chars)
/// 2. Generate AI summary of old items via Responses API
/// 3. Fallback: drop oldest items
///
/// Returns `true` when Phase 2 or 3 occurred (caller should reset
/// `previous_response_id` for Responses path and inject post-compact context).
pub(crate) fn compact_conversation(
    items: &mut Vec<ConversationItem>,
    provider: &ProviderDescriptor,
    model_id: &str,
    request_config: &OpenAIRequestConfig,
    input_tokens_hint: Option<usize>,
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
                if output.len() > 500 {
                    let snipped: String = output.chars().take(500).collect();
                    *output = format!("{snipped}\n[...output snipped...]");
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
    // items that would produce invalid Chat Completions history.
    let keep_count = find_valid_keep_boundary(items, 4);
    let to_summarize = &items[..items.len() - keep_count];
    if !to_summarize.is_empty() {
        let old_context = build_summary_text(to_summarize);
        let summary = generate_summary(&old_context, model_id, provider, request_config);

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
    // preceding assistant message — these would produce invalid Chat Completions
    // history (tool messages without a corresponding assistant tool_calls message).
    while !items.is_empty() && !matches!(items.first(), Some(ConversationItem::Message { .. })) {
        items.remove(0);
    }

    if items.is_empty() || !matches!(items.first(), Some(ConversationItem::Message { .. })) {
        items.insert(
            0,
            ConversationItem::user_message("[Earlier context compacted]"),
        );
    }
    true
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
fn build_summary_text(items: &[ConversationItem]) -> String {
    let mut text = String::new();
    for item in items {
        match item {
            ConversationItem::Message { role, content, .. } => {
                let preview: String = content.chars().take(500).collect();
                text.push_str(&format!("[{role}]: {preview}\n\n"));
            }
            ConversationItem::FunctionCall {
                name, arguments, ..
            } => {
                let preview: String = arguments.chars().take(200).collect();
                text.push_str(&format!("[tool_call {name}]: {preview}\n\n"));
            }
            ConversationItem::FunctionCallOutput { output, .. } => {
                let preview: String = output.chars().take(300).collect();
                text.push_str(&format!("[tool_result]: {preview}\n\n"));
            }
        }
    }
    text
}

/// Generates an AI summary of old context via the Responses API.
fn generate_summary(
    old_context: &str,
    model_id: &str,
    _provider: &ProviderDescriptor,
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
        assert!(
            matches!(&items[0], ConversationItem::Message { role, content, .. }
            if role == "user" && content == "hello")
        );
    }

    #[test]
    fn transcript_to_items_with_history() {
        let mut state = test_state();
        state.push_message(crate::MessageRole::User, "first");
        state.push_message(crate::MessageRole::Assistant, "reply");
        let items = transcript_to_items(&state, "second");
        assert_eq!(items.len(), 3);
        assert!(matches!(&items[0], ConversationItem::Message { role, .. } if role == "user"));
        assert!(
            matches!(&items[1], ConversationItem::Message { role, id, .. }
            if role == "assistant" && id.is_some())
        );
        assert!(
            matches!(&items[2], ConversationItem::Message { role, content, .. }
            if role == "user" && content == "second")
        );
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
                output: "file.txt".into(),
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
                output: "content1".into(),
            },
            ConversationItem::FunctionCallOutput {
                call_id: "c2".into(),
                output: "content2".into(),
            },
        ];
        let msgs = items_to_chat_messages(&items, Some("sys prompt"), None, None);
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
        let tool_calls = vec![OpenAIResponseToolCall {
            item_id: None,
            status: None,
            call_id: "c1".into(),
            name: "Bash".into(),
            arguments: json!({"command": "ls"}),
        }];
        let outputs = vec![OpenAIResponsesFunctionCallOutput {
            kind: "function_call_output".into(),
            call_id: "c1".into(),
            output: "result".into(),
        }];
        append_tool_results(&mut items, &tool_calls, &outputs);
        assert_eq!(items.len(), 3);
        assert!(matches!(&items[1], ConversationItem::FunctionCall { name, .. } if name == "Bash"));
        assert!(
            matches!(&items[2], ConversationItem::FunctionCallOutput { output, .. } if output == "result")
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
        if let ConversationItem::Message { content, .. } = &items[1] {
            assert!(content.contains("Context restored after compaction"));
            assert!(content.contains("/work"));
        } else {
            panic!("expected Message at position 1");
        }
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
                output: "file.rs".into(),
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
                output: "x".repeat(2000),
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
                assert!(output.len() <= 600, "output should be snipped");
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
}

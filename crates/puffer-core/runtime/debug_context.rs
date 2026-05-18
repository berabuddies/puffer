use super::resolve_model_api;
use super::resolve_provider_and_model;
use super::structured_output_support::{
    anthropic_tool_definitions_for_request, openai_tool_definitions_for_request,
};
use super::system_prompt::render_runtime_system_prompt;
use crate::permissions::load_runtime_permission_context;
use crate::plan_mode::preview_plan_mode_context_message;
use crate::{AppState, MessageRole};
use anyhow::Result;
use puffer_provider_registry::ProviderRegistry;
use puffer_resources::LoadedResources;
use puffer_tools::ToolRegistry;
use std::fmt::Write as _;

/// Renders the full raw context that would be sent to the model.
///
/// Faithfully shows: system prompt, tool definitions, plan mode context,
/// and every conversation message with its role — no summarization, no
/// splitting, just the raw construction.
pub(crate) fn render_debug_context(
    state: &AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
) -> Result<String> {
    let (provider, model_id) = resolve_provider_and_model(state, providers)?;
    let api = resolve_model_api(state, providers, provider, &model_id);
    let permission_context = load_runtime_permission_context(&state.cwd, resources, state)?;
    let registry = ToolRegistry::from_resources(resources);
    let enabled_tools =
        crate::runtime::context_usage::enabled_tool_names(&api, &registry, &permission_context)?;
    let system_prompt = render_runtime_system_prompt(state, resources, &model_id, &enabled_tools)?;
    let plan_mode_context = preview_plan_mode_context_message(state, resources)?;

    let mut out = String::new();

    // ── Model / Provider ──
    let _ = writeln!(
        &mut out,
        "━━━ MODEL: {}/{} (api: {}) ━━━\n",
        provider.id, model_id, api
    );

    // ── System prompt ──
    let _ = writeln!(
        &mut out,
        "┌─── SYSTEM PROMPT ({} chars) ───",
        system_prompt.len()
    );
    let _ = writeln!(&mut out, "{}", system_prompt);
    let _ = writeln!(&mut out, "└─── END SYSTEM PROMPT ───\n");

    // ── Plan mode context ──
    if let Some(plan) = plan_mode_context.as_deref() {
        let _ = writeln!(
            &mut out,
            "┌─── PLAN MODE CONTEXT ({} chars) ───",
            plan.len()
        );
        let _ = writeln!(&mut out, "{}", plan);
        let _ = writeln!(&mut out, "└─── END PLAN MODE CONTEXT ───\n");
    }

    // ── Tool definitions ──
    let _ = writeln!(&mut out, "┌─── TOOLS ({}) ───", enabled_tools.len());
    if api == "anthropic-messages" {
        let definitions =
            anthropic_tool_definitions_for_request(&registry, None, Some(&permission_context))?;
        for def in &definitions {
            let pretty = serde_json::to_string_pretty(def).unwrap_or_else(|_| format!("{:?}", def));
            let _ = writeln!(&mut out, "{}", pretty);
        }
    } else {
        let definitions =
            openai_tool_definitions_for_request(&registry, None, false, Some(&permission_context))?;
        for def in &definitions {
            let pretty = serde_json::to_string_pretty(def).unwrap_or_else(|_| format!("{:?}", def));
            let _ = writeln!(&mut out, "{}", pretty);
        }
    }
    let _ = writeln!(&mut out, "└─── END TOOLS ───\n");

    // ── Conversation messages ──
    let _ = writeln!(
        &mut out,
        "┌─── CONVERSATION ({} messages) ───",
        state.transcript.len()
    );
    for (i, msg) in state.transcript.iter().enumerate() {
        let role = match msg.role {
            MessageRole::User => "USER",
            MessageRole::Assistant => "ASSISTANT",
            MessageRole::System => "SYSTEM",
            MessageRole::ToolCall => "TOOL_CALL",
            MessageRole::ToolResult => "TOOL_RESULT",
        };
        let tool_info = match msg.role {
            MessageRole::ToolCall | MessageRole::ToolResult => {
                let tool_id = msg.tool_id.as_deref().unwrap_or("");
                let call_id = msg.call_id.as_deref().unwrap_or("");
                format!(" [tool={}, call={}]", tool_id, call_id)
            }
            _ => String::new(),
        };
        let _ = writeln!(&mut out, "\n── [{}] {}{} ──", i, role, tool_info);
        if let Some(thinking) = &msg.thinking {
            let _ = writeln!(&mut out, "<thinking>\n{}\n</thinking>", thinking);
        }
        let text = &msg.text;
        if text.len() > 2000 {
            let _ = writeln!(
                &mut out,
                "{}…\n  ({} chars total, truncated for display)",
                &text[..2000],
                text.len()
            );
        } else {
            let _ = writeln!(&mut out, "{}", text);
        }
        if let Some(input) = &msg.tool_input {
            if input.len() > 500 {
                let _ = writeln!(
                    &mut out,
                    "  input: {}… ({} chars)",
                    &input[..500],
                    input.len()
                );
            } else {
                let _ = writeln!(&mut out, "  input: {}", input);
            }
        }
    }
    let _ = writeln!(&mut out, "\n└─── END CONVERSATION ───");

    Ok(out)
}

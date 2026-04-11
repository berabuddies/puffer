use super::resolve_model_api;
use super::resolve_provider_and_model;
use super::structured_output_support::{
    anthropic_tool_definitions_for_request, openai_tool_definitions_for_request,
};
use super::system_prompt::render_runtime_system_prompt;
use crate::permissions::load_runtime_permission_context;
use crate::plan_mode::preview_plan_mode_context_message;
use crate::{AppState, MessageRole, RenderedMessage};
use anyhow::Result;
use puffer_provider_registry::ProviderRegistry;
use puffer_resources::LoadedResources;
use puffer_tools::ToolRegistry;
use serde_json::json;
use std::collections::BTreeSet;
use std::fmt::Write as _;

/// Renders a Claude-style `/context` summary for the active provider/model.
pub(crate) fn render_context_usage_summary(
    state: &AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
) -> Result<String> {
    let (provider, model_id) = resolve_provider_and_model(state, providers)?;
    let api = resolve_model_api(state, providers, provider, &model_id);
    let model_selector = format!("{}/{}", provider.id, model_id);
    let context_window = provider
        .models
        .iter()
        .find(|model| model.id == model_id)
        .map(|model| model.context_window)
        .unwrap_or(0);

    let permission_context = load_runtime_permission_context(&state.cwd, resources, state)?;
    let registry = ToolRegistry::from_resources(resources);
    let (tool_rows, enabled_tools) = tool_rows_for_summary(&api, &registry, &permission_context)?;
    let system_prompt = render_runtime_system_prompt(state, resources, &model_id, &enabled_tools)?;
    let plan_mode_context = preview_plan_mode_context_message(state, resources)?;
    let system_tokens = estimate_tokens(&system_prompt)
        + plan_mode_context
            .as_deref()
            .map(estimate_tokens)
            .unwrap_or_default();
    let conversation_rows = conversation_rows(state, &api);
    let conversation_tokens = conversation_rows.iter().map(|row| row.tokens).sum::<u32>();
    let tool_tokens = tool_rows.iter().map(|row| row.tokens).sum::<u32>();
    let total_tokens = conversation_tokens
        .saturating_add(system_tokens)
        .saturating_add(tool_tokens);
    let percent = percentage(total_tokens, context_window);

    let mut text = String::new();
    let _ = writeln!(&mut text, "## Context Usage");
    let _ = writeln!(&mut text);
    let _ = writeln!(&mut text, "**Model:** {model_selector}  ");
    let _ = writeln!(
        &mut text,
        "**Tokens:** {} / {} ({percent}%)",
        format_count(total_tokens),
        format_count(context_window),
    );
    let _ = writeln!(&mut text);
    let _ = writeln!(&mut text, "### Estimated usage by category");
    let _ = writeln!(&mut text);
    let _ = writeln!(&mut text, "| Category | Tokens | Percentage |");
    let _ = writeln!(&mut text, "|----------|--------|------------|");
    write_category_row(
        &mut text,
        "Conversation",
        conversation_tokens,
        context_window,
    );
    write_category_row(&mut text, "System prompt", system_tokens, context_window);
    write_category_row(&mut text, "Tools", tool_tokens, context_window);
    write_category_row(
        &mut text,
        "Free space",
        context_window.saturating_sub(total_tokens),
        context_window,
    );

    let _ = writeln!(&mut text);
    let _ = writeln!(&mut text, "### Conversation");
    let _ = writeln!(&mut text);
    let _ = writeln!(&mut text, "| Role | Messages | Tokens |");
    let _ = writeln!(&mut text, "|------|----------|--------|");
    for row in &conversation_rows {
        let _ = writeln!(
            &mut text,
            "| {} | {} | {} |",
            row.label,
            row.count,
            format_count(row.tokens)
        );
    }

    let _ = writeln!(&mut text);
    let _ = writeln!(&mut text, "### System prompt");
    let _ = writeln!(&mut text);
    let _ = writeln!(
        &mut text,
        "- Base prompt: {} tokens",
        format_count(estimate_tokens(&system_prompt))
    );
    if let Some(plan_mode_context) = plan_mode_context.as_deref() {
        let _ = writeln!(
            &mut text,
            "- Plan mode context: {} tokens",
            format_count(estimate_tokens(plan_mode_context))
        );
    }

    let _ = writeln!(&mut text);
    let _ = writeln!(&mut text, "### Tools");
    let _ = writeln!(&mut text);
    let _ = writeln!(&mut text, "| Tool | Tokens |");
    let _ = writeln!(&mut text, "|------|--------|");
    if tool_rows.is_empty() {
        let _ = writeln!(&mut text, "| <none> | 0 |");
    } else {
        for row in &tool_rows {
            let _ = writeln!(
                &mut text,
                "| {} | {} |",
                row.label,
                format_count(row.tokens)
            );
        }
    }

    Ok(text.trim_end().to_string())
}

#[derive(Debug, Clone)]
struct UsageRow {
    label: String,
    count: usize,
    tokens: u32,
}

fn conversation_rows(state: &AppState, api: &str) -> Vec<UsageRow> {
    [
        ("User", MessageRole::User),
        ("Assistant", MessageRole::Assistant),
        ("System", MessageRole::System),
    ]
    .into_iter()
    .map(|(label, role)| {
        let messages = state
            .transcript
            .iter()
            .enumerate()
            .filter(|(_, message)| message.role == role)
            .collect::<Vec<_>>();
        let tokens = messages
            .iter()
            .map(|(index, message)| {
                estimate_tokens(&provider_message_payload(message, api, *index))
            })
            .sum();
        UsageRow {
            label: label.to_string(),
            count: messages.len(),
            tokens,
        }
    })
    .collect()
}

fn provider_message_payload(message: &RenderedMessage, api: &str, index: usize) -> String {
    match api {
        "anthropic-messages" => serde_json::to_string(&json!({
            "role": match message.role {
                MessageRole::Assistant => "assistant",
                MessageRole::User
                | MessageRole::System
                | MessageRole::ToolCall
                | MessageRole::ToolResult => "user",
            },
            "content": match message.role {
                MessageRole::System
                | MessageRole::ToolCall
                | MessageRole::ToolResult => format!("[system]\n{}", message.text),
                _ => message.text.clone(),
            },
        }))
        .unwrap_or_default(),
        "openai-completions" => serde_json::to_string(&json!({
            "role": match message.role {
                MessageRole::User => "user",
                MessageRole::Assistant => "assistant",
                MessageRole::System
                | MessageRole::ToolCall
                | MessageRole::ToolResult => "system",
            },
            "content": message.text,
        }))
        .unwrap_or_default(),
        _ => serde_json::to_string(&match message.role {
            MessageRole::User => json!({
                "role": "user",
                "content": [
                    {
                        "type": "input_text",
                        "text": message.text,
                    }
                ],
            }),
            MessageRole::Assistant => json!({
                "type": "message",
                "role": "assistant",
                "content": [
                    {
                        "type": "output_text",
                        "text": message.text,
                        "annotations": [],
                    }
                ],
                "status": "completed",
                "id": format!("msg_{index}"),
            }),
            MessageRole::System
            | MessageRole::ToolCall
            | MessageRole::ToolResult => json!({
                "role": "system",
                "content": message.text,
            }),
        })
        .unwrap_or_default(),
    }
}

fn tool_rows_for_summary(
    api: &str,
    registry: &ToolRegistry,
    permission_context: &crate::permissions::RuntimePermissionContext,
) -> Result<(Vec<UsageRow>, BTreeSet<String>)> {
    if api == "anthropic-messages" {
        let definitions =
            anthropic_tool_definitions_for_request(registry, None, Some(permission_context), None)?;
        let rows = definitions
            .into_iter()
            .filter_map(|definition| {
                let name = definition.get("name").and_then(|value| value.as_str())?;
                Some(UsageRow {
                    label: name.to_string(),
                    count: 1,
                    tokens: estimate_tokens(&definition.to_string()),
                })
            })
            .collect::<Vec<_>>();
        let enabled_tools = rows
            .iter()
            .map(|row| row.label.clone())
            .collect::<BTreeSet<_>>();
        return Ok((rows, enabled_tools));
    }

    let use_native = false;
    let definitions = openai_tool_definitions_for_request(
        registry,
        None,
        use_native,
        Some(permission_context),
        None,
    )?;
    let rows = definitions
        .into_iter()
        .map(|definition| UsageRow {
            label: definition.name.clone(),
            count: 1,
            tokens: estimate_tokens(&serde_json::to_string(&definition).unwrap_or_default()),
        })
        .collect::<Vec<_>>();
    let enabled_tools = rows
        .iter()
        .map(|row| row.label.clone())
        .collect::<BTreeSet<_>>();
    Ok((rows, enabled_tools))
}

fn write_category_row(text: &mut String, label: &str, tokens: u32, context_window: u32) {
    let _ = writeln!(
        text,
        "| {label} | {} | {}% |",
        format_count(tokens),
        percentage(tokens, context_window)
    );
}

fn estimate_tokens(text: &str) -> u32 {
    let chars = text.chars().count() as u32;
    if chars == 0 {
        0
    } else {
        (chars + 3) / 4
    }
}

fn percentage(value: u32, total: u32) -> String {
    if total == 0 {
        return "0.0".to_string();
    }
    format!("{:.1}", (value as f64 / total as f64) * 100.0)
}

fn format_count(value: u32) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + (digits.len() / 3));
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

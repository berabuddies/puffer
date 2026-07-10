use anyhow::Result;
use puffer_provider_registry::ModelDescriptor;
use puffer_resources::LoadedResources;
use std::collections::BTreeSet;

use super::conversation::{ContentPart, ConversationItem};
use crate::permissions::RuntimePermissionContext;
use crate::runtime::system_prompt::{
    load_openai_project_memory_context, render_openai_codex_contextual_user_prompt,
    render_runtime_prompt_resource,
};
use crate::AppState;

const OPENAI_CODEX_BASE_PROMPT_ID: &str = "openai-codex-base";
const OPENAI_CODEX_DEVELOPER_PROMPT_ID: &str = "openai-codex-developer";

pub(super) struct CodexPromptLayers {
    pub instructions: String,
    pub developer_text: String,
    pub contextual_user_text: Option<String>,
}

pub(super) fn build_codex_prompt_layers(
    state: &AppState,
    resources: &LoadedResources,
    model: &ModelDescriptor,
    enabled_tools: &BTreeSet<String>,
    permission_context: &RuntimePermissionContext,
) -> Result<CodexPromptLayers> {
    let instructions = render_runtime_prompt_resource(
        state,
        resources,
        &model.id,
        enabled_tools,
        OPENAI_CODEX_BASE_PROMPT_ID,
        false,
    )?;
    let developer_text = render_runtime_prompt_resource(
        state,
        resources,
        &model.id,
        enabled_tools,
        OPENAI_CODEX_DEVELOPER_PROMPT_ID,
        false,
    )?;
    let mut contextual_user_text = render_openai_codex_contextual_user_prompt(
        state,
        resources,
        &model.id,
        enabled_tools,
        permission_context,
    )?;
    if let Some(project_memory) = load_openai_project_memory_context(&state.cwd) {
        append_section(&mut contextual_user_text, &project_memory);
    }

    Ok(CodexPromptLayers {
        instructions,
        developer_text,
        contextual_user_text: non_empty(contextual_user_text),
    })
}

pub(super) fn developer_message(content: impl Into<String>) -> ConversationItem {
    ConversationItem::Message {
        role: "developer".to_string(),
        content: vec![ContentPart::Text {
            text: content.into(),
        }],
    }
}

pub(super) fn append_section(target: &mut String, section: &str) {
    let section = section.trim();
    if section.is_empty() {
        return;
    }
    if !target.trim().is_empty() {
        target.push_str("\n\n");
    }
    target.push_str(section);
}

fn non_empty(text: String) -> Option<String> {
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

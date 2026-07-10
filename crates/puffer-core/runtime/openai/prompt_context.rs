use anyhow::Result;
use puffer_provider_openai::OpenAIResponsesTextConfig;
use puffer_provider_registry::{
    ModelCompat, ModelDescriptor, OpenAiResponsesCompat, ProviderDescriptor,
};
use puffer_resources::LoadedResources;
use std::collections::BTreeSet;

use super::conversation::{ContentPart, ConversationItem};
use crate::permissions::RuntimePermissionContext;
use crate::runtime::system_prompt::{
    load_openai_project_memory_context, render_openai_runtime_base_system_prompt,
};
use crate::runtime::TurnRequestOptions;
use crate::AppState;
use serde_json::{json, Value};

pub(super) struct OpenAiPromptBundle {
    pub instructions: String,
    pub developer_items: Vec<ConversationItem>,
    pub contextual_user_items: Vec<ConversationItem>,
    pub leading_input_items: Vec<ConversationItem>,
}

pub(super) fn build_openai_responses_prompt_bundle(
    state: &AppState,
    resources: &LoadedResources,
    _provider: &ProviderDescriptor,
    model: &ModelDescriptor,
    enabled_tools: &BTreeSet<String>,
    permission_context: &RuntimePermissionContext,
    options: &TurnRequestOptions<'_>,
) -> Result<OpenAiPromptBundle> {
    if options.lightweight_context {
        return Ok(OpenAiPromptBundle {
            instructions: "Reply directly and concisely.".to_string(),
            developer_items: Vec::new(),
            contextual_user_items: Vec::new(),
            leading_input_items: Vec::new(),
        });
    }

    if uses_codex_prompt_style(model) {
        return build_codex_responses_prompt_bundle(
            state,
            resources,
            model,
            enabled_tools,
            permission_context,
        );
    }

    let mut instructions = render_openai_runtime_base_system_prompt(
        state,
        resources,
        &model.id,
        enabled_tools,
        permission_context,
    )?;
    let mut leading_input_items = Vec::new();
    if let Some(project_memory) = load_openai_project_memory_context(&state.cwd) {
        if supports_contextual_user_messages(model) {
            leading_input_items.push(ConversationItem::user_message(project_memory));
        } else {
            instructions.push_str("\n\n");
            instructions.push_str(&project_memory);
        }
    }
    Ok(OpenAiPromptBundle {
        instructions,
        developer_items: Vec::new(),
        contextual_user_items: Vec::new(),
        leading_input_items,
    })
}

fn build_codex_responses_prompt_bundle(
    state: &AppState,
    resources: &LoadedResources,
    model: &ModelDescriptor,
    enabled_tools: &BTreeSet<String>,
    permission_context: &RuntimePermissionContext,
) -> Result<OpenAiPromptBundle> {
    let layers = super::codex_prompt::build_codex_prompt_layers(
        state,
        resources,
        model,
        enabled_tools,
        permission_context,
    )?;
    let mut instructions = layers.instructions;
    let mut developer_items = Vec::new();
    let mut contextual_user_items = Vec::new();

    if supports_developer_messages(model) {
        if !layers.developer_text.trim().is_empty() {
            developer_items.push(super::codex_prompt::developer_message(
                layers.developer_text,
            ));
        }
    } else {
        append_compat_instruction_section(
            &mut instructions,
            "# Runtime Developer Context",
            &layers.developer_text,
        );
    }

    if let Some(contextual_user_text) = layers.contextual_user_text {
        if supports_contextual_user_messages(model) {
            contextual_user_items.push(ConversationItem::user_message(contextual_user_text));
        } else {
            append_compat_instruction_section(
                &mut instructions,
                "# Contextual User Information",
                &contextual_user_text,
            );
        }
    }

    let leading_input_items = developer_items
        .iter()
        .chain(contextual_user_items.iter())
        .cloned()
        .collect();

    Ok(OpenAiPromptBundle {
        instructions,
        developer_items,
        contextual_user_items,
        leading_input_items,
    })
}

fn append_compat_instruction_section(instructions: &mut String, heading: &str, text: &str) {
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    if !instructions.trim().is_empty() {
        instructions.push_str("\n\n");
    }
    instructions.push_str(heading);
    instructions.push_str("\n\n");
    instructions.push_str(text);
}

pub(super) fn insert_leading_input_items(
    items: &mut Vec<ConversationItem>,
    leading_input_items: &[ConversationItem],
) {
    if leading_input_items.is_empty() {
        return;
    }
    let insert_pos = items
        .iter()
        .take_while(
            |item| matches!(item, ConversationItem::Message { role, .. } if role == "system"),
        )
        .count();
    items.splice(insert_pos..insert_pos, leading_input_items.iter().cloned());
}

pub(super) fn apply_managed_system_prompt_to_bundle(
    bundle: &mut OpenAiPromptBundle,
    model: &ModelDescriptor,
    prompt: Option<&str>,
) {
    let Some(prompt) = prompt.map(str::trim).filter(|prompt| !prompt.is_empty()) else {
        return;
    };

    if uses_codex_prompt_style(model) && supports_developer_messages(model) {
        if let Some(item) = bundle.developer_items.first_mut() {
            append_text_to_message(item, prompt);
        } else {
            bundle
                .developer_items
                .push(super::codex_prompt::developer_message(prompt));
        }
        bundle.leading_input_items = bundle
            .developer_items
            .iter()
            .chain(bundle.contextual_user_items.iter())
            .cloned()
            .collect();
        return;
    }

    if uses_codex_prompt_style(model) {
        append_compat_instruction_section(
            &mut bundle.instructions,
            "# Managed Developer Context",
            prompt,
        );
    } else {
        super::conversation::append_managed_system_prompt_1_to_instructions(
            &mut bundle.instructions,
            Some(prompt),
        );
    }
}

pub(super) fn apply_plan_mode_context_to_bundle(
    bundle: &mut OpenAiPromptBundle,
    model: &ModelDescriptor,
    prompt: Option<&str>,
) {
    let Some(prompt) = prompt.map(str::trim).filter(|prompt| !prompt.is_empty()) else {
        return;
    };

    if uses_codex_prompt_style(model) && supports_contextual_user_messages(model) {
        let item = ConversationItem::user_message(prompt.to_string());
        bundle.contextual_user_items.push(item.clone());
        bundle.leading_input_items.push(item);
        return;
    }

    append_compat_instruction_section(&mut bundle.instructions, "# Plan Mode Context", prompt);
}

fn append_text_to_message(item: &mut ConversationItem, text: &str) {
    if let ConversationItem::Message { content, .. } = item {
        if let Some(ContentPart::Text { text: existing }) = content
            .iter_mut()
            .find(|part| matches!(part, ContentPart::Text { .. }))
        {
            if !existing.trim().is_empty() {
                existing.push_str("\n\n");
            }
            existing.push_str(text);
            return;
        }
        content.push(ContentPart::Text {
            text: text.to_string(),
        });
    }
}

pub(super) fn apply_text_verbosity_compat(
    mut text: Option<OpenAIResponsesTextConfig>,
    model: &ModelDescriptor,
) -> Option<OpenAIResponsesTextConfig> {
    let Some(compat) = responses_compat(model) else {
        return text;
    };
    if compat.supports_text_verbosity != Some(true) {
        return text;
    }
    let Some(default_verbosity) = compat.default_verbosity.as_ref() else {
        return text;
    };
    let config = text.get_or_insert_with(OpenAIResponsesTextConfig::default);
    if config.verbosity.is_none() {
        config.verbosity = Some(default_verbosity.clone());
    }
    text
}

pub(super) fn supports_client_metadata(model: &ModelDescriptor) -> bool {
    responses_compat(model)
        .and_then(|compat| compat.supports_client_metadata)
        .unwrap_or(false)
}

pub(super) fn supports_parallel_tool_calls(model: &ModelDescriptor) -> bool {
    responses_compat(model)
        .and_then(|compat| compat.supports_parallel_tool_calls)
        .unwrap_or(true)
}

pub(super) fn apply_request_wire_compat(
    body: &mut Value,
    state: &AppState,
    supports_client_metadata: bool,
    supports_parallel_tool_calls: bool,
) {
    if supports_client_metadata {
        body["client_metadata"] = json!({
            "session_id": state.session.id.to_string(),
            "cwd": state.cwd.display().to_string(),
        });
    }
    if !supports_parallel_tool_calls {
        body.as_object_mut()
            .map(|object| object.remove("parallel_tool_calls"));
    }
}

fn responses_compat(model: &ModelDescriptor) -> Option<&OpenAiResponsesCompat> {
    model
        .compat
        .as_ref()
        .and_then(ModelCompat::as_openai_responses)
}

fn uses_codex_prompt_style(model: &ModelDescriptor) -> bool {
    responses_compat(model).and_then(|compat| compat.prompt_style.as_deref()) == Some("codex")
}

fn supports_developer_messages(model: &ModelDescriptor) -> bool {
    responses_compat(model)
        .and_then(|compat| compat.supports_developer_messages)
        .unwrap_or(false)
}

fn supports_contextual_user_messages(model: &ModelDescriptor) -> bool {
    responses_compat(model)
        .and_then(|compat| compat.supports_contextual_user_messages)
        .unwrap_or_else(|| {
            responses_compat(model).and_then(|compat| compat.prompt_style.as_deref())
                == Some("codex")
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::{
        load_runtime_permission_context_with_inputs, RuntimePermissionInputs,
    };
    use crate::runtime::tests::{bundled_resources, state};
    use puffer_provider_registry::{Modality, ModelCompat, OpenAiResponsesCompat};
    use puffer_resources::render_prompt_for;
    use std::ffi::OsString;
    use std::{env, fs};

    #[test]
    fn codex_prompt_bundle_keeps_project_memory_out_of_instructions() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("AGENTS.md"), "PROJECT_RULE_MARKER").unwrap();
        fs::write(tmp.path().join("CLAUDE.md"), "CLAUDE_RULE_MARKER").unwrap();
        let mut state = state();
        state.cwd = tmp.path().to_path_buf();
        state.session.cwd = tmp.path().to_path_buf();
        state.current_provider = Some("openai".to_string());

        let resources = bundled_resources();
        let provider = resources
            .providers
            .iter()
            .find(|provider| provider.value.id == "openai")
            .map(|provider| provider.value.clone().into_descriptor())
            .unwrap();
        let mut model = provider
            .models
            .iter()
            .find(|model| model.id == "gpt-5.5")
            .cloned()
            .unwrap_or_else(|| {
                let mut model = provider.models[0].clone();
                model.id = "gpt-5.5".to_string();
                model.display_name = "GPT-5.5".to_string();
                model.provider = "openai".to_string();
                model.api = "openai-responses".to_string();
                model.input = vec![Modality::Text];
                model.compat = Some(ModelCompat::OpenAiResponses(OpenAiResponsesCompat {
                    prompt_style: Some("codex".to_string()),
                    supports_contextual_user_messages: Some(true),
                    ..OpenAiResponsesCompat::default()
                }));
                model
            });
        model.compat = Some(ModelCompat::OpenAiResponses(OpenAiResponsesCompat {
            prompt_style: Some("codex".to_string()),
            supports_contextual_user_messages: Some(true),
            ..OpenAiResponsesCompat::default()
        }));
        let permission_context = load_runtime_permission_context_with_inputs(
            &state.cwd,
            &resources,
            &state,
            RuntimePermissionInputs::default(),
        )
        .unwrap();

        let bundle = build_openai_responses_prompt_bundle(
            &state,
            &resources,
            &provider,
            &model,
            &BTreeSet::new(),
            &permission_context,
            &TurnRequestOptions::default(),
        )
        .unwrap();

        assert!(!bundle.instructions.contains("PROJECT_RULE_MARKER"));
        assert!(!bundle.instructions.contains("CLAUDE_RULE_MARKER"));

        let leading_text = bundle
            .leading_input_items
            .iter()
            .filter_map(ConversationItem::text_content)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(leading_text.contains("# AGENTS.md instructions for"));
        assert!(leading_text.contains("PROJECT_RULE_MARKER"));
        assert!(!leading_text.contains("CLAUDE_RULE_MARKER"));
    }

    #[test]
    fn codex_prompt_bundle_includes_agents_from_root_to_cwd() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        let nested = project.join("crates/puffer-core");
        fs::create_dir_all(&nested).unwrap();
        fs::write(project.join("AGENTS.md"), "ROOT_AGENTS_MARKER").unwrap();
        fs::write(nested.join("AGENTS.md"), "NESTED_AGENTS_MARKER").unwrap();
        let mut state = state();
        state.cwd = nested.clone();
        state.session.cwd = nested;
        state.current_provider = Some("openai".to_string());

        let bundle = codex_prompt_bundle_for_state(
            &state,
            OpenAiResponsesCompat {
                prompt_style: Some("codex".to_string()),
                supports_developer_messages: Some(true),
                supports_contextual_user_messages: Some(true),
                ..OpenAiResponsesCompat::default()
            },
        );

        let leading_text = bundle
            .leading_input_items
            .iter()
            .filter_map(ConversationItem::text_content)
            .collect::<Vec<_>>()
            .join("\n");
        let root_pos = leading_text.find("ROOT_AGENTS_MARKER").unwrap();
        let nested_pos = leading_text.find("NESTED_AGENTS_MARKER").unwrap();
        assert!(root_pos < nested_pos);
    }

    #[test]
    fn codex_prompt_bundle_uses_claude_fallback_only_without_agents() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        let nested = project.join("src");
        fs::create_dir_all(&nested).unwrap();
        fs::write(project.join("AGENTS.md"), "ROOT_AGENTS_MARKER").unwrap();
        fs::write(nested.join("CLAUDE.md"), "CLAUDE_RULE_MARKER").unwrap();
        let mut state = state();
        state.cwd = nested.clone();
        state.session.cwd = nested;
        state.current_provider = Some("openai".to_string());

        let bundle = codex_prompt_bundle_for_state(
            &state,
            OpenAiResponsesCompat {
                prompt_style: Some("codex".to_string()),
                supports_developer_messages: Some(true),
                supports_contextual_user_messages: Some(true),
                ..OpenAiResponsesCompat::default()
            },
        );

        let leading_text = bundle
            .leading_input_items
            .iter()
            .filter_map(ConversationItem::text_content)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(leading_text.contains("ROOT_AGENTS_MARKER"));
        assert!(!leading_text.contains("CLAUDE_RULE_MARKER"));
    }

    #[test]
    fn codex_prompt_bundle_uses_xml_environment_context() {
        let mut state = state();
        state.current_provider = Some("openai".to_string());

        let bundle = codex_prompt_bundle_for_state(
            &state,
            OpenAiResponsesCompat {
                prompt_style: Some("codex".to_string()),
                supports_developer_messages: Some(true),
                supports_contextual_user_messages: Some(true),
                ..OpenAiResponsesCompat::default()
            },
        );

        let leading_text = bundle
            .leading_input_items
            .iter()
            .filter_map(ConversationItem::text_content)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(leading_text.contains("<environment_context>"));
        assert!(leading_text.contains("<cwd>"));
        assert!(leading_text.contains("<filesystem>"));
        assert!(leading_text.contains("</environment_context>"));
        assert!(!leading_text.contains("# Environment"));
        assert!(!leading_text.contains("Primary working directory:"));
    }

    #[test]
    fn non_codex_openai_prompt_bundle_uses_xml_environment_context() {
        let mut state = state();
        state.current_provider = Some("openai".to_string());
        let resources = bundled_resources();
        let provider = resources
            .providers
            .iter()
            .find(|provider| provider.value.id == "openai")
            .map(|provider| provider.value.clone().into_descriptor())
            .unwrap();
        let model = codex_model_with_compat(OpenAiResponsesCompat {
            prompt_style: None,
            supports_developer_messages: Some(false),
            supports_contextual_user_messages: Some(false),
            ..OpenAiResponsesCompat::default()
        });
        let permission_context = load_runtime_permission_context_with_inputs(
            &state.cwd,
            &resources,
            &state,
            RuntimePermissionInputs::default(),
        )
        .unwrap();

        let bundle = build_openai_responses_prompt_bundle(
            &state,
            &resources,
            &provider,
            &model,
            &BTreeSet::new(),
            &permission_context,
            &TurnRequestOptions::default(),
        )
        .unwrap();

        assert!(bundle.instructions.contains("<environment_context>"));
        assert!(bundle.instructions.contains("<cwd>"));
        assert!(bundle.instructions.contains("<filesystem>"));
        assert!(!bundle.instructions.contains("Primary working directory:"));
    }

    #[test]
    fn codex_plan_mode_context_uses_contextual_user_channel() {
        let model = codex_model_with_compat(OpenAiResponsesCompat {
            prompt_style: Some("codex".to_string()),
            supports_developer_messages: Some(true),
            supports_contextual_user_messages: Some(true),
            ..OpenAiResponsesCompat::default()
        });
        let mut bundle = OpenAiPromptBundle {
            instructions: "base instructions".to_string(),
            developer_items: Vec::new(),
            contextual_user_items: Vec::new(),
            leading_input_items: Vec::new(),
        };

        apply_plan_mode_context_to_bundle(&mut bundle, &model, Some("PLAN_MODE_MARKER"));

        assert!(!bundle.instructions.contains("PLAN_MODE_MARKER"));
        let leading_text = bundle
            .leading_input_items
            .iter()
            .filter_map(ConversationItem::text_content)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(leading_text.contains("PLAN_MODE_MARKER"));
    }

    #[test]
    fn codex_plan_mode_context_falls_back_to_instructions_when_contextual_user_unsupported() {
        let model = codex_model_with_compat(OpenAiResponsesCompat {
            prompt_style: Some("codex".to_string()),
            supports_developer_messages: Some(true),
            supports_contextual_user_messages: Some(false),
            ..OpenAiResponsesCompat::default()
        });
        let mut bundle = OpenAiPromptBundle {
            instructions: "base instructions".to_string(),
            developer_items: Vec::new(),
            contextual_user_items: Vec::new(),
            leading_input_items: Vec::new(),
        };

        apply_plan_mode_context_to_bundle(&mut bundle, &model, Some("PLAN_MODE_MARKER"));

        assert!(bundle.instructions.contains("PLAN_MODE_MARKER"));
        assert!(bundle.leading_input_items.is_empty());
    }

    #[test]
    fn codex_prompt_bundle_emits_developer_item_when_supported() {
        let state = state();
        let resources = bundled_resources();
        let provider = resources
            .providers
            .iter()
            .find(|provider| provider.value.id == "openai")
            .map(|provider| provider.value.clone().into_descriptor())
            .unwrap();
        let model = codex_model_with_compat(OpenAiResponsesCompat {
            prompt_style: Some("codex".to_string()),
            supports_developer_messages: Some(true),
            supports_contextual_user_messages: Some(true),
            ..OpenAiResponsesCompat::default()
        });
        let permission_context = load_runtime_permission_context_with_inputs(
            &state.cwd,
            &resources,
            &state,
            RuntimePermissionInputs::default(),
        )
        .unwrap();

        let bundle = build_openai_responses_prompt_bundle(
            &state,
            &resources,
            &provider,
            &model,
            &BTreeSet::new(),
            &permission_context,
            &TurnRequestOptions::default(),
        )
        .unwrap();

        assert!(!bundle.developer_items.is_empty());
        assert!(matches!(
            &bundle.leading_input_items[0],
            ConversationItem::Message { role, .. } if role == "developer"
        ));
    }

    #[test]
    fn codex_base_prompt_uses_current_resource() {
        let mut state = state();
        state.current_provider = Some("openai".to_string());
        let resources = bundled_resources();
        let provider = resources
            .providers
            .iter()
            .find(|provider| provider.value.id == "openai")
            .map(|provider| provider.value.clone().into_descriptor())
            .unwrap();
        let model = codex_model_with_compat(OpenAiResponsesCompat {
            prompt_style: Some("codex".to_string()),
            supports_developer_messages: Some(true),
            supports_contextual_user_messages: Some(true),
            ..OpenAiResponsesCompat::default()
        });
        let permission_context = load_runtime_permission_context_with_inputs(
            &state.cwd,
            &resources,
            &state,
            RuntimePermissionInputs::default(),
        )
        .unwrap();

        let bundle = build_openai_responses_prompt_bundle(
            &state,
            &resources,
            &provider,
            &model,
            &BTreeSet::new(),
            &permission_context,
            &TurnRequestOptions::default(),
        )
        .unwrap();

        let expected = normalize_prompt_for_test(
            &render_prompt_for(
                &resources,
                "openai-codex-base",
                state.current_provider.as_deref(),
                Some(&model.id),
                &Default::default(),
            )
            .expect("bundled openai-codex-base resource"),
        );

        assert_eq!(bundle.instructions, expected);
        assert!(!bundle.instructions.contains("<environment_context>"));
        assert!(!bundle.instructions.contains("AGENTS.md instructions for"));
    }

    #[test]
    fn codex_prompt_bundle_includes_global_agents_context() {
        let _guard = crate::test_locks::env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let project = tmp.path().join("project");
        fs::create_dir_all(home.join(".puffer")).unwrap();
        fs::create_dir_all(&project).unwrap();
        fs::write(home.join(".puffer/AGENTS.md"), "GLOBAL_AGENTS_MARKER").unwrap();
        let _home = ScopedEnvVar::set("HOME", home.as_os_str());
        let mut state = state();
        state.cwd = project.clone();
        state.session.cwd = project;
        state.current_provider = Some("openai".to_string());

        let bundle = codex_prompt_bundle_for_state(
            &state,
            OpenAiResponsesCompat {
                prompt_style: Some("codex".to_string()),
                supports_developer_messages: Some(true),
                supports_contextual_user_messages: Some(true),
                ..OpenAiResponsesCompat::default()
            },
        );

        let leading_text = bundle
            .leading_input_items
            .iter()
            .filter_map(ConversationItem::text_content)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(leading_text.contains("GLOBAL_AGENTS_MARKER"));
        assert!(leading_text.contains("# AGENTS.md instructions for"));
    }

    #[test]
    fn codex_prompt_bundle_includes_global_claude_fallback_without_agents() {
        let _guard = crate::test_locks::env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let project = tmp.path().join("project");
        fs::create_dir_all(home.join(".claude")).unwrap();
        fs::create_dir_all(&project).unwrap();
        fs::write(home.join(".claude/CLAUDE.md"), "GLOBAL_CLAUDE_MARKER").unwrap();
        let _home = ScopedEnvVar::set("HOME", home.as_os_str());
        let mut state = state();
        state.cwd = project.clone();
        state.session.cwd = project;
        state.current_provider = Some("openai".to_string());

        let bundle = codex_prompt_bundle_for_state(
            &state,
            OpenAiResponsesCompat {
                prompt_style: Some("codex".to_string()),
                supports_developer_messages: Some(true),
                supports_contextual_user_messages: Some(true),
                ..OpenAiResponsesCompat::default()
            },
        );

        let leading_text = bundle
            .leading_input_items
            .iter()
            .filter_map(ConversationItem::text_content)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(leading_text.contains("GLOBAL_CLAUDE_MARKER"));
        assert!(leading_text.contains("# CLAUDE.md instructions for"));
    }

    struct ScopedEnvVar {
        name: &'static str,
        old_value: Option<OsString>,
    }

    impl ScopedEnvVar {
        fn set(name: &'static str, value: &std::ffi::OsStr) -> Self {
            let old_value = env::var_os(name);
            env::set_var(name, value);
            Self { name, old_value }
        }
    }

    impl Drop for ScopedEnvVar {
        fn drop(&mut self) {
            if let Some(value) = self.old_value.take() {
                env::set_var(self.name, value);
            } else {
                env::remove_var(self.name);
            }
        }
    }

    fn normalize_prompt_for_test(rendered: &str) -> String {
        let mut lines = Vec::new();
        let mut blank_run = 0usize;
        for line in rendered.lines() {
            if line.trim().is_empty() {
                blank_run += 1;
                if blank_run > 1 {
                    continue;
                }
                lines.push(String::new());
                continue;
            }
            blank_run = 0;
            lines.push(line.trim_end().to_string());
        }
        lines.join("\n").trim().to_string()
    }

    #[test]
    fn codex_prompt_bundle_folds_developer_text_when_unsupported() {
        let state = state();
        let resources = bundled_resources();
        let provider = resources
            .providers
            .iter()
            .find(|provider| provider.value.id == "openai")
            .map(|provider| provider.value.clone().into_descriptor())
            .unwrap();
        let model = codex_model_with_compat(OpenAiResponsesCompat {
            prompt_style: Some("codex".to_string()),
            supports_developer_messages: Some(false),
            supports_contextual_user_messages: Some(true),
            ..OpenAiResponsesCompat::default()
        });
        let permission_context = load_runtime_permission_context_with_inputs(
            &state.cwd,
            &resources,
            &state,
            RuntimePermissionInputs::default(),
        )
        .unwrap();

        let bundle = build_openai_responses_prompt_bundle(
            &state,
            &resources,
            &provider,
            &model,
            &BTreeSet::new(),
            &permission_context,
            &TurnRequestOptions::default(),
        )
        .unwrap();

        assert!(bundle.developer_items.is_empty());
        assert!(bundle.instructions.contains("# Runtime Developer Context"));
    }

    #[test]
    fn codex_prompt_bundle_folds_project_memory_when_contextual_user_unsupported() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("AGENTS.md"), "PROJECT_RULE_MARKER").unwrap();
        let mut state = state();
        state.cwd = tmp.path().to_path_buf();
        state.session.cwd = tmp.path().to_path_buf();
        let resources = bundled_resources();
        let provider = resources
            .providers
            .iter()
            .find(|provider| provider.value.id == "openai")
            .map(|provider| provider.value.clone().into_descriptor())
            .unwrap();
        let model = codex_model_with_compat(OpenAiResponsesCompat {
            prompt_style: Some("codex".to_string()),
            supports_developer_messages: Some(true),
            supports_contextual_user_messages: Some(false),
            ..OpenAiResponsesCompat::default()
        });
        let permission_context = load_runtime_permission_context_with_inputs(
            &state.cwd,
            &resources,
            &state,
            RuntimePermissionInputs::default(),
        )
        .unwrap();

        let bundle = build_openai_responses_prompt_bundle(
            &state,
            &resources,
            &provider,
            &model,
            &BTreeSet::new(),
            &permission_context,
            &TurnRequestOptions::default(),
        )
        .unwrap();

        assert!(bundle.contextual_user_items.is_empty());
        assert!(bundle.instructions.contains("PROJECT_RULE_MARKER"));
    }

    #[test]
    fn non_codex_prompt_bundle_preserves_current_prompt_shape() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("AGENTS.md"), "PROJECT_RULE_MARKER").unwrap();
        let mut state = state();
        state.cwd = tmp.path().to_path_buf();
        state.session.cwd = tmp.path().to_path_buf();
        let resources = bundled_resources();
        let provider = resources
            .providers
            .iter()
            .find(|provider| provider.value.id == "openai")
            .map(|provider| provider.value.clone().into_descriptor())
            .unwrap();
        let model = ModelDescriptor {
            id: "gpt-5.4".to_string(),
            display_name: "GPT-5.4".to_string(),
            provider: "openai".to_string(),
            api: "openai-responses".to_string(),
            context_window: 1,
            max_output_tokens: 1,
            supports_reasoning: false,
            compat: Some(ModelCompat::OpenAiResponses(
                OpenAiResponsesCompat::default(),
            )),
            input: vec![Modality::Text],
            cost: None,
        };
        let permission_context = load_runtime_permission_context_with_inputs(
            &state.cwd,
            &resources,
            &state,
            RuntimePermissionInputs::default(),
        )
        .unwrap();

        let bundle = build_openai_responses_prompt_bundle(
            &state,
            &resources,
            &provider,
            &model,
            &BTreeSet::new(),
            &permission_context,
            &TurnRequestOptions::default(),
        )
        .unwrap();

        assert!(bundle.developer_items.is_empty());
        assert!(bundle.contextual_user_items.is_empty());
        assert!(bundle.leading_input_items.is_empty());
        assert!(bundle.instructions.contains("PROJECT_RULE_MARKER"));
    }

    fn codex_model_with_compat(compat: OpenAiResponsesCompat) -> ModelDescriptor {
        ModelDescriptor {
            id: "gpt-5.5".to_string(),
            display_name: "GPT-5.5".to_string(),
            provider: "openai".to_string(),
            api: "openai-responses".to_string(),
            context_window: 1,
            max_output_tokens: 1,
            supports_reasoning: false,
            compat: Some(ModelCompat::OpenAiResponses(compat)),
            input: vec![Modality::Text],
            cost: None,
        }
    }

    fn codex_prompt_bundle_for_state(
        state: &AppState,
        compat: OpenAiResponsesCompat,
    ) -> OpenAiPromptBundle {
        let resources = bundled_resources();
        let provider = resources
            .providers
            .iter()
            .find(|provider| provider.value.id == "openai")
            .map(|provider| provider.value.clone().into_descriptor())
            .unwrap();
        let model = codex_model_with_compat(compat);
        let permission_context = load_runtime_permission_context_with_inputs(
            &state.cwd,
            &resources,
            state,
            RuntimePermissionInputs::default(),
        )
        .unwrap();

        build_openai_responses_prompt_bundle(
            state,
            &resources,
            &provider,
            &model,
            &BTreeSet::new(),
            &permission_context,
            &TurnRequestOptions::default(),
        )
        .unwrap()
    }

    #[test]
    fn text_verbosity_requires_explicit_compat_support() {
        let mut model = ModelDescriptor {
            id: "custom".to_string(),
            display_name: "custom".to_string(),
            provider: "custom".to_string(),
            api: "openai-responses".to_string(),
            context_window: 1,
            max_output_tokens: 1,
            supports_reasoning: false,
            compat: Some(ModelCompat::OpenAiResponses(OpenAiResponsesCompat {
                default_verbosity: Some("low".to_string()),
                ..OpenAiResponsesCompat::default()
            })),
            input: vec![Modality::Text],
            cost: None,
        };

        assert!(apply_text_verbosity_compat(None, &model).is_none());

        model.compat = Some(ModelCompat::OpenAiResponses(OpenAiResponsesCompat {
            supports_text_verbosity: Some(true),
            default_verbosity: Some("low".to_string()),
            ..OpenAiResponsesCompat::default()
        }));
        let text = apply_text_verbosity_compat(None, &model).unwrap();
        assert_eq!(text.verbosity.as_deref(), Some("low"));
    }

    #[test]
    fn wire_compat_gates_client_metadata_and_parallel_tool_calls() {
        let state = state();
        let mut body = serde_json::json!({
            "parallel_tool_calls": true
        });

        apply_request_wire_compat(&mut body, &state, false, false);

        assert!(body.get("client_metadata").is_none());
        assert!(body.get("parallel_tool_calls").is_none());

        apply_request_wire_compat(&mut body, &state, true, true);
        assert_eq!(
            body["client_metadata"]["session_id"].as_str(),
            Some("00000000-0000-0000-0000-000000000000")
        );
    }
}

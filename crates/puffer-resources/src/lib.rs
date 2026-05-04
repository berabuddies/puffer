mod loader;
mod model;

use std::collections::BTreeSet;

pub use loader::{
    agent_by_id, hook_by_id, load_resources, plugin_by_id, plugin_lsp_servers, plugin_mcp_servers,
    prompt_by_id, prompt_for, skill_by_name,
};
pub use model::{
    AgentMcpServerSpec, AgentMemoryScope, AgentSpec, HookSpec, IdeSpec, LoadedItem,
    LoadedResources, LspServerSpec, MascotSpec, McpOAuthDetail, McpOAuthSpec, McpServerSpec,
    PluginCommandSpec, PluginSpec, PromptTemplate, PromptVariableSpec, ProviderPack, SkillSpec,
    SourceInfo, SourceKind, ToolDisplaySpec, ToolMetadataSpec, ToolSpec,
};

/// Looks up a mascot by id.
pub fn mascot_by_id<'a>(resources: &'a LoadedResources, id: &str) -> Option<&'a MascotSpec> {
    resources
        .mascots
        .iter()
        .find(|mascot| mascot.value.id == id)
        .map(|mascot| &mascot.value)
}

/// Returns all loaded hooks matching the requested event name.
pub fn hooks_for_event<'a>(
    resources: &'a LoadedResources,
    event: &str,
) -> Vec<&'a LoadedItem<HookSpec>> {
    resources
        .hooks
        .iter()
        .filter(|hook| hook.value.event == event)
        .collect()
}

/// Renders a prompt template by id, including any chained parent prompts.
///
/// Equivalent to [`render_prompt_for`] with no provider/model context — base
/// prompts are always selected regardless of which variants are loaded.
pub fn render_prompt_by_id(
    resources: &LoadedResources,
    id: &str,
    variables: &std::collections::BTreeMap<String, String>,
) -> Option<String> {
    render_prompt_for(resources, id, None, None, variables)
}

/// Renders a prompt template by id with provider/model-specific override fallback.
///
/// When both `provider` and `model` are `None`, behaves identically to
/// [`render_prompt_by_id`]. Each chained parent is resolved with the same
/// provider/model context, so a base chain can pull in model-specific variants.
pub fn render_prompt_for(
    resources: &LoadedResources,
    id: &str,
    provider: Option<&str>,
    model: Option<&str>,
    variables: &std::collections::BTreeMap<String, String>,
) -> Option<String> {
    let mut visited = BTreeSet::new();
    let mut sections = Vec::new();
    append_prompt_sections(
        resources,
        id,
        provider,
        model,
        variables,
        &mut visited,
        &mut sections,
    );
    (!sections.is_empty()).then(|| sections.join("\n\n"))
}

fn append_prompt_sections(
    resources: &LoadedResources,
    id: &str,
    provider: Option<&str>,
    model: Option<&str>,
    variables: &std::collections::BTreeMap<String, String>,
    visited: &mut BTreeSet<String>,
    sections: &mut Vec<String>,
) {
    if !visited.insert(id.to_string()) {
        return;
    }
    let Some(prompt) = prompt_for(resources, id, provider, model) else {
        return;
    };
    for chained in &prompt.value.chained_from {
        append_prompt_sections(
            resources, chained, provider, model, variables, visited, sections,
        );
    }
    sections.push(prompt.value.render(variables));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LoadedItem, PromptTemplate, SourceInfo, SourceKind};
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    #[test]
    fn render_prompt_by_id_includes_chained_templates_and_variables() {
        let resources = LoadedResources {
            prompts: vec![
                LoadedItem {
                    value: PromptTemplate {
                        id: "base".to_string(),
                        description: "Base".to_string(),
                        template: "Base $ARGUMENTS".to_string(),
                        variables: vec![PromptVariableSpec {
                            name: "ARGUMENTS".to_string(),
                            description: String::new(),
                            required: false,
                            default: None,
                        }],
                        allowed_tools: Vec::new(),
                        provider_override: None,
                        model_override: None,
                        mode: None,
                        chained_from: Vec::new(),
                        for_provider: None,
                        for_model: None,
                    },
                    source_info: SourceInfo {
                        path: PathBuf::from("base.yaml"),
                        kind: SourceKind::Builtin,
                    },
                },
                LoadedItem {
                    value: PromptTemplate {
                        id: "review".to_string(),
                        description: "Review".to_string(),
                        template: "Review $CWD".to_string(),
                        variables: vec![PromptVariableSpec {
                            name: "CWD".to_string(),
                            description: String::new(),
                            required: false,
                            default: None,
                        }],
                        allowed_tools: Vec::new(),
                        provider_override: None,
                        model_override: None,
                        mode: Some("review".to_string()),
                        chained_from: vec!["base".to_string()],
                        for_provider: None,
                        for_model: None,
                    },
                    source_info: SourceInfo {
                        path: PathBuf::from("review.yaml"),
                        kind: SourceKind::Builtin,
                    },
                },
            ],
            ..LoadedResources::default()
        };
        let rendered = render_prompt_by_id(
            &resources,
            "review",
            &BTreeMap::from([
                ("ARGUMENTS".to_string(), "now".to_string()),
                ("CWD".to_string(), "/tmp/work".to_string()),
            ]),
        )
        .expect("rendered prompt");
        assert!(rendered.contains("Base now"));
        assert!(rendered.contains("Review /tmp/work"));
    }

    fn make_prompt(
        id: &str,
        template: &str,
        for_model: Option<&str>,
    ) -> LoadedItem<PromptTemplate> {
        LoadedItem {
            value: PromptTemplate {
                id: id.to_string(),
                description: String::new(),
                template: template.to_string(),
                variables: Vec::new(),
                allowed_tools: Vec::new(),
                provider_override: None,
                model_override: None,
                mode: None,
                chained_from: Vec::new(),
                for_provider: None,
                for_model: for_model.map(str::to_string),
            },
            source_info: SourceInfo {
                path: PathBuf::from(format!("{id}.yaml")),
                kind: SourceKind::Builtin,
            },
        }
    }

    #[test]
    fn render_prompt_for_selects_model_override_when_available() {
        let resources = LoadedResources {
            prompts: vec![
                make_prompt("system-base", "base body", None),
                make_prompt("system-base", "override body", Some("claude-opus-4-6")),
            ],
            ..LoadedResources::default()
        };
        let rendered = render_prompt_for(
            &resources,
            "system-base",
            Some("anthropic"),
            Some("claude-opus-4-6"),
            &BTreeMap::new(),
        )
        .expect("rendered prompt");
        assert_eq!(rendered, "override body");
    }

    #[test]
    fn render_prompt_for_falls_back_to_base_when_model_does_not_match() {
        let resources = LoadedResources {
            prompts: vec![
                make_prompt("system-base", "base body", None),
                make_prompt("system-base", "override body", Some("claude-opus-4-6")),
            ],
            ..LoadedResources::default()
        };
        let rendered = render_prompt_for(
            &resources,
            "system-base",
            Some("openai"),
            Some("gpt-5"),
            &BTreeMap::new(),
        )
        .expect("rendered prompt");
        assert_eq!(rendered, "base body");
    }

    #[test]
    fn render_prompt_for_strips_provider_prefix_from_model_id() {
        let resources = LoadedResources {
            prompts: vec![
                make_prompt("system-base", "base body", None),
                make_prompt("system-base", "override body", Some("gpt-5")),
            ],
            ..LoadedResources::default()
        };
        let rendered = render_prompt_for(
            &resources,
            "system-base",
            None,
            Some("openai/gpt-5"),
            &BTreeMap::new(),
        )
        .expect("rendered prompt");
        assert_eq!(rendered, "override body");
    }

    #[test]
    fn prompt_by_id_returns_base_variant_ignoring_overrides() {
        let resources = LoadedResources {
            prompts: vec![
                make_prompt("system-base", "base body", None),
                make_prompt("system-base", "override body", Some("gpt-5")),
            ],
            ..LoadedResources::default()
        };
        let prompt = prompt_by_id(&resources, "system-base").expect("base prompt");
        assert_eq!(prompt.value.template, "base body");
    }
}

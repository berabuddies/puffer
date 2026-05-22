use puffer_config::{
    supported_config_settings, ConfigSettingScope, ConfigSettingSpec, ConfigSettingValueKind,
};
use puffer_resources::LoadedResources;
use std::collections::BTreeMap;

const CONFIG_PROMPT_HEADER: &str = "Get or set Claude Code configuration settings.\n\n\
View or change Claude Code settings. Use when the user requests configuration changes, asks about current settings, or when adjusting a setting would benefit them.\n\n\
## Usage\n\
- **Get current value:** Omit the \"value\" parameter\n\
- **Set new value:** Include the \"value\" parameter\n\n\
## Configurable settings list\n\
The following settings are available for you to change:\n\n";
const CONFIG_PROMPT_EXAMPLES: &str = "## Examples\n\
- Get theme: { \"setting\": \"theme\" }\n\
- Set theme: { \"setting\": \"theme\", \"value\": \"harbor\" }\n\
- Enable vim mode: { \"setting\": \"editorMode\", \"value\": \"vim\" }\n\
- Set copy behavior: { \"setting\": \"copy_full_response\", \"value\": true }\n\
- Change model: { \"setting\": \"model\", \"value\": \"openai/gpt-5\" }\n\
- Set OpenAI headers: { \"setting\": \"openaiHeaders\", \"value\": { \"x-test\": \"one\" } }\n\
- Set status line padding: { \"setting\": \"statusLinePadding\", \"value\": 2 }\n";

/// Renders the dynamic Config tool description from the shared settings catalog.
pub(crate) fn render_config_tool_description(resources: &LoadedResources) -> String {
    let global = render_settings_section(
        "Global Settings (stored in ~/.puffer/config.toml)",
        ConfigSettingScope::User,
    );
    let project = render_settings_section(
        "Project Settings (stored in .puffer/config.toml)",
        ConfigSettingScope::Workspace,
    );
    let session = render_settings_section(
        "Session Settings (apply to the current session only)",
        ConfigSettingScope::Session,
    );
    let model = render_model_section(resources);

    format!(
        "{CONFIG_PROMPT_HEADER}{global}\n\n{project}\n\n{session}\n\n{model}\n\n{examples}",
        examples = CONFIG_PROMPT_EXAMPLES.trim_end(),
    )
}

fn render_settings_section(heading: &str, scope: ConfigSettingScope) -> String {
    format!("### {heading}\n{}", render_settings_lines(scope))
}

fn render_settings_lines(scope: ConfigSettingScope) -> String {
    supported_config_settings()
        .iter()
        .filter(|spec| spec.scope == scope)
        .map(render_setting_line)
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_setting_line(spec: &ConfigSettingSpec) -> String {
    let mut line = format!("- {}", spec.canonical_key);
    let value_hint = render_value_hint(spec);
    if !value_hint.is_empty() {
        line.push_str(": ");
        line.push_str(&value_hint);
    }
    line.push_str(" - ");
    line.push_str(spec.description);
    if matches!(
        spec.value_kind,
        ConfigSettingValueKind::NullableString
            | ConfigSettingValueKind::StringMap
            | ConfigSettingValueKind::NullableUnsignedInteger
    ) {
        line.push_str(" Use null to clear.");
    }
    line
}

fn render_value_hint(spec: &ConfigSettingSpec) -> String {
    if !spec.options.is_empty() {
        return spec
            .options
            .iter()
            .map(|option| format!("\"{option}\""))
            .collect::<Vec<_>>()
            .join(", ");
    }
    match spec.value_kind {
        ConfigSettingValueKind::Boolean => "true/false".to_string(),
        ConfigSettingValueKind::StringMap => "{\"key\":\"value\"}".to_string(),
        ConfigSettingValueKind::NullableUnsignedInteger => "integer".to_string(),
        ConfigSettingValueKind::String | ConfigSettingValueKind::NullableString => String::new(),
    }
}

fn render_model_section(resources: &LoadedResources) -> String {
    let mut models = BTreeMap::new();
    for provider in &resources.providers {
        for model in &provider.value.models {
            models.insert(
                format!("{}/{}", model.provider, model.id),
                model.display_name.clone(),
            );
        }
    }

    if models.is_empty() {
        return "## Model\n- model - Override the default model (use a provider/model selector or null/\"default\")".to_string();
    }

    let mut text =
        "## Model\n- model - Override the default model. Available options:\n  - null/\"default\": Clear the model override\n".to_string();
    for (selector, description) in models {
        text.push_str(&format!("  - \"{selector}\": {description}\n"));
    }
    text.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_resources::{LoadedItem, LoadedResources, ProviderPack, SourceInfo, SourceKind};
    use std::fs;
    use std::path::PathBuf;

    fn provider(id: &str, display_name: &str, models_yaml: &str) -> LoadedItem<ProviderPack> {
        LoadedItem {
            value: serde_yaml::from_str::<ProviderPack>(&format!(
                "id: {id}\n\
display_name: {display_name}\n\
base_url: https://{id}.example.invalid\n\
default_api: openai-responses\n\
auth_modes:\n\
  - api_key\n\
models:\n{models_yaml}"
            ))
            .expect("parse provider"),
            source_info: SourceInfo {
                path: PathBuf::from(format!("{id}.yaml")),
                kind: SourceKind::Builtin,
            },
        }
    }

    #[test]
    fn config_prompt_lists_user_workspace_and_session_settings() {
        let resources = LoadedResources::default();
        let rendered = render_config_tool_description(&resources);
        assert!(rendered.contains("### Global Settings"));
        assert!(rendered.contains("### Project Settings"));
        assert!(rendered.contains("### Session Settings"));
        assert!(rendered.contains("- copy_full_response: true/false"));
        assert!(rendered.contains("- openai_headers: {\"key\":\"value\"}"));
        assert!(rendered.contains("- statuslineEnabled: true/false"));
    }

    #[test]
    fn config_prompt_lists_available_provider_models() {
        let resources = LoadedResources {
            providers: vec![
                provider(
                    "anthropic",
                    "Anthropic",
                    "  - provider: anthropic\n    id: claude-sonnet-4-5\n    display_name: Claude Sonnet 4.5\n    api: anthropic-messages\n    context_window: 200000\n    max_output_tokens: 8192\n    supports_reasoning: true\n",
                ),
                provider(
                    "openai",
                    "OpenAI",
                    "  - provider: openai\n    id: gpt-5\n    display_name: GPT-5\n    api: openai-responses\n    context_window: 272000\n    max_output_tokens: 16384\n    supports_reasoning: true\n",
                ),
            ],
            ..LoadedResources::default()
        };
        let rendered = render_config_tool_description(&resources);
        assert!(rendered.contains("\"anthropic/claude-sonnet-4-5\": Claude Sonnet 4.5"));
        assert!(rendered.contains("\"openai/gpt-5\": GPT-5"));
    }

    #[test]
    fn config_prompt_matches_claude_reference_scaffold_with_local_settings() {
        let reference_path =
            repo_root().join("references/claude-code/src/tools/ConfigTool/prompt.ts");
        if !reference_path.exists() {
            eprintln!(
                "skipping config prompt parity test; {} is absent",
                reference_path.display()
            );
            return;
        }
        let resources = LoadedResources {
            providers: vec![provider(
                "openai",
                "OpenAI",
                "  - provider: openai\n    id: gpt-5\n    display_name: GPT-5\n    api: openai-responses\n    context_window: 272000\n    max_output_tokens: 16384\n    supports_reasoning: true\n",
            )],
            ..LoadedResources::default()
        };
        let expected = reference_config_prompt(&resources);

        assert_eq!(render_config_tool_description(&resources), expected);
    }

    fn reference_config_prompt(resources: &LoadedResources) -> String {
        let reference = read_repo_file("references/claude-code/src/tools/ConfigTool/prompt.ts");
        normalize_reference_template(&extract_template_literal(&reference, "  return `"))
            .replace(
                "\n\n  View or change Claude Code settings.",
                "\n\nView or change Claude Code settings.",
            )
            .replace("\n\n\n## Usage", "\n\n## Usage")
            .replace(
                "### Global Settings (stored in ~/.claude.json)",
                "### Global Settings (stored in ~/.puffer/config.toml)",
            )
            .replace(
                "### Project Settings (stored in settings.json)",
                "### Project Settings (stored in .puffer/config.toml)",
            )
            .replace(
                "${globalSettings.join('\\n')}",
                &render_settings_lines(ConfigSettingScope::User),
            )
            .replace(
                "${projectSettings.join('\\n')}",
                &render_settings_lines(ConfigSettingScope::Workspace),
            )
            .replace(
                "${modelSection}",
                &format!(
                    "### Session Settings (apply to the current session only)\n{}\n\n{}",
                    render_settings_lines(ConfigSettingScope::Session),
                    render_model_section(resources)
                ),
            )
            .replace(
                "- Set dark theme: { \"setting\": \"theme\", \"value\": \"dark\" }",
                "- Set theme: { \"setting\": \"theme\", \"value\": \"harbor\" }",
            )
            .replace(
                "- Enable verbose: { \"setting\": \"verbose\", \"value\": true }",
                "- Set copy behavior: { \"setting\": \"copy_full_response\", \"value\": true }",
            )
            .replace(
                "- Change model: { \"setting\": \"model\", \"value\": \"opus\" }",
                "- Change model: { \"setting\": \"model\", \"value\": \"openai/gpt-5\" }",
            )
            .replace(
                "- Change permission mode: { \"setting\": \"permissions.defaultMode\", \"value\": \"plan\" }",
                "- Set OpenAI headers: { \"setting\": \"openaiHeaders\", \"value\": { \"x-test\": \"one\" } }\n- Set status line padding: { \"setting\": \"statusLinePadding\", \"value\": 2 }",
            )
            .replace(
                "- \"openai/gpt-5\": GPT-5\n## Examples",
                "- \"openai/gpt-5\": GPT-5\n\n## Examples",
            )
    }

    fn read_repo_file(relative_path: &str) -> String {
        fs::read_to_string(repo_root().join(relative_path)).unwrap()
    }

    fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap()
    }

    fn extract_template_literal(contents: &str, marker: &str) -> String {
        let start = contents.find(marker).unwrap() + marker.len();
        let source = &contents[start..];
        let mut end = None;
        let mut index = 0usize;
        let mut escaped = false;
        let mut interpolation_depth = 0usize;

        while index < source.len() {
            let ch = source[index..].chars().next().unwrap();
            let width = ch.len_utf8();
            if escaped {
                escaped = false;
                index += width;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                index += width;
                continue;
            }
            if interpolation_depth == 0 && ch == '`' {
                end = Some(start + index);
                break;
            }
            if source[index..].starts_with("${") {
                interpolation_depth += 1;
                index += 2;
                continue;
            }
            if interpolation_depth > 0 {
                match ch {
                    '{' => interpolation_depth += 1,
                    '}' => interpolation_depth = interpolation_depth.saturating_sub(1),
                    _ => {}
                }
            }
            index += width;
        }

        contents[start..end.unwrap()].to_string()
    }

    fn normalize_reference_template(raw: &str) -> String {
        let unescaped = raw.replace("\\`", "`");
        let trimmed = unescaped.strip_prefix('\n').unwrap_or(&unescaped);
        dedent(trimmed)
    }

    fn dedent(raw: &str) -> String {
        let indent = raw
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| line.chars().take_while(|ch| *ch == ' ').count())
            .min()
            .unwrap_or(0);
        raw.lines()
            .map(|line| line.strip_prefix(&" ".repeat(indent)).unwrap_or(line))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

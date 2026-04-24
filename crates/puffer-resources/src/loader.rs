use crate::model::{
    AgentSpec, HookSpec, IdeSpec, LoadedItem, LoadedResources, LspServerSpec, MascotSpec,
    McpServerSpec, PluginSpec, PromptTemplate, ProviderPack, SkillSpec, SourceInfo, SourceKind,
    ToolSpec,
};
use anyhow::{Context, Result};
use indexmap::IndexMap;
use puffer_config::ConfigPaths;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::Value;
use serde_yaml::{Mapping, Value as YamlValue};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Loads bundled, user, and workspace resources into one in-memory registry.
pub fn load_resources(paths: &ConfigPaths) -> Result<LoadedResources> {
    let mut loaded = LoadedResources::default();
    for (root, kind) in resource_roots(paths) {
        let plugins = load_yaml_dir::<PluginSpec>(&root.join("plugins"), kind)?;
        merge_by_id(
            &mut loaded.providers,
            load_yaml_dir::<ProviderPack>(&root.join("providers"), kind)?,
            |item| MergeKey::simple(item.value.id.clone()),
            "provider",
            &mut loaded.diagnostics,
        );
        merge_by_id(
            &mut loaded.tools,
            load_yaml_dir::<ToolSpec>(&root.join("tools"), kind)?,
            |item| MergeKey::simple(item.value.id.clone()),
            "tool",
            &mut loaded.diagnostics,
        );
        merge_by_id(
            &mut loaded.agents,
            load_yaml_dir::<AgentSpec>(&root.join("agents"), kind)?,
            |item| MergeKey::simple(item.value.id.clone()),
            "agent",
            &mut loaded.diagnostics,
        );
        merge_by_id(
            &mut loaded.prompts,
            load_yaml_dir::<PromptTemplate>(&root.join("prompts"), kind)?,
            |item| prompt_variant_key(&item.value),
            "prompt",
            &mut loaded.diagnostics,
        );
        merge_by_id(
            &mut loaded.hooks,
            load_yaml_dir::<HookSpec>(&root.join("hooks"), kind)?,
            |item| MergeKey::simple(item.value.id.clone()),
            "hook",
            &mut loaded.diagnostics,
        );
        merge_by_id(
            &mut loaded.skills,
            load_skill_dir(&root.join("skills"), kind)?,
            |item| MergeKey::simple(item.value.name.clone()),
            "skill",
            &mut loaded.diagnostics,
        );
        merge_by_id(
            &mut loaded.mascots,
            load_yaml_dir::<MascotSpec>(&root.join("mascots"), kind)?,
            |item| MergeKey::simple(item.value.id.clone()),
            "mascot",
            &mut loaded.diagnostics,
        );
        merge_by_id(
            &mut loaded.plugins,
            plugins.clone(),
            |item| MergeKey::simple(item.value.id.clone()),
            "plugin",
            &mut loaded.diagnostics,
        );
        merge_by_id(
            &mut loaded.agents,
            plugin_agent_specs(&plugins),
            |item| MergeKey::simple(item.value.id.clone()),
            "agent",
            &mut loaded.diagnostics,
        );
        merge_by_id(
            &mut loaded.mcp_servers,
            load_mcp_server_manifests(&root, kind)?,
            |item| MergeKey::simple(item.value.id.clone()),
            "mcp_server",
            &mut loaded.diagnostics,
        );
        merge_by_id(
            &mut loaded.ides,
            load_yaml_dir::<IdeSpec>(&root.join("ides"), kind)?,
            |item| MergeKey::simple(item.value.id.clone()),
            "ide",
            &mut loaded.diagnostics,
        );
    }
    Ok(loaded)
}

/// Looks up the base (no-variant) prompt template by id.
pub fn prompt_by_id<'a>(
    resources: &'a LoadedResources,
    id: &str,
) -> Option<&'a LoadedItem<PromptTemplate>> {
    resources.prompts.iter().find(|prompt| {
        prompt.value.id == id
            && prompt.value.for_provider.is_none()
            && prompt.value.for_model.is_none()
    })
}

/// Looks up a prompt template by id, preferring provider/model-specific variants.
///
/// Fallback order: (id + provider + model) → (id + model) → (id + provider) → (id, base).
/// `provider` and `model` are normalized by lowercasing; `model` additionally has any
/// `provider/` prefix stripped before matching.
pub fn prompt_for<'a>(
    resources: &'a LoadedResources,
    id: &str,
    provider: Option<&str>,
    model: Option<&str>,
) -> Option<&'a LoadedItem<PromptTemplate>> {
    let provider_norm = provider.map(|value| value.trim().to_ascii_lowercase());
    let model_norm = model.map(normalize_model_id);

    let candidates: [(Option<&str>, Option<&str>); 4] = [
        (provider_norm.as_deref(), model_norm.as_deref()),
        (None, model_norm.as_deref()),
        (provider_norm.as_deref(), None),
        (None, None),
    ];

    for (want_provider, want_model) in candidates {
        if let Some(found) = find_prompt_variant(resources, id, want_provider, want_model) {
            return Some(found);
        }
    }
    None
}

fn find_prompt_variant<'a>(
    resources: &'a LoadedResources,
    id: &str,
    want_provider: Option<&str>,
    want_model: Option<&str>,
) -> Option<&'a LoadedItem<PromptTemplate>> {
    resources.prompts.iter().find(|prompt| {
        prompt.value.id == id
            && prompt_field_matches(prompt.value.for_provider.as_deref(), want_provider)
            && prompt_field_matches(prompt.value.for_model.as_deref(), want_model)
    })
}

fn prompt_field_matches(field: Option<&str>, want: Option<&str>) -> bool {
    match (field, want) {
        (None, None) => true,
        (Some(lhs), Some(rhs)) => lhs.trim().eq_ignore_ascii_case(rhs),
        _ => false,
    }
}

fn normalize_model_id(raw: &str) -> String {
    let trimmed = raw.trim();
    let without_provider = trimmed.split_once('/').map(|(_, rest)| rest).unwrap_or(trimmed);
    without_provider.to_ascii_lowercase()
}

fn prompt_variant_key(template: &PromptTemplate) -> MergeKey {
    let dedup = format!(
        "{}|{}|{}",
        template.id,
        template.for_provider.as_deref().unwrap_or(""),
        template.for_model.as_deref().unwrap_or(""),
    );
    let mut display = template.id.clone();
    let qualifiers = [
        ("provider", template.for_provider.as_deref()),
        ("model", template.for_model.as_deref()),
    ]
    .into_iter()
    .filter_map(|(label, value)| value.map(|v| format!("{label}={v}")))
    .collect::<Vec<_>>();
    if !qualifiers.is_empty() {
        display = format!("{} ({})", display, qualifiers.join(", "));
    }
    MergeKey::new(dedup, display)
}

/// Looks up an agent definition by id.
pub fn agent_by_id<'a>(
    resources: &'a LoadedResources,
    id: &str,
) -> Option<&'a LoadedItem<AgentSpec>> {
    resources.agents.iter().find(|agent| agent.value.id == id)
}

/// Looks up a hook specification by id.
pub fn hook_by_id<'a>(
    resources: &'a LoadedResources,
    id: &str,
) -> Option<&'a LoadedItem<HookSpec>> {
    resources.hooks.iter().find(|hook| hook.value.id == id)
}

/// Looks up a skill specification by its stable name.
pub fn skill_by_name<'a>(
    resources: &'a LoadedResources,
    name: &str,
) -> Option<&'a LoadedItem<SkillSpec>> {
    let normalized = normalize_skill_name(name);
    resources
        .skills
        .iter()
        .find(|skill| skill.value.name == normalized)
}

/// Looks up a plugin manifest by id.
pub fn plugin_by_id<'a>(
    resources: &'a LoadedResources,
    plugin_id: &str,
) -> Option<&'a LoadedItem<PluginSpec>> {
    resources
        .plugins
        .iter()
        .find(|plugin| plugin.value.id == plugin_id)
}

/// Collects every MCP server declared by loaded plugins.
pub fn plugin_mcp_servers(resources: &LoadedResources) -> Vec<(&PluginSpec, &McpServerSpec)> {
    resources
        .plugins
        .iter()
        .flat_map(|plugin| {
            plugin
                .value
                .mcp_servers
                .iter()
                .map(move |server| (&plugin.value, server))
        })
        .collect()
}

/// Collects every LSP server declared by loaded plugins.
pub fn plugin_lsp_servers(resources: &LoadedResources) -> Vec<(&PluginSpec, &LspServerSpec)> {
    resources
        .plugins
        .iter()
        .flat_map(|plugin| {
            plugin
                .value
                .lsp_servers
                .iter()
                .map(move |server| (&plugin.value, server))
        })
        .collect()
}

fn plugin_agent_specs(plugins: &[LoadedItem<PluginSpec>]) -> Vec<LoadedItem<AgentSpec>> {
    plugins
        .iter()
        .flat_map(|plugin| {
            plugin.value.agents.iter().cloned().map(|agent| LoadedItem {
                value: agent,
                source_info: plugin.source_info.clone(),
            })
        })
        .collect()
}

fn resource_roots(paths: &ConfigPaths) -> Vec<(PathBuf, SourceKind)> {
    vec![
        (paths.builtin_resources_dir.clone(), SourceKind::Builtin),
        (paths.user_config_dir.join("resources"), SourceKind::User),
        (
            paths.workspace_config_dir.join("resources"),
            SourceKind::Workspace,
        ),
    ]
}

#[derive(Debug, Clone, Deserialize)]
struct McpManifestFile {
    #[serde(flatten)]
    server: McpServerSpec,
    #[serde(default = "default_mcp_enabled")]
    enabled: bool,
    #[serde(default, flatten)]
    _extra: BTreeMap<String, Value>,
}

fn default_mcp_enabled() -> bool {
    true
}

fn load_mcp_server_manifests(
    root: &Path,
    kind: SourceKind,
) -> Result<Vec<LoadedItem<McpServerSpec>>> {
    let canonical = load_mcp_manifest_dir(&root.join("mcp_servers"), kind)?;
    let canonical_ids = canonical
        .iter()
        .map(|item| item.value.id.clone())
        .collect::<std::collections::BTreeSet<_>>();

    let legacy = load_mcp_manifest_dir(&root.join("mcp"), kind)?
        .into_iter()
        .filter(|item| !canonical_ids.contains(&item.value.id))
        .collect::<Vec<_>>();

    let mut merged = legacy;
    merged.extend(canonical);
    Ok(merged)
}

fn load_mcp_manifest_dir(dir: &Path, kind: SourceKind) -> Result<Vec<LoadedItem<McpServerSpec>>> {
    Ok(load_yaml_dir::<McpManifestFile>(dir, kind)?
        .into_iter()
        .filter_map(|item| {
            item.value.enabled.then_some(LoadedItem {
                value: item.value.server,
                source_info: item.source_info,
            })
        })
        .collect())
}

fn load_yaml_dir<T>(dir: &Path, kind: SourceKind) -> Result<Vec<LoadedItem<T>>>
where
    T: DeserializeOwned,
{
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut items = Vec::new();
    for entry in sorted_dir_entries(dir)? {
        let path = entry.path();
        if !matches!(
            path.extension().and_then(|ext| ext.to_str()),
            Some("yaml" | "yml")
        ) {
            continue;
        }
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("failed to read resource file {}", path.display()))?;
        let value = serde_yaml::from_str::<T>(&raw)
            .with_context(|| format!("failed to parse resource file {}", path.display()))?;
        items.push(LoadedItem {
            value,
            source_info: SourceInfo { path, kind },
        });
    }
    Ok(items)
}

fn load_skill_dir(dir: &Path, kind: SourceKind) -> Result<Vec<LoadedItem<SkillSpec>>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut items = Vec::new();
    for entry in sorted_dir_entries(dir)? {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let skill_path = path.join("SKILL.md");
        if !skill_path.exists() {
            continue;
        }
        let raw = fs::read_to_string(&skill_path)
            .with_context(|| format!("failed to read skill file {}", skill_path.display()))?;
        let (frontmatter, body) = split_frontmatter(&raw).with_context(|| {
            format!("failed to parse skill frontmatter {}", skill_path.display())
        })?;
        let raw_name = frontmatter_string(&frontmatter, &["name"])
            .unwrap_or_else(|| path.file_name().unwrap().to_string_lossy().to_string());
        let name = normalize_skill_name(&raw_name);
        let description = frontmatter_string(&frontmatter, &["description"])
            .unwrap_or_else(|| first_descriptive_line(&body).to_string());
        let disable_model_invocation = frontmatter_bool(
            &frontmatter,
            &["disable-model-invocation", "disableModelInvocation"],
        )
        .unwrap_or(false);
        let allowed_tools =
            frontmatter_string_list(&frontmatter, &["allowed-tools", "allowedTools"]);
        let argument_hint = frontmatter_string(&frontmatter, &["argument-hint", "argumentHint"]);
        let argument_names =
            frontmatter_whitespace_list(&frontmatter, &["arguments", "argumentNames"]);
        let user_invocable =
            frontmatter_bool(&frontmatter, &["user-invocable", "userInvocable"]).unwrap_or(true);
        let model = frontmatter_string(&frontmatter, &["model"]);
        let effort = frontmatter_string(&frontmatter, &["effort"]);
        let context = frontmatter_string(&frontmatter, &["context"]);

        items.push(LoadedItem {
            value: SkillSpec {
                name,
                description,
                content: body,
                allowed_tools,
                argument_hint,
                argument_names,
                user_invocable,
                model,
                effort,
                context,
                disable_model_invocation,
            },
            source_info: SourceInfo {
                path: skill_path,
                kind,
            },
        });
    }
    Ok(items)
}

fn normalize_skill_name(raw: &str) -> String {
    let mut normalized = String::new();
    let mut last_was_dash = false;
    for ch in raw.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            normalized.push(ch.to_ascii_lowercase());
            last_was_dash = false;
        } else if !last_was_dash {
            normalized.push('-');
            last_was_dash = true;
        }
    }
    let trimmed = normalized.trim_matches('-');
    if trimmed.is_empty() {
        "skill".to_string()
    } else {
        trimmed.to_string()
    }
}

fn sorted_dir_entries(dir: &Path) -> Result<Vec<fs::DirEntry>> {
    let mut entries = fs::read_dir(dir)
        .with_context(|| format!("failed to read resource dir {}", dir.display()))?
        .collect::<std::io::Result<Vec<_>>>()
        .with_context(|| format!("failed to list resource dir {}", dir.display()))?;
    entries.sort_by(|left, right| left.path().cmp(&right.path()));
    Ok(entries)
}

fn split_frontmatter(raw: &str) -> Result<(Mapping, String)> {
    let normalized = raw.replace("\r\n", "\n");
    let mut lines = normalized.lines();
    if lines.next() != Some("---") {
        return Ok((Mapping::new(), normalized));
    }

    let mut frontmatter_raw = String::new();
    let mut offset = 4usize;
    for line in normalized.lines().skip(1) {
        offset += line.len() + 1;
        if line == "---" {
            break;
        }
        frontmatter_raw.push_str(line);
        frontmatter_raw.push('\n');
    }
    let frontmatter = if frontmatter_raw.trim().is_empty() {
        Mapping::new()
    } else {
        serde_yaml::from_str::<Mapping>(&frontmatter_raw)
            .context("invalid YAML frontmatter in skill")?
    };
    Ok((
        frontmatter,
        normalized
            .get(offset..)
            .map(str::trim_start)
            .unwrap_or_default()
            .to_string(),
    ))
}

fn frontmatter_string(frontmatter: &Mapping, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        frontmatter
            .get(YamlValue::String((*key).to_string()))
            .and_then(yaml_scalar_to_string)
    })
}

fn frontmatter_bool(frontmatter: &Mapping, keys: &[&str]) -> Option<bool> {
    keys.iter().find_map(|key| {
        let value = frontmatter.get(YamlValue::String((*key).to_string()))?;
        match value {
            YamlValue::Bool(flag) => Some(*flag),
            YamlValue::String(flag) => match flag.trim().to_ascii_lowercase().as_str() {
                "true" | "1" | "yes" | "on" => Some(true),
                "false" | "0" | "no" | "off" => Some(false),
                _ => None,
            },
            _ => None,
        }
    })
}

fn frontmatter_string_list(frontmatter: &Mapping, keys: &[&str]) -> Vec<String> {
    keys.iter()
        .find_map(|key| {
            let value = frontmatter.get(YamlValue::String((*key).to_string()))?;
            Some(match value {
                YamlValue::Sequence(values) => values
                    .iter()
                    .filter_map(yaml_scalar_to_string)
                    .flat_map(|value| {
                        value
                            .split(',')
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                            .map(str::to_string)
                            .collect::<Vec<_>>()
                    })
                    .collect(),
                YamlValue::String(values) => values
                    .split(',')
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
                    .collect(),
                other => yaml_scalar_to_string(other)
                    .into_iter()
                    .filter(|value| !value.is_empty())
                    .collect(),
            })
        })
        .unwrap_or_default()
}

fn frontmatter_whitespace_list(frontmatter: &Mapping, keys: &[&str]) -> Vec<String> {
    keys.iter()
        .find_map(|key| {
            let value = frontmatter.get(YamlValue::String((*key).to_string()))?;
            Some(match value {
                YamlValue::Sequence(values) => values
                    .iter()
                    .filter_map(yaml_scalar_to_string)
                    .flat_map(|value| {
                        value
                            .split_whitespace()
                            .map(str::to_string)
                            .collect::<Vec<_>>()
                    })
                    .collect(),
                YamlValue::String(values) => {
                    values.split_whitespace().map(str::to_string).collect()
                }
                other => yaml_scalar_to_string(other)
                    .into_iter()
                    .flat_map(|value| {
                        value
                            .split_whitespace()
                            .map(str::to_string)
                            .collect::<Vec<_>>()
                    })
                    .collect(),
            })
        })
        .unwrap_or_default()
}

fn yaml_scalar_to_string(value: &YamlValue) -> Option<String> {
    match value {
        YamlValue::String(text) => Some(text.clone()),
        YamlValue::Bool(flag) => Some(flag.to_string()),
        YamlValue::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

fn first_descriptive_line(raw: &str) -> &str {
    raw.lines()
        .find(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty() && !trimmed.starts_with('#')
        })
        .unwrap_or("Skill")
        .trim()
}

fn merge_by_id<T, F>(
    existing: &mut Vec<LoadedItem<T>>,
    incoming: Vec<LoadedItem<T>>,
    key: F,
    label: &str,
    diagnostics: &mut Vec<String>,
) where
    T: Clone,
    F: Fn(&LoadedItem<T>) -> MergeKey,
{
    let mut merged: IndexMap<String, (LoadedItem<T>, String)> = IndexMap::new();
    for item in existing.iter().cloned() {
        let MergeKey { dedup, display } = key(&item);
        merged.insert(dedup, (item, display));
    }
    for item in incoming {
        let MergeKey { dedup, display } = key(&item);
        if let Some((previous, _)) = merged.get(&dedup) {
            diagnostics.push(describe_override(
                label,
                &display,
                &previous.source_info,
                &item.source_info,
            ));
        }
        merged.insert(dedup, (item, display));
    }
    *existing = merged.into_values().map(|(item, _)| item).collect();
}

/// Pairs the dedup key used to merge resources with a user-friendly id for diagnostics.
struct MergeKey {
    dedup: String,
    display: String,
}

impl MergeKey {
    fn simple(id: impl Into<String>) -> Self {
        let value = id.into();
        Self {
            display: value.clone(),
            dedup: value,
        }
    }

    fn new(dedup: impl Into<String>, display: impl Into<String>) -> Self {
        Self {
            dedup: dedup.into(),
            display: display.into(),
        }
    }
}

fn describe_override(
    label: &str,
    id: &str,
    previous: &SourceInfo,
    incoming: &SourceInfo,
) -> String {
    if previous.kind == incoming.kind {
        return format!(
            "duplicate {label} `{id}` in {} resources: {} overrides {}",
            source_kind_label(incoming.kind),
            incoming.path.display(),
            previous.path.display()
        );
    }

    format!(
        "{} {label} `{id}` from {} overrides {} resource from {}",
        source_kind_label(incoming.kind),
        incoming.path.display(),
        source_kind_label(previous.kind),
        previous.path.display()
    )
}

fn source_kind_label(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::Builtin => "builtin",
        SourceKind::User => "user",
        SourceKind::Workspace => "workspace",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks_for_event;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn load_resources_reads_skill_markdown_and_plugin_yaml() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("workspace");
        let resources_dir = root.join("resources");
        fs::create_dir_all(resources_dir.join("agents")).unwrap();
        fs::create_dir_all(resources_dir.join("prompts")).unwrap();
        fs::create_dir_all(resources_dir.join("hooks")).unwrap();
        fs::create_dir_all(resources_dir.join("skills/reviewer")).unwrap();
        fs::create_dir_all(resources_dir.join("plugins")).unwrap();
        fs::write(
            resources_dir.join("agents/default.yaml"),
            "id: default\ndescription: Default agent\nprompt: You are the default agent.\n",
        )
        .unwrap();
        fs::write(
            resources_dir.join("prompts/plan.yaml"),
            "id: plan\ndescription: Plan\ntemplate: body\n",
        )
        .unwrap();
        fs::write(
            resources_dir.join("hooks/tool_end.yaml"),
            "id: tool-end\nevent: tool_end\ncommand: echo hook\n",
        )
        .unwrap();
        fs::write(
            resources_dir.join("skills/reviewer/SKILL.md"),
            "---\nname: reviewer\ndescription: Review changes\n---\nBody\n",
        )
        .unwrap();
        fs::write(
            resources_dir.join("plugins/example.yaml"),
            "id: example\ndisplay_name: Example\ncommands:\n  - name: demo\n    description: Demo\n",
        )
        .unwrap();

        let paths = ConfigPaths::discover(&root);
        let loaded = load_resources(&paths).unwrap();
        assert_eq!(loaded.agents.len(), 1);
        assert_eq!(loaded.prompts.len(), 1);
        assert_eq!(loaded.hooks.len(), 1);
        assert_eq!(loaded.skills.len(), 1);
        assert_eq!(loaded.plugins.len(), 1);
        assert_eq!(loaded.agents[0].value.id, "default");
        assert_eq!(loaded.skills[0].value.name, "reviewer");
        assert_eq!(loaded.plugins[0].value.id, "example");
    }

    #[test]
    fn plugin_agents_are_loaded_into_agent_inventory() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("workspace");
        let resources_dir = root.join("resources");
        fs::create_dir_all(resources_dir.join("plugins")).unwrap();
        fs::write(
            resources_dir.join("plugins/example.yaml"),
            "id: example\ndisplay_name: Example\nagents:\n  - id: reviewer\n    description: Review changes\n    prompt: You are a reviewer.\n    tools:\n      - Read\n",
        )
        .unwrap();

        let paths = ConfigPaths::discover(&root);
        let loaded = load_resources(&paths).unwrap();
        assert_eq!(loaded.plugins.len(), 1);
        assert!(loaded
            .agents
            .iter()
            .any(|agent| agent.value.id == "reviewer"));
        assert!(loaded
            .agents
            .iter()
            .any(|agent| agent.source_info.path.ends_with("plugins/example.yaml")));
    }

    #[test]
    fn skill_names_are_normalized_for_slash_commands() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("workspace");
        let resources_dir = root.join(".puffer/resources");
        fs::create_dir_all(resources_dir.join("skills/review-helper")).unwrap();
        fs::write(
            resources_dir.join("skills/review-helper/SKILL.md"),
            "---\nname: Review Helper ++\ndescription: Review changes\n---\nBody\n",
        )
        .unwrap();

        let paths = ConfigPaths {
            workspace_root: root.clone(),
            workspace_config_dir: root.join(".puffer"),
            user_config_dir: root.join(".home/.puffer"),
            builtin_resources_dir: root.join("resources"),
        };
        let loaded = load_resources(&paths).unwrap();
        assert_eq!(loaded.skills.len(), 1);
        assert_eq!(loaded.skills[0].value.name, "review-helper");
        assert!(skill_by_name(&loaded, "Review Helper ++").is_some());
        assert!(skill_by_name(&loaded, "review helper").is_some());
        assert!(skill_by_name(&loaded, "review-helper").is_some());
    }

    #[test]
    fn skill_loader_parses_extended_frontmatter_fields() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("workspace");
        let resources_dir = root.join(".puffer/resources");
        fs::create_dir_all(resources_dir.join("skills/review-helper")).unwrap();
        fs::write(
            resources_dir.join("skills/review-helper/SKILL.md"),
            "---\nname: Review Helper ++\ndescription: Review changes\nallowed-tools:\n  - Read\n  - Grep, Glob\nargument-hint: <ticket>\narguments: ticket env\nmodel: openai/gpt-5\neffort: high\nuser-invocable: false\ndisable-model-invocation: true\ncontext: fork\n---\nBody\n",
        )
        .unwrap();

        let paths = ConfigPaths {
            workspace_root: root.clone(),
            workspace_config_dir: root.join(".puffer"),
            user_config_dir: root.join(".home/.puffer"),
            builtin_resources_dir: root.join("resources"),
        };
        let loaded = load_resources(&paths).unwrap();
        let skill = &loaded.skills[0].value;
        assert_eq!(skill.name, "review-helper");
        assert_eq!(skill.description, "Review changes");
        assert_eq!(skill.allowed_tools, vec!["Read", "Grep", "Glob"]);
        assert_eq!(skill.argument_hint.as_deref(), Some("<ticket>"));
        assert_eq!(skill.argument_names, vec!["ticket", "env"]);
        assert_eq!(skill.model.as_deref(), Some("openai/gpt-5"));
        assert_eq!(skill.effort.as_deref(), Some("high"));
        assert_eq!(skill.context.as_deref(), Some("fork"));
        assert!(!skill.user_invocable);
        assert!(skill.disable_model_invocation);
    }

    #[test]
    fn workspace_resources_override_user_and_bundled_resources_by_id() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("workspace");
        let builtin = root.join("resources/prompts");
        let user = root.join(".home/.puffer/resources/prompts");
        let workspace = root.join(".puffer/resources/prompts");
        fs::create_dir_all(&builtin).unwrap();
        fs::create_dir_all(&user).unwrap();
        fs::create_dir_all(&workspace).unwrap();
        fs::write(
            builtin.join("review.yaml"),
            "id: review\ndescription: Builtin\ntemplate: builtin\n",
        )
        .unwrap();
        fs::write(
            user.join("review.yaml"),
            "id: review\ndescription: User\ntemplate: user\n",
        )
        .unwrap();
        fs::write(
            workspace.join("review.yaml"),
            "id: review\ndescription: Workspace\ntemplate: workspace\n",
        )
        .unwrap();

        let paths = ConfigPaths {
            workspace_root: root.clone(),
            workspace_config_dir: root.join(".puffer"),
            user_config_dir: root.join(".home/.puffer"),
            builtin_resources_dir: root.join("resources"),
        };
        let loaded = load_resources(&paths).unwrap();
        assert_eq!(loaded.prompts.len(), 1);
        assert_eq!(loaded.prompts[0].value.description, "Workspace");
        assert!(loaded.prompts[0]
            .source_info
            .path
            .to_string_lossy()
            .contains(".puffer/resources/prompts/review.yaml"));
        assert!(loaded
            .diagnostics
            .iter()
            .any(|item| item.contains("user prompt `review`")));
        assert!(loaded
            .diagnostics
            .iter()
            .any(|item| item.contains("workspace prompt `review`")));
    }

    #[test]
    fn duplicate_ids_in_same_layer_are_deterministic_and_reported() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("workspace");
        let builtin = root.join("resources/prompts");
        fs::create_dir_all(&builtin).unwrap();
        fs::write(
            builtin.join("a_review.yaml"),
            "id: review\ndescription: First\ntemplate: first\n",
        )
        .unwrap();
        fs::write(
            builtin.join("z_review.yaml"),
            "id: review\ndescription: Second\ntemplate: second\n",
        )
        .unwrap();

        let paths = ConfigPaths {
            workspace_root: root.clone(),
            workspace_config_dir: root.join(".puffer"),
            user_config_dir: root.join(".home/.puffer"),
            builtin_resources_dir: root.join("resources"),
        };
        let loaded = load_resources(&paths).unwrap();
        assert_eq!(loaded.prompts.len(), 1);
        assert_eq!(loaded.prompts[0].value.description, "Second");
        assert!(loaded.diagnostics.iter().any(|item| {
            item.contains("duplicate prompt `review` in builtin resources")
                && item.contains("z_review.yaml")
                && item.contains("a_review.yaml")
        }));
    }

    #[test]
    fn hook_resources_override_by_id_and_filter_by_event() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("workspace");
        let builtin = root.join("resources/hooks");
        let workspace = root.join(".puffer/resources/hooks");
        fs::create_dir_all(&builtin).unwrap();
        fs::create_dir_all(&workspace).unwrap();
        fs::write(
            builtin.join("tool_end.yaml"),
            "id: tool-end\nevent: tool_end\ncommand: echo builtin\n",
        )
        .unwrap();
        fs::write(
            workspace.join("tool_end.yaml"),
            "id: tool-end\nevent: tool_end\ncommand: echo workspace\n",
        )
        .unwrap();
        fs::write(
            workspace.join("tool_start.yaml"),
            "id: tool-start\nevent: tool_start\ncommand: echo start\n",
        )
        .unwrap();

        let paths = ConfigPaths {
            workspace_root: root.clone(),
            workspace_config_dir: root.join(".puffer"),
            user_config_dir: root.join(".home/.puffer"),
            builtin_resources_dir: root.join("resources"),
        };
        let loaded = load_resources(&paths).unwrap();
        assert_eq!(loaded.hooks.len(), 2);
        assert_eq!(
            hook_by_id(&loaded, "tool-end").unwrap().value.command,
            "echo workspace"
        );
        let tool_end_hooks = hooks_for_event(&loaded, "tool_end");
        assert_eq!(tool_end_hooks.len(), 1);
        assert_eq!(tool_end_hooks[0].value.id, "tool-end");
        assert_eq!(tool_end_hooks[0].value.command, "echo workspace");
        assert!(loaded
            .diagnostics
            .iter()
            .any(|item| item.contains("workspace hook `tool-end`")));
    }

    #[test]
    fn per_model_prompt_variants_coexist_and_resolve_with_fallback() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("workspace");
        let builtin = root.join("resources/prompts");
        fs::create_dir_all(&builtin).unwrap();
        fs::write(
            builtin.join("system-base.yaml"),
            "id: system-base\ndescription: Base\ntemplate: base body\n",
        )
        .unwrap();
        fs::write(
            builtin.join("system-base.opus.yaml"),
            "id: system-base\ndescription: Opus override\ntemplate: opus body\nfor_model: claude-opus-4-6\n",
        )
        .unwrap();
        fs::write(
            builtin.join("system-base.openai.yaml"),
            "id: system-base\ndescription: OpenAI override\ntemplate: openai body\nfor_provider: openai\n",
        )
        .unwrap();

        let paths = ConfigPaths {
            workspace_root: root.clone(),
            workspace_config_dir: root.join(".puffer"),
            user_config_dir: root.join(".home/.puffer"),
            builtin_resources_dir: root.join("resources"),
        };
        let loaded = load_resources(&paths).unwrap();
        assert_eq!(loaded.prompts.len(), 3);
        assert_eq!(
            prompt_for(&loaded, "system-base", None, Some("claude-opus-4-6"))
                .unwrap()
                .value
                .template,
            "opus body"
        );
        assert_eq!(
            prompt_for(&loaded, "system-base", Some("openai"), Some("gpt-5"))
                .unwrap()
                .value
                .template,
            "openai body"
        );
        assert_eq!(
            prompt_for(&loaded, "system-base", None, None)
                .unwrap()
                .value
                .template,
            "base body"
        );
        assert_eq!(prompt_by_id(&loaded, "system-base").unwrap().value.template, "base body");
    }

    #[test]
    fn workspace_variant_overrides_builtin_variant_with_same_model() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("workspace");
        let builtin = root.join("resources/prompts");
        let workspace = root.join(".puffer/resources/prompts");
        fs::create_dir_all(&builtin).unwrap();
        fs::create_dir_all(&workspace).unwrap();
        fs::write(
            builtin.join("system-base.yaml"),
            "id: system-base\ndescription: Base\ntemplate: base body\n",
        )
        .unwrap();
        fs::write(
            builtin.join("system-base.opus.yaml"),
            "id: system-base\ndescription: Builtin opus\ntemplate: builtin opus\nfor_model: claude-opus-4-6\n",
        )
        .unwrap();
        fs::write(
            workspace.join("system-base.opus.yaml"),
            "id: system-base\ndescription: Workspace opus\ntemplate: workspace opus\nfor_model: claude-opus-4-6\n",
        )
        .unwrap();

        let paths = ConfigPaths {
            workspace_root: root.clone(),
            workspace_config_dir: root.join(".puffer"),
            user_config_dir: root.join(".home/.puffer"),
            builtin_resources_dir: root.join("resources"),
        };
        let loaded = load_resources(&paths).unwrap();
        assert_eq!(loaded.prompts.len(), 2);
        assert_eq!(
            prompt_for(&loaded, "system-base", None, Some("claude-opus-4-6"))
                .unwrap()
                .value
                .template,
            "workspace opus"
        );
        assert_eq!(
            prompt_for(&loaded, "system-base", None, None)
                .unwrap()
                .value
                .template,
            "base body"
        );
    }

    #[test]
    fn load_resources_reads_legacy_mcp_dir_when_mcp_servers_absent() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("workspace");
        let legacy_mcp = root.join("resources/mcp");
        fs::create_dir_all(&legacy_mcp).unwrap();
        fs::write(
            legacy_mcp.join("legacy.yaml"),
            "id: legacy\ndisplay_name: Legacy MCP\ntransport: stdio\ntarget: legacy-server\n",
        )
        .unwrap();

        let paths = ConfigPaths {
            workspace_root: root.clone(),
            workspace_config_dir: root.join(".puffer"),
            user_config_dir: root.join(".home/.puffer"),
            builtin_resources_dir: root.join("resources"),
        };
        let loaded = load_resources(&paths).unwrap();
        assert_eq!(loaded.mcp_servers.len(), 1);
        assert_eq!(loaded.mcp_servers[0].value.id, "legacy");
        assert!(loaded.mcp_servers[0]
            .source_info
            .path
            .to_string_lossy()
            .contains("resources/mcp/legacy.yaml"));
    }

    #[test]
    fn mcp_servers_dir_takes_precedence_over_legacy_dir_for_same_id() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("workspace");
        let legacy_mcp = root.join("resources/mcp");
        let canonical_mcp = root.join("resources/mcp_servers");
        fs::create_dir_all(&legacy_mcp).unwrap();
        fs::create_dir_all(&canonical_mcp).unwrap();
        fs::write(
            legacy_mcp.join("docs.yaml"),
            "id: docs\ndisplay_name: Legacy Docs\ntransport: stdio\ntarget: legacy-docs\n",
        )
        .unwrap();
        fs::write(
            canonical_mcp.join("docs.yaml"),
            "id: docs\ndisplay_name: Canonical Docs\ntransport: stdio\ntarget: canonical-docs\n",
        )
        .unwrap();

        let paths = ConfigPaths {
            workspace_root: root.clone(),
            workspace_config_dir: root.join(".puffer"),
            user_config_dir: root.join(".home/.puffer"),
            builtin_resources_dir: root.join("resources"),
        };
        let loaded = load_resources(&paths).unwrap();
        assert_eq!(loaded.mcp_servers.len(), 1);
        assert_eq!(loaded.mcp_servers[0].value.id, "docs");
        assert_eq!(loaded.mcp_servers[0].value.display_name, "Canonical Docs");
        assert!(loaded.mcp_servers[0]
            .source_info
            .path
            .to_string_lossy()
            .contains("resources/mcp_servers/docs.yaml"));
        assert!(!loaded
            .diagnostics
            .iter()
            .any(|item| item.contains("mcp_server `docs`")));
    }

    #[test]
    fn disabled_mcp_manifests_are_filtered_out() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("workspace");
        let canonical_mcp = root.join("resources/mcp_servers");
        fs::create_dir_all(&canonical_mcp).unwrap();
        fs::write(
            canonical_mcp.join("enabled.yaml"),
            "id: enabled\ndisplay_name: Enabled MCP\ntransport: stdio\ntarget: enabled-server\nenabled: true\n",
        )
        .unwrap();
        fs::write(
            canonical_mcp.join("disabled.yaml"),
            "id: disabled\ndisplay_name: Disabled MCP\ntransport: stdio\ntarget: disabled-server\nenabled: false\n",
        )
        .unwrap();

        let paths = ConfigPaths {
            workspace_root: root.clone(),
            workspace_config_dir: root.join(".puffer"),
            user_config_dir: root.join(".home/.puffer"),
            builtin_resources_dir: root.join("resources"),
        };
        let loaded = load_resources(&paths).unwrap();
        assert_eq!(loaded.mcp_servers.len(), 1);
        assert_eq!(loaded.mcp_servers[0].value.id, "enabled");
    }
}

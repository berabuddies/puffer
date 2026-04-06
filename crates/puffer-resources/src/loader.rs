use crate::model::{
    HookSpec, IdeSpec, LoadedItem, LoadedResources, MascotSpec, McpServerSpec, PluginSpec,
    PromptTemplate, ProviderPack, SkillSpec, SourceInfo, SourceKind, ToolSpec,
};
use anyhow::{Context, Result};
use indexmap::IndexMap;
use puffer_config::ConfigPaths;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Loads bundled, user, and workspace resources into one in-memory registry.
pub fn load_resources(paths: &ConfigPaths) -> Result<LoadedResources> {
    let mut loaded = LoadedResources::default();
    for (root, kind) in resource_roots(paths) {
        merge_by_id(
            &mut loaded.providers,
            load_yaml_dir::<ProviderPack>(&root.join("providers"), kind)?,
            |item| item.value.id.clone(),
            "provider",
            &mut loaded.diagnostics,
        );
        merge_by_id(
            &mut loaded.tools,
            load_yaml_dir::<ToolSpec>(&root.join("tools"), kind)?,
            |item| item.value.id.clone(),
            "tool",
            &mut loaded.diagnostics,
        );
        merge_by_id(
            &mut loaded.prompts,
            load_yaml_dir::<PromptTemplate>(&root.join("prompts"), kind)?,
            |item| item.value.id.clone(),
            "prompt",
            &mut loaded.diagnostics,
        );
        merge_by_id(
            &mut loaded.hooks,
            load_yaml_dir::<HookSpec>(&root.join("hooks"), kind)?,
            |item| item.value.id.clone(),
            "hook",
            &mut loaded.diagnostics,
        );
        merge_by_id(
            &mut loaded.skills,
            load_skill_dir(&root.join("skills"), kind)?,
            |item| item.value.name.clone(),
            "skill",
            &mut loaded.diagnostics,
        );
        merge_by_id(
            &mut loaded.mascots,
            load_yaml_dir::<MascotSpec>(&root.join("mascots"), kind)?,
            |item| item.value.id.clone(),
            "mascot",
            &mut loaded.diagnostics,
        );
        merge_by_id(
            &mut loaded.plugins,
            load_yaml_dir::<PluginSpec>(&root.join("plugins"), kind)?,
            |item| item.value.id.clone(),
            "plugin",
            &mut loaded.diagnostics,
        );
        merge_by_id(
            &mut loaded.mcp_servers,
            load_mcp_server_manifests(&root, kind)?,
            |item| item.value.id.clone(),
            "mcp_server",
            &mut loaded.diagnostics,
        );
        merge_by_id(
            &mut loaded.ides,
            load_yaml_dir::<IdeSpec>(&root.join("ides"), kind)?,
            |item| item.value.id.clone(),
            "ide",
            &mut loaded.diagnostics,
        );
    }
    Ok(loaded)
}

/// Looks up a prompt template by id.
pub fn prompt_by_id<'a>(
    resources: &'a LoadedResources,
    id: &str,
) -> Option<&'a LoadedItem<PromptTemplate>> {
    resources
        .prompts
        .iter()
        .find(|prompt| prompt.value.id == id)
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
    resources
        .skills
        .iter()
        .find(|skill| skill.value.name == name)
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
        let (frontmatter, body) = split_frontmatter(&raw);
        let name = frontmatter
            .get("name")
            .cloned()
            .unwrap_or_else(|| path.file_name().unwrap().to_string_lossy().to_string());
        let description = frontmatter
            .get("description")
            .cloned()
            .unwrap_or_else(|| first_descriptive_line(&body).to_string());
        let disable_model_invocation = frontmatter
            .get("disable-model-invocation")
            .map(|value| matches!(value.as_str(), "true" | "1" | "yes"))
            .unwrap_or(false);

        items.push(LoadedItem {
            value: SkillSpec {
                name,
                description,
                content: body,
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

fn sorted_dir_entries(dir: &Path) -> Result<Vec<fs::DirEntry>> {
    let mut entries = fs::read_dir(dir)
        .with_context(|| format!("failed to read resource dir {}", dir.display()))?
        .collect::<std::io::Result<Vec<_>>>()
        .with_context(|| format!("failed to list resource dir {}", dir.display()))?;
    entries.sort_by(|left, right| left.path().cmp(&right.path()));
    Ok(entries)
}

fn split_frontmatter(raw: &str) -> (BTreeMap<String, String>, String) {
    let normalized = raw.replace("\r\n", "\n");
    let mut lines = normalized.lines();
    if lines.next() != Some("---") {
        return (BTreeMap::new(), normalized);
    }

    let mut frontmatter = BTreeMap::new();
    let mut offset = 4usize;
    for line in normalized.lines().skip(1) {
        offset += line.len() + 1;
        if line == "---" {
            break;
        }
        if let Some((key, value)) = line.split_once(':') {
            frontmatter.insert(key.trim().to_string(), value.trim().to_string());
        }
    }
    (
        frontmatter,
        normalized
            .get(offset..)
            .map(str::trim_start)
            .unwrap_or_default()
            .to_string(),
    )
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
    F: Fn(&LoadedItem<T>) -> String,
{
    let mut merged = IndexMap::new();
    for item in existing.iter().cloned() {
        merged.insert(key(&item), item);
    }
    for item in incoming {
        let id = key(&item);
        if let Some(previous) = merged.get(&id) {
            diagnostics.push(describe_override(
                label,
                &id,
                &previous.source_info,
                &item.source_info,
            ));
        }
        merged.insert(id, item);
    }
    *existing = merged.into_values().collect();
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
        fs::create_dir_all(resources_dir.join("prompts")).unwrap();
        fs::create_dir_all(resources_dir.join("hooks")).unwrap();
        fs::create_dir_all(resources_dir.join("skills/reviewer")).unwrap();
        fs::create_dir_all(resources_dir.join("plugins")).unwrap();
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
        assert_eq!(loaded.prompts.len(), 1);
        assert_eq!(loaded.hooks.len(), 1);
        assert_eq!(loaded.skills.len(), 1);
        assert_eq!(loaded.plugins.len(), 1);
        assert_eq!(loaded.skills[0].value.name, "reviewer");
        assert_eq!(loaded.plugins[0].value.id, "example");
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

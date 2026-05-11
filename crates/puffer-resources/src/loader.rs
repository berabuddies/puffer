use crate::model::{
    AgentSpec, HookSpec, IdeSpec, LoadedItem, LoadedResources, LspServerSpec, MascotSpec,
    McpServerSpec, PluginSpec, PromptTemplate, ProviderPack, SkillSpec, SourceInfo, SourceKind,
    ToolSpec,
};
use anyhow::{anyhow, Context, Result};
use include_dir::{include_dir, Dir};
use indexmap::IndexMap;
use puffer_config::ConfigPaths;
use puffer_runner_api::{RunnerError, ToolRunner};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::Value;
use serde_yaml::{Mapping, Value as YamlValue};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Built-in `<repo>/resources/` baked into the binary at compile time.
/// Same pattern as codex's `include_str!("../templates/foo.md")` (used at
/// e.g. codex-rs/core/src/compact.rs:43), generalized to a directory tree
/// because puffer ships ~95 yaml/md files. Without this, running `puffer`
/// from a directory that has no sibling `resources/` would silently load
/// 0 providers and the very first turn would fail with the misleading
/// `no providers are registered`.
static BUILTIN_RESOURCES: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../resources");

/// Loads bundled, user, and workspace resources into one in-memory registry.
///
/// Merge order (later wins on id collision):
/// 1. **Embedded** — built-in `BUILTIN_RESOURCES` baked into the binary.
///    Always present, no filesystem dependency.
/// 2. **Filesystem builtin** — `paths.builtin_resources_dir`, set via
///    `PUFFER_BUILTIN_RESOURCES_DIR` env var or defaulting to
///    `<workspace>/resources`. Skipped if the directory does not exist
///    (developer convenience: editing yaml in-place without rebuilding).
/// 3. **User** — `~/.puffer/resources/`. Lets users override individual
///    files (e.g. add a custom provider) without touching the install.
/// 4. **Workspace** — `<cwd>/.puffer/resources/`. Project-level overrides.
pub fn load_resources(paths: &ConfigPaths, runner: &dyn ToolRunner) -> Result<LoadedResources> {
    let mut loaded = LoadedResources::default();
    apply_embedded_resources(&mut loaded)?;
    for (root, kind) in resource_roots(paths) {
        // Filesystem layers are optional. Without this guard the misleading
        // "no providers are registered" error used to fire whenever the
        // user ran puffer from a directory with no sibling `resources/`.
        if !runner_path_exists(runner, &root) {
            continue;
        }
        let plugins = load_yaml_dir::<PluginSpec>(runner, &root.join("plugins"), kind)?;
        merge_by_id(
            &mut loaded.providers,
            load_yaml_dir::<ProviderPack>(runner, &root.join("providers"), kind)?,
            |item| MergeKey::simple(item.value.id.clone()),
            "provider",
            &mut loaded.diagnostics,
        );
        merge_by_id(
            &mut loaded.tools,
            load_yaml_dir::<ToolSpec>(runner, &root.join("tools"), kind)?,
            |item| MergeKey::simple(item.value.id.clone()),
            "tool",
            &mut loaded.diagnostics,
        );
        merge_by_id(
            &mut loaded.agents,
            load_yaml_dir::<AgentSpec>(runner, &root.join("agents"), kind)?,
            |item| MergeKey::simple(item.value.id.clone()),
            "agent",
            &mut loaded.diagnostics,
        );
        merge_by_id(
            &mut loaded.prompts,
            load_yaml_dir::<PromptTemplate>(runner, &root.join("prompts"), kind)?,
            |item| prompt_variant_key(&item.value),
            "prompt",
            &mut loaded.diagnostics,
        );
        merge_by_id(
            &mut loaded.hooks,
            load_yaml_dir::<HookSpec>(runner, &root.join("hooks"), kind)?,
            |item| MergeKey::simple(item.value.id.clone()),
            "hook",
            &mut loaded.diagnostics,
        );
        merge_by_id(
            &mut loaded.skills,
            load_skill_dir(runner, &root.join("skills"), kind)?,
            |item| MergeKey::simple(item.value.name.clone()),
            "skill",
            &mut loaded.diagnostics,
        );
        merge_by_id(
            &mut loaded.mascots,
            load_yaml_dir::<MascotSpec>(runner, &root.join("mascots"), kind)?,
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
            load_mcp_server_manifests(runner, &root, kind)?,
            |item| MergeKey::simple(item.value.id.clone()),
            "mcp_server",
            &mut loaded.diagnostics,
        );
        merge_by_id(
            &mut loaded.ides,
            load_yaml_dir::<IdeSpec>(runner, &root.join("ides"), kind)?,
            |item| MergeKey::simple(item.value.id.clone()),
            "ide",
            &mut loaded.diagnostics,
        );
    }
    apply_runtime_resource_filters(&mut loaded);
    Ok(loaded)
}

fn apply_runtime_resource_filters(resources: &mut LoadedResources) {
    filter_browser_resources(resources, puffer_builtin_browser_disabled());
}

fn filter_browser_resources(resources: &mut LoadedResources, no_browser: bool) {
    if !no_browser {
        return;
    }
    resources
        .skills
        .retain(|skill| skill.value.name.trim() != "browser");
    for plugin in &mut resources.plugins {
        plugin
            .value
            .skills
            .retain(|skill| skill.trim() != "browser");
    }
}

fn puffer_builtin_browser_disabled() -> bool {
    std::env::var("PUFFER_NO_BROWSER")
        .ok()
        .is_some_and(|value| enabled_flag(&value))
}

fn enabled_flag(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase();
    !matches!(normalized.as_str(), "" | "0" | "false" | "no" | "off")
}

/// Probe directory presence via the runner. Treats `NotFound` as absent and
/// any other error (e.g. permission denied) as also absent — same as the
/// previous `Path::exists` semantics, which silently swallowed io errors.
fn runner_path_exists(runner: &dyn ToolRunner, path: &Path) -> bool {
    match runner.list_dir(path) {
        Ok(_) => true,
        Err(RunnerError::NotFound(_)) => false,
        Err(_) => false,
    }
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
    let without_provider = trimmed
        .split_once('/')
        .map(|(_, rest)| rest)
        .unwrap_or(trimmed);
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

/// Applies the compile-time-embedded resources as the base layer of the
/// loader. Filesystem layers in `resource_roots` are merged on top.
fn apply_embedded_resources(loaded: &mut LoadedResources) -> Result<()> {
    let plugins = load_yaml_embedded::<PluginSpec>("plugins")?;
    merge_by_id(
        &mut loaded.providers,
        load_yaml_embedded::<ProviderPack>("providers")?,
        |item| MergeKey::simple(item.value.id.clone()),
        "provider",
        &mut loaded.diagnostics,
    );
    merge_by_id(
        &mut loaded.tools,
        load_yaml_embedded::<ToolSpec>("tools")?,
        |item| MergeKey::simple(item.value.id.clone()),
        "tool",
        &mut loaded.diagnostics,
    );
    merge_by_id(
        &mut loaded.agents,
        load_yaml_embedded::<AgentSpec>("agents")?,
        |item| MergeKey::simple(item.value.id.clone()),
        "agent",
        &mut loaded.diagnostics,
    );
    merge_by_id(
        &mut loaded.prompts,
        load_yaml_embedded::<PromptTemplate>("prompts")?,
        |item| prompt_variant_key(&item.value),
        "prompt",
        &mut loaded.diagnostics,
    );
    merge_by_id(
        &mut loaded.hooks,
        load_yaml_embedded::<HookSpec>("hooks")?,
        |item| MergeKey::simple(item.value.id.clone()),
        "hook",
        &mut loaded.diagnostics,
    );
    merge_by_id(
        &mut loaded.skills,
        load_skill_embedded()?,
        |item| MergeKey::simple(item.value.name.clone()),
        "skill",
        &mut loaded.diagnostics,
    );
    merge_by_id(
        &mut loaded.mascots,
        load_yaml_embedded::<MascotSpec>("mascots")?,
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
        load_mcp_server_manifests_embedded()?,
        |item| MergeKey::simple(item.value.id.clone()),
        "mcp_server",
        &mut loaded.diagnostics,
    );
    merge_by_id(
        &mut loaded.ides,
        load_yaml_embedded::<IdeSpec>("ides")?,
        |item| MergeKey::simple(item.value.id.clone()),
        "ide",
        &mut loaded.diagnostics,
    );
    Ok(())
}

/// Loads every `*.yaml` / `*.yml` file under `BUILTIN_RESOURCES/<subdir>`
/// and parses each as `T`.
fn load_yaml_embedded<T>(subdir: &str) -> Result<Vec<LoadedItem<T>>>
where
    T: DeserializeOwned,
{
    let Some(dir) = BUILTIN_RESOURCES.get_dir(subdir) else {
        return Ok(Vec::new());
    };
    let mut files: Vec<_> = dir.files().collect();
    files.sort_by_key(|file| file.path().to_path_buf());

    let mut items = Vec::new();
    for file in files {
        let path = file.path();
        let ext = path.extension().and_then(|e| e.to_str());
        if !matches!(ext, Some("yaml" | "yml")) {
            continue;
        }
        let raw = file
            .contents_utf8()
            .ok_or_else(|| anyhow!("embedded resource {} is not UTF-8", path.display()))?;
        let value: T = serde_yaml::from_str(raw)
            .with_context(|| format!("failed to parse embedded resource {}", path.display()))?;
        items.push(LoadedItem {
            value,
            source_info: SourceInfo {
                path: PathBuf::from("<embedded>").join(path),
                kind: SourceKind::Builtin,
            },
        });
    }
    Ok(items)
}

/// Embedded equivalent of `load_skill_dir` — walks `BUILTIN_RESOURCES/skills/`
/// and reads each `<skill>/SKILL.md` with the same frontmatter parsing the
/// filesystem path uses.
fn load_skill_embedded() -> Result<Vec<LoadedItem<SkillSpec>>> {
    let Some(skills_dir) = BUILTIN_RESOURCES.get_dir("skills") else {
        return Ok(Vec::new());
    };
    let mut subdirs: Vec<_> = skills_dir.dirs().collect();
    subdirs.sort_by_key(|d| d.path().to_path_buf());

    let mut items = Vec::new();
    for subdir in subdirs {
        let skill_path = subdir.path().join("SKILL.md");
        let Some(skill_file) = BUILTIN_RESOURCES.get_file(&skill_path) else {
            continue;
        };
        let raw = skill_file
            .contents_utf8()
            .ok_or_else(|| anyhow!("embedded skill {} is not UTF-8", skill_path.display()))?
            .to_string();
        let (frontmatter, body) = split_frontmatter(&raw).with_context(|| {
            format!("failed to parse skill frontmatter {}", skill_path.display())
        })?;
        let raw_name = frontmatter_string(&frontmatter, &["name"]).unwrap_or_else(|| {
            subdir
                .path()
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default()
        });
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
                path: PathBuf::from("<embedded>").join(&skill_path),
                kind: SourceKind::Builtin,
            },
        });
    }
    Ok(items)
}

/// Embedded equivalent of `load_mcp_server_manifests` — reads canonical
/// `mcp_servers/` plus the legacy `mcp/` directory if it exists.
fn load_mcp_server_manifests_embedded() -> Result<Vec<LoadedItem<McpServerSpec>>> {
    let canonical = load_mcp_manifest_dir_embedded("mcp_servers")?;
    let canonical_ids = canonical
        .iter()
        .map(|item| item.value.id.clone())
        .collect::<std::collections::BTreeSet<_>>();
    let legacy = load_mcp_manifest_dir_embedded("mcp")?
        .into_iter()
        .filter(|item| !canonical_ids.contains(&item.value.id))
        .collect::<Vec<_>>();
    let mut merged = legacy;
    merged.extend(canonical);
    Ok(merged)
}

fn load_mcp_manifest_dir_embedded(subdir: &str) -> Result<Vec<LoadedItem<McpServerSpec>>> {
    Ok(load_yaml_embedded::<McpManifestFile>(subdir)?
        .into_iter()
        .filter_map(|item| {
            item.value.enabled.then_some(LoadedItem {
                value: item.value.server,
                source_info: item.source_info,
            })
        })
        .collect())
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
    runner: &dyn ToolRunner,
    root: &Path,
    kind: SourceKind,
) -> Result<Vec<LoadedItem<McpServerSpec>>> {
    let canonical = load_mcp_manifest_dir(runner, &root.join("mcp_servers"), kind)?;
    let canonical_ids = canonical
        .iter()
        .map(|item| item.value.id.clone())
        .collect::<std::collections::BTreeSet<_>>();

    let legacy = load_mcp_manifest_dir(runner, &root.join("mcp"), kind)?
        .into_iter()
        .filter(|item| !canonical_ids.contains(&item.value.id))
        .collect::<Vec<_>>();

    let mut merged = legacy;
    merged.extend(canonical);
    Ok(merged)
}

fn load_mcp_manifest_dir(
    runner: &dyn ToolRunner,
    dir: &Path,
    kind: SourceKind,
) -> Result<Vec<LoadedItem<McpServerSpec>>> {
    Ok(load_yaml_dir::<McpManifestFile>(runner, dir, kind)?
        .into_iter()
        .filter_map(|item| {
            item.value.enabled.then_some(LoadedItem {
                value: item.value.server,
                source_info: item.source_info,
            })
        })
        .collect())
}

fn load_yaml_dir<T>(
    runner: &dyn ToolRunner,
    dir: &Path,
    kind: SourceKind,
) -> Result<Vec<LoadedItem<T>>>
where
    T: DeserializeOwned,
{
    let entries = match sorted_dir_entries(runner, dir) {
        Ok(entries) => entries,
        Err(RunnerError::NotFound(_)) => return Ok(Vec::new()),
        Err(err) => {
            return Err(anyhow!(err))
                .with_context(|| format!("failed to list resource dir {}", dir.display()))
        }
    };

    let mut items = Vec::new();
    for path in entries {
        if !matches!(
            path.extension().and_then(|ext| ext.to_str()),
            Some("yaml" | "yml")
        ) {
            continue;
        }
        let raw_bytes = runner
            .read_file(&path)
            .map_err(|err| anyhow!(err))
            .with_context(|| format!("failed to read resource file {}", path.display()))?;
        let raw = String::from_utf8(raw_bytes)
            .with_context(|| format!("resource file {} is not UTF-8", path.display()))?;
        let value = serde_yaml::from_str::<T>(&raw)
            .with_context(|| format!("failed to parse resource file {}", path.display()))?;
        items.push(LoadedItem {
            value,
            source_info: SourceInfo { path, kind },
        });
    }
    Ok(items)
}

fn load_skill_dir(
    runner: &dyn ToolRunner,
    dir: &Path,
    kind: SourceKind,
) -> Result<Vec<LoadedItem<SkillSpec>>> {
    let entries = match runner.list_dir(dir) {
        Ok(mut entries) => {
            entries.sort_by(|left, right| left.path.cmp(&right.path));
            entries
        }
        Err(RunnerError::NotFound(_)) => return Ok(Vec::new()),
        Err(err) => {
            return Err(anyhow!(err))
                .with_context(|| format!("failed to list resource dir {}", dir.display()))
        }
    };

    let mut items = Vec::new();
    for entry in entries {
        if !entry.is_dir {
            continue;
        }
        let path = entry.path;

        let skill_path = path.join("SKILL.md");
        let raw_bytes = match runner.read_file(&skill_path) {
            Ok(bytes) => bytes,
            Err(RunnerError::NotFound(_)) => continue,
            Err(err) => {
                return Err(anyhow!(err))
                    .with_context(|| format!("failed to read skill file {}", skill_path.display()))
            }
        };
        let raw = String::from_utf8(raw_bytes)
            .with_context(|| format!("skill file {} is not UTF-8", skill_path.display()))?;
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

fn sorted_dir_entries(
    runner: &dyn ToolRunner,
    dir: &Path,
) -> std::result::Result<Vec<PathBuf>, RunnerError> {
    let mut entries = runner.list_dir(dir)?;
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(entries.into_iter().map(|entry| entry.path).collect())
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
    use puffer_runner_api::{
        ChunkSink, DirEntry, McpPrompt, McpPromptContent, McpResourceContent, McpResourceRecord,
        McpResult, McpServerInfo, McpTool, RunnerCapabilities, ToolRequest, ToolResult,
    };
    use std::fs;
    use tempfile::tempdir;

    /// Minimal `ToolRunner` backed by `std::fs` for loader tests; mirrors
    /// `puffer_runner_local::LocalToolRunner` so tests don't take a circular
    /// dependency on the runner crate.
    #[derive(Debug)]
    struct FsTestRunner;

    impl ToolRunner for FsTestRunner {
        fn ping(&self) -> std::result::Result<puffer_runner_api::RunnerPing, RunnerError> {
            Ok(puffer_runner_api::RunnerPing {
                version: env!("CARGO_PKG_VERSION").to_string(),
                uptime: std::time::Duration::from_secs(0),
            })
        }
        fn capabilities(&self) -> RunnerCapabilities {
            RunnerCapabilities::default()
        }
        fn execute_tool(
            &self,
            _req: ToolRequest,
            _sink: &mut dyn ChunkSink,
        ) -> std::result::Result<ToolResult, RunnerError> {
            Err(RunnerError::Unsupported("test runner".into()))
        }
        fn read_file(&self, path: &Path) -> std::result::Result<Vec<u8>, RunnerError> {
            std::fs::read(path).map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => RunnerError::NotFound(path.display().to_string()),
                _ => RunnerError::Other(format!("read {path:?}: {e}")),
            })
        }
        fn list_dir(&self, path: &Path) -> std::result::Result<Vec<DirEntry>, RunnerError> {
            let read = std::fs::read_dir(path).map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => RunnerError::NotFound(path.display().to_string()),
                _ => RunnerError::Other(format!("read_dir {path:?}: {e}")),
            })?;
            let mut entries = Vec::new();
            for entry in read {
                let entry =
                    entry.map_err(|e| RunnerError::Other(format!("dir entry {path:?}: {e}")))?;
                let file_type = entry
                    .file_type()
                    .map_err(|e| RunnerError::Other(format!("file_type for {entry:?}: {e}")))?;
                entries.push(DirEntry {
                    path: entry.path(),
                    is_dir: file_type.is_dir(),
                    is_file: file_type.is_file(),
                    is_symlink: file_type.is_symlink(),
                });
            }
            Ok(entries)
        }
        fn glob(
            &self,
            _root: &Path,
            _pattern: &str,
        ) -> std::result::Result<Vec<PathBuf>, RunnerError> {
            Err(RunnerError::Unsupported("glob".into()))
        }
        fn list_mcp_servers(&self) -> std::result::Result<Vec<McpServerInfo>, RunnerError> {
            Err(RunnerError::Unsupported("mcp".into()))
        }
        fn list_mcp_tools(&self, _server: &str) -> std::result::Result<Vec<McpTool>, RunnerError> {
            Err(RunnerError::Unsupported("mcp".into()))
        }
        fn call_mcp_tool(
            &self,
            _server: &str,
            _tool: &str,
            _args: serde_json::Value,
            _sink: &mut dyn ChunkSink,
        ) -> std::result::Result<McpResult, RunnerError> {
            Err(RunnerError::Unsupported("mcp".into()))
        }
        fn list_mcp_resources(
            &self,
            _server: Option<&str>,
        ) -> std::result::Result<Vec<McpResourceRecord>, RunnerError> {
            Err(RunnerError::Unsupported("mcp".into()))
        }
        fn read_mcp_resource(
            &self,
            _server: &str,
            _uri: &str,
        ) -> std::result::Result<McpResourceContent, RunnerError> {
            Err(RunnerError::Unsupported("mcp".into()))
        }
        fn list_mcp_prompts(
            &self,
            _server: &str,
        ) -> std::result::Result<Vec<McpPrompt>, RunnerError> {
            Err(RunnerError::Unsupported("mcp".into()))
        }
        fn get_mcp_prompt(
            &self,
            _server: &str,
            _name: &str,
            _args: serde_json::Value,
        ) -> std::result::Result<McpPromptContent, RunnerError> {
            Err(RunnerError::Unsupported("mcp".into()))
        }
    }

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
        let loaded = load_resources(&paths, &FsTestRunner).unwrap();
        // `load_resources` always merges in the embedded builtin
        // resources baked into the binary, so we can't assert strict
        // counts — verify the workspace-layer entries we wrote are
        // present instead.
        assert!(loaded.agents.iter().any(|a| a.value.id == "default"));
        assert!(loaded.prompts.iter().any(|p| p.value.id == "plan"));
        assert!(loaded.hooks.iter().any(|h| h.value.id == "tool-end"));
        assert!(loaded.skills.iter().any(|s| s.value.name == "reviewer"));
        assert!(loaded.plugins.iter().any(|p| p.value.id == "example"));
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
        let loaded = load_resources(&paths, &FsTestRunner).unwrap();
        assert!(loaded.plugins.iter().any(|p| p.value.id == "example"));
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
    fn no_browser_filter_removes_builtin_browser_skill_references() {
        let mut resources = LoadedResources {
            skills: vec![LoadedItem {
                value: SkillSpec {
                    name: "browser".to_string(),
                    description: "Browser".to_string(),
                    content: "Use Browser".to_string(),
                    ..SkillSpec::default()
                },
                source_info: SourceInfo {
                    path: PathBuf::from("skills/browser/SKILL.md"),
                    kind: SourceKind::Builtin,
                },
            }],
            plugins: vec![LoadedItem {
                value: PluginSpec {
                    id: "puffer-builtins".to_string(),
                    display_name: "Puffer Builtins".to_string(),
                    description: String::new(),
                    commands: Vec::new(),
                    skills: vec!["reviewer".to_string(), "browser".to_string()],
                    agents: Vec::new(),
                    mcp_servers: Vec::new(),
                    lsp_servers: Vec::new(),
                },
                source_info: SourceInfo {
                    path: PathBuf::from("plugins/puffer-builtins.yaml"),
                    kind: SourceKind::Builtin,
                },
            }],
            ..LoadedResources::default()
        };

        filter_browser_resources(&mut resources, true);

        assert!(skill_by_name(&resources, "browser").is_none());
        assert_eq!(resources.plugins[0].value.skills, vec!["reviewer"]);
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
        let loaded = load_resources(&paths, &FsTestRunner).unwrap();
        // Embedded builtin skills get merged in too; verify the
        // workspace skill we wrote is present and addressable by
        // normalized names.
        assert!(loaded.skills.iter().any(|s| s.value.name == "review-helper"));
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
        let loaded = load_resources(&paths, &FsTestRunner).unwrap();
        let skill = &skill_by_name(&loaded, "review-helper")
            .expect("workspace skill should load")
            .value;
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
        let loaded = load_resources(&paths, &FsTestRunner).unwrap();
        // Embedded prompts also load; verify the `review` prompt
        // ultimately resolves to the workspace override.
        let review = prompt_by_id(&loaded, "review")
            .expect("review prompt should resolve");
        assert_eq!(review.value.description, "Workspace");
        assert!(review
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
        let loaded = load_resources(&paths, &FsTestRunner).unwrap();
        // The `z_review.yaml` override wins because it sorts after
        // `a_review.yaml` and load_yaml_dir is sorted.
        let review = prompt_by_id(&loaded, "review").expect("review");
        assert_eq!(review.value.description, "Second");
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
        let loaded = load_resources(&paths, &FsTestRunner).unwrap();
        // Embedded hooks may add more entries; verify our additions and
        // that the workspace override wins for `tool-end`.
        assert!(hook_by_id(&loaded, "tool-end").is_some());
        assert!(hook_by_id(&loaded, "tool-start").is_some());
        assert_eq!(
            hook_by_id(&loaded, "tool-end").unwrap().value.command,
            "echo workspace"
        );
        let tool_end_hooks = hooks_for_event(&loaded, "tool_end");
        assert!(tool_end_hooks.iter().any(|h| h.value.id == "tool-end"
            && h.value.command == "echo workspace"));
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
        let loaded = load_resources(&paths, &FsTestRunner).unwrap();
        // Embedded prompts are also loaded; verify the workspace
        // overrides our `system-base` id by checking variant resolution.
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
        assert_eq!(
            prompt_by_id(&loaded, "system-base").unwrap().value.template,
            "base body"
        );
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
        let loaded = load_resources(&paths, &FsTestRunner).unwrap();
        // Embedded prompts also load; verify the workspace variant
        // overrides the builtin opus variant for the same model.
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
        let loaded = load_resources(&paths, &FsTestRunner).unwrap();
        // Embedded MCP servers also load; verify the legacy entry is
        // present and sourced from the legacy directory.
        let legacy = loaded
            .mcp_servers
            .iter()
            .find(|item| item.value.id == "legacy")
            .expect("legacy MCP entry should be present");
        assert!(legacy
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
        let loaded = load_resources(&paths, &FsTestRunner).unwrap();
        let docs = loaded
            .mcp_servers
            .iter()
            .find(|item| item.value.id == "docs")
            .expect("docs MCP entry should be present");
        assert_eq!(docs.value.display_name, "Canonical Docs");
        assert!(docs
            .source_info
            .path
            .to_string_lossy()
            .contains("resources/mcp_servers/docs.yaml"));
        // The filesystem builtin layer overrides the embedded `docs`
        // entry shipped under `resources/mcp_servers/docs.yaml`, so a
        // diagnostic is expected — what we care about is that the
        // *canonical* directory wins over the legacy `mcp/` directory
        // (i.e. no `from .../resources/mcp/docs.yaml` provenance).
        assert!(!loaded
            .diagnostics
            .iter()
            .any(|item| item.contains("resources/mcp/docs.yaml")));
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
        let loaded = load_resources(&paths, &FsTestRunner).unwrap();
        // Embedded MCP servers are also loaded; verify the
        // workspace-layer `enabled` survives but `disabled` is dropped.
        assert!(loaded.mcp_servers.iter().any(|item| item.value.id == "enabled"));
        assert!(!loaded
            .mcp_servers
            .iter()
            .any(|item| item.value.id == "disabled"));
    }

    /// Smoke test: every embedded `resources/tools/*.yaml` must parse as a
    /// `ToolSpec`. Regression guard for bugs like browser.yaml's unquoted
    /// `actions: u` substring, which YAML silently parsed as a nested
    /// mapping and broke 6 of 14 `tool_visibility::*` tests on master
    /// before being fixed in this PR. A single broken file now fails this
    /// fast, with the offending path in the panic message.
    #[test]
    fn all_embedded_tool_yamls_parse() {
        let dir = BUILTIN_RESOURCES
            .get_dir("tools")
            .expect("embedded resources/tools directory");
        let mut files: Vec<_> = dir.files().collect();
        files.sort_by_key(|f| f.path().to_path_buf());

        let mut yaml_count = 0usize;
        for file in files {
            let path = file.path();
            let ext = path.extension().and_then(|e| e.to_str());
            if !matches!(ext, Some("yaml" | "yml")) {
                continue;
            }
            yaml_count += 1;
            let raw = file
                .contents_utf8()
                .unwrap_or_else(|| panic!("embedded tool yaml {} is not UTF-8", path.display()));
            if let Err(err) = serde_yaml::from_str::<ToolSpec>(raw) {
                panic!(
                    "embedded tool yaml {} failed to parse: {err}",
                    path.display()
                );
            }
        }

        assert!(
            yaml_count > 0,
            "expected at least one embedded tool yaml, found none"
        );
    }
}

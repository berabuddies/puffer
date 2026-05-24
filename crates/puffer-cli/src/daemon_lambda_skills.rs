use crate::daemon::DaemonState;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveLambdaSkillLibraryParams {
    id: String,
    root: String,
    #[serde(default)]
    generated_subpath: Option<String>,
    #[serde(default)]
    host_catalogue_subpath: Option<String>,
    #[serde(default)]
    compiler_path: Option<String>,
    #[serde(default)]
    allowed_tools: Vec<String>,
    #[serde(default)]
    host_tool_bindings: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    skill_host_tool_bindings: BTreeMap<String, BTreeMap<String, Vec<String>>>,
    #[serde(default = "default_lambda_skill_user_invocable")]
    user_invocable: bool,
    #[serde(default)]
    disable_model_invocation: bool,
    #[serde(default)]
    scope: Option<String>,
}

#[derive(Deserialize, Serialize, Default)]
struct LambdaSkillLibraryManifestDto {
    id: String,
    root: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    generated_subpath: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    host_catalogue_subpath: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    compiler_path: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    allowed_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    host_tool_bindings: BTreeMap<String, Vec<String>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    skill_host_tool_bindings: BTreeMap<String, BTreeMap<String, Vec<String>>>,
    #[serde(default = "default_lambda_skill_user_invocable")]
    user_invocable: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    disable_model_invocation: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LambdaSkillLibraryInfoDto {
    id: String,
    root: String,
    generated_subpath: Option<String>,
    host_catalogue_subpath: Option<String>,
    compiler_path: Option<String>,
    allowed_tools: Vec<String>,
    host_tool_bindings: BTreeMap<String, Vec<String>>,
    skill_host_tool_bindings: BTreeMap<String, BTreeMap<String, Vec<String>>>,
    user_invocable: bool,
    disable_model_invocation: bool,
    source_kind: String,
    source_path: String,
}

fn default_lambda_skill_user_invocable() -> bool {
    true
}

fn is_false(value: &bool) -> bool {
    !*value
}

pub(crate) fn handle_list_lambda_skill_libraries(state: &DaemonState) -> Result<Value> {
    lambda_skill_libraries_snapshot(state)
}

pub(crate) fn handle_save_lambda_skill_library(
    state: &DaemonState,
    params: &Value,
) -> Result<Value> {
    let params: SaveLambdaSkillLibraryParams = serde_json::from_value(params.clone())?;
    let id = params.id.trim();
    validate_lambda_skill_library_id(id)?;
    let root = params.root.trim();
    if root.is_empty() {
        anyhow::bail!("Lambda Skill library root is required");
    }
    let mut manifest = LambdaSkillLibraryManifestDto {
        id: id.to_string(),
        root: root.to_string(),
        generated_subpath: trimmed_optional(params.generated_subpath),
        host_catalogue_subpath: trimmed_optional(params.host_catalogue_subpath),
        compiler_path: trimmed_optional(params.compiler_path),
        allowed_tools: normalize_non_empty_list(params.allowed_tools),
        host_tool_bindings: normalize_tool_bindings(params.host_tool_bindings),
        skill_host_tool_bindings: normalize_skill_tool_bindings(params.skill_host_tool_bindings),
        user_invocable: params.user_invocable,
        disable_model_invocation: params.disable_model_invocation,
    };
    infer_missing_lambda_skill_manifest_fields(&mut manifest);
    let paths = state.config_paths();
    let dir = match params.scope.as_deref().unwrap_or("workspace") {
        "user" => paths
            .user_config_dir
            .join("resources/lambda_skill_libraries"),
        "local" | "project" | "workspace" => paths
            .workspace_config_dir
            .join("resources/lambda_skill_libraries"),
        other => anyhow::bail!("unsupported Lambda Skill library scope `{other}`"),
    };
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{id}.yaml"));
    std::fs::write(&path, serde_yaml::to_string(&manifest)?)
        .with_context(|| format!("write {}", path.display()))?;
    lambda_skill_libraries_snapshot(state)
}

fn lambda_skill_libraries_snapshot(state: &DaemonState) -> Result<Value> {
    let (doctor, warnings) = state.lambda_skill_doctor_snapshot()?;
    let paths = state.config_paths();
    let workspace_dir = paths
        .workspace_config_dir
        .join("resources/lambda_skill_libraries");
    let user_dir = paths
        .user_config_dir
        .join("resources/lambda_skill_libraries");
    Ok(json!({
        "directories": {
            "workspace": workspace_dir.display().to_string(),
            "user": user_dir.display().to_string(),
        },
        "libraries": lambda_skill_library_manifest_dtos(&workspace_dir, "workspace")?
            .into_iter()
            .chain(lambda_skill_library_manifest_dtos(&user_dir, "user")?)
            .collect::<Vec<_>>(),
        "doctor": doctor,
        "warnings": warnings,
    }))
}

fn lambda_skill_library_manifest_dtos(
    dir: &Path,
    source_kind: &str,
) -> Result<Vec<LambdaSkillLibraryInfoDto>> {
    let mut items = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(items),
        Err(error) => return Err(error).with_context(|| format!("read {}", dir.display())),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() || !is_yaml_path(&path) {
            continue;
        }
        let text =
            std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let manifest: LambdaSkillLibraryManifestDto =
            serde_yaml::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
        items.push(LambdaSkillLibraryInfoDto {
            id: manifest.id,
            root: manifest.root,
            generated_subpath: manifest.generated_subpath,
            host_catalogue_subpath: manifest.host_catalogue_subpath,
            compiler_path: manifest.compiler_path,
            allowed_tools: manifest.allowed_tools,
            host_tool_bindings: manifest.host_tool_bindings,
            skill_host_tool_bindings: manifest.skill_host_tool_bindings,
            user_invocable: manifest.user_invocable,
            disable_model_invocation: manifest.disable_model_invocation,
            source_kind: source_kind.to_string(),
            source_path: path.display().to_string(),
        });
    }
    items.sort_by(|left, right| {
        left.source_kind
            .cmp(&right.source_kind)
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(items)
}

fn validate_lambda_skill_library_id(id: &str) -> Result<()> {
    if id.is_empty() {
        anyhow::bail!("Lambda Skill library id is required");
    }
    if !id
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.')
    {
        anyhow::bail!(
            "Lambda Skill library id may only contain letters, numbers, dots, dashes, and underscores"
        );
    }
    Ok(())
}

fn trimmed_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn normalize_non_empty_list(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect()
}

fn normalize_tool_bindings(
    bindings: BTreeMap<String, Vec<String>>,
) -> BTreeMap<String, Vec<String>> {
    bindings
        .into_iter()
        .filter_map(|(tool, concrete)| {
            let tool = tool.trim().to_string();
            if tool.is_empty() {
                return None;
            }
            let concrete = normalize_non_empty_list(concrete);
            (!concrete.is_empty()).then_some((tool, concrete))
        })
        .collect()
}

fn normalize_skill_tool_bindings(
    bindings: BTreeMap<String, BTreeMap<String, Vec<String>>>,
) -> BTreeMap<String, BTreeMap<String, Vec<String>>> {
    bindings
        .into_iter()
        .filter_map(|(skill, tool_bindings)| {
            let skill = skill.trim().to_string();
            if skill.is_empty() {
                return None;
            }
            let tool_bindings = normalize_tool_bindings(tool_bindings);
            (!tool_bindings.is_empty()).then_some((skill, tool_bindings))
        })
        .collect()
}

fn is_yaml_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension, "yaml" | "yml"))
}

#[derive(Deserialize)]
struct HostCatalogueForInference {
    #[serde(default)]
    tools: Vec<HostToolForInference>,
}

#[derive(Deserialize)]
struct HostToolForInference {
    #[serde(default, rename = "concreteTools", alias = "concrete_tools")]
    concrete_tools: Vec<String>,
}

fn infer_missing_lambda_skill_manifest_fields(manifest: &mut LambdaSkillLibraryManifestDto) {
    let root = PathBuf::from(&manifest.root);
    if manifest.host_catalogue_subpath.is_none() && manifest.allowed_tools.is_empty() {
        if let Some(allowed_tools) = infer_allowed_tools_from_default_host_catalogues(&root) {
            if !allowed_tools.is_empty() {
                manifest.host_catalogue_subpath = Some("out/host.json".to_string());
                manifest.allowed_tools = allowed_tools;
                return;
            }
        }
    }
    if manifest.compiler_path.is_none() {
        manifest.compiler_path =
            discover_lskillc_for_library(&root).map(|path| path.display().to_string());
    }
    if manifest.allowed_tools.is_empty() || manifest.host_tool_bindings.is_empty() {
        let inferred = infer_bindings_from_lskill_sources(&root);
        if manifest.host_tool_bindings.is_empty() && !inferred.host_tool_bindings.is_empty() {
            manifest.host_tool_bindings = inferred.host_tool_bindings;
        }
        if manifest.allowed_tools.is_empty() && !inferred.allowed_tools.is_empty() {
            manifest.allowed_tools = inferred.allowed_tools;
        }
    }
}

fn infer_allowed_tools_from_default_host_catalogues(root: &Path) -> Option<Vec<String>> {
    let mut catalogues = Vec::new();
    collect_default_host_catalogues(root, &mut catalogues);
    if catalogues.is_empty() {
        return None;
    }
    let mut tools = BTreeSet::new();
    for catalogue in catalogues {
        let raw = std::fs::read_to_string(catalogue).ok()?;
        let parsed: HostCatalogueForInference = serde_json::from_str(&raw).ok()?;
        for tool in parsed.tools {
            for concrete in tool.concrete_tools {
                let concrete = concrete.trim();
                if !concrete.is_empty() {
                    tools.insert(concrete.to_string());
                }
            }
        }
    }
    Some(tools.into_iter().collect())
}

#[derive(Default)]
struct LambdaSkillBindingInference {
    allowed_tools: Vec<String>,
    host_tool_bindings: BTreeMap<String, Vec<String>>,
}

fn infer_bindings_from_lskill_sources(root: &Path) -> LambdaSkillBindingInference {
    let mut source_paths = Vec::new();
    collect_lambda_skill_sources(root, &mut source_paths);
    let mut bindings = BTreeMap::<String, BTreeSet<String>>::new();
    let mut allowed_tools = BTreeSet::<String>::new();
    for source_path in source_paths {
        let Ok(source) = std::fs::read_to_string(source_path) else {
            continue;
        };
        for tool in host_tools_from_lskill_source(&source) {
            let concrete_tools = concrete_tools_for_host_effects(&tool.effects);
            for concrete_tool in concrete_tools {
                allowed_tools.insert(concrete_tool.clone());
                bindings
                    .entry(tool.name.clone())
                    .or_default()
                    .insert(concrete_tool);
            }
        }
    }
    LambdaSkillBindingInference {
        allowed_tools: allowed_tools.into_iter().collect(),
        host_tool_bindings: bindings
            .into_iter()
            .map(|(tool, concrete)| (tool, concrete.into_iter().collect()))
            .collect(),
    }
}

fn collect_lambda_skill_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    if is_ignored_library_dir(dir) {
        return;
    }
    let skill_source = dir.join("skill.lskill");
    if skill_source.is_file() {
        out.push(skill_source);
        return;
    }
    let main_source = dir.join("main.lskill");
    if main_source.is_file() {
        out.push(main_source);
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_lambda_skill_sources(&path, out);
        }
    }
}

#[derive(Debug)]
struct HostToolSourceInference {
    name: String,
    effects: Vec<String>,
}

fn host_tools_from_lskill_source(source: &str) -> Vec<HostToolSourceInference> {
    let mut tools = Vec::new();
    let lines = source.lines().collect::<Vec<_>>();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index].trim_start();
        if !line.starts_with("tool ") {
            index += 1;
            continue;
        }
        let Some(name) = host_tool_name_from_line(line) else {
            index += 1;
            continue;
        };
        let mut block = String::new();
        let mut depth = 0_i32;
        let mut saw_open = false;
        while index < lines.len() {
            let current = lines[index];
            block.push_str(current);
            block.push('\n');
            for ch in current.chars() {
                match ch {
                    '{' => {
                        saw_open = true;
                        depth += 1;
                    }
                    '}' if saw_open => depth -= 1,
                    _ => {}
                }
            }
            index += 1;
            if saw_open && depth <= 0 {
                break;
            }
        }
        tools.push(HostToolSourceInference {
            name,
            effects: effects_from_lskill_tool_block(&block),
        });
    }
    tools
}

fn host_tool_name_from_line(line: &str) -> Option<String> {
    let rest = line.strip_prefix("tool ")?;
    let name = rest
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_' || *ch == '-')
        .collect::<String>();
    (!name.is_empty()).then_some(name)
}

fn effects_from_lskill_tool_block(block: &str) -> Vec<String> {
    let Some(effects_start) = block.find("effects") else {
        return Vec::new();
    };
    let after_effects = &block[effects_start..];
    let Some(open) = after_effects.find('[') else {
        return Vec::new();
    };
    let after_open = &after_effects[open + 1..];
    let Some(close) = after_open.find(']') else {
        return Vec::new();
    };
    after_open[..close]
        .split(',')
        .map(|effect| effect.trim().trim_matches('"').to_string())
        .filter(|effect| !effect.is_empty())
        .collect()
}

fn concrete_tools_for_host_effects(effects: &[String]) -> Vec<String> {
    let mut tools = BTreeSet::new();
    if effects.is_empty() {
        tools.insert("AskUserQuestion".to_string());
    }
    for effect in effects {
        match effect.as_str() {
            "fs_r" => {
                tools.insert("Read".to_string());
                tools.insert("Glob".to_string());
                tools.insert("Grep".to_string());
                tools.insert("Bash".to_string());
            }
            "fs_w" => {
                tools.insert("Edit".to_string());
                tools.insert("Write".to_string());
                tools.insert("Bash".to_string());
            }
            "net_r" => {
                tools.insert("WebFetch".to_string());
                tools.insert("WebSearch".to_string());
                tools.insert("Bash".to_string());
            }
            "net_w" | "proc" | "sign" => {
                tools.insert("Bash".to_string());
            }
            "user_in" => {
                tools.insert("AskUserQuestion".to_string());
            }
            _ => {
                tools.insert("Bash".to_string());
            }
        }
    }
    tools.into_iter().collect()
}

fn discover_lskillc_for_library(root: &Path) -> Option<PathBuf> {
    configured_lskillc_from_env().or_else(|| {
        root.ancestors()
            .flat_map(lskillc_candidates_under)
            .find(|candidate| candidate.is_file())
    })
}

fn configured_lskillc_from_env() -> Option<PathBuf> {
    ["PUFFER_LSKILLC", "LSKILLC"]
        .into_iter()
        .filter_map(|name| std::env::var_os(name).map(PathBuf::from))
        .find(|path| path.is_file())
}

fn lskillc_candidates_under(root: &Path) -> Vec<PathBuf> {
    vec![
        root.join("lean/LambdaW/.lake/build/bin/lskillc"),
        root.join("LambdaW/.lake/build/bin/lskillc"),
        root.join(".lake/build/bin/lskillc"),
        root.join("bin/lskillc"),
        root.join("lskillc"),
    ]
}

fn collect_default_host_catalogues(dir: &Path, out: &mut Vec<PathBuf>) {
    if is_ignored_library_dir(dir) {
        return;
    }
    let host_path = dir.join("out/host.json");
    if host_path.is_file() {
        out.push(host_path);
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_default_host_catalogues(&path, out);
        }
    }
}

fn is_ignored_library_dir(dir: &Path) -> bool {
    dir.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.starts_with('.') || matches!(name, "node_modules" | "target" | "out")
        })
}

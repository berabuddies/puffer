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
    disabled_skills: Vec<String>,
    #[serde(default)]
    scope: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetLambdaSkillEnabledParams {
    library_id: String,
    source_kind: String,
    skill_name: String,
    enabled: bool,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    disabled_skills: Vec<String>,
}

#[derive(Clone, Serialize)]
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
    disabled_skills: Vec<String>,
    source_kind: String,
    source_path: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LambdaVerifiedSkillInfoDto {
    name: String,
    description: String,
    library_id: Option<String>,
    library_root: Option<String>,
    source_kind: Option<String>,
    source_path: Option<String>,
    generated_path: Option<String>,
    ready: bool,
    enabled: bool,
    model_invocable: bool,
    gate_source: Option<String>,
    failure_reason: Option<String>,
    allowed_tools: Vec<String>,
    tools: Option<usize>,
    actions: Option<usize>,
}

#[derive(Default, Deserialize)]
struct GeneratedSkillFrontmatter {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Default, Deserialize)]
struct LambdaSkillStatsDto {
    tools: Option<usize>,
    actions: Option<usize>,
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
        compiler_path: None,
        allowed_tools: normalize_non_empty_list(params.allowed_tools),
        host_tool_bindings: normalize_tool_bindings(params.host_tool_bindings),
        skill_host_tool_bindings: normalize_skill_tool_bindings(params.skill_host_tool_bindings),
        user_invocable: params.user_invocable,
        disable_model_invocation: params.disable_model_invocation,
        disabled_skills: normalize_lambda_skill_names(params.disabled_skills),
    };
    infer_missing_lambda_skill_manifest_fields(&mut manifest);
    validate_lambda_skill_library_import(&manifest)?;
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

pub(crate) fn handle_set_lambda_skill_enabled(
    state: &DaemonState,
    params: &Value,
) -> Result<Value> {
    let params: SetLambdaSkillEnabledParams = serde_json::from_value(params.clone())?;
    let id = params.library_id.trim();
    validate_lambda_skill_library_id(id)?;
    let skill_name = normalize_lambda_skill_name(params.skill_name.trim());
    if skill_name.is_empty() {
        anyhow::bail!("Lambda Skill name is required");
    }
    let path = lambda_skill_manifest_path(state, params.source_kind.trim(), id)?;
    let raw = std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let mut manifest: LambdaSkillLibraryManifestDto =
        serde_yaml::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
    manifest.disabled_skills = normalize_lambda_skill_names(manifest.disabled_skills);
    if params.enabled {
        manifest.disabled_skills.retain(|name| name != &skill_name);
        manifest.disable_model_invocation = false;
    } else if !manifest
        .disabled_skills
        .iter()
        .any(|name| name == &skill_name)
    {
        manifest.disabled_skills.push(skill_name);
        manifest.disabled_skills.sort();
    }
    std::fs::write(&path, serde_yaml::to_string(&manifest)?)
        .with_context(|| format!("write {}", path.display()))?;
    lambda_skill_libraries_snapshot(state)
}

fn lambda_skill_libraries_snapshot(state: &DaemonState) -> Result<Value> {
    let paths = state.config_paths();
    let workspace_dir = paths
        .workspace_config_dir
        .join("resources/lambda_skill_libraries");
    let user_dir = paths
        .user_config_dir
        .join("resources/lambda_skill_libraries");
    let libraries = if workspace_dir == user_dir {
        lambda_skill_library_manifest_dtos(&workspace_dir, "workspace")?
    } else {
        lambda_skill_library_manifest_dtos(&workspace_dir, "workspace")?
            .into_iter()
            .chain(lambda_skill_library_manifest_dtos(&user_dir, "user")?)
            .collect::<Vec<_>>()
    };
    let skills = lambda_verified_skill_dtos(&libraries);
    let doctor = lambda_desktop_doctor_summary(&skills);
    let warnings = lambda_desktop_warning_lines(&skills);
    Ok(json!({
        "directories": {
            "workspace": workspace_dir.display().to_string(),
            "user": user_dir.display().to_string(),
        },
        "libraries": libraries,
        "skills": skills,
        "doctor": doctor,
        "warnings": warnings,
    }))
}

fn lambda_skill_manifest_path(state: &DaemonState, source_kind: &str, id: &str) -> Result<PathBuf> {
    let paths = state.config_paths();
    let dir = match source_kind {
        "user" => paths
            .user_config_dir
            .join("resources/lambda_skill_libraries"),
        "local" | "project" | "workspace" => paths
            .workspace_config_dir
            .join("resources/lambda_skill_libraries"),
        other => anyhow::bail!("unsupported Lambda Skill library scope `{other}`"),
    };
    Ok(dir.join(format!("{id}.yaml")))
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
        let mut manifest: LambdaSkillLibraryManifestDto =
            serde_yaml::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
        infer_missing_lambda_skill_manifest_fields(&mut manifest);
        items.push(LambdaSkillLibraryInfoDto {
            id: manifest.id,
            root: manifest.root,
            generated_subpath: manifest.generated_subpath,
            host_catalogue_subpath: manifest.host_catalogue_subpath,
            compiler_path: manifest.compiler_path,
            allowed_tools: manifest.allowed_tools,
            host_tool_bindings: BTreeMap::new(),
            skill_host_tool_bindings: BTreeMap::new(),
            user_invocable: manifest.user_invocable,
            disable_model_invocation: manifest.disable_model_invocation,
            disabled_skills: normalize_lambda_skill_names(manifest.disabled_skills),
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

fn lambda_verified_skill_dtos(
    libraries: &[LambdaSkillLibraryInfoDto],
) -> Vec<LambdaVerifiedSkillInfoDto> {
    let mut skills = libraries
        .iter()
        .flat_map(lambda_verified_skill_dtos_for_library)
        .collect::<Vec<_>>();
    skills.sort_by(|left, right| {
        left.source_kind
            .cmp(&right.source_kind)
            .then_with(|| left.library_id.cmp(&right.library_id))
            .then_with(|| left.name.cmp(&right.name))
    });
    skills
}

fn lambda_verified_skill_dtos_for_library(
    library: &LambdaSkillLibraryInfoDto,
) -> Vec<LambdaVerifiedSkillInfoDto> {
    let root = resolved_lambda_skill_library_root(library);
    let mut skill_dirs = Vec::new();
    collect_lambda_skill_dirs_for_snapshot(&root, &mut skill_dirs);
    skill_dirs.sort();
    skill_dirs
        .into_iter()
        .filter_map(|skill_dir| lambda_verified_skill_dto_for_dir(library, &skill_dir))
        .collect()
}

fn lambda_verified_skill_dto_for_dir(
    library: &LambdaSkillLibraryInfoDto,
    skill_dir: &Path,
) -> Option<LambdaVerifiedSkillInfoDto> {
    let source_path = lambda_skill_source_path_for_snapshot(skill_dir)?;
    let generated_path = skill_dir.join(
        library
            .generated_subpath
            .as_deref()
            .unwrap_or("out/GENERATED.SKILL.md"),
    );
    let raw = std::fs::read_to_string(&generated_path).ok()?;
    let (frontmatter, body) = parse_generated_skill_descriptor(&raw).ok()?;
    let raw_name = frontmatter.name.unwrap_or_else(|| {
        skill_dir
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "lambda-skill".to_string())
    });
    let mut name = normalize_lambda_skill_name(&raw_name);
    if name.is_empty() {
        name = "skill".to_string();
    }
    let description = frontmatter
        .description
        .unwrap_or_else(|| first_descriptive_line(body).to_string());
    let stats = load_lambda_skill_stats_for_snapshot(&skill_dir.join("out/stats.json"));
    let host_catalogue_path = library
        .host_catalogue_subpath
        .as_deref()
        .map(|subpath| skill_dir.join(subpath));
    let readiness =
        lambda_desktop_readiness(&library.allowed_tools, host_catalogue_path.as_deref());
    let enabled = !library.disable_model_invocation
        && !library
            .disabled_skills
            .iter()
            .any(|disabled| disabled == &name);
    Some(LambdaVerifiedSkillInfoDto {
        name,
        description,
        library_id: Some(library.id.clone()),
        library_root: Some(library.root.clone()),
        source_kind: Some(library.source_kind.clone()),
        source_path: Some(source_path.display().to_string()),
        generated_path: Some(generated_path.display().to_string()),
        ready: readiness.failure_reason.is_none(),
        enabled,
        model_invocable: enabled && readiness.failure_reason.is_none(),
        gate_source: readiness.gate_source,
        failure_reason: readiness.failure_reason,
        allowed_tools: library.allowed_tools.clone(),
        tools: stats.as_ref().and_then(|stats| stats.tools),
        actions: stats.as_ref().and_then(|stats| stats.actions),
    })
}

struct LambdaDesktopReadiness {
    gate_source: Option<String>,
    failure_reason: Option<String>,
}

fn lambda_desktop_readiness(
    allowed_tools: &[String],
    host_catalogue_path: Option<&Path>,
) -> LambdaDesktopReadiness {
    if let Some(host_catalogue_path) = host_catalogue_path {
        if !host_catalogue_path.is_file() {
            return lambda_desktop_not_ready("host catalogue not found");
        }
        if allowed_tools.is_empty() {
            return lambda_desktop_not_ready("missing concrete tool scope");
        }
        return lambda_desktop_ready("host catalogue");
    }
    lambda_desktop_not_ready("missing precompiled host catalogue")
}

fn lambda_desktop_ready(source: &str) -> LambdaDesktopReadiness {
    LambdaDesktopReadiness {
        gate_source: Some(source.to_string()),
        failure_reason: None,
    }
}

fn lambda_desktop_not_ready(reason: impl Into<String>) -> LambdaDesktopReadiness {
    LambdaDesktopReadiness {
        gate_source: None,
        failure_reason: Some(reason.into()),
    }
}

fn lambda_desktop_doctor_summary(skills: &[LambdaVerifiedSkillInfoDto]) -> String {
    let ready = skills.iter().filter(|skill| skill.ready).count();
    let model_invocable = skills.iter().filter(|skill| skill.model_invocable).count();
    let missing_gate_config = skills.len().saturating_sub(ready);
    format!(
        "lambda_skills={} model_invocable={} missing_gate_config={} desktop_preflight=lightweight",
        skills.len(),
        model_invocable,
        missing_gate_config
    )
}

fn lambda_desktop_warning_lines(skills: &[LambdaVerifiedSkillInfoDto]) -> Vec<String> {
    skills
        .iter()
        .filter_map(|skill| {
            skill
                .failure_reason
                .as_ref()
                .map(|reason| format!("{}; {}", skill.name, reason))
        })
        .collect()
}

fn resolved_lambda_skill_library_root(library: &LambdaSkillLibraryInfoDto) -> PathBuf {
    let root = PathBuf::from(&library.root);
    if root.is_absolute() {
        return root;
    }
    PathBuf::from(&library.source_path)
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join(root)
}

fn collect_lambda_skill_dirs_for_snapshot(dir: &Path, out: &mut Vec<PathBuf>) {
    if is_ignored_library_dir(dir) {
        return;
    }
    if lambda_skill_source_path_for_snapshot(dir).is_some() {
        out.push(dir.to_path_buf());
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_lambda_skill_dirs_for_snapshot(&path, out);
        }
    }
}

fn lambda_skill_source_path_for_snapshot(dir: &Path) -> Option<PathBuf> {
    let skill_source = dir.join("skill.lskill");
    if skill_source.is_file() {
        return Some(skill_source);
    }
    let main_source = dir.join("main.lskill");
    main_source.is_file().then_some(main_source)
}

fn parse_generated_skill_descriptor(raw: &str) -> Result<(GeneratedSkillFrontmatter, &str)> {
    let Some(rest) = raw.strip_prefix("---") else {
        return Ok((GeneratedSkillFrontmatter::default(), raw));
    };
    let rest = rest.strip_prefix('\n').unwrap_or(rest);
    let Some(end) = rest.find("\n---") else {
        return Ok((GeneratedSkillFrontmatter::default(), raw));
    };
    let frontmatter = serde_yaml::from_str(&rest[..end]).unwrap_or_default();
    let body = rest[end + "\n---".len()..]
        .strip_prefix('\n')
        .unwrap_or(&rest[end + "\n---".len()..]);
    Ok((frontmatter, body))
}

fn first_descriptive_line(body: &str) -> &str {
    body.lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .unwrap_or("")
}

fn load_lambda_skill_stats_for_snapshot(path: &Path) -> Option<LambdaSkillStatsDto> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
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

fn normalize_lambda_skill_names(values: Vec<String>) -> Vec<String> {
    let mut names = values
        .into_iter()
        .map(|value| normalize_lambda_skill_name(&value))
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    names
}

fn normalize_lambda_skill_name(raw: &str) -> String {
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
    normalized.trim_matches('-').to_string()
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
    #[serde(default)]
    name: Option<String>,
    #[serde(default, rename = "concreteTools", alias = "concrete_tools")]
    concrete_tools: Vec<String>,
}

fn infer_missing_lambda_skill_manifest_fields(manifest: &mut LambdaSkillLibraryManifestDto) {
    let root = PathBuf::from(&manifest.root);
    if manifest.host_catalogue_subpath.is_none() {
        if let Some(allowed_tools) = infer_allowed_tools_from_default_host_catalogues(&root) {
            manifest.host_catalogue_subpath = Some("out/host.json".to_string());
            if manifest.allowed_tools.is_empty() && !allowed_tools.is_empty() {
                manifest.allowed_tools = allowed_tools;
            }
        }
    }
}

fn validate_lambda_skill_library_import(manifest: &LambdaSkillLibraryManifestDto) -> Result<()> {
    let root = PathBuf::from(&manifest.root);
    if !root.is_dir() {
        anyhow::bail!(
            "Verified Skills folder does not exist or is not a directory: {}",
            root.display()
        );
    }

    let mut skill_dirs = Vec::new();
    collect_lambda_skill_dirs_for_snapshot(&root, &mut skill_dirs);
    skill_dirs.sort();
    if skill_dirs.is_empty() {
        anyhow::bail!(
            "No Verified Skills found in {}; choose a folder containing skill.lskill or main.lskill files",
            root.display()
        );
    }

    let generated_subpath = manifest
        .generated_subpath
        .as_deref()
        .unwrap_or("out/GENERATED.SKILL.md");
    let host_catalogue_subpath = manifest
        .host_catalogue_subpath
        .as_deref()
        .unwrap_or("out/host.json");
    let mut missing_generated = Vec::new();
    let mut missing_host = Vec::new();
    let mut invalid_host = Vec::new();
    let mut missing_concrete = Vec::new();

    for skill_dir in &skill_dirs {
        let generated_path = skill_dir.join(generated_subpath);
        if !generated_path.is_file() {
            missing_generated.push(display_relative_to(&root, &generated_path));
        }

        let host_path = skill_dir.join(host_catalogue_subpath);
        if !host_path.is_file() {
            missing_host.push(display_relative_to(&root, &host_path));
            continue;
        }
        match host_catalogue_missing_concrete_tools(&host_path) {
            Ok(missing) => {
                missing_concrete.extend(
                    missing
                        .into_iter()
                        .map(|tool| format!("{}:{tool}", display_relative_to(&root, &host_path))),
                );
            }
            Err(error) => invalid_host.push(format!(
                "{} ({error})",
                display_relative_to(&root, &host_path)
            )),
        }
    }

    if !missing_generated.is_empty() {
        anyhow::bail!(
            "Verified Skills import is incomplete: missing generated skill descriptors at {}",
            format_examples(&missing_generated)
        );
    }
    if !missing_host.is_empty() {
        anyhow::bail!(
            "Verified Skills import is incomplete: missing precompiled host catalogues at {}. Run `lskillc export-json <skill.lskill> -o <skill>/out/host.json` before importing.",
            format_examples(&missing_host)
        );
    }
    if !invalid_host.is_empty() {
        anyhow::bail!(
            "Verified Skills import is incomplete: invalid host catalogues at {}",
            format_examples(&invalid_host)
        );
    }
    if !missing_concrete.is_empty() {
        anyhow::bail!(
            "Verified Skills import is incomplete: host catalogues lack concreteTools bindings at {}",
            format_examples(&missing_concrete)
        );
    }
    if manifest.host_catalogue_subpath.is_none() {
        anyhow::bail!(
            "Verified Skills import is incomplete: set host_catalogue_subpath or provide default out/host.json catalogues"
        );
    }
    if manifest.allowed_tools.is_empty() {
        anyhow::bail!(
            "Verified Skills import is incomplete: allowed_tools could not be inferred from concreteTools bindings"
        );
    }
    Ok(())
}

fn host_catalogue_missing_concrete_tools(path: &Path) -> Result<Vec<String>> {
    let raw = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let parsed: HostCatalogueForInference =
        serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
    let mut missing = Vec::new();
    for (index, tool) in parsed.tools.into_iter().enumerate() {
        if tool
            .concrete_tools
            .iter()
            .all(|concrete| concrete.trim().is_empty())
        {
            missing.push(tool.name.unwrap_or_else(|| format!("tool#{index}")));
        }
    }
    Ok(missing)
}

fn display_relative_to(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn format_examples(values: &[String]) -> String {
    let mut examples = values.iter().take(5).cloned().collect::<Vec<_>>();
    if values.len() > examples.len() {
        examples.push(format!("and {} more", values.len() - examples.len()));
    }
    examples.join(", ")
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

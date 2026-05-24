use crate::daemon::DaemonState;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;

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
    let manifest = LambdaSkillLibraryManifestDto {
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

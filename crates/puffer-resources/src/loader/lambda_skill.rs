use super::{
    first_descriptive_line, frontmatter_string, normalize_skill_name, runner_path_exists,
    split_frontmatter,
};
use crate::model::{LoadedItem, SkillSpec, SkillVerificationSpec, SourceInfo, SourceKind};
use anyhow::{anyhow, Context, Result};
use puffer_runner_api::{RunnerError, ToolRunner};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Declares an external Lambda Skill library resource.
#[derive(Debug, Clone, Default, Deserialize)]
pub(super) struct LambdaSkillLibrarySpec {
    id: String,
    root: String,
    #[serde(default)]
    generated_subpath: Option<String>,
    #[serde(default)]
    host_catalogue_subpath: Option<String>,
    #[serde(default)]
    allowed_tools: Vec<String>,
    #[serde(default, alias = "tool_bindings")]
    host_tool_bindings: BTreeMap<String, Vec<String>>,
    #[serde(
        default,
        alias = "per_skill_host_tool_bindings",
        alias = "skill_tool_bindings"
    )]
    skill_host_tool_bindings: BTreeMap<String, BTreeMap<String, Vec<String>>>,
    #[serde(default = "default_lambda_skill_user_invocable")]
    user_invocable: bool,
    #[serde(default = "default_lambda_skill_disable_model_invocation")]
    disable_model_invocation: bool,
    #[serde(
        default,
        alias = "requireApproval",
        alias = "require_concrete_tool_approval"
    )]
    require_approval: bool,
    #[serde(default, alias = "disabledSkills")]
    disabled_skills: Vec<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    effort: Option<String>,
    #[serde(default)]
    context: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct LambdaSkillStats {
    tools: Option<usize>,
    actions: Option<usize>,
}

fn default_lambda_skill_user_invocable() -> bool {
    true
}

fn default_lambda_skill_disable_model_invocation() -> bool {
    false
}

/// Loads Lambda Skills declared by resource manifests.
pub(super) fn load_lambda_skill_libraries(
    runner: &dyn ToolRunner,
    libraries: &[LoadedItem<LambdaSkillLibrarySpec>],
    diagnostics: &mut Vec<String>,
) -> Result<Vec<LoadedItem<SkillSpec>>> {
    let mut items = Vec::new();
    for (library, root) in effective_lambda_skill_libraries(libraries) {
        items.extend(load_lambda_skill_library(
            runner,
            &library.value,
            &library.source_info,
            &root,
            diagnostics,
        )?);
    }
    Ok(items)
}

fn effective_lambda_skill_libraries<'a>(
    libraries: &'a [LoadedItem<LambdaSkillLibrarySpec>],
) -> Vec<(&'a LoadedItem<LambdaSkillLibrarySpec>, PathBuf)> {
    let mut candidates = libraries
        .iter()
        .map(|library| {
            let root = resolve_lambda_skill_root(&library.value.root, &library.source_info.path);
            (clean_path(&root), library, root)
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|(left_key, left, _), (right_key, right, _)| {
        left_key
            .components()
            .count()
            .cmp(&right_key.components().count())
            .then_with(|| {
                source_kind_priority(left.source_info.kind)
                    .cmp(&source_kind_priority(right.source_info.kind))
            })
            .then_with(|| left.value.id.cmp(&right.value.id))
    });

    let mut kept: Vec<(PathBuf, &'a LoadedItem<LambdaSkillLibrarySpec>, PathBuf)> = Vec::new();
    for (key, library, root) in candidates {
        if kept
            .iter()
            .any(|(kept_key, _, _)| path_contains(kept_key, &key))
        {
            continue;
        }
        kept.push((key, library, root));
    }

    kept.into_iter()
        .map(|(_, library, root)| (library, root))
        .collect()
}

fn source_kind_priority(kind: SourceKind) -> usize {
    match kind {
        SourceKind::Workspace => 0,
        SourceKind::User => 1,
        SourceKind::Builtin => 2,
    }
}

fn load_lambda_skill_library(
    runner: &dyn ToolRunner,
    spec: &LambdaSkillLibrarySpec,
    source_info: &SourceInfo,
    root: &Path,
    diagnostics: &mut Vec<String>,
) -> Result<Vec<LoadedItem<SkillSpec>>> {
    if !runner_path_exists(runner, root) {
        diagnostics.push(format!(
            "lambda_skill_library `{}` skipped: root not found at {}",
            spec.id,
            root.display()
        ));
        return Ok(Vec::new());
    }

    let mut skill_dirs = Vec::new();
    collect_lambda_skill_dirs(runner, root, &mut skill_dirs)?;
    skill_dirs.sort();
    let mut items = Vec::new();
    for skill_dir in skill_dirs {
        match load_lambda_skill_dir(runner, spec, source_info, &skill_dir) {
            Ok(Some(item)) => items.push(item),
            Ok(None) => {}
            Err(error) => diagnostics.push(format!(
                "lambda_skill_library `{}` skipped {}: {error:#}",
                spec.id,
                skill_dir.display()
            )),
        }
    }
    Ok(items)
}

fn collect_lambda_skill_dirs(
    runner: &dyn ToolRunner,
    dir: &Path,
    out: &mut Vec<PathBuf>,
) -> Result<()> {
    if is_ignored_lambda_skill_dir(dir) {
        return Ok(());
    }
    if lambda_skill_source_path(runner, dir).is_some() {
        out.push(dir.to_path_buf());
        return Ok(());
    }
    let entries = match runner.list_dir(dir) {
        Ok(mut entries) => {
            entries.sort_by(|left, right| left.path.cmp(&right.path));
            entries
        }
        Err(RunnerError::NotFound(_)) => return Ok(()),
        Err(err) => {
            return Err(anyhow!(err))
                .with_context(|| format!("failed to list lambda skill dir {}", dir.display()))
        }
    };
    for entry in entries {
        if entry.is_dir {
            collect_lambda_skill_dirs(runner, &entry.path, out)?;
        }
    }
    Ok(())
}

fn load_lambda_skill_dir(
    runner: &dyn ToolRunner,
    spec: &LambdaSkillLibrarySpec,
    source_info: &SourceInfo,
    skill_dir: &Path,
) -> Result<Option<LoadedItem<SkillSpec>>> {
    let Some(lambda_source_path) = lambda_skill_source_path(runner, skill_dir) else {
        return Ok(None);
    };
    let generated_subpath = spec
        .generated_subpath
        .as_deref()
        .unwrap_or("out/GENERATED.SKILL.md");
    let generated_path = skill_dir.join(generated_subpath);
    let raw_bytes = match runner.read_file(&generated_path) {
        Ok(bytes) => bytes,
        Err(RunnerError::NotFound(_)) => {
            return Err(anyhow!(
                "generated descriptor not found at {}",
                generated_path.display()
            ))
        }
        Err(err) => {
            return Err(anyhow!(err))
                .with_context(|| format!("failed to read {}", generated_path.display()))
        }
    };
    let raw = String::from_utf8(raw_bytes).with_context(|| {
        format!(
            "lambda skill file {} is not UTF-8",
            generated_path.display()
        )
    })?;
    let (frontmatter, body) = split_frontmatter(&raw).with_context(|| {
        format!(
            "failed to parse skill frontmatter {}",
            generated_path.display()
        )
    })?;
    let raw_name = frontmatter_string(&frontmatter, &["name"]).unwrap_or_else(|| {
        skill_dir
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "lambda-skill".to_string())
    });
    let name = normalize_skill_name(&raw_name);
    let description = frontmatter_string(&frontmatter, &["description"])
        .unwrap_or_else(|| first_descriptive_line(&body).to_string());
    let stats = load_lambda_skill_stats(runner, &skill_dir.join("out/stats.json"));
    let host_catalogue_path = spec
        .host_catalogue_subpath
        .as_deref()
        .map(|subpath| skill_dir.join(subpath));

    let host_tool_bindings = host_tool_bindings_for_skill(spec, &name);
    let disable_model_invocation =
        spec.disable_model_invocation || lambda_skill_disabled_by_manifest(spec, &name);

    Ok(Some(LoadedItem {
        value: SkillSpec {
            name,
            description,
            content: render_lambda_skill_content(&body, &lambda_source_path, &generated_path),
            allowed_tools: spec.allowed_tools.clone(),
            argument_hint: None,
            argument_names: Vec::new(),
            user_invocable: spec.user_invocable,
            model: spec.model.clone(),
            effort: spec.effort.clone(),
            context: spec.context.clone(),
            disable_model_invocation,
            verification: Some(SkillVerificationSpec {
                system: "lambda-skill".to_string(),
                source_path: Some(lambda_source_path.display().to_string()),
                generated_path: Some(generated_path.display().to_string()),
                host_catalogue_path: host_catalogue_path.map(|path| path.display().to_string()),
                compiler_path: None,
                host_tool_bindings,
                require_approval: spec.require_approval,
                tools: stats.as_ref().and_then(|stats| stats.tools),
                actions: stats.as_ref().and_then(|stats| stats.actions),
            }),
        },
        source_info: SourceInfo {
            path: lambda_source_path,
            kind: source_info.kind,
        },
    }))
}

fn lambda_skill_disabled_by_manifest(spec: &LambdaSkillLibrarySpec, skill_name: &str) -> bool {
    spec.disabled_skills
        .iter()
        .any(|configured| normalize_skill_name(configured) == skill_name)
}

fn host_tool_bindings_for_skill(
    spec: &LambdaSkillLibrarySpec,
    skill_name: &str,
) -> BTreeMap<String, Vec<String>> {
    let mut bindings = spec.host_tool_bindings.clone();
    for (configured_name, skill_bindings) in &spec.skill_host_tool_bindings {
        if normalize_skill_name(configured_name) == skill_name {
            bindings.extend(skill_bindings.clone());
        }
    }
    bindings
}

fn load_lambda_skill_stats(runner: &dyn ToolRunner, path: &Path) -> Option<LambdaSkillStats> {
    let bytes = runner.read_file(path).ok()?;
    let raw = String::from_utf8(bytes).ok()?;
    serde_json::from_str(&raw).ok()
}

fn render_lambda_skill_content(body: &str, source_path: &Path, generated_path: &Path) -> String {
    format!(
        "Lambda Skill verification:\n- system: lambda-skill\n- formal source: {}\n- generated descriptor: {}\n- runtime bridge: call LambdaHostCall with host_tool, formal args, and concrete tool before the concrete Puffer tool that implements a formal host operation\n- binding: omit LambdaHostCall input unless Puffer explicitly returned an exact input to retry; Puffer materializes concrete input from the verified catalogue contract\n\n{}",
        source_path.display(),
        generated_path.display(),
        body.trim_start()
    )
}

fn resolve_lambda_skill_root(raw: &str, manifest_path: &Path) -> PathBuf {
    let expanded = expand_home_path(raw.trim());
    if expanded.is_absolute() {
        return expanded;
    }
    manifest_path
        .parent()
        .map(|parent| parent.join(&expanded))
        .unwrap_or(expanded)
}

fn expand_home_path(raw: &str) -> PathBuf {
    if raw == "~" {
        return home_dir().unwrap_or_else(|| PathBuf::from(raw));
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        if let Some(home) = home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(raw)
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn clean_path(path: &Path) -> PathBuf {
    let mut clean = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                clean.pop();
            }
            other => clean.push(other.as_os_str()),
        }
    }
    clean
}

fn path_contains(parent: &Path, child: &Path) -> bool {
    parent == child || child.starts_with(parent)
}

fn lambda_skill_source_path(runner: &dyn ToolRunner, dir: &Path) -> Option<PathBuf> {
    let skill_source = dir.join("skill.lskill");
    if runner_file_exists(runner, &skill_source) {
        return Some(skill_source);
    }
    let main_source = dir.join("main.lskill");
    if runner_file_exists(runner, &main_source) {
        return Some(main_source);
    }
    None
}

fn runner_file_exists(runner: &dyn ToolRunner, path: &Path) -> bool {
    match runner.read_file(path) {
        Ok(_) => true,
        Err(RunnerError::NotFound(_)) => false,
        Err(_) => false,
    }
}

fn is_ignored_lambda_skill_dir(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "out" || name.starts_with('.'))
}

use super::{collect_lambda_skill_dirs_for_snapshot, LambdaSkillLibraryManifestDto};
use anyhow::{Context, Result};
use puffer_resources::LoadedResources;
use puffer_tools::ToolRegistry;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Registry-backed concrete tool support for Verified Skill imports.
pub(super) struct LambdaConcreteToolSupport {
    registry: ToolRegistry,
}

impl LambdaConcreteToolSupport {
    /// Builds concrete tool support from the same resources used by runtime execution.
    pub(super) fn from_resources(resources: &LoadedResources) -> Self {
        Self {
            registry: ToolRegistry::from_resources(resources),
        }
    }

    fn supports(&self, tool: &str) -> bool {
        self.registry.definition(tool).is_some()
    }
}

pub(super) fn validate_lambda_skill_library_import(
    manifest: &LambdaSkillLibraryManifestDto,
    tool_support: &LambdaConcreteToolSupport,
) -> Result<()> {
    let root = PathBuf::from(&manifest.root);
    if !root.is_dir() {
        anyhow::bail!(
            "Verified Skills folder does not exist or is not a directory: {}",
            root.display()
        );
    }

    let mut skill_dirs: Vec<PathBuf> = Vec::new();
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
        match validate_host_catalogue_for_import(&host_path, tool_support) {
            Ok(()) => {}
            Err(error) => invalid_host.push(format!(
                "{} ({error:#})",
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

fn validate_host_catalogue_for_import(
    path: &Path,
    tool_support: &LambdaConcreteToolSupport,
) -> Result<()> {
    let raw = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    validate_host_catalogue_concrete_tools(&raw, tool_support)
        .with_context(|| format!("parse {}", path.display()))
}

pub(super) fn validate_host_catalogue_concrete_tools(
    raw: &str,
    tool_support: &LambdaConcreteToolSupport,
) -> Result<()> {
    let bindings = puffer_core::lambda_host_catalogue_concrete_tool_bindings(raw)
        .context("parse host catalogue")?;
    for (index, binding) in bindings.into_iter().enumerate() {
        let name = if binding.host_tool.trim().is_empty() {
            format!("tool#{index}")
        } else {
            binding.host_tool
        };
        if binding
            .concrete_tools
            .iter()
            .all(|concrete| concrete.trim().is_empty())
        {
            anyhow::bail!("host tool {name} lacks concreteTools bindings");
        }
        let unsupported = binding
            .concrete_tools
            .iter()
            .map(|concrete| concrete.trim())
            .filter(|concrete| !concrete.is_empty())
            .filter(|concrete| !tool_support.supports(concrete))
            .map(str::to_string)
            .collect::<Vec<_>>();
        if !unsupported.is_empty() {
            anyhow::bail!(
                "host tool {name} binds unsupported concrete tool {}",
                unsupported.join(", ")
            );
        }
    }
    Ok(())
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

pub(super) fn infer_allowed_tools_from_default_host_catalogues(root: &Path) -> Option<Vec<String>> {
    let mut catalogues = Vec::new();
    collect_default_host_catalogues(root, &mut catalogues);
    if catalogues.is_empty() {
        return None;
    }
    let mut tools = BTreeSet::new();
    for catalogue in catalogues {
        let raw = std::fs::read_to_string(catalogue).ok()?;
        let bindings = puffer_core::lambda_host_catalogue_concrete_tool_bindings(&raw).ok()?;
        for binding in bindings {
            for concrete in binding.concrete_tools {
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

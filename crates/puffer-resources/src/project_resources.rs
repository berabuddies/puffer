use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Component, Path, PathBuf};

/// Carries one project-local resource file relative to the project root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectResourceFile {
    pub relative_path: PathBuf,
    pub content: String,
}

/// Collects resource files from `<project>/resources` and `<project>/.puffer/resources`.
pub fn collect_project_resource_files(project_root: &Path) -> Result<Vec<ProjectResourceFile>> {
    let mut files = Vec::new();
    collect_from_root(project_root, &project_root.join("resources"), &mut files)?;
    collect_from_root(
        project_root,
        &project_root.join(".puffer/resources"),
        &mut files,
    )?;
    files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(files)
}

fn collect_from_root(
    project_root: &Path,
    current: &Path,
    files: &mut Vec<ProjectResourceFile>,
) -> Result<()> {
    if !current.exists() {
        return Ok(());
    }
    for entry in sorted_dir_entries(current)? {
        let path = entry.path();
        if path.is_dir() {
            collect_from_root(project_root, &path, files)?;
            continue;
        }
        if !resource_file_supported(&path) {
            continue;
        }
        let relative_path = path
            .strip_prefix(project_root)
            .with_context(|| format!("strip project prefix from {}", path.display()))?
            .to_path_buf();
        ensure_relative_resource_path(&relative_path)?;
        let content = fs::read_to_string(&path)
            .with_context(|| format!("failed to read resource file {}", path.display()))?;
        files.push(ProjectResourceFile {
            relative_path,
            content,
        });
    }
    Ok(())
}

fn resource_file_supported(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some("yaml" | "yml")
    ) || path.file_name().and_then(|name| name.to_str()) == Some("SKILL.md")
}

fn ensure_relative_resource_path(path: &Path) -> Result<()> {
    if path.is_absolute() {
        bail!("resource path `{}` must be relative", path.display());
    }
    for component in path.components() {
        match component {
            Component::Normal(_) => {}
            Component::CurDir => {}
            Component::ParentDir => {
                bail!(
                    "resource path `{}` must not escape the project root",
                    path.display()
                )
            }
            Component::RootDir | Component::Prefix(_) => {
                bail!(
                    "resource path `{}` must stay within the project root",
                    path.display()
                )
            }
        }
    }
    Ok(())
}

fn sorted_dir_entries(dir: &Path) -> Result<Vec<fs::DirEntry>> {
    let mut entries = fs::read_dir(dir)
        .with_context(|| format!("failed to read project resource dir {}", dir.display()))?
        .collect::<std::io::Result<Vec<_>>>()
        .with_context(|| format!("failed to list project resource dir {}", dir.display()))?;
    entries.sort_by(|left, right| left.path().cmp(&right.path()));
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn collect_project_resource_files_reads_yaml_and_skills() {
        let temp = tempdir().unwrap();
        let project = temp.path();
        fs::create_dir_all(project.join("resources/prompts")).unwrap();
        fs::create_dir_all(project.join(".puffer/resources/skills/reviewer")).unwrap();
        fs::write(
            project.join("resources/prompts/review.yaml"),
            "id: review\ndescription: Review\ntemplate: hi\n",
        )
        .unwrap();
        fs::write(
            project.join(".puffer/resources/skills/reviewer/SKILL.md"),
            "---\nname: reviewer\n---\nBody\n",
        )
        .unwrap();
        fs::write(project.join("resources/prompts/README.md"), "ignored\n").unwrap();

        let files = collect_project_resource_files(project).unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(
            files[0].relative_path,
            PathBuf::from(".puffer/resources/skills/reviewer/SKILL.md")
        );
        assert_eq!(
            files[1].relative_path,
            PathBuf::from("resources/prompts/review.yaml")
        );
    }
}

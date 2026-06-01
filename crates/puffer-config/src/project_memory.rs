use crate::{ensure_workspace_dirs, ConfigPaths};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ProjectRegistry {
    #[serde(default)]
    pub projects: Vec<ProjectEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectEntry {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedProjectMemory {
    pub name: String,
    pub root: PathBuf,
    pub memory_file: PathBuf,
}

/// Loads the user-level project registry stored in `~/.puffer/projects.toml`.
pub fn load_project_registry(paths: &ConfigPaths) -> Result<ProjectRegistry> {
    let path = paths.projects_file();
    if !path.exists() {
        return Ok(ProjectRegistry::default());
    }
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("failed to read project registry {}", path.display()))?;
    toml::from_str(&raw)
        .with_context(|| format!("failed to parse project registry {}", path.display()))
}

/// Resolves the current working directory to a configured project memory file.
pub fn resolve_project_memory(
    paths: &ConfigPaths,
    cwd: &Path,
) -> Result<Option<ResolvedProjectMemory>> {
    let registry = load_project_registry(paths)?;
    let normalized_cwd = normalize_project_path(cwd);
    let mut best_match: Option<(usize, &ProjectEntry, PathBuf)> = None;

    for project in &registry.projects {
        let normalized_root = normalize_project_path(&project.path);
        if !path_matches_root(&normalized_cwd, &normalized_root) {
            continue;
        }
        let score = normalized_root.components().count();
        match &best_match {
            Some((best_score, _, _)) if *best_score >= score => continue,
            _ => best_match = Some((score, project, normalized_root)),
        }
    }

    Ok(
        best_match.map(|(_, project, normalized_root)| ResolvedProjectMemory {
            name: project.name.clone(),
            root: normalized_root.clone(),
            memory_file: paths
                .projects_memory_dir()
                .join(project_storage_slug(&project.name, &normalized_root))
                .join("MEMORY.md"),
        }),
    )
}

/// Ensures `cwd` is registered as a project and returns its memory file.
pub fn ensure_project_memory(paths: &ConfigPaths, cwd: &Path) -> Result<ResolvedProjectMemory> {
    if let Some(resolved) = resolve_project_memory(paths, cwd)? {
        ensure_project_memory_file(&resolved)?;
        return Ok(resolved);
    }

    let normalized_cwd = normalize_project_path(cwd);
    let project_name = normalized_cwd
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("project")
        .to_string();
    let mut registry = load_project_registry(paths)?;
    registry.projects.push(ProjectEntry {
        name: project_name.clone(),
        path: normalized_cwd.clone(),
    });
    save_project_registry(paths, &registry)?;

    let resolved = ResolvedProjectMemory {
        name: project_name.clone(),
        root: normalized_cwd.clone(),
        memory_file: paths
            .projects_memory_dir()
            .join(project_storage_slug(&project_name, &normalized_cwd))
            .join("MEMORY.md"),
    };
    ensure_project_memory_file(&resolved)?;
    Ok(resolved)
}

fn save_project_registry(paths: &ConfigPaths, registry: &ProjectRegistry) -> Result<()> {
    ensure_workspace_dirs(paths)?;
    let raw = toml::to_string_pretty(registry)
        .with_context(|| format!("failed to serialize {}", paths.projects_file().display()))?;
    fs::write(paths.projects_file(), raw)
        .with_context(|| format!("failed to write {}", paths.projects_file().display()))
}

fn ensure_project_memory_file(resolved: &ResolvedProjectMemory) -> Result<()> {
    if let Some(parent) = resolved.memory_file.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    if !resolved.memory_file.exists() {
        fs::write(&resolved.memory_file, "").with_context(|| {
            format!(
                "failed to create project memory file {}",
                resolved.memory_file.display()
            )
        })?;
    }
    Ok(())
}

fn normalize_project_path(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn path_matches_root(path: &Path, root: &Path) -> bool {
    path == root || path.starts_with(root)
}

fn project_storage_slug(name: &str, root: &Path) -> String {
    let mut slug = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    slug = slug
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if slug.is_empty() {
        slug = "project".to_string();
    }

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    root.hash(&mut hasher);
    format!("{slug}-{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ensure_workspace_dirs, ConfigPaths};
    use std::ffi::OsString;
    use std::sync::{Mutex, MutexGuard, OnceLock};
    use tempfile::tempdir;

    fn puffer_home_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn lock_puffer_home() -> MutexGuard<'static, ()> {
        puffer_home_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    struct ScopedPufferHome {
        old_home: Option<OsString>,
    }

    impl ScopedPufferHome {
        fn set(path: &Path) -> Self {
            let old_home = std::env::var_os("PUFFER_HOME");
            std::env::set_var("PUFFER_HOME", path);
            Self { old_home }
        }
    }

    impl Drop for ScopedPufferHome {
        fn drop(&mut self) {
            if let Some(value) = self.old_home.take() {
                std::env::set_var("PUFFER_HOME", value);
            } else {
                std::env::remove_var("PUFFER_HOME");
            }
        }
    }

    #[test]
    fn resolve_project_memory_uses_registered_project_path() {
        let _guard = lock_puffer_home();
        let tempdir = tempdir().expect("tempdir");
        let home = tempdir.path().join("home");
        let workspace = tempdir.path().join("workspace");
        let project_root = workspace.join("apps/api");
        let nested = project_root.join("src/features");
        fs::create_dir_all(&home).expect("home");
        fs::create_dir_all(&nested).expect("nested");
        let _home = ScopedPufferHome::set(&home);

        let paths = ConfigPaths::discover(&workspace);
        ensure_workspace_dirs(&paths).expect("dirs");
        fs::write(
            paths.projects_file(),
            format!(
                "[[projects]]\nname = \"api\"\npath = \"{}\"\n",
                project_root.display()
            ),
        )
        .expect("projects file");

        let resolved = resolve_project_memory(&paths, &nested)
            .expect("resolve")
            .expect("project memory");
        assert_eq!(resolved.name, "api");
        assert_eq!(resolved.root, normalize_project_path(&project_root));
        assert!(resolved.memory_file.ends_with("MEMORY.md"));
        assert!(resolved
            .memory_file
            .starts_with(paths.projects_memory_dir()));
    }

    #[test]
    fn resolve_project_memory_prefers_longest_matching_root() {
        let _guard = lock_puffer_home();
        let tempdir = tempdir().expect("tempdir");
        let home = tempdir.path().join("home");
        let workspace = tempdir.path().join("workspace");
        let mono_root = workspace.join("monorepo");
        let nested_root = mono_root.join("services/api");
        let cwd = nested_root.join("src");
        fs::create_dir_all(&home).expect("home");
        fs::create_dir_all(&cwd).expect("cwd");
        let _home = ScopedPufferHome::set(&home);

        let paths = ConfigPaths::discover(&workspace);
        ensure_workspace_dirs(&paths).expect("dirs");
        fs::write(
            paths.projects_file(),
            format!(
                "[[projects]]\nname = \"mono\"\npath = \"{}\"\n\n[[projects]]\nname = \"api\"\npath = \"{}\"\n",
                mono_root.display(),
                nested_root.display()
            ),
        )
        .expect("projects file");

        let resolved = resolve_project_memory(&paths, &cwd)
            .expect("resolve")
            .expect("project memory");
        assert_eq!(resolved.name, "api");
        assert_eq!(resolved.root, normalize_project_path(&nested_root));
    }

    #[test]
    fn ensure_project_memory_registers_current_directory() {
        let _guard = lock_puffer_home();
        let tempdir = tempdir().expect("tempdir");
        let home = tempdir.path().join("home");
        let workspace = tempdir.path().join("workspace");
        let project_root = workspace.join("demo-project");
        fs::create_dir_all(&project_root).expect("project");
        let _home = ScopedPufferHome::set(&home);

        let paths = ConfigPaths::discover(&workspace);
        ensure_workspace_dirs(&paths).expect("dirs");

        let resolved = ensure_project_memory(&paths, &project_root).expect("ensure");
        assert_eq!(resolved.name, "demo-project");
        assert_eq!(resolved.root, normalize_project_path(&project_root));
        assert!(resolved.memory_file.exists());

        let registry = load_project_registry(&paths).expect("registry");
        assert_eq!(registry.projects.len(), 1);
        assert_eq!(registry.projects[0].name, "demo-project");

        let again = ensure_project_memory(&paths, &project_root).expect("ensure again");
        assert_eq!(again.memory_file, resolved.memory_file);
        let registry = load_project_registry(&paths).expect("registry again");
        assert_eq!(registry.projects.len(), 1);
    }
}

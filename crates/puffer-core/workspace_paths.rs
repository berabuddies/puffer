use crate::AppState;
use anyhow::{anyhow, Context, Result};
use puffer_runner_api::FilesystemSandboxMode;
use std::collections::BTreeSet;
use std::fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

/// Describes the outcome of validating one `/add-dir` directory candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AddDirectoryValidation {
    Success {
        absolute_path: PathBuf,
    },
    EmptyPath,
    PathNotFound {
        directory_path: String,
        absolute_path: PathBuf,
    },
    NotADirectory {
        directory_path: String,
        absolute_path: PathBuf,
    },
    AlreadyInWorkingDirectory {
        directory_path: String,
        working_dir: PathBuf,
    },
}

pub(crate) type AddWorkingDirectoryResult = AddDirectoryValidation;

/// Validates one user-supplied `/add-dir` path against the current workspace roots.
pub(crate) fn validate_directory_for_workspace(
    cwd: &Path,
    working_dirs: &[PathBuf],
    directory_path: &str,
) -> Result<AddDirectoryValidation> {
    let trimmed = directory_path.trim();
    if trimmed.is_empty() {
        return Ok(AddDirectoryValidation::EmptyPath);
    }

    let absolute_path = normalize_user_path(cwd, trimmed);
    match fs::metadata(&absolute_path) {
        Ok(metadata) => {
            if !metadata.is_dir() {
                return Ok(AddDirectoryValidation::NotADirectory {
                    directory_path: trimmed.to_string(),
                    absolute_path,
                });
            }
        }
        Err(error) if is_missing_like_error(&error) => {
            return Ok(AddDirectoryValidation::PathNotFound {
                directory_path: trimmed.to_string(),
                absolute_path,
            });
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to inspect {}", absolute_path.display()));
        }
    }

    let canonical_candidate = fs::canonicalize(&absolute_path)
        .with_context(|| format!("failed to canonicalize {}", absolute_path.display()))?;
    for working_dir in workspace_roots(cwd, working_dirs) {
        let canonical_working_dir = canonicalize_or_normalize(&working_dir)?;
        if canonical_candidate.starts_with(&canonical_working_dir) {
            return Ok(AddDirectoryValidation::AlreadyInWorkingDirectory {
                directory_path: trimmed.to_string(),
                working_dir,
            });
        }
    }

    Ok(AddDirectoryValidation::Success { absolute_path })
}

/// Formats one user-facing `/add-dir` validation result message.
pub(crate) fn add_directory_help_message(result: &AddDirectoryValidation) -> String {
    match result {
        AddDirectoryValidation::Success { absolute_path } => format!(
            "Added {} as a working directory for this session. /permissions to manage",
            absolute_path.display()
        ),
        AddDirectoryValidation::EmptyPath => "Usage: /add-dir <directory>".to_string(),
        AddDirectoryValidation::PathNotFound { absolute_path, .. } => {
            format!("Path {} was not found.", absolute_path.display())
        }
        AddDirectoryValidation::NotADirectory {
            directory_path,
            absolute_path,
        } => {
            let parent = absolute_path
                .parent()
                .unwrap_or(absolute_path.as_path())
                .display()
                .to_string();
            format!(
                "{} is not a directory. Did you mean to add the parent directory {}?",
                directory_path, parent
            )
        }
        AddDirectoryValidation::AlreadyInWorkingDirectory {
            directory_path,
            working_dir,
        } => format!(
            "{} is already accessible within the existing working directory {}.",
            directory_path,
            working_dir.display()
        ),
    }
}

/// Validates one `/add-dir` directory candidate using the current application state.
pub(crate) fn validate_additional_working_directory(
    state: &AppState,
    directory_path: &str,
) -> Result<AddWorkingDirectoryResult> {
    validate_directory_for_workspace(&state.cwd, &state.working_dirs, directory_path)
}

/// Formats one user-facing `/add-dir` validation result message.
pub(crate) fn format_add_working_directory_result(result: &AddWorkingDirectoryResult) -> String {
    add_directory_help_message(result)
}

/// Resolves one tool path against the primary working directory plus `/add-dir` roots.
pub(crate) fn resolve_path_in_workspaces(
    cwd: &Path,
    additional_roots: &[PathBuf],
    path: &Path,
) -> Result<PathBuf> {
    let candidate = normalize_user_path(cwd, path.to_string_lossy().as_ref());
    let roots = workspace_roots(cwd, additional_roots);
    if !roots.iter().any(|root| candidate.starts_with(root)) {
        anyhow::bail!(
            "Path {} is outside the current working directories. Use `/add-dir <directory>` to add access first.",
            candidate.display()
        );
    }

    let canonical_roots = roots
        .iter()
        .map(|root| canonicalize_or_normalize(root))
        .collect::<Result<Vec<_>>>()?;
    let ancestor = nearest_existing_ancestor(&candidate).ok_or_else(|| {
        anyhow!(
            "failed to resolve path {} inside the current working directories",
            path.display()
        )
    })?;
    let canonical_ancestor = fs::canonicalize(&ancestor)
        .with_context(|| format!("failed to canonicalize {}", ancestor.display()))?;
    if !starts_with_any_root(&canonical_ancestor, &canonical_roots) {
        anyhow::bail!(
            "Path {} resolves outside the current working directories. Use `/add-dir <directory>` to add access first.",
            candidate.display()
        );
    }

    if candidate.exists() {
        let canonical_candidate = fs::canonicalize(&candidate)
            .with_context(|| format!("failed to canonicalize {}", candidate.display()))?;
        if !starts_with_any_root(&canonical_candidate, &canonical_roots) {
            anyhow::bail!(
                "Path {} resolves outside the current working directories. Use `/add-dir <directory>` to add access first.",
                candidate.display()
            );
        }
    }

    Ok(candidate)
}

/// Resolves one tool path, skipping workspace-root restrictions when the
/// session runs in `danger-full-access` mode.
pub(crate) fn resolve_path_for_session(
    cwd: &Path,
    additional_roots: &[PathBuf],
    sandbox_mode: &str,
    path: &Path,
) -> Result<PathBuf> {
    if sandbox_allows_all_paths(sandbox_mode) {
        return Ok(normalize_user_path(cwd, path.to_string_lossy().as_ref()));
    }
    resolve_path_in_workspaces(cwd, additional_roots, path)
}

/// Resolves one tool path, skipping workspace-root restrictions when the
/// typed filesystem policy grants danger-full-access.
pub(crate) fn resolve_path_for_filesystem_policy(
    cwd: &Path,
    additional_roots: &[PathBuf],
    sandbox_mode: FilesystemSandboxMode,
    path: &Path,
) -> Result<PathBuf> {
    if matches!(sandbox_mode, FilesystemSandboxMode::DangerFullAccess) {
        return Ok(normalize_user_path(cwd, path.to_string_lossy().as_ref()));
    }
    resolve_path_in_workspaces(cwd, additional_roots, path)
}

/// Resolves one tool path against the primary working directory plus `/add-dir` roots.
pub(crate) fn resolve_path_in_working_dirs(
    cwd: &Path,
    additional_roots: &[PathBuf],
    path: &Path,
) -> Result<PathBuf> {
    resolve_path_in_workspaces(cwd, additional_roots, path)
}

/// Returns true when tool path guards should be bypassed for the current
/// session sandbox mode.
pub(crate) fn sandbox_allows_all_paths(sandbox_mode: &str) -> bool {
    sandbox_mode.trim() == "danger-full-access"
}

/// Returns the normalized primary workspace root plus any distinct
/// additional roots.
///
/// Default writable set mirrors codex's
/// [`SandboxPolicy::WorkspaceWrite::get_writable_roots_with_cwd`]
/// (`openai/codex` `codex-rs/protocol/src/protocol.rs::1156-1210`):
///
///   * the primary `cwd`
///   * `/tmp` on Unix (the standard ephemeral scratch location every
///     coding agent's training data assumes is writable)
///   * `$TMPDIR` when set (per-user on macOS, opt-in elsewhere)
///   * any explicit `/add-dir` roots the user added at runtime
///
/// Including `/tmp` and `$TMPDIR` here removes the need for the old
/// puffer-only "scratchpad" advertisement (commit `5e7251c`), which
/// asked the model to write to `~/.puffer/scratchpad/<sid>/` despite
/// the path sandbox always rejecting that path. The result was 1800+
/// empty scratchpad dirs accumulated on disk, plus broken nested
/// subagent dispatch (see PR #119). With this change the model can
/// follow standard Unix scratch conventions and `workspace-write`
/// sandbox mode actually permits it.
pub(crate) fn workspace_roots(cwd: &Path, additional_roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let mut seen = BTreeSet::new();
    let mut push_root = |raw: &Path| {
        let normalized = normalize_user_path(cwd, raw.to_string_lossy().as_ref());
        if seen.insert(normalized.clone()) {
            roots.push(normalized);
        }
    };
    push_root(cwd);
    for root in additional_roots {
        push_root(root.as_path());
    }
    // Default ephemeral scratch locations the model is trained to use.
    if cfg!(unix) {
        let slash_tmp = Path::new("/tmp");
        if slash_tmp.is_dir() {
            push_root(slash_tmp);
        }
    }
    if let Some(tmpdir) = std::env::var_os("TMPDIR") {
        if !tmpdir.is_empty() {
            push_root(Path::new(&tmpdir));
        }
    }
    roots
}

fn normalize_user_path(cwd: &Path, raw_path: &str) -> PathBuf {
    let expanded = expand_tilde(raw_path).unwrap_or_else(|| PathBuf::from(raw_path));
    if expanded.is_absolute() {
        normalize_path(&expanded)
    } else {
        normalize_path(&cwd.join(expanded))
    }
}

fn expand_tilde(raw_path: &str) -> Option<PathBuf> {
    if raw_path == "~" {
        return std::env::var_os("HOME").map(PathBuf::from);
    }
    let stripped = raw_path
        .strip_prefix("~/")
        .or_else(|| raw_path.strip_prefix("~\\"));
    stripped
        .and_then(|suffix| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(suffix)))
}

fn canonicalize_or_normalize(path: &Path) -> Result<PathBuf> {
    match fs::canonicalize(path) {
        Ok(canonical) => Ok(canonical),
        Err(error) if is_missing_like_error(&error) => Ok(normalize_path(path)),
        Err(error) => {
            Err(error).with_context(|| format!("failed to canonicalize {}", path.display()))
        }
    }
}

fn starts_with_any_root(candidate: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|root| candidate.starts_with(root))
}

fn is_missing_like_error(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        ErrorKind::NotFound | ErrorKind::PermissionDenied
    ) || matches!(error.raw_os_error(), Some(20))
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(
                    normalized.components().next_back(),
                    Some(Component::Normal(_))
                ) {
                    normalized.pop();
                } else if !normalized.has_root() {
                    normalized.push(component.as_os_str());
                }
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    if normalized.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        normalized
    }
}

fn nearest_existing_ancestor(path: &Path) -> Option<PathBuf> {
    let mut current = path.to_path_buf();
    loop {
        if current.exists() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{resolve_path_for_session, resolve_path_in_workspaces};
    use std::path::Path;
    use tempfile::tempdir;

    #[cfg(unix)]
    #[test]
    fn resolve_path_in_workspaces_allows_symlink_target_within_another_root() {
        use std::os::unix::fs::symlink;

        let temp = tempdir().unwrap();
        let cwd = temp.path().join("workspace");
        let etc = temp.path().join("etc");
        let usr_lib = temp.path().join("usr/lib");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(&etc).unwrap();
        std::fs::create_dir_all(&usr_lib).unwrap();
        std::fs::write(usr_lib.join("os-release"), "NAME=test\n").unwrap();
        symlink("../usr/lib/os-release", etc.join("os-release")).unwrap();

        let resolved = resolve_path_in_workspaces(
            &cwd,
            &[etc.clone(), temp.path().join("usr")],
            Path::new(&etc.join("os-release")),
        )
        .unwrap();

        assert_eq!(resolved, etc.join("os-release"));
    }

    #[test]
    fn resolve_path_for_session_allows_outside_workspace_in_danger_full_access() {
        let temp = tempdir().unwrap();
        let cwd = temp.path().join("workspace");
        let outside = temp.path().join("outside").join("notes.txt");
        std::fs::create_dir_all(&cwd).unwrap();

        let resolved =
            resolve_path_for_session(&cwd, &[], "danger-full-access", Path::new(&outside)).unwrap();

        assert_eq!(resolved, outside);
    }

    #[cfg(unix)]
    #[test]
    fn workspace_roots_default_set_includes_slash_tmp() {
        use super::workspace_roots;
        use std::path::PathBuf;
        let cwd = tempdir().unwrap();
        let roots = workspace_roots(cwd.path(), &[]);
        assert!(
            roots.contains(&PathBuf::from("/tmp")) || !std::path::Path::new("/tmp").is_dir(),
            "default writable set must include /tmp on Unix when it exists; got {roots:?}"
        );
    }

    #[test]
    fn workspace_roots_default_set_includes_tmpdir_env_when_set() {
        use super::workspace_roots;
        use std::path::PathBuf;
        let _guard = crate::test_locks::env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let temp = tempdir().unwrap();
        let original = std::env::var_os("TMPDIR");
        std::env::set_var("TMPDIR", temp.path());

        let cwd = tempdir().unwrap();
        let roots = workspace_roots(cwd.path(), &[]);
        let normalized = PathBuf::from(temp.path());

        match original {
            Some(value) => std::env::set_var("TMPDIR", value),
            None => std::env::remove_var("TMPDIR"),
        }

        assert!(
            roots.iter().any(|root| root == &normalized),
            "default writable set must include $TMPDIR when set; got {roots:?}"
        );
    }

    #[test]
    fn resolve_path_in_workspaces_rejects_path_clearly_outside_default_writable_set() {
        let temp = tempdir().unwrap();
        let err = resolve_path_in_workspaces(
            temp.path(),
            &[],
            Path::new("/__puffer_test_outside_writable_set__/foo.txt"),
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("outside the current working directories"),
            "expected reject error; got: {err}"
        );
    }
}

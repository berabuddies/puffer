use crate::AppState;
use anyhow::{Context, Result};
use puffer_config::{ensure_workspace_dirs, ConfigPaths};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use uuid::Uuid;

const DEFAULT_PLAN_TEXT: &str = "# Current Plan\n\n- Add concrete implementation steps here.\n";
const DEFAULT_PLAN_SLUG_PREFIX: &str = "session";
static PLAN_SLUG_CACHE: OnceLock<Mutex<std::collections::HashMap<Uuid, String>>> = OnceLock::new();

/// Returns true when the plan body contains user-authored content beyond the default scaffold.
pub(crate) fn plan_has_user_content(plan_body: &str) -> bool {
    let trimmed = plan_body.trim();
    !trimmed.is_empty() && trimmed != DEFAULT_PLAN_TEXT.trim()
}

/// Returns the cached slug used to resolve this session's plan file.
pub(crate) fn plan_slug(state: &AppState) -> Result<String> {
    let mut cache = plan_slug_cache()
        .lock()
        .map_err(|_| anyhow::anyhow!("plan slug cache lock poisoned"))?;
    if let Some(existing) = cache.get(&state.session.id) {
        return Ok(existing.clone());
    }
    let chosen = state
        .session
        .slug
        .as_deref()
        .and_then(sanitize_plan_slug)
        .unwrap_or_else(|| default_plan_slug(state.session.id));
    cache.insert(state.session.id, chosen.clone());
    Ok(chosen)
}

/// Overrides the cached slug for one session id.
pub(crate) fn set_plan_slug(session_id: Uuid, slug: String) -> Result<()> {
    let mut cache = plan_slug_cache()
        .lock()
        .map_err(|_| anyhow::anyhow!("plan slug cache lock poisoned"))?;
    cache.insert(
        session_id,
        sanitize_plan_slug(&slug).unwrap_or_else(|| default_plan_slug(session_id)),
    );
    Ok(())
}

/// Clears one cached plan slug entry so the next lookup recomputes from metadata defaults.
pub(crate) fn clear_plan_slug(session_id: Uuid) -> Result<()> {
    let mut cache = plan_slug_cache()
        .lock()
        .map_err(|_| anyhow::anyhow!("plan slug cache lock poisoned"))?;
    cache.remove(&session_id);
    Ok(())
}

/// Clears all cached plan slug entries.
pub(crate) fn clear_all_plan_slugs() -> Result<()> {
    let mut cache = plan_slug_cache()
        .lock()
        .map_err(|_| anyhow::anyhow!("plan slug cache lock poisoned"))?;
    cache.clear();
    Ok(())
}

/// Returns the session-scoped plan file path used by plan mode and workflow tools.
pub(crate) fn plan_file_path(state: &AppState) -> Result<PathBuf> {
    Ok(plans_directory(&state.cwd)?.join(format!("{}.md", plan_slug(state)?)))
}

/// Reuses a source session's plan file and slug for resumed sessions.
pub(crate) fn copy_plan_for_resume(source: &AppState, target: &AppState) -> Result<bool> {
    let source_path = plan_file_path(source)?;
    if !source_path.exists() {
        return Ok(false);
    }
    let source_slug = plan_slug(source)?;
    set_plan_slug(target.session.id, source_slug)?;
    let target_path = plan_file_path(target)?;
    if target_path != source_path {
        fs::copy(&source_path, &target_path).with_context(|| {
            format!(
                "failed to copy plan {} -> {}",
                source_path.display(),
                target_path.display()
            )
        })?;
    }
    Ok(true)
}

/// Copies plan contents for a forked session while assigning a distinct target slug.
pub(crate) fn copy_plan_for_fork(source: &AppState, target: &AppState) -> Result<bool> {
    let source_path = plan_file_path(source)?;
    if !source_path.exists() {
        return Ok(false);
    }
    let source_slug = plan_slug(source)?;
    let mut fork_slug = target
        .session
        .slug
        .as_deref()
        .and_then(sanitize_plan_slug)
        .unwrap_or_else(|| default_plan_slug(target.session.id));
    if fork_slug == source_slug {
        fork_slug = format!(
            "{}-fork-{}",
            source_slug,
            target.session.id.simple().to_string()[..8].to_string()
        );
    }
    set_plan_slug(target.session.id, fork_slug)?;
    let target_path = plan_file_path(target)?;
    fs::copy(&source_path, &target_path).with_context(|| {
        format!(
            "failed to copy plan {} -> {}",
            source_path.display(),
            target_path.display()
        )
    })?;
    Ok(true)
}

/// Ensures the session-scoped plan file exists and returns its path.
#[cfg(test)]
pub(crate) fn ensure_plan_file(state: &AppState) -> Result<PathBuf> {
    let path = plan_file_path(state)?;
    if !path.exists() {
        fs::write(&path, DEFAULT_PLAN_TEXT)
            .with_context(|| format!("failed to write {}", path.display()))?;
    }
    Ok(path)
}

/// Loads the current plan contents when a plan file has already been written.
pub(crate) fn read_plan_text(state: &AppState) -> Result<Option<String>> {
    let path = plan_file_path(state)?;
    match fs::read_to_string(&path) {
        Ok(contents) => Ok(Some(contents)),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            let legacy_path = legacy_plan_file_path(state)?;
            match fs::read_to_string(&legacy_path) {
                Ok(contents) => {
                    fs::write(&path, &contents).with_context(|| {
                        format!(
                            "failed to migrate legacy plan {} -> {}",
                            legacy_path.display(),
                            path.display()
                        )
                    })?;
                    Ok(Some(contents))
                }
                Err(legacy_error) if legacy_error.kind() == ErrorKind::NotFound => Ok(None),
                Err(legacy_error) => Err(legacy_error).with_context(|| {
                    format!("failed to read legacy plan {}", legacy_path.display())
                }),
            }
        }
        Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
    }
}

/// Persists updated plan contents to the session-scoped plan file.
#[cfg(test)]
pub(crate) fn persist_plan_output(state: &AppState, plan_text: &str) -> Result<PathBuf> {
    let path = plan_file_path(state)?;
    fs::write(&path, plan_text).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

fn plan_slug_cache() -> &'static Mutex<std::collections::HashMap<Uuid, String>> {
    PLAN_SLUG_CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

fn plans_directory(cwd: &Path) -> Result<PathBuf> {
    let paths = ConfigPaths::discover(cwd);
    ensure_workspace_dirs(&paths)?;
    let plan_dir = paths.workspace_config_dir.join("plans");
    fs::create_dir_all(&plan_dir)
        .with_context(|| format!("failed to create {}", plan_dir.display()))?;
    Ok(plan_dir)
}

fn default_plan_slug(session_id: Uuid) -> String {
    format!("{DEFAULT_PLAN_SLUG_PREFIX}-{}", session_id.simple())
}

fn sanitize_plan_slug(raw: &str) -> Option<String> {
    let mut normalized = String::new();
    for character in raw.trim().chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
            normalized.push(character);
        } else if !normalized.ends_with('-') {
            normalized.push('-');
        }
    }
    let cleaned = normalized.trim_matches('-').to_string();
    (!cleaned.is_empty()).then_some(cleaned)
}

fn legacy_plan_file_path(state: &AppState) -> Result<PathBuf> {
    Ok(plans_directory(&state.cwd)?.join(format!("{}.md", state.session.id)))
}

#[cfg(test)]
mod tests {
    use super::{
        clear_all_plan_slugs, clear_plan_slug, copy_plan_for_fork, copy_plan_for_resume,
        ensure_plan_file, plan_file_path, plan_has_user_content, plan_slug, set_plan_slug,
        DEFAULT_PLAN_TEXT,
    };
    use crate::AppState;
    use puffer_config::{ensure_workspace_dirs, ConfigPaths, PufferConfig};
    use puffer_session_store::SessionStore;
    use std::fs;
    use std::sync::{Mutex, MutexGuard, OnceLock};
    use tempfile::tempdir;

    fn state() -> AppState {
        let tempdir = tempdir().unwrap();
        let root = tempdir.keep();
        let paths = ConfigPaths::discover(&root);
        ensure_workspace_dirs(&paths).unwrap();
        let session_store = SessionStore::from_paths(&paths).unwrap();
        let session = session_store.create_session(root.clone()).unwrap();
        AppState::new(PufferConfig::default(), root, session)
    }

    fn plan_test_guard() -> MutexGuard<'static, ()> {
        static PLAN_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        PLAN_TEST_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap()
    }

    #[test]
    fn plan_file_path_does_not_materialize_the_file() {
        let _guard = plan_test_guard();
        clear_all_plan_slugs().unwrap();
        let state = state();
        let path = plan_file_path(&state).unwrap();

        assert!(!path.exists());
    }

    #[test]
    fn ensure_plan_file_writes_the_default_scaffold() {
        let _guard = plan_test_guard();
        clear_all_plan_slugs().unwrap();
        let state = state();
        let path = ensure_plan_file(&state).unwrap();

        assert_eq!(std::fs::read_to_string(path).unwrap(), DEFAULT_PLAN_TEXT);
    }

    #[test]
    fn plan_has_user_content_ignores_the_default_scaffold() {
        let _guard = plan_test_guard();
        assert!(!plan_has_user_content(DEFAULT_PLAN_TEXT));
        assert!(plan_has_user_content(
            "# Current Plan\n\n1. Verify the fix.\n"
        ));
    }

    #[test]
    fn plan_slug_prefers_session_slug_and_is_cached_until_cleared() {
        let _guard = plan_test_guard();
        clear_all_plan_slugs().unwrap();
        let mut state = state();
        state.session.slug = Some("dockyard-plan".to_string());

        assert_eq!(plan_slug(&state).unwrap(), "dockyard-plan");
        state.session.slug = Some("changed-after-cache".to_string());
        assert_eq!(plan_slug(&state).unwrap(), "dockyard-plan");
        clear_plan_slug(state.session.id).unwrap();
        assert_eq!(plan_slug(&state).unwrap(), "changed-after-cache");
    }

    #[test]
    fn set_plan_slug_sanitizes_input() {
        let _guard = plan_test_guard();
        clear_all_plan_slugs().unwrap();
        let state = state();
        set_plan_slug(state.session.id, "plan !!! alpha".to_string()).unwrap();
        assert_eq!(plan_slug(&state).unwrap(), "plan-alpha");
    }

    #[test]
    fn read_plan_text_migrates_legacy_plan_path() {
        let _guard = plan_test_guard();
        clear_all_plan_slugs().unwrap();
        let state = state();
        let legacy_path = ConfigPaths::discover(&state.cwd)
            .workspace_config_dir
            .join("plans")
            .join(format!("{}.md", state.session.id));
        fs::create_dir_all(legacy_path.parent().unwrap()).unwrap();
        fs::write(&legacy_path, "# Current Plan\n\n1. Legacy\n").unwrap();

        let loaded = super::read_plan_text(&state).unwrap();
        assert_eq!(loaded.as_deref(), Some("# Current Plan\n\n1. Legacy\n"));
        let new_path = plan_file_path(&state).unwrap();
        assert_eq!(
            fs::read_to_string(new_path).unwrap(),
            "# Current Plan\n\n1. Legacy\n"
        );
    }

    #[test]
    fn copy_plan_for_resume_reuses_source_slug() {
        let _guard = plan_test_guard();
        clear_all_plan_slugs().unwrap();
        let source = state();
        let mut target = state();
        target.cwd = source.cwd.clone();
        fs::write(
            plan_file_path(&source).unwrap(),
            "# Current Plan\n\n1. Resume\n",
        )
        .unwrap();

        assert!(copy_plan_for_resume(&source, &target).unwrap());
        assert_eq!(plan_slug(&source).unwrap(), plan_slug(&target).unwrap());
        assert_eq!(
            fs::read_to_string(plan_file_path(&target).unwrap()).unwrap(),
            "# Current Plan\n\n1. Resume\n"
        );
    }

    #[test]
    fn copy_plan_for_fork_uses_distinct_slug() {
        let _guard = plan_test_guard();
        clear_all_plan_slugs().unwrap();
        let source = state();
        let mut target = state();
        target.cwd = source.cwd.clone();
        target.session.slug = source.session.slug.clone();
        fs::write(
            plan_file_path(&source).unwrap(),
            "# Current Plan\n\n1. Fork\n",
        )
        .unwrap();

        assert!(copy_plan_for_fork(&source, &target).unwrap());
        assert_ne!(
            plan_file_path(&source).unwrap(),
            plan_file_path(&target).unwrap()
        );
        assert_eq!(
            fs::read_to_string(plan_file_path(&target).unwrap()).unwrap(),
            "# Current Plan\n\n1. Fork\n"
        );
    }

    #[test]
    fn clear_all_plan_slugs_resets_cache() {
        let _guard = plan_test_guard();
        clear_all_plan_slugs().unwrap();
        let state = state();
        set_plan_slug(state.session.id, "manual".to_string()).unwrap();
        assert_eq!(plan_slug(&state).unwrap(), "manual");
        clear_all_plan_slugs().unwrap();
        assert_ne!(plan_slug(&state).unwrap(), "manual");
    }

    #[test]
    fn fallback_slug_uses_session_id_when_slug_is_invalid() {
        let _guard = plan_test_guard();
        clear_all_plan_slugs().unwrap();
        let mut state = state();
        state.session.slug = Some("  !!!  ".to_string());
        let slug = plan_slug(&state).unwrap();
        let expected_prefix = format!("session-{}", state.session.id.simple());
        assert_eq!(slug, expected_prefix);
    }
}

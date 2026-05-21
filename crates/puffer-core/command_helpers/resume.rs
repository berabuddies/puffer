use super::emit_system;
use crate::AppState;
use anyhow::Result;
use puffer_session_store::{SessionStore, SessionSummary};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use uuid::Uuid;

/// Handles `/resume` by listing resumable sessions or switching to a unique match.
pub(crate) fn handle_resume_command(
    state: &mut AppState,
    session_store: &SessionStore,
    args: &str,
) -> Result<()> {
    let sessions = resumable_sessions_for_picker(session_store, state.session.id, &state.cwd)?;
    let all_sessions = all_resumable_sessions(session_store, state.session.id)?;
    let query = args.trim();
    if query.is_empty() {
        return emit_system(state, session_store, render_resume_listing(&sessions));
    }

    let matches = if looks_like_session_id(query) {
        search_sessions(&all_sessions, query)
    } else {
        search_sessions(&sessions, query)
    };
    let Some(best_match) = matches.first() else {
        return emit_system(
            state,
            session_store,
            format!("No session matched `{query}`.\nRun /resume to pick from recent sessions."),
        );
    };

    let best_rank = best_match.rank;
    let best_rank_count = matches
        .iter()
        .take_while(|candidate| candidate.rank == best_rank)
        .count();
    if matches.len() == 1 || (best_rank_count == 1 && best_rank.is_precise()) {
        return resume_or_reroute(state, session_store, &best_match.session);
    }

    emit_system(
        state,
        session_store,
        render_resume_ambiguity(query, &matches),
    )
}

/// Describes how CLI startup should handle a `--resume` request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumeLaunchResolution {
    /// Resume directly into the matched session.
    Exact(SessionSummary),
    /// Open the interactive picker, optionally seeded with a search term.
    Picker {
        sessions: Vec<SessionSummary>,
        query: Option<String>,
    },
    /// No resumable sessions were available for the requested query.
    NotFound { query: Option<String> },
}

/// Resolves a Claude-style startup resume request into a direct session or picker state.
pub fn resolve_resume_launch(
    session_store: &SessionStore,
    current_cwd: &Path,
    query: Option<&str>,
) -> Result<ResumeLaunchResolution> {
    let sessions = resumable_sessions_for_picker(session_store, Uuid::nil(), current_cwd)?;
    let normalized_query = query.map(str::trim).filter(|value| !value.is_empty());
    let Some(query) = normalized_query else {
        return if sessions.is_empty() {
            Ok(ResumeLaunchResolution::NotFound { query: None })
        } else {
            Ok(ResumeLaunchResolution::Picker {
                sessions,
                query: None,
            })
        };
    };

    let all_sessions = all_resumable_sessions(session_store, Uuid::nil())?;
    let matches = if looks_like_session_id(query) {
        search_sessions(&all_sessions, query)
    } else {
        search_sessions(&sessions, query)
    };
    if let Some(best_match) = matches.first() {
        let best_rank = best_match.rank;
        let best_rank_count = matches
            .iter()
            .take_while(|candidate| candidate.rank == best_rank)
            .count();
        if matches.len() == 1 || (best_rank_count == 1 && best_rank.is_precise()) {
            return Ok(ResumeLaunchResolution::Exact(best_match.session.clone()));
        }
    }

    if sessions.is_empty() {
        Ok(ResumeLaunchResolution::NotFound {
            query: Some(query.to_string()),
        })
    } else {
        Ok(ResumeLaunchResolution::Picker {
            sessions,
            query: Some(query.to_string()),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ResumeMatchRank {
    ExactId,
    PrefixId,
    ExactName,
    ExactSlug,
    ExactCwdName,
    ExactTag,
    NameContains,
    SlugContains,
    NoteContains,
    CwdContains,
}

impl ResumeMatchRank {
    fn is_precise(self) -> bool {
        matches!(
            self,
            Self::ExactId
                | Self::PrefixId
                | Self::ExactName
                | Self::ExactSlug
                | Self::ExactCwdName
                | Self::ExactTag
        )
    }
}

#[derive(Debug, Clone)]
struct ResumeCandidate {
    session: SessionSummary,
    rank: ResumeMatchRank,
}

/// Lists sessions that belong in the current `/resume` picker scope.
pub(crate) fn resumable_sessions_for_picker(
    session_store: &SessionStore,
    current_session_id: uuid::Uuid,
    current_cwd: &Path,
) -> Result<Vec<SessionSummary>> {
    let scope = resume_scope(current_cwd);
    Ok(all_resumable_sessions(session_store, current_session_id)?
        .into_iter()
        .filter(|session| session_scope_matches(&scope, &session.cwd))
        .collect())
}

fn all_resumable_sessions(
    session_store: &SessionStore,
    current_session_id: uuid::Uuid,
) -> Result<Vec<SessionSummary>> {
    Ok(session_store
        .list_sessions()?
        .into_iter()
        .filter(|session| session.id != current_session_id)
        .collect())
}

fn search_sessions(sessions: &[SessionSummary], query: &str) -> Vec<ResumeCandidate> {
    let normalized = query.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return Vec::new();
    }

    let mut matches = sessions
        .iter()
        .filter_map(|session| {
            session_match_rank(session, &normalized).map(|rank| ResumeCandidate {
                session: session.clone(),
                rank,
            })
        })
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| {
        left.rank
            .cmp(&right.rank)
            .then_with(|| right.session.updated_at_ms.cmp(&left.session.updated_at_ms))
            .then_with(|| left.session.id.cmp(&right.session.id))
    });
    matches
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ResumeScope {
    Repo(PathBuf),
    Directory(PathBuf),
}

fn resume_scope(path: &Path) -> ResumeScope {
    git_toplevel(path)
        .map(ResumeScope::Repo)
        .unwrap_or_else(|| ResumeScope::Directory(normalize_resume_path(path)))
}

fn session_scope_matches(scope: &ResumeScope, session_cwd: &Path) -> bool {
    match scope {
        ResumeScope::Repo(repo_root) => git_toplevel(session_cwd).as_ref() == Some(repo_root),
        ResumeScope::Directory(directory) => {
            let session_path = normalize_resume_path(session_cwd);
            session_path == *directory
                || session_path.starts_with(directory)
                || directory.starts_with(&session_path)
        }
    }
}

fn git_toplevel(path: &Path) -> Option<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if root.is_empty() {
        return None;
    }
    Some(normalize_resume_path(Path::new(&root)))
}

fn normalize_resume_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn looks_like_session_id(query: &str) -> bool {
    let trimmed = query.trim();
    trimmed.len() >= 8
        && trimmed
            .chars()
            .all(|ch| ch.is_ascii_hexdigit() || ch == '-')
}

fn session_match_rank(session: &SessionSummary, query: &str) -> Option<ResumeMatchRank> {
    let id = session.id.to_string();
    let name = session
        .display_name
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let slug = session
        .slug
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let note = session
        .note
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let cwd = session.cwd.display().to_string().to_ascii_lowercase();
    let cwd_name = session
        .cwd
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let exact_tag = session
        .tags
        .iter()
        .any(|tag| tag.eq_ignore_ascii_case(query));

    if id == query {
        Some(ResumeMatchRank::ExactId)
    } else if id.starts_with(query) {
        Some(ResumeMatchRank::PrefixId)
    } else if !name.is_empty() && name == query {
        Some(ResumeMatchRank::ExactName)
    } else if !slug.is_empty() && slug == query {
        Some(ResumeMatchRank::ExactSlug)
    } else if !cwd_name.is_empty() && cwd_name == query {
        Some(ResumeMatchRank::ExactCwdName)
    } else if exact_tag {
        Some(ResumeMatchRank::ExactTag)
    } else if !name.is_empty() && name.contains(query) {
        Some(ResumeMatchRank::NameContains)
    } else if !slug.is_empty() && slug.contains(query) {
        Some(ResumeMatchRank::SlugContains)
    } else if !note.is_empty() && note.contains(query) {
        Some(ResumeMatchRank::NoteContains)
    } else if cwd.contains(query) {
        Some(ResumeMatchRank::CwdContains)
    } else {
        None
    }
}

fn render_resume_listing(sessions: &[SessionSummary]) -> String {
    if sessions.is_empty() {
        return "No conversations found to resume.".to_string();
    }

    let mut text = String::from("Recent sessions:\n");
    for session in sessions.iter().take(20) {
        let _ = writeln!(&mut text, "{}", format_session_line(session));
    }
    text.push_str("\nRun `/resume <session-id|name|slug|tag>` to restore one session.");
    text
}

fn render_resume_ambiguity(query: &str, matches: &[ResumeCandidate]) -> String {
    let mut text = format!(
        "Found {} sessions matching `{query}`.\nUse `/resume <session-id>` to pick one.\n\nMatches:\n",
        matches.len()
    );
    for candidate in matches.iter().take(10) {
        let _ = writeln!(&mut text, "{}", format_session_line(&candidate.session));
    }
    if matches.len() > 10 {
        let _ = writeln!(&mut text, "... {} more", matches.len() - 10);
    }
    text
}

fn format_session_line(session: &SessionSummary) -> String {
    let mut extras = Vec::new();
    if let Some(slug) = session
        .slug
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        extras.push(format!("slug={slug}"));
    }
    if !session.tags.is_empty() {
        extras.push(format!("tags={}", session.tags.join(",")));
    }
    let extra_suffix = if extras.is_empty() {
        String::new()
    } else {
        format!(" {}", extras.join(" "))
    };
    format!(
        "- {} {} [{}]{}",
        session.id,
        session.display_name.as_deref().unwrap_or("<unnamed>"),
        session.cwd.display(),
        extra_suffix
    )
}

fn resume_or_reroute(
    state: &mut AppState,
    session_store: &SessionStore,
    summary: &SessionSummary,
) -> Result<()> {
    if session_scope_matches(&resume_scope(&state.cwd), &summary.cwd) {
        return resume_into_session(state, session_store, summary);
    }
    emit_system(state, session_store, render_cross_project_resume(summary))
}

fn render_cross_project_resume(summary: &SessionSummary) -> String {
    let command = format!(
        "cd {} && puffer resume {}",
        shell_quote(summary.cwd.display().to_string().as_str()),
        summary.id
    );
    format!("This conversation is from a different directory.\n\nTo resume, run:\n  {command}")
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    let escaped = value.replace('\'', "'\"'\"'");
    format!("'{escaped}'")
}

fn resume_into_session(
    state: &mut AppState,
    session_store: &SessionStore,
    summary: &SessionSummary,
) -> Result<()> {
    let record = session_store.load_session(summary.id)?;
    let pending_query_prompt = state.take_pending_query_prompt();
    let config = state.config.clone();
    *state = AppState::from_session_record(config, record);
    if let Some(prompt) = pending_query_prompt {
        state.queue_pending_query_prompt(prompt);
    }
    emit_system(
        state,
        session_store,
        format!(
            "Resumed session {} [{}].",
            state.session.id,
            state.session.display_name.as_deref().unwrap_or("<unnamed>")
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::{
        resolve_resume_launch, resume_into_session, resume_scope, search_sessions,
        session_scope_matches, ResumeLaunchResolution,
    };
    use crate::AppState;
    use puffer_config::PufferConfig;
    use puffer_config::{ensure_workspace_dirs, ConfigPaths};
    use puffer_session_store::SessionStore;
    use puffer_session_store::SessionSummary;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use tempfile::tempdir;
    use uuid::Uuid;

    fn session(
        id: &str,
        name: Option<&str>,
        cwd: &str,
        slug: Option<&str>,
        tags: &[&str],
        note: Option<&str>,
        updated_at_ms: u64,
    ) -> SessionSummary {
        SessionSummary {
            id: Uuid::parse_str(id).unwrap(),
            display_name: name.map(str::to_string),
            generated_title: None,
            cwd: PathBuf::from(cwd),
            created_at_ms: updated_at_ms,
            updated_at_ms,
            event_count: 0,
            parent_session_id: None,
            slug: slug.map(str::to_string),
            tags: tags.iter().map(|tag| tag.to_string()).collect(),
            note: note.map(str::to_string),
        }
    }

    #[test]
    fn search_prefers_exact_slug_over_note_match() {
        let sessions = vec![
            session(
                "11111111-1111-1111-1111-111111111111",
                Some("Review"),
                "/tmp/one",
                Some("dockyard"),
                &[],
                None,
                1,
            ),
            session(
                "22222222-2222-2222-2222-222222222222",
                Some("Other"),
                "/tmp/two",
                None,
                &[],
                Some("dockyard"),
                2,
            ),
        ];

        let matches = search_sessions(&sessions, "dockyard");
        assert_eq!(matches[0].session.id, sessions[0].id);
    }

    #[test]
    fn search_matches_tags_and_cwd_names() {
        let sessions = vec![
            session(
                "11111111-1111-1111-1111-111111111111",
                Some("Review"),
                "/tmp/shipyard",
                None,
                &["review"],
                None,
                1,
            ),
            session(
                "22222222-2222-2222-2222-222222222222",
                Some("Refactor"),
                "/tmp/dockyard",
                None,
                &[],
                None,
                2,
            ),
        ];

        assert_eq!(
            search_sessions(&sessions, "review")[0].session.id,
            sessions[0].id
        );
        assert_eq!(
            search_sessions(&sessions, "dockyard")[0].session.id,
            sessions[1].id
        );
    }

    #[test]
    fn directory_scope_matches_nested_workspace_paths() {
        let scope = resume_scope(Path::new("/tmp/workspace"));

        assert!(session_scope_matches(
            &scope,
            Path::new("/tmp/workspace/dockyard")
        ));
        assert!(session_scope_matches(&scope, Path::new("/tmp/workspace")));
        assert!(!session_scope_matches(&scope, Path::new("/tmp/elsewhere")));
    }

    #[test]
    fn resolve_resume_launch_uses_picker_for_empty_query() {
        let tempdir = tempdir().unwrap();
        let repo_root = tempdir.path().join("repo");
        let current_cwd = repo_root.join("current");
        let sibling_cwd = repo_root.join("dockyard");
        std::fs::create_dir_all(&current_cwd).unwrap();
        std::fs::create_dir_all(&sibling_cwd).unwrap();
        init_git_repo(&repo_root);
        let paths = ConfigPaths::discover(tempdir.path());
        ensure_workspace_dirs(&paths).unwrap();
        let session_store = SessionStore::from_paths(&paths).unwrap();
        session_store.create_session(current_cwd.clone()).unwrap();
        let other = session_store.create_session(sibling_cwd).unwrap();

        let resolution = resolve_resume_launch(&session_store, &current_cwd, None).unwrap();
        match resolution {
            ResumeLaunchResolution::Picker { sessions, query } => {
                assert_eq!(query, None);
                assert_eq!(sessions.len(), 2);
                assert!(sessions.iter().any(|session| session.id == other.id));
            }
            other => panic!("expected picker resolution, got {other:?}"),
        }
    }

    #[test]
    fn resolve_resume_launch_returns_exact_for_precise_match() {
        let tempdir = tempdir().unwrap();
        let repo_root = tempdir.path().join("repo");
        let current_cwd = repo_root.join("current");
        let sibling_cwd = repo_root.join("dockyard");
        std::fs::create_dir_all(&current_cwd).unwrap();
        std::fs::create_dir_all(&sibling_cwd).unwrap();
        init_git_repo(&repo_root);
        let paths = ConfigPaths::discover(tempdir.path());
        ensure_workspace_dirs(&paths).unwrap();
        let session_store = SessionStore::from_paths(&paths).unwrap();
        session_store.create_session(current_cwd.clone()).unwrap();
        let other = session_store.create_session(sibling_cwd).unwrap();
        session_store
            .rename_session(other.id, "dockyard".to_string())
            .unwrap();

        let resolution =
            resolve_resume_launch(&session_store, &current_cwd, Some("dockyard")).unwrap();
        match resolution {
            ResumeLaunchResolution::Exact(session) => assert_eq!(session.id, other.id),
            other => panic!("expected exact resolution, got {other:?}"),
        }
    }

    #[test]
    fn resolve_resume_launch_falls_back_to_picker_for_search_terms() {
        let tempdir = tempdir().unwrap();
        let repo_root = tempdir.path().join("repo");
        let current_cwd = repo_root.join("current");
        let sibling_a = repo_root.join("dockyard-a");
        let sibling_b = repo_root.join("dockyard-b");
        std::fs::create_dir_all(&current_cwd).unwrap();
        std::fs::create_dir_all(&sibling_a).unwrap();
        std::fs::create_dir_all(&sibling_b).unwrap();
        init_git_repo(&repo_root);
        let paths = ConfigPaths::discover(tempdir.path());
        ensure_workspace_dirs(&paths).unwrap();
        let session_store = SessionStore::from_paths(&paths).unwrap();
        session_store.create_session(current_cwd.clone()).unwrap();
        let first = session_store.create_session(sibling_a).unwrap();
        let second = session_store.create_session(sibling_b).unwrap();

        let resolution =
            resolve_resume_launch(&session_store, &current_cwd, Some("dockyard")).unwrap();
        match resolution {
            ResumeLaunchResolution::Picker { sessions, query } => {
                assert_eq!(query.as_deref(), Some("dockyard"));
                assert_eq!(sessions.len(), 3);
                assert!(sessions.iter().any(|session| session.id == first.id));
                assert!(sessions.iter().any(|session| session.id == second.id));
            }
            other => panic!("expected picker resolution, got {other:?}"),
        }
    }

    #[test]
    fn resume_into_session_preserves_pending_query_prompt() {
        let tempdir = tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        ensure_workspace_dirs(&paths).unwrap();
        let session_store = SessionStore::from_paths(&paths).unwrap();
        let current = session_store
            .create_session(tempdir.path().join("current"))
            .unwrap();
        let target = session_store
            .create_session(tempdir.path().join("target"))
            .unwrap();
        let summary = session_store
            .list_sessions()
            .unwrap()
            .into_iter()
            .find(|session| session.id == target.id)
            .unwrap();
        let mut state = AppState::new(
            PufferConfig::default(),
            tempdir.path().join("current"),
            current,
        );
        state.queue_pending_query_prompt("follow up after picking session");

        resume_into_session(&mut state, &session_store, &summary).unwrap();

        assert_eq!(state.session.id, target.id);
        assert_eq!(
            state.take_pending_query_prompt().as_deref(),
            Some("follow up after picking session")
        );
    }

    fn init_git_repo(path: &Path) {
        let output = Command::new("git").arg("init").arg(path).output().unwrap();
        assert!(
            output.status.success(),
            "git init failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

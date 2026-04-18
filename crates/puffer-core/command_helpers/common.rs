use super::{append_tool_invocations, append_trace_events};
use crate::{AppState, MessageRole};
use anyhow::{Context, Result};
use puffer_provider_registry::{AuthStore, ProviderRegistry};
use puffer_resources::{skill_by_name, LoadedItem, LoadedResources, SkillSpec, SourceKind};
use puffer_session_store::{GitDiffSnapshot, SessionStore, TranscriptEvent};
use std::fmt::Write as _;
use std::io::{self, IsTerminal, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const SKILL_DESCRIPTION_CHARS_PER_TOKEN: usize = 4;

/// Lists loaded skills in slash-command form.
pub(crate) fn list_skills(
    state: &mut AppState,
    resources: &LoadedResources,
    session_store: &SessionStore,
) -> Result<()> {
    emit_system(state, session_store, render_skills_panel(resources))
}

/// Renders the grouped `/skills` panel used by the TUI and transcript fallback.
pub fn render_skills_panel(resources: &LoadedResources) -> String {
    if resources.skills.is_empty() {
        return [
            "No skills found.",
            "",
            "Create skills in one of these locations:",
            "- ~/.puffer/resources/skills/",
            "- .puffer/resources/skills/",
            "",
            "Use /skill:<name> as a compatibility alias after adding a user-invocable skill.",
        ]
        .join("\n");
    }

    let mut text = String::new();
    let skill_count = resources.skills.len();
    let _ = writeln!(
        &mut text,
        "{} {}",
        skill_count,
        if skill_count == 1 { "skill" } else { "skills" }
    );

    append_skill_group(&mut text, resources, SourceKind::Workspace);
    append_skill_group(&mut text, resources, SourceKind::User);
    append_skill_group(&mut text, resources, SourceKind::Builtin);
    let _ = writeln!(&mut text);
    let _ = writeln!(
        &mut text,
        "Use /skill:<name> as a compatibility alias for any user-invocable skill."
    );

    text.trim_end().to_string()
}

/// Prints a compact summary of transcript and loaded-resource context.
pub(crate) fn describe_context(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    session_store: &SessionStore,
) -> Result<()> {
    emit_system(
        state,
        session_store,
        crate::runtime::render_context_usage_summary(state, resources, providers)?,
    )
}

/// Lists the files currently tracked in Claude-style file context.
pub(crate) fn describe_files_in_context(
    state: &mut AppState,
    session_store: &SessionStore,
) -> Result<()> {
    emit_system(state, session_store, render_files_in_context(state))
}

/// Shows the current git status summary for the workspace.
pub(crate) fn describe_git_diff(state: &mut AppState, session_store: &SessionStore) -> Result<()> {
    emit_system(
        state,
        session_store,
        render_git_diff_summary(&state.cwd, session_store, state.session.id),
    )
}

/// Renders the current Claude-style file-context listing.
pub(crate) fn render_files_in_context(state: &AppState) -> String {
    let mut files = state
        .claude_read_state
        .keys()
        .map(|path| {
            path.strip_prefix(&state.cwd)
                .unwrap_or(path.as_path())
                .display()
                .to_string()
        })
        .collect::<Vec<_>>();
    files.sort();
    files.dedup();
    if files.is_empty() {
        "No files in context".to_string()
    } else {
        format!("Files in context:\n{}", files.join("\n"))
    }
}

/// Opens or creates a text file in an external editor and returns a status line.
pub(crate) fn open_text_file_in_editor(path: &Path) -> Result<String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    if !path.exists() {
        std::fs::write(path, b"")
            .with_context(|| format!("failed to create {}", path.display()))?;
    }

    if let Some((source, command)) = configured_editor() {
        run_shell_editor(&command, path)
            .with_context(|| format!("failed to launch editor configured by {source}"))?;
        return Ok(format!("Opened {} with {source}.", path.display()));
    }

    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        anyhow::bail!("No non-interactive editor is configured. Set $VISUAL or $EDITOR.");
    }

    for fallback in ["nano", "vim", "vi"] {
        match Command::new(fallback).arg(path).status() {
            Ok(status) if status.success() => {
                return Ok(format!("Opened {} with {}.", path.display(), fallback));
            }
            Ok(_) => continue,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error).context("failed to launch fallback editor"),
        }
    }

    anyhow::bail!("No editor is configured. Set $VISUAL or $EDITOR.");
}

/// Renders a UTF-8 QR block when `qrencode` is available on PATH.
pub(crate) fn render_utf8_qr(data: &str) -> Option<String> {
    let mut child = Command::new("qrencode")
        .args(["-t", "UTF8", "-m", "1"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    child.stdin.as_mut()?.write_all(data.as_bytes()).ok()?;
    let output = child.wait_with_output().ok()?;
    if !output.status.success() {
        return None;
    }
    let qr = String::from_utf8(output.stdout).ok()?;
    let trimmed = qr.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/// Executes one loaded skill command through the provider runtime.
pub(crate) fn execute_skill_command(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &mut ProviderRegistry,
    auth_store: &mut AuthStore,
    session_store: &SessionStore,
    skill_name: &str,
    args: &str,
) -> Result<()> {
    let Some(skill) = skill_by_name(resources, skill_name) else {
        return emit_system(state, session_store, format!("Unknown skill {skill_name}."));
    };
    if !skill.value.user_invocable {
        return emit_system(
            state,
            session_store,
            format!(
                "This skill can only be invoked by Claude, not directly by users. Ask Claude to use the \"{}\" skill for you.",
                skill.value.name
            ),
        );
    }

    let _ = providers.discover_and_merge_all(auth_store);
    let saved_provider = state.current_provider.clone();
    let saved_model = state.current_model.clone();
    let saved_effort = state.effort_level.clone();
    apply_skill_runtime_overrides(state, providers, &skill.value);

    let rendered =
        crate::skill_support::render_skill_prompt(skill, args, &state.session.id.to_string());
    let tool_filter = crate::runtime::build_request_tool_filter(&skill.value.allowed_tools)?;

    state.push_message(MessageRole::User, rendered.clone());
    session_store.append_event(
        state.session.id,
        TranscriptEvent::UserMessage {
            text: rendered.clone(),
        },
    )?;

    let outcome = crate::runtime::execute_user_prompt_with_tool_filter(
        state,
        resources,
        providers,
        auth_store,
        &rendered,
        tool_filter.as_ref(),
    );

    state.current_provider = saved_provider;
    state.current_model = saved_model;
    state.effort_level = saved_effort;

    match outcome {
        Ok(turn) => {
            append_tool_invocations(state, session_store, &turn.tool_invocations)?;
            append_trace_events(session_store, state.session.id, &turn.reflection_traces);
            state.push_message(MessageRole::Assistant, turn.assistant_text.clone());
            session_store.append_event(
                state.session.id,
                TranscriptEvent::AssistantMessage {
                    text: turn.assistant_text,
                },
            )?;
            Ok(())
        }
        Err(error) => emit_system(
            state,
            session_store,
            format!("Skill command /skill:{} failed: {error}", skill.value.name),
        ),
    }
}

fn apply_skill_runtime_overrides(
    state: &mut AppState,
    providers: &ProviderRegistry,
    skill: &SkillSpec,
) {
    if let Some(model_id) = skill.model.as_deref() {
        if let Some(model) = providers.resolve_model(model_id) {
            state.current_provider = Some(model.provider.clone());
            state.current_model = Some(format!("{}/{}", model.provider, model.id));
        } else {
            state.current_model = Some(model_id.to_string());
            state.current_provider = model_id
                .split_once('/')
                .map(|(provider, _)| provider.to_string())
                .or_else(|| state.current_provider.clone());
        }
    }
    if let Some(effort) = skill.effort.as_deref() {
        state.effort_level = effort.to_string();
    }
}

/// Appends a system message to the in-memory transcript and session log.
pub(crate) fn emit_system(
    state: &mut AppState,
    session_store: &SessionStore,
    text: String,
) -> Result<()> {
    state.push_message(MessageRole::System, text.clone());
    session_store.append_event(state.session.id, TranscriptEvent::SystemMessage { text })?;
    Ok(())
}

/// Rewinds the transcript to the requested point.
pub(crate) fn rewind_transcript(
    state: &mut AppState,
    session_store: &SessionStore,
    args: &str,
) -> Result<()> {
    if state.transcript.is_empty() {
        return emit_system(
            state,
            session_store,
            "Transcript is already empty.".to_string(),
        );
    }
    let trimmed = args.trim();
    let Some(pop_count) = rewind_pop_count(state, trimmed) else {
        return emit_system(
            state,
            session_store,
            if trimmed.is_empty() {
                "Nothing is available to rewind to yet.".to_string()
            } else {
                format!("Unknown rewind target `{trimmed}`.")
            },
        );
    };
    if pop_count == 0 {
        return emit_system(
            state,
            session_store,
            "Already at the selected rewind point.".to_string(),
        );
    }
    session_store.append_transcript_pop_last(state.session.id, pop_count)?;
    state.apply_transcript_rewrite(&puffer_session_store::TranscriptRewrite::PopLast {
        count: pop_count,
    });
    let message = if trimmed.is_empty() {
        "Removed the latest rendered transcript item.".to_string()
    } else {
        format!("Rewound conversation to before user turn {trimmed}.")
    };
    emit_system(state, session_store, message)
}

/// Records post-command session state and a git diff history snapshot.
pub(crate) fn record_command_checkpoint(
    state: &AppState,
    session_store: &SessionStore,
    command_name: &str,
    args: &str,
) -> Result<()> {
    session_store.append_event(state.session.id, state.snapshot_event())?;
    let snapshot = capture_git_diff_snapshot(&state.cwd, command_name, args);
    session_store.append_git_diff_snapshot(state.session.id, snapshot)
}

fn render_git_diff_summary(
    cwd: &PathBuf,
    session_store: &SessionStore,
    session_id: uuid::Uuid,
) -> String {
    let current = render_current_git_diff_summary(cwd);
    let history = render_git_diff_history(session_store, session_id);
    if history.is_empty() {
        current
    } else {
        format!("{current}\n\nRecent turn-by-turn diff snapshots:\n{history}")
    }
}

fn render_current_git_diff_summary(cwd: &PathBuf) -> String {
    let status = match run_git(cwd, &["status", "--short"]) {
        Ok(status) => status,
        Err(error) => return error,
    };
    if status.trim().is_empty() {
        return "Working tree is clean.".to_string();
    }

    let mut text = String::new();
    let _ = writeln!(&mut text, "Git status:");
    let _ = writeln!(&mut text, "{}", status.trim_end());

    append_stat_section(&mut text, cwd, "Unstaged diffstat", &["diff", "--stat"]);
    append_stat_section(
        &mut text,
        cwd,
        "Staged diffstat",
        &["diff", "--cached", "--stat"],
    );
    append_patch_section(&mut text, cwd, "Unstaged patch", &["diff"]);
    append_patch_section(&mut text, cwd, "Staged patch", &["diff", "--cached"]);

    text.trim_end().to_string()
}

fn render_git_diff_history(session_store: &SessionStore, session_id: uuid::Uuid) -> String {
    let Ok(record) = session_store.load_session(session_id) else {
        return String::new();
    };
    let snapshots = record
        .events
        .into_iter()
        .filter_map(|event| match event {
            TranscriptEvent::GitDiffSnapshot { snapshot } => Some(snapshot),
            _ => None,
        })
        .collect::<Vec<_>>();
    if snapshots.is_empty() {
        return String::new();
    }

    snapshots
        .into_iter()
        .rev()
        .take(6)
        .enumerate()
        .map(|(index, snapshot)| render_git_snapshot(index + 1, &snapshot))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn render_git_snapshot(number: usize, snapshot: &GitDiffSnapshot) -> String {
    let mut text = format!("{}. {}", number, snapshot.command);
    if !snapshot.status.trim().is_empty() {
        let _ = write!(&mut text, "\nstatus:\n{}", snapshot.status.trim_end());
    }
    if !snapshot.unstaged_diffstat.trim().is_empty() {
        let _ = write!(
            &mut text,
            "\nunstaged_diffstat:\n{}",
            snapshot.unstaged_diffstat.trim_end()
        );
    }
    if !snapshot.staged_diffstat.trim().is_empty() {
        let _ = write!(
            &mut text,
            "\nstaged_diffstat:\n{}",
            snapshot.staged_diffstat.trim_end()
        );
    }
    if !snapshot.patch_excerpt.trim().is_empty() {
        let _ = write!(
            &mut text,
            "\npatch_excerpt:\n{}",
            snapshot.patch_excerpt.trim_end()
        );
    }
    text
}

fn capture_git_diff_snapshot(cwd: &PathBuf, command_name: &str, args: &str) -> GitDiffSnapshot {
    let command = if args.is_empty() {
        format!("/{command_name}")
    } else {
        format!("/{command_name} {args}")
    };
    let status = run_git(cwd, &["status", "--short"]).unwrap_or_else(|error| error);
    let unstaged_diffstat = run_git(cwd, &["diff", "--stat"]).unwrap_or_else(|error| error);
    let staged_diffstat =
        run_git(cwd, &["diff", "--cached", "--stat"]).unwrap_or_else(|error| error);
    let patch = run_git(cwd, &["diff", "--cached", "--patch", "--no-ext-diff"])
        .or_else(|_| run_git(cwd, &["diff", "--patch", "--no-ext-diff"]))
        .unwrap_or_default();
    let (patch_excerpt, truncated) = truncate_lines(&patch, 120);
    let patch_excerpt = if truncated {
        format!("{}\n... output truncated ...", patch_excerpt.trim_end())
    } else {
        patch_excerpt
    };
    GitDiffSnapshot {
        command,
        status,
        unstaged_diffstat,
        staged_diffstat,
        patch,
        patch_excerpt,
    }
}

fn append_skill_group(text: &mut String, resources: &LoadedResources, kind: SourceKind) {
    let mut skills = resources
        .skills
        .iter()
        .filter(|skill| skill.source_info.kind == kind)
        .collect::<Vec<_>>();
    if skills.is_empty() {
        return;
    }

    skills.sort_by(|left, right| left.value.name.cmp(&right.value.name));
    let _ = writeln!(text);
    let _ = writeln!(
        text,
        "{} ({})",
        skill_source_heading(kind),
        skill_group_root(&skills)
    );
    for skill in skills {
        let mut details = vec![format!(
            "~{} description tokens",
            estimate_skill_description_tokens(&skill.value)
        )];
        if !skill.value.user_invocable {
            details.push("hidden from slash-command invocation".to_string());
        }
        if skill.value.disable_model_invocation {
            details.push("model invocation disabled".to_string());
        }
        if let Some(argument_hint) = skill.value.argument_hint.as_deref() {
            details.push(format!("args {argument_hint}"));
        }
        if let Some(model) = skill.value.model.as_deref() {
            details.push(format!("model {model}"));
        }
        let _ = writeln!(
            text,
            "- {} · {}",
            skill_command_label(skill),
            details.join(" · ")
        );
    }
}

fn skill_source_heading(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::Workspace => "Workspace skills",
        SourceKind::User => "User skills",
        SourceKind::Builtin => "Built-in skills",
    }
}

fn skill_group_root(skills: &[&LoadedItem<SkillSpec>]) -> String {
    skills
        .first()
        .map(|skill| {
            skill
                .source_info
                .path
                .parent()
                .and_then(Path::parent)
                .unwrap_or(skill.source_info.path.as_path())
                .display()
                .to_string()
        })
        .unwrap_or_else(|| "<unknown>".to_string())
}

fn skill_command_label(skill: &LoadedItem<SkillSpec>) -> String {
    if skill.value.user_invocable {
        format!("/{}", skill.value.name)
    } else {
        skill.value.name.clone()
    }
}

fn estimate_skill_description_tokens(skill: &SkillSpec) -> usize {
    let description_chars = skill.description.chars().count();
    description_chars
        .div_ceil(SKILL_DESCRIPTION_CHARS_PER_TOKEN)
        .max(1)
}

fn rewind_pop_count(state: &AppState, trimmed: &str) -> Option<usize> {
    if trimmed.is_empty() {
        return Some(1);
    }
    let selected_turn = trimmed.parse::<usize>().ok()?;
    if selected_turn == 0 {
        return None;
    }
    let transcript_index = state
        .transcript
        .iter()
        .enumerate()
        .filter(|(_, message)| message.role == MessageRole::User)
        .nth(selected_turn.saturating_sub(1))
        .map(|(index, _)| index)?;
    Some(state.transcript.len().saturating_sub(transcript_index))
}

fn append_stat_section(text: &mut String, cwd: &PathBuf, title: &str, args: &[&str]) {
    match run_git(cwd, args) {
        Ok(content) if !content.trim().is_empty() => {
            let _ = writeln!(text);
            let _ = writeln!(text, "{title}:");
            let _ = writeln!(text, "{}", content.trim_end());
        }
        Ok(_) => {}
        Err(error) => {
            let _ = writeln!(text);
            let _ = writeln!(text, "{title}:");
            let _ = writeln!(text, "{error}");
        }
    }
}

fn append_patch_section(text: &mut String, cwd: &PathBuf, title: &str, args: &[&str]) {
    match run_git(cwd, args) {
        Ok(content) if !content.trim().is_empty() => {
            let (truncated, was_truncated) = truncate_lines(&content, 220);
            let _ = writeln!(text);
            let _ = writeln!(text, "{title}:");
            let _ = writeln!(text, "{}", truncated.trim_end());
            if was_truncated {
                let _ = writeln!(text, "... output truncated ...");
            }
        }
        Ok(_) => {}
        Err(error) => {
            let _ = writeln!(text);
            let _ = writeln!(text, "{title}:");
            let _ = writeln!(text, "{error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::render_skills_panel;
    use puffer_resources::{LoadedItem, LoadedResources, SkillSpec, SourceInfo, SourceKind};
    use std::path::PathBuf;

    fn loaded_skill(
        name: &str,
        description: &str,
        path: &str,
        kind: SourceKind,
    ) -> LoadedItem<SkillSpec> {
        LoadedItem {
            value: SkillSpec {
                name: name.to_string(),
                description: description.to_string(),
                content: "content".to_string(),
                disable_model_invocation: false,
                ..SkillSpec::default()
            },
            source_info: SourceInfo {
                path: PathBuf::from(path),
                kind,
            },
        }
    }

    #[test]
    fn render_skills_panel_groups_skills_by_source() {
        let resources = LoadedResources {
            skills: vec![
                loaded_skill(
                    "workspace-review",
                    "Review workspace changes",
                    "/tmp/project/.puffer/resources/skills/workspace-review/SKILL.md",
                    SourceKind::Workspace,
                ),
                loaded_skill(
                    "user-review",
                    "Review shared changes",
                    "/home/test/.puffer/resources/skills/user-review/SKILL.md",
                    SourceKind::User,
                ),
                loaded_skill(
                    "builtin-review",
                    "Review builtin changes",
                    "/app/resources/skills/builtin-review/SKILL.md",
                    SourceKind::Builtin,
                ),
            ],
            ..LoadedResources::default()
        };

        let rendered = render_skills_panel(&resources);
        assert!(rendered.contains("3 skills"));
        assert!(rendered.contains("Workspace skills (/tmp/project/.puffer/resources/skills)"));
        assert!(rendered.contains("/workspace-review · ~6 description tokens"));
        assert!(rendered.contains("User skills (/home/test/.puffer/resources/skills)"));
        assert!(rendered.contains("/user-review · ~6 description tokens"));
        assert!(rendered.contains("Built-in skills (/app/resources/skills)"));
        assert!(rendered.contains("/builtin-review · ~6 description tokens"));
        assert!(rendered
            .contains("Use /skill:<name> as a compatibility alias for any user-invocable skill."));
    }

    #[test]
    fn render_skills_panel_reports_missing_skills() {
        let rendered = render_skills_panel(&LoadedResources::default());
        assert!(rendered.contains("No skills found."));
        assert!(rendered.contains("~/.puffer/resources/skills/"));
        assert!(rendered.contains("/skill:<name>"));
    }

    #[test]
    fn render_skills_panel_marks_hidden_and_model_disabled_skills() {
        let resources = LoadedResources {
            skills: vec![LoadedItem {
                value: SkillSpec {
                    name: "hidden-review".to_string(),
                    description: "Hidden review entry".to_string(),
                    user_invocable: false,
                    disable_model_invocation: true,
                    ..SkillSpec::default()
                },
                source_info: SourceInfo {
                    path: PathBuf::from(
                        "/tmp/project/.puffer/resources/skills/hidden-review/SKILL.md",
                    ),
                    kind: SourceKind::Workspace,
                },
            }],
            ..LoadedResources::default()
        };

        let rendered = render_skills_panel(&resources);
        assert!(rendered.contains(
            "- hidden-review · ~5 description tokens · hidden from slash-command invocation · model invocation disabled"
        ));
    }
}

fn truncate_lines(content: &str, max_lines: usize) -> (String, bool) {
    let lines = content.lines().collect::<Vec<_>>();
    if lines.len() <= max_lines {
        return (content.to_string(), false);
    }
    let truncated = lines
        .into_iter()
        .take(max_lines)
        .collect::<Vec<_>>()
        .join("\n");
    (truncated, true)
}

fn run_git(cwd: &PathBuf, args: &[&str]) -> std::result::Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .map_err(|error| format!("Failed to run git {}: {error}", args.join(" ")))?;
    if !output.status.success() {
        return Err(format!(
            "Failed to run git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn configured_editor() -> Option<(&'static str, String)> {
    std::env::var("VISUAL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| ("$VISUAL", value))
        .or_else(|| {
            std::env::var("EDITOR")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .map(|value| ("$EDITOR", value))
        })
}

fn run_shell_editor(command: &str, path: &Path) -> Result<()> {
    let status = Command::new("sh")
        .arg("-lc")
        .arg(format!("{command} \"$PUFFER_EDITOR_TARGET\""))
        .env("PUFFER_EDITOR_TARGET", path)
        .status()
        .context("failed to run shell for editor")?;
    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("editor command exited with {}", status);
    }
}

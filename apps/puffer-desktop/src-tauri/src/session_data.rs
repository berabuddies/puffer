use crate::dtos::{
    AgentDiffDto, AgentDiffEntryDto, AgentDiffFileDto, DiffSummaryDto, DivergenceReportDto,
    FolderGroupDto, SessionDetailDto, SessionListItemDto, TimelineItemDto,
};
use crate::repo_actions;
use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use puffer_session_store::{
    GitDiffSnapshot, MessageActor, SessionRecord, SessionStore, TranscriptEvent,
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Lists sessions grouped by their containing project folder for the desktop sidebar.
pub(crate) fn list_grouped_sessions() -> Result<Vec<FolderGroupDto>> {
    let root = workspace_root()?;
    let paths = ConfigPaths::discover(&root);
    let store = SessionStore::from_paths(&paths)?;
    let sessions = store.list_sessions()?;
    let mut groups = BTreeMap::<String, Vec<SessionListItemDto>>::new();

    for session in sessions {
        let folder_path = session_group_root(&session.cwd).display().to_string();
        groups
            .entry(folder_path.clone())
            .or_default()
            .push(SessionListItemDto {
                session_id: session.id.to_string(),
                display_name: session.display_name.clone(),
                generated_title: session.generated_title.clone(),
                title: session_title(
                    session.display_name.as_ref(),
                    session.generated_title.as_ref(),
                    session.slug.as_ref(),
                    &session.cwd,
                    &session.id.to_string(),
                ),
                cwd: session.cwd.display().to_string(),
                folder_path: folder_path.clone(),
                updated_at_ms: session.updated_at_ms,
                created_at_ms: session.created_at_ms,
                event_count: session.event_count,
                slug: session.slug.clone(),
                tags: session.tags.clone(),
                note: session.note.clone(),
                parent_session_id: session.parent_session_id.map(|value| value.to_string()),
            });
    }

    let mut folders = groups
        .into_iter()
        .map(|(folder_path, mut sessions)| {
            sessions.sort_by(|left, right| right.updated_at_ms.cmp(&left.updated_at_ms));
            FolderGroupDto {
                folder_id: folder_path.clone(),
                folder_label: folder_label(Path::new(&folder_path)),
                folder_path: folder_path.clone(),
                session_count: sessions.len(),
                sessions,
            }
        })
        .collect::<Vec<_>>();
    folders.sort_by(|left, right| {
        let left_latest = left
            .sessions
            .iter()
            .map(|session| session.updated_at_ms)
            .max()
            .unwrap_or(0);
        let right_latest = right
            .sessions
            .iter()
            .map(|session| session.updated_at_ms)
            .max()
            .unwrap_or(0);
        right_latest
            .cmp(&left_latest)
            .then_with(|| left.folder_label.cmp(&right.folder_label))
    });
    Ok(folders)
}

/// Loads one session and returns desktop-oriented timeline, diff, and repo metadata.
pub(crate) fn load_session_detail(session_id: &str) -> Result<SessionDetailDto> {
    let root = workspace_root()?;
    let paths = ConfigPaths::discover(&root);
    let store = SessionStore::from_paths(&paths)?;
    let session_uuid = Uuid::parse_str(session_id).context("invalid session id")?;
    let record = store.load_session(session_uuid)?;
    let folder_path = session_group_root(&record.metadata.cwd)
        .display()
        .to_string();
    let diff_history = diff_history(&record);
    let latest_diff = diff_history.first().cloned();
    let repo_status =
        repo_actions::deferred_repo_status(&record.metadata.id.to_string(), &record.metadata.cwd);
    let agent_diff = build_agent_diff(&record);
    let divergence = compute_divergence(&agent_diff, latest_diff.as_ref(), &record.metadata.cwd);

    Ok(SessionDetailDto {
        session_id: record.metadata.id.to_string(),
        display_name: record.metadata.display_name.clone(),
        generated_title: record.metadata.generated_title.clone(),
        title: session_title(
            record.metadata.display_name.as_ref(),
            record.metadata.generated_title.as_ref(),
            record.metadata.slug.as_ref(),
            &record.metadata.cwd,
            &record.metadata.id.to_string(),
        ),
        cwd: record.metadata.cwd.display().to_string(),
        folder_path,
        updated_at_ms: record.metadata.updated_at_ms,
        created_at_ms: record.metadata.created_at_ms,
        slug: record.metadata.slug.clone(),
        tags: record.metadata.tags.clone(),
        note: record.metadata.note.clone(),
        parent_session_id: record
            .metadata
            .parent_session_id
            .map(|value| value.to_string()),
        timeline: timeline_items(&record),
        latest_diff,
        diff_history,
        repo_status,
        agent_diff,
        divergence,
    })
}

/// Resolves the current working directory for one stored session id.
pub(crate) fn load_session_cwd(session_id: &str) -> Result<PathBuf> {
    let root = workspace_root()?;
    let paths = ConfigPaths::discover(&root);
    let store = SessionStore::from_paths(&paths)?;
    let session_uuid = Uuid::parse_str(session_id).context("invalid session id")?;
    let record = store.load_session(session_uuid)?;
    Ok(record.metadata.cwd)
}

fn workspace_root() -> Result<PathBuf> {
    if let Ok(value) = env::var("PUFFER_DESKTOP_ROOT") {
        let path = PathBuf::from(value);
        if path.exists() {
            return Ok(path);
        }
    }

    let current_dir = env::current_dir().context("failed to read current directory")?;
    if let Some(path) = find_workspace_root(&current_dir) {
        return Ok(path);
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if let Some(path) = find_workspace_root(&manifest_dir) {
        return Ok(path);
    }

    Ok(current_dir)
}

fn find_workspace_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|ancestor| ancestor.join(".puffer").exists() || ancestor.join(".git").exists())
        .map(PathBuf::from)
}

fn session_group_root(cwd: &Path) -> PathBuf {
    cwd.to_path_buf()
}

fn session_title(
    display_name: Option<&String>,
    generated_title: Option<&String>,
    slug: Option<&String>,
    cwd: &Path,
    fallback: &str,
) -> String {
    display_name
        .cloned()
        .or_else(|| generated_title.cloned())
        .or_else(|| slug.cloned())
        .or_else(|| {
            cwd.file_name()
                .map(|value| value.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| fallback.to_string())
}

fn folder_label(path: &Path) -> String {
    path.file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| path.display().to_string())
}

fn diff_history(record: &SessionRecord) -> Vec<DiffSummaryDto> {
    record
        .events
        .iter()
        .enumerate()
        .filter_map(|(index, event)| match event {
            TranscriptEvent::GitDiffSnapshot { snapshot } => Some(diff_summary(index, snapshot)),
            _ => None,
        })
        .rev()
        .collect()
}

fn diff_summary(index: usize, snapshot: &GitDiffSnapshot) -> DiffSummaryDto {
    DiffSummaryDto {
        id: format!("diff-{index}"),
        source: "session_history".to_string(),
        command_label: snapshot.command.clone(),
        status_text: snapshot.status.clone(),
        unstaged_diffstat: snapshot.unstaged_diffstat.clone(),
        staged_diffstat: snapshot.staged_diffstat.clone(),
        patch: if snapshot.patch.is_empty() {
            snapshot.patch_excerpt.clone()
        } else {
            snapshot.patch.clone()
        },
        patch_excerpt: snapshot.patch_excerpt.clone(),
    }
}

fn build_agent_diff(record: &SessionRecord) -> AgentDiffDto {
    let mut entries: Vec<AgentDiffEntryDto> = Vec::new();
    let mut by_path: BTreeMap<String, AgentDiffFileDto> = BTreeMap::new();

    for event in record.events.iter() {
        let TranscriptEvent::ToolInvocation {
            call_id,
            tool_id,
            input,
            success,
            ..
        } = event
        else {
            continue;
        };
        let Some(intent) = agent_edit_intent(tool_id, input) else {
            continue;
        };

        let entry = AgentDiffEntryDto {
            call_id: call_id.clone(),
            tool_id: tool_id.clone(),
            kind: intent.kind.to_string(),
            path: intent.path.clone(),
            success: *success,
            summary: intent.summary.clone(),
        };

        if *success {
            by_path
                .entry(intent.path.clone())
                .and_modify(|file| {
                    file.edit_count += 1;
                    file.latest_kind = intent.kind.to_string();
                    file.latest_summary = intent.summary.clone();
                })
                .or_insert_with(|| AgentDiffFileDto {
                    path: intent.path.clone(),
                    latest_kind: intent.kind.to_string(),
                    edit_count: 1,
                    latest_summary: intent.summary.clone(),
                });
        }

        entries.push(entry);
    }

    AgentDiffDto {
        files: by_path.into_values().collect(),
        entries,
    }
}

struct AgentEditIntent {
    kind: &'static str,
    path: String,
    summary: String,
}

fn agent_edit_intent(tool_id: &str, raw_input: &str) -> Option<AgentEditIntent> {
    let value: Value = serde_json::from_str(raw_input).ok()?;
    let obj = value.as_object()?;
    match tool_id {
        "write_file" | "Write" => {
            let path = obj
                .get("path")
                .or_else(|| obj.get("file_path"))
                .and_then(Value::as_str)?
                .to_string();
            let contents = obj
                .get("contents")
                .or_else(|| obj.get("content"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            Some(AgentEditIntent {
                kind: "write",
                path,
                summary: render_write_summary(&contents),
            })
        }
        "replace_in_file" | "edit_file" | "Edit" => {
            let path = obj
                .get("path")
                .or_else(|| obj.get("file_path"))
                .and_then(Value::as_str)?
                .to_string();
            let old = obj
                .get("old")
                .or_else(|| obj.get("old_string"))
                .or_else(|| obj.get("oldText"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let new_text = obj
                .get("new")
                .or_else(|| obj.get("new_string"))
                .or_else(|| obj.get("newText"))
                .and_then(Value::as_str)
                .unwrap_or("");
            Some(AgentEditIntent {
                kind: "replace",
                path,
                summary: render_replace_summary(old, new_text),
            })
        }
        "move_path" => {
            let from = obj.get("from").and_then(Value::as_str)?.to_string();
            let to = obj.get("to").and_then(Value::as_str)?.to_string();
            Some(AgentEditIntent {
                kind: "move",
                path: to.clone(),
                summary: format!("renamed {from} -> {to}"),
            })
        }
        "remove_path" => {
            let path = obj.get("path").and_then(Value::as_str)?.to_string();
            Some(AgentEditIntent {
                kind: "remove",
                path: path.clone(),
                summary: format!("removed {path}"),
            })
        }
        _ => None,
    }
}

fn render_write_summary(contents: &str) -> String {
    const MAX_LINES: usize = 80;
    let mut out = String::new();
    let mut lines = 0;
    for line in contents.lines() {
        if lines >= MAX_LINES {
            out.push_str("... (truncated)\n");
            break;
        }
        out.push('+');
        out.push_str(line);
        out.push('\n');
        lines += 1;
    }
    out
}

fn render_replace_summary(old: &str, new_text: &str) -> String {
    const MAX_LINES: usize = 200;
    let mut out = String::new();
    let mut lines = 0;
    for line in old.lines() {
        if lines >= MAX_LINES {
            out.push_str("... (truncated)\n");
            break;
        }
        out.push('-');
        out.push_str(line);
        out.push('\n');
        lines += 1;
    }
    for line in new_text.lines() {
        if lines >= MAX_LINES {
            out.push_str("... (truncated)\n");
            break;
        }
        out.push('+');
        out.push_str(line);
        out.push('\n');
        lines += 1;
    }
    out
}

fn compute_divergence(
    agent_diff: &AgentDiffDto,
    latest_git_diff: Option<&DiffSummaryDto>,
    cwd: &Path,
) -> DivergenceReportDto {
    let git_paths = latest_git_diff
        .map(|d| extract_paths_from_patch(&d.patch))
        .unwrap_or_default();
    let agent_relative: BTreeSet<String> = agent_diff
        .files
        .iter()
        .map(|f| relativize_path(&f.path, cwd))
        .collect();

    DivergenceReportDto {
        agent_only: agent_relative.difference(&git_paths).cloned().collect(),
        git_only: git_paths.difference(&agent_relative).cloned().collect(),
        agent_total: agent_relative.len(),
        git_total: git_paths.len(),
    }
}

fn relativize_path(path: &str, cwd: &Path) -> String {
    let trimmed = path.trim();
    if let Ok(stripped) = Path::new(trimmed).strip_prefix(cwd) {
        return stripped.display().to_string();
    }
    trimmed.to_string()
}

fn extract_paths_from_patch(patch: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in patch.lines() {
        let line = line.trim_start();
        if let Some(rest) = line.strip_prefix("diff --git ") {
            if let Some(b_index) = rest.find(" b/") {
                let after = &rest[b_index + 3..];
                let path = after.split_whitespace().next().unwrap_or("").to_string();
                if !path.is_empty() {
                    out.insert(path);
                }
            }
        }
    }
    out
}

fn timeline_items(record: &SessionRecord) -> Vec<TimelineItemDto> {
    let mut items = Vec::new();
    let mut pending_assistant = None;
    for (index, event) in record.events.iter().enumerate() {
        match event {
            TranscriptEvent::UserMessage { text, actor } => {
                flush_pending_assistant(&mut items, &mut pending_assistant);
                items.push(TimelineItemDto::UserMessage {
                    id: format!("timeline-{index}"),
                    text: text.clone(),
                    actor: actor.clone(),
                });
            }
            TranscriptEvent::AssistantMessage { text, actor } => {
                flush_pending_assistant(&mut items, &mut pending_assistant);
                pending_assistant = Some(TimelineItemDto::AssistantMessage {
                    id: format!("timeline-{index}"),
                    text: text.clone(),
                    actor: actor.clone(),
                });
            }
            TranscriptEvent::SystemMessage { text, actor } => {
                let parsed = parse_system_message(index, text, actor.clone());
                if parse_tool_message(text).is_none() {
                    flush_pending_assistant(&mut items, &mut pending_assistant);
                }
                items.extend(parsed);
            }
            TranscriptEvent::CommandInvoked { name, args, actor } => {
                flush_pending_assistant(&mut items, &mut pending_assistant);
                items.push(TimelineItemDto::Command {
                    id: format!("timeline-{index}"),
                    command_name: name.clone(),
                    command_args: args.clone(),
                    actor: actor.clone(),
                })
            }
            TranscriptEvent::GitDiffSnapshot { snapshot } => {
                flush_pending_assistant(&mut items, &mut pending_assistant);
                items.push(TimelineItemDto::DiffSnapshot {
                    id: format!("timeline-{index}"),
                    snapshot: diff_summary(index, snapshot),
                })
            }
            TranscriptEvent::SessionRenamed { name } => {
                flush_pending_assistant(&mut items, &mut pending_assistant);
                items.push(TimelineItemDto::SystemMessage {
                    id: format!("timeline-{index}"),
                    text: format!("Session renamed to {name}."),
                    actor: None,
                })
            }
            TranscriptEvent::ToolInvocation {
                call_id,
                tool_id,
                input,
                output,
                success,
                actor,
                subject,
            } => {
                let input_json = serde_json::from_str::<serde_json::Value>(input).ok();
                items.push(TimelineItemDto::ToolCall {
                    id: format!("timeline-{index}-{call_id}"),
                    tool_id: tool_id.clone(),
                    status: if *success {
                        "success".to_string()
                    } else {
                        "error".to_string()
                    },
                    summary: Some(format!(
                        "{tool_id} · {}",
                        if *success { "success" } else { "error" }
                    )),
                    input_text: input.clone(),
                    input_json,
                    output_text: output.clone(),
                    actor: actor.clone(),
                    subject: subject.clone(),
                })
            }
            TranscriptEvent::TranscriptRewritten { .. } | TranscriptEvent::StateSnapshot { .. } => {
            }
        }
    }
    flush_pending_assistant(&mut items, &mut pending_assistant);
    items
}

fn flush_pending_assistant(
    items: &mut Vec<TimelineItemDto>,
    pending_assistant: &mut Option<TimelineItemDto>,
) {
    if let Some(item) = pending_assistant.take() {
        items.push(item);
    }
}

fn parse_system_message(
    index: usize,
    text: &str,
    actor: Option<MessageActor>,
) -> Vec<TimelineItemDto> {
    if let Some(parsed) = parse_tool_message(text) {
        let summary = summarize_tool_input(&parsed.tool_id, &parsed.input_text);
        let mut items = vec![TimelineItemDto::ToolCall {
            id: format!("timeline-{index}"),
            tool_id: parsed.tool_id.clone(),
            status: parsed.status,
            summary: summary.clone(),
            input_text: parsed.input_text.clone(),
            input_json: parsed.input_json,
            output_text: parsed.output_text.clone(),
            actor: actor.clone(),
            subject: None,
        }];

        if let Some((state, reason)) = permission_state(&parsed.output_text) {
            items.push(TimelineItemDto::PermissionDialog {
                id: format!("timeline-{index}-permission"),
                tool_id: parsed.tool_id,
                state: state.to_string(),
                summary,
                reason: reason.to_string(),
                input_text: Some(parsed.input_text),
                actor: actor.clone(),
            });
        }
        return items;
    }

    vec![TimelineItemDto::SystemMessage {
        id: format!("timeline-{index}"),
        text: text.to_string(),
        actor,
    }]
}

struct ParsedToolMessage {
    tool_id: String,
    status: String,
    input_text: String,
    input_json: Option<Value>,
    output_text: String,
}

fn parse_tool_message(text: &str) -> Option<ParsedToolMessage> {
    let (header, rest) = text.split_once('\n')?;
    let header = header.strip_prefix("Tool ")?;
    let (tool_id, status) = header.rsplit_once(" [")?;
    let status = status.strip_suffix(']')?;
    let input = rest.strip_prefix("input: ")?;
    let (input_text, output_text) = input
        .split_once('\n')
        .map(|(left, right)| (left.to_string(), right.to_string()))
        .unwrap_or_else(|| (input.to_string(), String::new()));
    Some(ParsedToolMessage {
        tool_id: tool_id.to_string(),
        status: status.to_string(),
        input_json: serde_json::from_str(&input_text).ok(),
        input_text,
        output_text,
    })
}

fn permission_state(output_text: &str) -> Option<(&'static str, &str)> {
    let trimmed = output_text.trim();
    if let Some(reason) = trimmed.strip_prefix("Permission required:") {
        return Some(("required", reason.trim()));
    }
    if let Some(reason) = trimmed.strip_prefix("Permission denied:") {
        return Some(("denied", reason.trim()));
    }
    None
}

fn summarize_tool_input(tool_id: &str, input_text: &str) -> Option<String> {
    let parsed = serde_json::from_str::<Value>(input_text).ok()?;
    match tool_id {
        "Bash" | "PowerShell" => parsed
            .get("command")
            .and_then(Value::as_str)
            .map(|value| format!("Command: {value}")),
        "Config" => parsed
            .get("setting")
            .and_then(Value::as_str)
            .map(|value| format!("Setting: {value}")),
        "WebSearch" => parsed
            .get("query")
            .and_then(Value::as_str)
            .map(|value| format!("Query: {value}")),
        "SendMessage" => parsed
            .get("to")
            .and_then(Value::as_str)
            .map(|value| format!("Recipient: {value}")),
        "AskUserQuestion" => parsed
            .get("questions")
            .and_then(Value::as_array)
            .map(|value| format!("Questions: {}", value.len())),
        "Read" | "Edit" | "Write" => parsed
            .get("file_path")
            .or_else(|| parsed.get("path"))
            .and_then(Value::as_str)
            .map(|value| format!("Path: {value}")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_tool_message, permission_state, summarize_tool_input};

    #[test]
    fn parses_tool_messages() {
        let parsed = parse_tool_message(
            "Tool Config [error]\ninput: {\"setting\":\"theme\"}\nPermission required: config writes require approval",
        )
        .expect("tool message");
        assert_eq!(parsed.tool_id, "Config");
        assert_eq!(parsed.status, "error");
        assert_eq!(parsed.input_text, "{\"setting\":\"theme\"}");
        assert_eq!(
            permission_state(&parsed.output_text),
            Some(("required", "config writes require approval"))
        );
        assert_eq!(
            summarize_tool_input(&parsed.tool_id, &parsed.input_text),
            Some("Setting: theme".to_string())
        );
    }
}

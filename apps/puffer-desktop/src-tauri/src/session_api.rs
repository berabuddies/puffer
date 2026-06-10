use crate::dto::{
    ChatAttachmentDto, DiffSnapshotDto, FolderGroupDto, PermissionDialogDto, SessionDiffsDto,
    SessionListItemDto, SessionTimelineDto, TimelineItemDto,
};
use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use puffer_session_store::{
    GitDiffSnapshot, SessionMetadata, SessionRecord, SessionStore, SessionSummary, TranscriptEvent,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Lists sessions grouped by their canonical working directory for the desktop sidebar.
pub(crate) fn list_grouped_sessions() -> Result<Vec<FolderGroupDto>> {
    let store = session_store()?;
    let mut groups: BTreeMap<String, FolderGroupDto> = BTreeMap::new();

    for session in store.list_sessions()? {
        let path = normalize_session_path(&session.cwd);
        let group = groups
            .entry(path.clone())
            .or_insert_with(|| FolderGroupDto {
                id: path.clone(),
                label: folder_label(&path),
                path: path.clone(),
                sessions: Vec::new(),
            });
        group.sessions.push(summary_to_dto(&session));
    }

    let mut folders: Vec<_> = groups.into_values().collect();
    folders.sort_by(|left, right| {
        left.label
            .cmp(&right.label)
            .then(left.path.cmp(&right.path))
    });
    for folder in &mut folders {
        folder
            .sessions
            .sort_by(|left, right| right.updated_at_ms.cmp(&left.updated_at_ms));
    }
    Ok(folders)
}

/// Loads one session timeline with conversation, tool, and snapshot items.
pub(crate) fn load_session_timeline(session_id: &str) -> Result<SessionTimelineDto> {
    let store = session_store()?;
    let record = load_session_record_from_store(&store, session_id)?;
    Ok(SessionTimelineDto {
        session: metadata_to_dto(&record.metadata, record.events.len()),
        items: timeline_items(&store, &record),
    })
}

/// Loads the latest and historical diff snapshots stored for one session.
pub(crate) fn load_session_diffs(session_id: &str) -> Result<SessionDiffsDto> {
    let record = load_session_record(session_id)?;
    let history = diff_history(&record);
    let latest_diff = history.first().cloned();
    Ok(SessionDiffsDto {
        session_id: session_id.to_string(),
        latest_diff,
        history,
    })
}

/// Resolves a session id to its working directory for repo-scoped actions.
pub(crate) fn load_session_cwd(session_id: &str) -> Result<PathBuf> {
    Ok(load_session_record(session_id)?.metadata.cwd)
}

fn session_store() -> Result<SessionStore> {
    let root = workspace_root()?;
    SessionStore::from_paths(&ConfigPaths::discover(root))
}

fn workspace_root() -> Result<PathBuf> {
    if let Ok(path) = env::var("PUFFER_DESKTOP_ROOT") {
        let root = PathBuf::from(path);
        if root.exists() {
            return Ok(root);
        }
    }

    let cwd = env::current_dir().context("failed to read current directory")?;
    if let Some(root) = find_repo_root(&cwd) {
        return Ok(root);
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if let Some(root) = find_repo_root(&manifest_dir) {
        return Ok(root);
    }

    Ok(cwd)
}

fn find_repo_root(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|candidate| candidate.join(".git").exists())
        .map(PathBuf::from)
}

fn load_session_record(session_id: &str) -> Result<SessionRecord> {
    let store = session_store()?;
    load_session_record_from_store(&store, session_id)
}

fn load_session_record_from_store(store: &SessionStore, session_id: &str) -> Result<SessionRecord> {
    let id = Uuid::parse_str(session_id).context("invalid session id")?;
    store.load_session(id)
}

fn normalize_session_path(path: &Path) -> String {
    fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .display()
        .to_string()
}

fn folder_label(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|value| value.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| path.to_string())
}

fn summary_to_dto(summary: &SessionSummary) -> SessionListItemDto {
    SessionListItemDto {
        id: summary.id.to_string(),
        title: session_title(summary),
        display_name: summary.display_name.clone(),
        generated_title: summary.generated_title.clone(),
        cwd: normalize_session_path(&summary.cwd),
        created_at_ms: summary.created_at_ms,
        updated_at_ms: summary.updated_at_ms,
        event_count: summary.event_count,
        parent_session_id: summary.parent_session_id.map(|value| value.to_string()),
        slug: summary.slug.clone(),
        tags: summary.tags.clone(),
        note: summary.note.clone(),
    }
}

fn metadata_to_dto(metadata: &SessionMetadata, event_count: usize) -> SessionListItemDto {
    SessionListItemDto {
        id: metadata.id.to_string(),
        title: metadata
            .display_name
            .clone()
            .or(metadata.generated_title.clone())
            .or(metadata.slug.clone())
            .unwrap_or_else(|| metadata.id.to_string()),
        display_name: metadata.display_name.clone(),
        generated_title: metadata.generated_title.clone(),
        cwd: normalize_session_path(&metadata.cwd),
        created_at_ms: metadata.created_at_ms,
        updated_at_ms: metadata.updated_at_ms,
        event_count,
        parent_session_id: metadata.parent_session_id.map(|value| value.to_string()),
        slug: metadata.slug.clone(),
        tags: metadata.tags.clone(),
        note: metadata.note.clone(),
    }
}

fn session_title(summary: &SessionSummary) -> String {
    summary
        .display_name
        .clone()
        .or(summary.generated_title.clone())
        .or(summary.slug.clone())
        .unwrap_or_else(|| summary.id.to_string())
}

fn diff_history(record: &SessionRecord) -> Vec<DiffSnapshotDto> {
    record
        .events
        .iter()
        .filter_map(|event| match event {
            TranscriptEvent::GitDiffSnapshot { snapshot } => Some(snapshot_to_dto(snapshot)),
            _ => None,
        })
        .rev()
        .collect()
}

fn snapshot_to_dto(snapshot: &GitDiffSnapshot) -> DiffSnapshotDto {
    DiffSnapshotDto {
        command: snapshot.command.clone(),
        status: snapshot.status.clone(),
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

fn timeline_items(store: &SessionStore, record: &SessionRecord) -> Vec<TimelineItemDto> {
    let mut items = Vec::new();
    let mut pending_assistant = None;
    for (index, event) in record.events.iter().enumerate() {
        match event {
            TranscriptEvent::AssistantMessage { text, actor } => {
                flush_pending_assistant(&mut items, &mut pending_assistant);
                pending_assistant = Some(TimelineItemDto::AssistantMessage {
                    id: format!("event-{index}"),
                    text: text.clone(),
                    actor: actor.clone(),
                });
            }
            TranscriptEvent::SystemMessage { text, .. } if parse_tool_message(text).is_some() => {
                if let Some(item) = timeline_item(store, record.metadata.id, index, event) {
                    items.push(item);
                }
            }
            TranscriptEvent::ToolInvocation { .. } => {
                if let Some(item) = timeline_item(store, record.metadata.id, index, event) {
                    items.push(item);
                }
            }
            _ => {
                flush_pending_assistant(&mut items, &mut pending_assistant);
                if let Some(item) = timeline_item(store, record.metadata.id, index, event) {
                    items.push(item);
                }
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

fn timeline_item(
    store: &SessionStore,
    session_id: Uuid,
    index: usize,
    event: &TranscriptEvent,
) -> Option<TimelineItemDto> {
    let id = format!("event-{index}");
    match event {
        TranscriptEvent::UserMessage {
            text,
            attachments,
            actor,
        } => Some(TimelineItemDto::UserMessage {
            id,
            text: text.clone(),
            attachments: attachment_dtos(store, session_id, attachments),
            actor: actor.clone(),
        }),
        TranscriptEvent::AssistantMessage { text, actor } => {
            Some(TimelineItemDto::AssistantMessage {
                id,
                text: text.clone(),
                actor: actor.clone(),
            })
        }
        TranscriptEvent::SystemMessage { text, actor } => {
            if let Some(tool) = parse_tool_message(text) {
                Some(TimelineItemDto::ToolCall {
                    id,
                    tool_id: tool.tool_id,
                    status: tool.status,
                    input_text: tool.input_text,
                    input_json: tool.input_json,
                    output_text: tool.output_text.clone(),
                    permission_dialog: permission_dialog(&tool.output_text),
                    actor: actor.clone(),
                    subject: None,
                })
            } else {
                Some(TimelineItemDto::SystemMessage {
                    id,
                    text: text.clone(),
                    actor: actor.clone(),
                })
            }
        }
        TranscriptEvent::ToolInvocation {
            call_id,
            tool_id,
            input,
            output,
            success,
            metadata: _,
            actor,
            subject,
        } => Some(TimelineItemDto::ToolCall {
            id: format!("{id}-{call_id}"),
            tool_id: tool_id.clone(),
            status: if *success {
                "success".to_string()
            } else {
                "error".to_string()
            },
            input_text: input.clone(),
            input_json: serde_json::from_str(input).ok(),
            output_text: output.clone(),
            permission_dialog: permission_dialog(output),
            actor: actor.clone(),
            subject: subject.clone(),
        }),
        TranscriptEvent::CommandInvoked { name, args, actor } => {
            Some(TimelineItemDto::CommandInvoked {
                id,
                name: name.clone(),
                args: args.clone(),
                actor: actor.clone(),
            })
        }
        TranscriptEvent::SessionRenamed { name } => Some(TimelineItemDto::SessionRenamed {
            id,
            name: name.clone(),
        }),
        TranscriptEvent::GitDiffSnapshot { snapshot } => Some(TimelineItemDto::DiffSnapshot {
            id,
            snapshot: snapshot_to_dto(snapshot),
        }),
        TranscriptEvent::StateSnapshot {
            current_model,
            current_provider,
            effort_level,
            plan_mode,
            sandbox_mode,
            remote_name,
            remote_environment,
            statusline_enabled,
            working_dirs,
            ..
        } => Some(TimelineItemDto::StateSnapshot {
            id,
            current_model: current_model.clone(),
            current_provider: current_provider.clone(),
            effort_level: effort_level.clone(),
            plan_mode: *plan_mode,
            sandbox_mode: sandbox_mode.clone(),
            remote_name: remote_name.clone(),
            remote_environment: remote_environment.clone(),
            statusline_enabled: *statusline_enabled,
            working_dirs: working_dirs.clone(),
        }),
        TranscriptEvent::TurnBoundary { .. } | TranscriptEvent::TranscriptRewritten { .. } => None,
    }
}

fn attachment_dtos(
    store: &SessionStore,
    session_id: Uuid,
    attachments: &[puffer_session_store::StoredAttachment],
) -> Vec<ChatAttachmentDto> {
    attachments
        .iter()
        .map(|attachment| ChatAttachmentDto::from_stored(store, session_id, attachment))
        .collect()
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
        .map(|(value, rest)| (value.to_string(), rest.to_string()))
        .unwrap_or_else(|| (input.to_string(), String::new()));

    Some(ParsedToolMessage {
        tool_id: tool_id.to_string(),
        status: status.to_string(),
        input_json: serde_json::from_str(&input_text).ok(),
        input_text,
        output_text,
    })
}

fn permission_dialog(output: &str) -> Option<PermissionDialogDto> {
    for line in output.lines() {
        let trimmed = line.trim();
        if let Some(message) = trimmed.strip_prefix("Permission required:") {
            return Some(PermissionDialogDto {
                kind: "required".to_string(),
                message: message.trim().to_string(),
            });
        }
        if let Some(message) = trimmed.strip_prefix("Permission denied:") {
            return Some(PermissionDialogDto {
                kind: "denied".to_string(),
                message: message.trim().to_string(),
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_session_store::{SessionMetadata, TranscriptEvent};

    #[test]
    fn parses_tool_messages_with_permission_output() {
        let parsed = parse_tool_message(
            "Tool Bash [error]\ninput: {\"command\":\"git push\"}\nPermission required: shell command matches project shell exclusion `git push`",
        )
        .unwrap();
        let dialog = permission_dialog(&parsed.output_text).unwrap();
        assert_eq!(parsed.tool_id, "Bash");
        assert_eq!(dialog.kind, "required");
    }

    #[test]
    fn keeps_diff_history_in_reverse_chronological_order() {
        let record = SessionRecord {
            metadata: SessionMetadata {
                id: Uuid::nil(),
                display_name: None,
                cwd: PathBuf::from("/tmp"),
                created_at_ms: 0,
                updated_at_ms: 0,
                parent_session_id: None,
                slug: None,
                tags: Vec::new(),
                note: None,
            },
            events: vec![
                TranscriptEvent::GitDiffSnapshot {
                    snapshot: GitDiffSnapshot {
                        command: "/review".to_string(),
                        status: "M file".to_string(),
                        unstaged_diffstat: String::new(),
                        staged_diffstat: String::new(),
                        patch: "old".to_string(),
                        patch_excerpt: "old".to_string(),
                    },
                },
                TranscriptEvent::GitDiffSnapshot {
                    snapshot: GitDiffSnapshot {
                        command: "/commit".to_string(),
                        status: "M file".to_string(),
                        unstaged_diffstat: String::new(),
                        staged_diffstat: String::new(),
                        patch: "new".to_string(),
                        patch_excerpt: "new".to_string(),
                    },
                },
            ],
        };
        let history = diff_history(&record);
        assert_eq!(history[0].command, "/commit");
        assert_eq!(history[1].command, "/review");
    }
}

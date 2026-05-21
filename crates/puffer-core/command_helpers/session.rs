use super::common::{open_text_file_in_editor, render_utf8_qr};
use super::emit_system;
use crate::runtime::ReflectionTraceEvent;
use crate::{AppState, ToolInvocation};
use anyhow::Result;
use puffer_config::ConfigPaths;
use puffer_session_store::{SessionStore, TRACE_RUNTIME};
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

const TAG_HELP_ARGS: &[&str] = &[
    "help", "-h", "--help", "list", "show", "display", "current", "view", "get", "check",
    "describe", "print", "version", "about", "status", "?",
];

const TAG_HELP_TEXT: &str = "Usage: /tag <tag-name>\n\n\
Toggle a searchable tag on the current session.\n\
Run the same command again to remove the tag.\n\
Tags are displayed after the branch name in /resume and can be searched with /.\n\n\
Examples:\n\
  /tag bugfix        # Add tag\n\
  /tag bugfix        # Remove tag (toggle)\n\
  /tag feature-auth\n\
  /tag wip";

/// Describes the remote-session content shown by the interactive `/session` overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionOverlayView {
    /// The effective remote-session URL when remote mode is configured.
    pub remote_url: Option<String>,
    /// The current remote-session status label when one is available.
    pub remote_status: Option<String>,
    /// The rendered terminal QR block for the remote URL when `qrencode` is available.
    pub qr: Option<String>,
    /// Supplemental guidance or warnings shown below the primary session details.
    pub notice: Option<String>,
}

/// Persists one turn's reflection trace events into the per-session sidecar
/// JSONL. Used by every `TurnExecution` consumer (both the non-streaming
/// command/skill/compact paths in `puffer-core` and the streaming TUI
/// receiver in `puffer-tui`) so reflection traces do not silently vanish on
/// paths the runtime already observed them on. Failures are surfaced via
/// `eprintln!` rather than swallowed so a mis-provisioned sidecar (disk
/// full, permission drop) shows up somewhere the operator can see.
pub fn append_trace_events(
    session_store: &SessionStore,
    session_id: uuid::Uuid,
    traces: &[ReflectionTraceEvent],
) {
    for trace in traces {
        if let Err(error) = session_store.append_trace_event(session_id, TRACE_RUNTIME, trace) {
            eprintln!("reflection trace persist failed: {error}");
        }
    }
}

/// Records tool invocations into task history and the visible transcript.
pub(crate) fn append_tool_invocations(
    state: &mut AppState,
    session_store: &SessionStore,
    invocations: &[ToolInvocation],
) -> Result<()> {
    for invocation in invocations {
        state.record_task(
            invocation.tool_id.clone(),
            invocation.input.clone(),
            invocation.success,
        );
        state.push_tool_invocation(
            &invocation.call_id,
            &invocation.tool_id,
            &invocation.input,
            &invocation.output,
            invocation.success,
        );
        session_store.append_event(
            state.session.id,
            puffer_session_store::TranscriptEvent::ToolInvocation {
                call_id: invocation.call_id.clone(),
                tool_id: invocation.tool_id.clone(),
                input: invocation.input.clone(),
                output: invocation.output.clone(),
                success: invocation.success,
                actor: Some(state.assistant_actor()),
                subject: state.tool_subject_actor(&invocation.tool_id, &invocation.output),
            },
        )?;
    }
    Ok(())
}

/// Handles `/tag` by toggling one searchable session tag.
pub(crate) fn handle_tag_command(
    state: &mut AppState,
    session_store: &SessionStore,
    args: &str,
) -> Result<()> {
    let trimmed = args.trim();
    if trimmed.is_empty() || TAG_HELP_ARGS.contains(&trimmed) {
        return emit_system(state, session_store, TAG_HELP_TEXT.to_string());
    }

    if let Some(rest) = trimmed.strip_prefix("--confirm-remove ") {
        let tag = rest.trim();
        if tag.is_empty() {
            return emit_system(state, session_store, TAG_HELP_TEXT.to_string());
        }
        return remove_tag_message(state, session_store, tag, "Removed tag");
    }

    if let Some(rest) = trimmed.strip_prefix("--keep ") {
        let tag = rest.trim();
        if tag.is_empty() {
            return emit_system(state, session_store, TAG_HELP_TEXT.to_string());
        }
        return emit_system(state, session_store, format!("Kept tag #{tag}"));
    }

    let tag = trimmed;
    if state.session.tags.iter().any(|existing| existing == tag) {
        return remove_tag_message(state, session_store, tag, "Removed tag");
    }

    add_session_tag(state, session_store, tag)?;
    emit_system(state, session_store, format!("Tagged session with #{tag}"))
}

fn add_session_tag(state: &mut AppState, session_store: &SessionStore, tag: &str) -> Result<()> {
    session_store.add_tag(state.session.id, tag)?;
    if !state.session.tags.iter().any(|existing| existing == tag) {
        state.session.tags.push(tag.to_string());
        state.session.tags.sort();
    }
    Ok(())
}

fn remove_session_tag(state: &mut AppState, session_store: &SessionStore, tag: &str) -> Result<()> {
    session_store.remove_tag(state.session.id, tag)?;
    state.session.tags.retain(|existing| existing != tag);
    Ok(())
}

fn remove_tag_message(
    state: &mut AppState,
    session_store: &SessionStore,
    tag: &str,
    prefix: &str,
) -> Result<()> {
    remove_session_tag(state, session_store, tag)?;
    emit_system(state, session_store, format!("{prefix} #{tag}"))
}

/// Handles `/memory` file helpers plus session note, slug, and tag operations.
pub(crate) fn handle_memory_command(
    state: &mut AppState,
    session_store: &SessionStore,
    args: &str,
) -> Result<()> {
    let trimmed = args.trim();
    if trimmed.is_empty() || matches!(trimmed, "show" | "list") {
        return emit_system(state, session_store, render_memory_summary(state));
    }

    if let Some(rest) = trimmed.strip_prefix("path") {
        let scope = match parse_memory_scope_for_command(rest.trim()) {
            Ok(scope) => scope,
            Err(message) => return emit_system(state, session_store, message),
        };
        return emit_memory_path(state, session_store, scope);
    }

    if let Some(rest) = trimmed.strip_prefix("cat") {
        let scope = match parse_memory_scope_for_command(rest.trim()) {
            Ok(scope) => scope,
            Err(message) => return emit_system(state, session_store, message),
        };
        return emit_memory_contents(state, session_store, scope);
    }

    if let Some(rest) = trimmed.strip_prefix("open") {
        let scope = match parse_memory_scope_for_command(rest.trim()) {
            Ok(scope) => scope,
            Err(message) => return emit_system(state, session_store, message),
        };
        return open_memory_file(state, session_store, scope);
    }

    if let Some(rest) = trimmed.strip_prefix("edit") {
        let scope = match parse_memory_scope_for_command(rest.trim()) {
            Ok(scope) => scope,
            Err(message) => return emit_system(state, session_store, message),
        };
        return open_memory_file(state, session_store, scope);
    }

    if trimmed == "clear" {
        let tags = state.session.tags.clone();
        session_store.set_note(state.session.id, None)?;
        session_store.set_slug(state.session.id, None)?;
        for tag in &tags {
            session_store.remove_tag(state.session.id, tag)?;
        }
        state.session.note = None;
        state.session.slug = None;
        state.session.tags.clear();
        return emit_system(
            state,
            session_store,
            "Cleared session note, slug, and tags.".to_string(),
        );
    }

    if let Some(rest) = trimmed.strip_prefix("note ") {
        if matches!(rest, "clear" | "none" | "off") {
            session_store.set_note(state.session.id, None)?;
            state.session.note = None;
            return emit_system(state, session_store, "Cleared session note.".to_string());
        }
        session_store.set_note(state.session.id, Some(rest.to_string()))?;
        state.session.note = Some(rest.to_string());
        return emit_system(
            state,
            session_store,
            format!("Session note set to `{rest}`."),
        );
    }

    if let Some(rest) = trimmed.strip_prefix("slug ") {
        if matches!(rest, "clear" | "none" | "off") {
            session_store.set_slug(state.session.id, None)?;
            state.session.slug = None;
            return emit_system(state, session_store, "Cleared session slug.".to_string());
        }
        session_store.set_slug(state.session.id, Some(rest.to_string()))?;
        state.session.slug = Some(rest.to_string());
        return emit_system(
            state,
            session_store,
            format!("Session slug set to `{rest}`."),
        );
    }

    if let Some(rest) = trimmed.strip_prefix("tag add ") {
        let tag = rest.trim();
        if tag.is_empty() {
            return emit_system(
                state,
                session_store,
                "Usage: /memory tag add <tag>".to_string(),
            );
        }
        add_session_tag(state, session_store, tag)?;
        return emit_system(state, session_store, format!("Added session tag `{tag}`."));
    }

    if let Some(rest) = trimmed.strip_prefix("tag remove ") {
        let tag = rest.trim();
        if tag.is_empty() {
            return emit_system(
                state,
                session_store,
                "Usage: /memory tag remove <tag>".to_string(),
            );
        }
        remove_session_tag(state, session_store, tag)?;
        return emit_system(
            state,
            session_store,
            format!("Removed session tag `{tag}`."),
        );
    }

    emit_system(
        state,
        session_store,
        "Usage: /memory [show|list|path [project|workspace|user]|cat [project|workspace|user]|open [project|workspace|user]|edit [project|workspace|user]|clear|note <text>|note clear|slug <value>|slug clear|tag add <tag>|tag remove <tag>]".to_string(),
    )
}

/// Renders the no-argument `/memory` panel summary.
pub(crate) fn render_memory_panel(state: &AppState) -> String {
    render_memory_summary(state)
}

/// Handles `/session` remote summary plus session metadata subcommands.
pub(crate) fn handle_session_command(
    state: &mut AppState,
    session_store: &SessionStore,
    args: &str,
) -> Result<()> {
    let trimmed = args.trim();
    match trimmed {
        "" | "show" => emit_system(state, session_store, render_session_summary(state)),
        "url" => emit_system(state, session_store, render_remote_url_summary(state)),
        "qr" => emit_system(state, session_store, render_remote_qr_summary(state)),
        "list" => {
            let sessions = session_store.list_sessions()?;
            let mut text = String::from("Sessions:\n");
            for session in sessions.iter().take(20) {
                let _ = writeln!(
                    &mut text,
                    "{} {}",
                    session.id,
                    session.display_name.as_deref().unwrap_or("<unnamed>")
                );
            }
            emit_system(state, session_store, text)
        }
        _ if trimmed.starts_with("rename ") => {
            let name = trimmed.trim_start_matches("rename ").trim();
            if name.is_empty() {
                return emit_system(
                    state,
                    session_store,
                    "Usage: /session rename <name>".to_string(),
                );
            }
            session_store.rename_session(state.session.id, name.to_string())?;
            state.session.display_name = Some(name.to_string());
            emit_system(state, session_store, format!("Session renamed to `{name}`."))
        }
        _ if trimmed.starts_with("note ") => {
            let note = trimmed.trim_start_matches("note ").trim();
            if matches!(note, "clear" | "none" | "off") {
                session_store.set_note(state.session.id, None)?;
                state.session.note = None;
                return emit_system(state, session_store, "Cleared session note.".to_string());
            }
            session_store.set_note(state.session.id, Some(note.to_string()))?;
            state.session.note = Some(note.to_string());
            emit_system(state, session_store, "Updated session note.".to_string())
        }
        _ if trimmed.starts_with("slug ") => {
            let slug = trimmed.trim_start_matches("slug ").trim();
            if matches!(slug, "clear" | "none" | "off") {
                session_store.set_slug(state.session.id, None)?;
                state.session.slug = None;
                return emit_system(state, session_store, "Cleared session slug.".to_string());
            }
            session_store.set_slug(state.session.id, Some(slug.to_string()))?;
            state.session.slug = Some(slug.to_string());
            emit_system(state, session_store, format!("Session slug set to `{slug}`."))
        }
        _ if trimmed.starts_with("tag add ") => {
            let tag = trimmed.trim_start_matches("tag add ").trim();
            if tag.is_empty() {
                return emit_system(
                    state,
                    session_store,
                    "Usage: /session tag add <tag>".to_string(),
                );
            }
            add_session_tag(state, session_store, tag)?;
            emit_system(state, session_store, format!("Added session tag `{tag}`."))
        }
        _ if trimmed.starts_with("tag remove ") => {
            let tag = trimmed.trim_start_matches("tag remove ").trim();
            if tag.is_empty() {
                return emit_system(
                    state,
                    session_store,
                    "Usage: /session tag remove <tag>".to_string(),
                );
            }
            remove_session_tag(state, session_store, tag)?;
            emit_system(state, session_store, format!("Removed session tag `{tag}`."))
        }
        _ => emit_system(
            state,
            session_store,
            "Usage: /session [show|url|qr|list|rename <name>|note <text|clear>|slug <value|clear>|tag add <tag>|tag remove <tag>]".to_string(),
        ),
    }
}

/// Renders the no-argument `/session` panel summary.
pub(crate) fn render_session_panel(state: &AppState) -> String {
    render_session_summary(state)
}

/// Builds the dedicated remote-session view used by the interactive `/session` overlay.
pub(crate) fn render_session_overlay(state: &AppState) -> SessionOverlayView {
    let Some(remote_url) = effective_remote_session_url(state) else {
        return SessionOverlayView {
            remote_url: None,
            remote_status: None,
            qr: None,
            notice: Some("Not in remote mode. Set /remote-control first.".to_string()),
        };
    };

    let qr = render_utf8_qr(&remote_url);
    SessionOverlayView {
        remote_status: Some(
            state
                .remote_session_status
                .as_deref()
                .unwrap_or("connected")
                .to_string(),
        ),
        remote_url: Some(remote_url),
        notice: qr.as_ref().map(|_| None).unwrap_or_else(|| {
            Some(
                "QR rendering unavailable (install `qrencode` to enable terminal QR output)."
                    .to_string(),
            )
        }),
        qr,
    }
}

/// Handles `/remote-control` by creating or reporting persisted remote session metadata.
pub(crate) fn handle_remote_control_command(
    state: &mut AppState,
    session_store: &SessionStore,
    args: &str,
) -> Result<()> {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return emit_system(state, session_store, render_remote_control_summary(state));
    }
    if trimmed == "clear" {
        clear_remote_session_state(state);
        return emit_system(
            state,
            session_store,
            "Cleared remote-control session metadata.".to_string(),
        );
    }

    let remote_name = trimmed
        .strip_prefix("connect ")
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(trimmed);
    state.remote_name = Some(remote_name.to_string());
    ensure_remote_session_metadata(state);
    state.remote_session_status = Some("connected".to_string());
    emit_system(
        state,
        session_store,
        format!(
            "Remote control session connected.\nname={}\nsession_id={}\nurl={}",
            state.remote_name.as_deref().unwrap_or("<unset>"),
            state.remote_session_id.as_deref().unwrap_or("<unset>"),
            state.remote_session_url.as_deref().unwrap_or("<unset>")
        ),
    )
}

/// Handles `/remote-env` by updating the persisted default remote environment.
pub(crate) fn handle_remote_env_command(
    state: &mut AppState,
    session_store: &SessionStore,
    args: &str,
) -> Result<()> {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return emit_system(
            state,
            session_store,
            format!(
                "Remote environment: {}\nRemote session URL: {}",
                state.remote_environment.as_deref().unwrap_or("<unset>"),
                state.remote_session_url.as_deref().unwrap_or("<unset>")
            ),
        );
    }
    if trimmed == "clear" {
        state.remote_environment = None;
        if state.remote_name.is_none() {
            state.remote_session_url = None;
            state.remote_session_status = None;
        } else {
            ensure_remote_session_metadata(state);
        }
        return emit_system(
            state,
            session_store,
            "Cleared remote environment.".to_string(),
        );
    }

    state.remote_environment = Some(trimmed.to_string());
    if state.remote_name.is_some() {
        ensure_remote_session_metadata(state);
        state.remote_session_status = Some("configured".to_string());
    }
    emit_system(
        state,
        session_store,
        format!(
            "Remote environment set to {}.\nRemote session URL: {}",
            state.remote_environment.as_deref().unwrap_or("<unset>"),
            state.remote_session_url.as_deref().unwrap_or("<unset>")
        ),
    )
}

fn render_memory_summary(state: &AppState) -> String {
    let files = memory_file_entries(state);
    let project_status = crate::memory::project_memory_status(state)
        .unwrap_or_else(|| "project=<unresolved>\nmemory_file=<unavailable>".to_string());
    format!(
        "Memory files:\n{}\n\nProject memory:\n{}\n\nSession memory summary:\nslug={}\nnote={}\ntags={}",
        files
            .iter()
            .map(format_memory_file_entry)
            .collect::<Vec<_>>()
            .join("\n"),
        project_status,
        state.session.slug.as_deref().unwrap_or("<none>"),
        state.session.note.as_deref().unwrap_or("<none>"),
        if state.session.tags.is_empty() {
            "<none>".to_string()
        } else {
            state.session.tags.join(", ")
        },
    )
}

fn render_session_summary(state: &AppState) -> String {
    format!(
        "session_id={}\ncwd={}\ndisplay_name={}\nslug={}\nparent={:?}\ntags={}\nnote={}\nremote_name={}\nremote_environment={}\nremote_session_id={}\nremote_session_status={}\nremote_session_url={}\n\n{}",
        state.session.id,
        state.session.cwd.display(),
        state.session.display_name.as_deref().unwrap_or("<unnamed>"),
        state.session.slug.as_deref().unwrap_or("<none>"),
        state.session.parent_session_id,
        if state.session.tags.is_empty() {
            "<none>".to_string()
        } else {
            state.session.tags.join(", ")
        },
        state.session.note.as_deref().unwrap_or("<none>"),
        state.remote_name.as_deref().unwrap_or("<none>"),
        state.remote_environment.as_deref().unwrap_or("<none>"),
        state.remote_session_id.as_deref().unwrap_or("<unset>"),
        state.remote_session_status.as_deref().unwrap_or("<unset>"),
        state.remote_session_url.as_deref().unwrap_or("<unset>"),
        render_remote_session_block(state)
    )
}

fn emit_memory_path(
    state: &mut AppState,
    session_store: &SessionStore,
    scope: Option<MemoryScope>,
) -> Result<()> {
    if let Some(scope) = scope {
        return emit_system(
            state,
            session_store,
            match memory_file_path(state, scope) {
                Some(path) => format!("{} memory path: {}", scope.label(), path.display()),
                None => "No configured project matches the current working directory, so project memory is unavailable.".to_string(),
            },
        );
    }
    let entries = memory_file_entries(state)
        .iter()
        .map(format_memory_file_entry)
        .collect::<Vec<_>>()
        .join("\n");
    emit_system(
        state,
        session_store,
        format!("Memory file paths:\n{entries}"),
    )
}

fn emit_memory_contents(
    state: &mut AppState,
    session_store: &SessionStore,
    scope: Option<MemoryScope>,
) -> Result<()> {
    let scope = scope.unwrap_or(MemoryScope::Project);
    let Some(path) = memory_file_path(state, scope) else {
        return emit_system(
            state,
            session_store,
            "No configured project matches the current working directory, so project memory is unavailable.".to_string(),
        );
    };
    if !path.exists() {
        return emit_system(
            state,
            session_store,
            format!(
                "{} memory file does not exist yet: {}",
                scope.label(),
                path.display()
            ),
        );
    }
    let contents = fs::read_to_string(&path)?;
    let rendered = if contents.trim().is_empty() {
        "<empty>".to_string()
    } else {
        contents
    };
    emit_system(
        state,
        session_store,
        format!(
            "{} memory file ({})\n{}",
            scope.label(),
            path.display(),
            rendered
        ),
    )
}

fn open_memory_file(
    state: &mut AppState,
    session_store: &SessionStore,
    scope: Option<MemoryScope>,
) -> Result<()> {
    let scope = scope.unwrap_or(MemoryScope::Project);
    let Some(path) = memory_file_path(state, scope) else {
        return emit_system(
            state,
            session_store,
            "No configured project matches the current working directory, so project memory is unavailable.".to_string(),
        );
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    match open_text_file_in_editor(&path) {
        Ok(status) => emit_system(
            state,
            session_store,
            format!("{status}\nTarget: {} memory", scope.label()),
        ),
        Err(error) => emit_system(
            state,
            session_store,
            format!(
                "Could not open {} memory file in an editor: {error}\nPath: {}\nFallback: edit this file manually or run `/memory cat {}`.",
                scope.label(),
                path.display(),
                scope.label()
            ),
        ),
    }
}

fn render_remote_url_summary(state: &AppState) -> String {
    match effective_remote_session_url(state) {
        Some(url) => format!("Remote session URL: {url}"),
        None => "Remote session URL is unavailable. Set /remote-control first.".to_string(),
    }
}

fn render_remote_qr_summary(state: &AppState) -> String {
    let Some(url) = effective_remote_session_url(state) else {
        return "Remote session URL is unavailable. Set /remote-control first.".to_string();
    };
    if let Some(qr) = render_utf8_qr(&url) {
        return format!("Remote session URL: {url}\n\n{qr}");
    }
    format!(
        "Remote session URL: {url}\n\nQR rendering unavailable (install `qrencode` to enable terminal QR output)."
    )
}

fn render_remote_session_block(state: &AppState) -> String {
    let Some(url) = effective_remote_session_url(state) else {
        return "Remote session summary: no active remote URL.\nSet /remote-control (and optionally /remote-env), then run /session show again."
            .to_string();
    };
    let status = state
        .remote_session_status
        .as_deref()
        .unwrap_or("connected");
    if let Some(qr) = render_utf8_qr(&url) {
        return format!(
            "Remote session summary:\nStatus: {status}\nOpen in browser: {url}\n\n{qr}"
        );
    }
    format!(
        "Remote session summary:\nStatus: {status}\nOpen in browser: {url}\nQR rendering unavailable (install `qrencode` to enable terminal QR output)."
    )
}

fn effective_remote_session_url(state: &AppState) -> Option<String> {
    state
        .remote_session_url
        .clone()
        .or_else(|| remote_session_url(state))
}

fn remote_session_url(state: &AppState) -> Option<String> {
    if let Ok(explicit) = std::env::var("PUFFER_REMOTE_SESSION_URL") {
        let trimmed = explicit.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    let name = state.remote_name.as_deref()?;
    let session_id = state
        .remote_session_id
        .clone()
        .unwrap_or_else(|| state.session.id.to_string());
    let mut url = format!(
        "puffer://remote/{}?name={}",
        url_encode(&session_id),
        url_encode(name)
    );
    if let Some(environment) = state.remote_environment.as_deref() {
        if !environment.trim().is_empty() {
            let separator = if url.contains('?') { '&' } else { '?' };
            let _ = write!(
                &mut url,
                "{separator}env={}",
                url_encode(environment.trim())
            );
        }
    }
    Some(url)
}

fn render_remote_control_summary(state: &AppState) -> String {
    format!(
        "Remote control session name: {}\nRemote session id: {}\nRemote session status: {}\nRemote session URL: {}",
        state.remote_name.as_deref().unwrap_or("<unset>"),
        state.remote_session_id.as_deref().unwrap_or("<unset>"),
        state.remote_session_status.as_deref().unwrap_or("<unset>"),
        state.remote_session_url.as_deref().unwrap_or("<unset>")
    )
}

fn clear_remote_session_state(state: &mut AppState) {
    state.remote_name = None;
    state.remote_session_id = None;
    state.remote_session_status = None;
    state.remote_session_url = None;
}

fn ensure_remote_session_metadata(state: &mut AppState) {
    if state.remote_name.is_none() {
        clear_remote_session_state(state);
        return;
    }
    state.remote_session_id = Some(state.session.id.to_string());
    state.remote_session_url = remote_session_url(state);
}

fn url_encode(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            b' ' => encoded.push_str("%20"),
            _ => {
                let _ = write!(&mut encoded, "%{:02X}", byte);
            }
        }
    }
    encoded
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MemoryScope {
    Project,
    Workspace,
    User,
}

impl MemoryScope {
    fn label(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Workspace => "workspace",
            Self::User => "user",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_locks::env_lock;
    use puffer_config::{ensure_workspace_dirs, PufferConfig};
    use puffer_session_store::SessionMetadata;
    use std::path::PathBuf;
    use tempfile::tempdir;
    use uuid::Uuid;

    fn sample_state() -> AppState {
        AppState::new(
            PufferConfig::default(),
            PathBuf::from("/workspace/puffer"),
            SessionMetadata {
                id: Uuid::nil(),
                display_name: Some("demo".to_string()),
                generated_title: None,
                cwd: PathBuf::from("/workspace/puffer"),
                created_at_ms: 0,
                updated_at_ms: 0,
                parent_session_id: None,
                slug: None,
                tags: Vec::new(),
                note: None,
            },
        )
    }

    #[test]
    fn append_trace_events_writes_each_event_to_sidecar() {
        use puffer_config::{ensure_workspace_dirs, ConfigPaths};
        use tempfile::tempdir;

        let tmp = tempdir().unwrap();
        let paths = ConfigPaths::discover(tmp.path());
        ensure_workspace_dirs(&paths).unwrap();
        let store = SessionStore::from_paths(&paths).unwrap();
        let session = store.create_session(tmp.path().to_path_buf()).unwrap();

        let traces = vec![
            ReflectionTraceEvent::LlmJudgeSkipped {
                mode: "disabled".to_string(),
                reason: "llm judge disabled in reflection config".to_string(),
            },
            ReflectionTraceEvent::LlmJudgeSkipped {
                mode: "confirm_code_judge".to_string(),
                reason: "confirm_code_judge mode requires a code-judge trigger first".to_string(),
            },
        ];

        append_trace_events(&store, session.id, &traces);

        let sidecar = store
            .root()
            .join(format!("{}.session.runtime_trace.jsonl", session.id));
        let body = std::fs::read_to_string(&sidecar).expect("sidecar file exists");
        // Two events, one JSON object per line.
        assert_eq!(body.lines().count(), 2, "full body:\n{body}");
        assert!(body.contains("llm judge disabled"));
        assert!(body.contains("confirm_code_judge mode requires"));
    }

    #[test]
    fn render_session_overlay_reports_missing_remote_session() {
        let state = sample_state();
        let view = render_session_overlay(&state);

        assert_eq!(view.remote_url, None);
        assert_eq!(view.remote_status, None);
        assert_eq!(
            view.notice.as_deref(),
            Some("Not in remote mode. Set /remote-control first.")
        );
    }

    #[test]
    fn render_session_overlay_uses_computed_remote_url() {
        let mut state = sample_state();
        state.remote_name = Some("buildbox".to_string());
        state.remote_environment = Some("linux".to_string());
        state.remote_session_id = Some(state.session.id.to_string());
        state.remote_session_status = Some("configured".to_string());

        let view = render_session_overlay(&state);

        assert_eq!(view.remote_status.as_deref(), Some("configured"));
        assert_eq!(
            view.remote_url.as_deref(),
            Some("puffer://remote/00000000-0000-0000-0000-000000000000?name=buildbox&env=linux")
        );
    }

    #[test]
    fn project_memory_path_uses_registered_project_memory_file() {
        let _guard = env_lock().lock().unwrap();
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let workspace = temp.path().join("workspace");
        let project_root = workspace.join("apps/demo");
        let cwd = project_root.join("src");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();
        let _home = puffer_config::set_puffer_home_override(&home);

        let paths = ConfigPaths::discover(&workspace);
        ensure_workspace_dirs(&paths).unwrap();
        std::fs::write(
            paths.projects_file(),
            format!(
                "[[projects]]\nname = \"demo\"\npath = \"{}\"\n",
                project_root.display()
            ),
        )
        .unwrap();

        let mut state = sample_state();
        state.cwd = cwd.clone();
        state.session.cwd = cwd;
        state.refresh_project_memory();
        let path = memory_file_path(&state, MemoryScope::Project).expect("project memory path");
        assert!(path.ends_with("MEMORY.md"));
        assert!(path.starts_with(paths.projects_memory_dir()));
    }
}

fn parse_memory_scope_for_command(raw: &str) -> std::result::Result<Option<MemoryScope>, String> {
    let normalized = raw.trim();
    if normalized.is_empty() {
        return Ok(None);
    }
    match normalized {
        "project" | "proj" => Ok(Some(MemoryScope::Project)),
        "workspace" | "work" | "local" => Ok(Some(MemoryScope::Workspace)),
        "user" | "global" => Ok(Some(MemoryScope::User)),
        _ => Err(format!(
            "Unknown memory scope `{normalized}`. Use project, workspace, or user."
        )),
    }
}

#[derive(Debug, Clone)]
struct MemoryFileEntry {
    scope: MemoryScope,
    path: Option<PathBuf>,
    exists: bool,
    bytes: u64,
}

fn memory_file_entries(state: &AppState) -> Vec<MemoryFileEntry> {
    [
        MemoryScope::Project,
        MemoryScope::Workspace,
        MemoryScope::User,
    ]
    .iter()
    .copied()
    .map(|scope| {
        let path = memory_file_path(state, scope);
        let (exists, bytes) = match path.as_ref().and_then(|path| fs::metadata(path).ok()) {
            Some(metadata) => (true, metadata.len()),
            None => (false, 0),
        };
        MemoryFileEntry {
            scope,
            path,
            exists,
            bytes,
        }
    })
    .collect()
}

fn format_memory_file_entry(entry: &MemoryFileEntry) -> String {
    let path = entry
        .path
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "<unavailable>".to_string());
    format!(
        "- {}: {} ({}, {} bytes)",
        entry.scope.label(),
        path,
        if entry.exists { "present" } else { "missing" },
        entry.bytes
    )
}

fn memory_file_path(state: &AppState, scope: MemoryScope) -> Option<PathBuf> {
    let paths = ConfigPaths::discover(&state.cwd);
    match scope {
        MemoryScope::Project => crate::memory::project_memory_path(state),
        MemoryScope::Workspace => Some(paths.workspace_config_dir.join("memory.md")),
        MemoryScope::User => Some(paths.user_config_dir.join("memory.md")),
    }
}

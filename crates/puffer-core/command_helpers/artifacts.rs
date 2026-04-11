use super::{emit_system, persist_user_settings};
use crate::{AppState, MessageRole, RenderedMessage};
use anyhow::{Context, Result};
use arboard::Clipboard;
use puffer_session_store::SessionStore;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use time::OffsetDateTime;

const COPY_RESPONSE_FILENAME: &str = "response.md";
const COPY_FULL_SELECTOR: &str = "--full";
const COPY_CODE_SELECTOR: &str = "--code";
const COPY_ALWAYS_FULL_SELECTOR: &str = "--always-full";

/// Describes one interactive `/copy` picker action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyActionEntry {
    pub label: String,
    pub description: String,
    pub command: String,
}

/// Describes the selected assistant message for `/copy`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CopySelection {
    pub(crate) text: String,
    pub(crate) age: usize,
    pub(crate) total: usize,
}

/// Handles `/copy`, including Claude-style `/copy N` history selection.
pub(crate) fn handle_copy_command(
    state: &mut AppState,
    session_store: &SessionStore,
    args: &str,
) -> Result<()> {
    if let Some(request) = parse_internal_copy_request(args)? {
        return execute_internal_copy_request(state, session_store, request);
    }
    let selection = match select_copy_target(&state.transcript, args) {
        Ok(selection) => selection,
        Err(error) => return emit_system(state, session_store, error.to_string()),
    };
    let summary = copy_text_selection(
        &selection.text,
        COPY_RESPONSE_FILENAME,
        "assistant response",
        selection.age,
        selection.total,
    );
    emit_system(state, session_store, summary)
}

/// Builds Claude-style interactive `/copy` picker actions for the current assistant response.
pub(crate) fn render_copy_actions(
    state: &AppState,
    args: &str,
) -> Result<Option<Vec<CopyActionEntry>>> {
    let selection = select_copy_target(&state.transcript, args)?;
    if state.config.copy_full_response {
        return Ok(None);
    }
    let code_blocks = extract_code_blocks(&selection.text);
    if code_blocks.is_empty() {
        return Ok(None);
    }

    let mut actions = vec![CopyActionEntry {
        label: "Full response".to_string(),
        description: format!(
            "{} chars, {} lines",
            selection.text.len(),
            line_count(&selection.text)
        ),
        command: format!("/copy {COPY_FULL_SELECTOR} {}", selection.age),
    }];
    actions.extend(code_blocks.iter().enumerate().map(|(index, block)| {
        let lines = line_count(&block.code);
        let mut description = Vec::new();
        if let Some(lang) = block.lang.as_deref() {
            description.push(lang.to_string());
        }
        if lines > 1 {
            description.push(format!("{lines} lines"));
        }
        CopyActionEntry {
            label: truncate_copy_label(&block.code, index),
            description: description.join(", "),
            command: format!("/copy {COPY_CODE_SELECTOR} {} {}", selection.age, index),
        }
    }));
    actions.push(CopyActionEntry {
        label: "Always copy full response".to_string(),
        description: "Skip this picker in the future (revert via /config)".to_string(),
        command: format!("/copy {COPY_ALWAYS_FULL_SELECTOR} {}", selection.age),
    });
    Ok(Some(actions))
}

/// Handles `/export` by rendering a plain-text conversation transcript.
pub(crate) fn handle_export_command(
    state: &mut AppState,
    session_store: &SessionStore,
    args: &str,
) -> Result<()> {
    let content = render_export_transcript(state);
    let trimmed = args.trim();
    if trimmed.eq_ignore_ascii_case("clipboard") {
        let fallback_path = write_temp_artifact(&content, &default_export_filename(state)).ok();
        let summary = clipboard_summary(
            &content,
            "conversation export",
            0,
            1,
            fallback_path.as_deref(),
        );
        return emit_system(state, session_store, summary);
    }

    let target = export_target_path(state, trimmed);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(&target, &content)
        .with_context(|| format!("failed to write {}", target.display()))?;

    let clipboard_message = match try_copy_to_clipboard(&content) {
        Ok(()) => " Also copied to clipboard.",
        Err(_) => "",
    };
    emit_system(
        state,
        session_store,
        format!(
            "Conversation exported to {}.{}",
            target.display(),
            clipboard_message
        ),
    )
}

/// Selects the assistant message to copy for `/copy [N]`.
pub(crate) fn select_copy_target(
    transcript: &[RenderedMessage],
    args: &str,
) -> Result<CopySelection> {
    let recent = recent_assistant_texts(transcript);
    if recent.is_empty() {
        anyhow::bail!("No assistant message is available to copy.");
    }

    let trimmed = args.trim();
    let age = if trimmed.is_empty() {
        0
    } else {
        let n = trimmed.parse::<usize>().with_context(|| {
            format!("Usage: /copy [N] where N is 1 (latest), 2, 3, ... Got: {trimmed}")
        })?;
        if n == 0 {
            anyhow::bail!("Usage: /copy [N] where N is 1 (latest), 2, 3, ... Got: {trimmed}");
        }
        n - 1
    };

    select_copy_target_by_age(&recent, age)
}

fn select_copy_target_by_age(recent: &[String], age: usize) -> Result<CopySelection> {
    if age >= recent.len() {
        anyhow::bail!(
            "Only {} assistant {} available to copy.",
            recent.len(),
            if recent.len() == 1 {
                "message"
            } else {
                "messages"
            }
        );
    }

    Ok(CopySelection {
        text: recent[age].clone(),
        age,
        total: recent.len(),
    })
}

/// Renders the current transcript as a plain-text conversation export.
pub(crate) fn render_export_transcript(state: &AppState) -> String {
    let mut text = String::new();
    let _ = writeln!(&mut text, "Puffer Code Conversation Export");
    let _ = writeln!(&mut text, "session_id={}", state.session.id);
    let _ = writeln!(
        &mut text,
        "display_name={}",
        state.session.display_name.as_deref().unwrap_or("<unnamed>")
    );
    let _ = writeln!(&mut text, "cwd={}", state.cwd.display());
    let _ = writeln!(
        &mut text,
        "provider={}",
        state.current_provider.as_deref().unwrap_or("<unset>")
    );
    let _ = writeln!(
        &mut text,
        "model={}",
        state.current_model.as_deref().unwrap_or("<unset>")
    );
    let _ = writeln!(&mut text, "exported_at={}", export_timestamp());

    for message in &state.transcript {
        let _ = writeln!(&mut text, "\n## {}", role_label(&message.role));
        let _ = writeln!(&mut text, "{}", message.text.trim_end());
    }

    text.trim_end().to_string()
}

fn recent_assistant_texts(transcript: &[RenderedMessage]) -> Vec<String> {
    transcript
        .iter()
        .rev()
        .filter(|message| message.role == MessageRole::Assistant)
        .map(|message| message.text.trim().to_string())
        .filter(|text| !text.is_empty())
        .take(20)
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CopyCodeBlock {
    code: String,
    lang: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum InternalCopyRequest {
    Full { age: usize },
    Code { age: usize, index: usize },
    AlwaysFull { age: usize },
}

fn parse_internal_copy_request(args: &str) -> Result<Option<InternalCopyRequest>> {
    let trimmed = args.trim();
    if !trimmed.starts_with("--") {
        return Ok(None);
    }

    let parts = trimmed.split_whitespace().collect::<Vec<_>>();
    let request = match parts.as_slice() {
        [COPY_FULL_SELECTOR] => InternalCopyRequest::Full { age: 0 },
        [COPY_FULL_SELECTOR, age] => InternalCopyRequest::Full {
            age: parse_internal_index(age, "copy age")?,
        },
        [COPY_CODE_SELECTOR, age, index] => InternalCopyRequest::Code {
            age: parse_internal_index(age, "copy age")?,
            index: parse_internal_index(index, "code block index")?,
        },
        [COPY_ALWAYS_FULL_SELECTOR] => InternalCopyRequest::AlwaysFull { age: 0 },
        [COPY_ALWAYS_FULL_SELECTOR, age] => InternalCopyRequest::AlwaysFull {
            age: parse_internal_index(age, "copy age")?,
        },
        _ => anyhow::bail!("Usage: /copy [N] where N is 1 (latest), 2, 3, ... Got: {trimmed}"),
    };
    Ok(Some(request))
}

fn parse_internal_index(value: &str, label: &str) -> Result<usize> {
    value
        .parse::<usize>()
        .with_context(|| format!("invalid {label}: {value}"))
}

fn execute_internal_copy_request(
    state: &mut AppState,
    session_store: &SessionStore,
    request: InternalCopyRequest,
) -> Result<()> {
    let recent = recent_assistant_texts(&state.transcript);
    let summary = match request {
        InternalCopyRequest::Full { age } => {
            let selection = select_copy_target_by_age(&recent, age)?;
            copy_text_selection(
                &selection.text,
                COPY_RESPONSE_FILENAME,
                "assistant response",
                selection.age,
                selection.total,
            )
        }
        InternalCopyRequest::Code { age, index } => {
            let selection = select_copy_target_by_age(&recent, age)?;
            let code_blocks = extract_code_blocks(&selection.text);
            let Some(block) = code_blocks.get(index) else {
                anyhow::bail!(
                    "Only {} code {} available to copy.",
                    code_blocks.len(),
                    if code_blocks.len() == 1 {
                        "block is"
                    } else {
                        "blocks are"
                    }
                );
            };
            copy_text_selection(
                &block.code,
                format!("copy{}", code_block_extension(block.lang.as_deref())).as_str(),
                format!("code block {} from assistant response", index + 1).as_str(),
                selection.age,
                selection.total,
            )
        }
        InternalCopyRequest::AlwaysFull { age } => {
            let selection = select_copy_target_by_age(&recent, age)?;
            state.config.copy_full_response = true;
            persist_user_settings(state)?;
            let summary = copy_text_selection(
                &selection.text,
                COPY_RESPONSE_FILENAME,
                "assistant response",
                selection.age,
                selection.total,
            );
            format!("{summary}\nPreference saved. Use /config to change copyFullResponse")
        }
    };
    emit_system(state, session_store, summary)
}

fn copy_text_selection(
    text: &str,
    filename: &str,
    label: &str,
    age: usize,
    total: usize,
) -> String {
    let fallback_path = write_temp_artifact(text, filename).ok();
    clipboard_summary(text, label, age, total, fallback_path.as_deref())
}

fn extract_code_blocks(markdown: &str) -> Vec<CopyCodeBlock> {
    let mut blocks = Vec::new();
    let mut current = None::<(Option<String>, Vec<String>)>;
    for line in markdown.lines() {
        if let Some(rest) = line.strip_prefix("```") {
            if let Some((lang, body)) = current.take() {
                blocks.push(CopyCodeBlock {
                    code: body.join("\n"),
                    lang,
                });
            } else {
                let lang = rest
                    .split_whitespace()
                    .next()
                    .filter(|value| !value.is_empty())
                    .map(str::to_string);
                current = Some((lang, Vec::new()));
            }
            continue;
        }
        if let Some((_, body)) = current.as_mut() {
            body.push(line.to_string());
        }
    }
    blocks
}

fn truncate_copy_label(text: &str, index: usize) -> String {
    let first_line = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .map(str::trim)
        .unwrap_or_default();
    if first_line.is_empty() {
        return format!("Code block {}", index + 1);
    }
    let label = if first_line.chars().count() <= 60 {
        first_line.to_string()
    } else {
        format!("{}...", first_line.chars().take(57).collect::<String>())
    };
    label
}

fn code_block_extension(lang: Option<&str>) -> String {
    let Some(lang) = lang else {
        return ".txt".to_string();
    };
    let sanitized = lang
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect::<String>();
    if sanitized.is_empty() || sanitized == "plaintext" {
        ".txt".to_string()
    } else {
        format!(".{sanitized}")
    }
}

fn clipboard_summary(
    text: &str,
    label: &str,
    age: usize,
    total: usize,
    fallback_path: Option<&Path>,
) -> String {
    let age_label = if total > 1 {
        format!(" {} of {}", age + 1, total)
    } else {
        String::new()
    };
    match try_copy_to_clipboard(text) {
        Ok(()) => {
            let mut message = format!(
                "Copied {label}{age_label} to clipboard ({} characters, {} lines).",
                text.len(),
                line_count(text)
            );
            if let Some(path) = fallback_path {
                let _ = write!(&mut message, "\nAlso written to {}.", path.display());
            }
            message
        }
        Err(_) => {
            if let Some(path) = fallback_path {
                format!(
                    "Clipboard copy unavailable. Wrote {label}{age_label} to {}.",
                    path.display()
                )
            } else {
                format!("{label}{age_label}:\n{text}")
            }
        }
    }
}

fn try_copy_to_clipboard(text: &str) -> Result<()> {
    Clipboard::new()
        .and_then(|mut clipboard| clipboard.set_text(text.to_string()))
        .context("clipboard unavailable")
}

fn write_temp_artifact(text: &str, filename: &str) -> Result<PathBuf> {
    let dir = std::env::temp_dir().join("puffer");
    fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let path = dir.join(filename);
    fs::write(&path, text).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

fn export_target_path(state: &AppState, args: &str) -> PathBuf {
    let filename = if args.is_empty() {
        default_export_filename(state)
    } else {
        args.to_string()
    };
    let mut path = PathBuf::from(filename);
    if !path.is_absolute() {
        path = state.cwd.join(path);
    }
    if path.extension().and_then(|value| value.to_str()) != Some("txt") {
        path.set_extension("txt");
    }
    path
}

fn default_export_filename(state: &AppState) -> String {
    let timestamp = export_timestamp();
    let prompt = first_prompt_text(&state.transcript);
    if prompt.is_empty() {
        return format!("conversation-{timestamp}.txt");
    }
    let sanitized = sanitize_filename(&prompt);
    if sanitized.is_empty() {
        format!("conversation-{timestamp}.txt")
    } else {
        format!("{timestamp}-{sanitized}.txt")
    }
}

fn first_prompt_text(transcript: &[RenderedMessage]) -> String {
    let Some(message) = transcript
        .iter()
        .find(|message| message.role == MessageRole::User)
    else {
        return String::new();
    };
    let mut text = message
        .text
        .trim()
        .lines()
        .next()
        .unwrap_or_default()
        .to_string();
    if text.chars().count() > 50 {
        text = text.chars().take(49).collect::<String>() + "...";
    }
    text
}

fn sanitize_filename(text: &str) -> String {
    let mut sanitized = String::new();
    let mut last_dash = false;
    for ch in text.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            sanitized.push(ch);
            last_dash = false;
        } else if (ch.is_ascii_whitespace() || ch == '-') && !last_dash && !sanitized.is_empty() {
            sanitized.push('-');
            last_dash = true;
        }
    }
    sanitized.trim_matches('-').to_string()
}

fn export_timestamp() -> String {
    let now = OffsetDateTime::now_local().unwrap_or_else(|_| OffsetDateTime::now_utc());
    format!(
        "{:04}-{:02}-{:02}-{:02}{:02}{:02}",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    )
}

fn role_label(role: &MessageRole) -> &'static str {
    match role {
        MessageRole::User => "User",
        MessageRole::Assistant => "Assistant",
        MessageRole::System => "System",
        MessageRole::ToolCall => "ToolCall",
        MessageRole::ToolResult => "ToolResult",
    }
}

fn line_count(text: &str) -> usize {
    text.chars().filter(|ch| *ch == '\n').count() + 1
}

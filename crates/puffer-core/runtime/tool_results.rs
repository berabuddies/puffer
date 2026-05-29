use puffer_provider_registry::ProviderDescriptor;

/// Maximum characters per individual tool result.
pub(crate) const MAX_TOOL_RESULT_CHARS: usize = 50_000;

/// Maximum aggregate characters for all tool results in a single turn.
pub(crate) const MAX_TOOL_RESULTS_PER_MESSAGE_CHARS: usize = 200_000;

/// Preview size for persisted tool outputs.
pub(crate) const PREVIEW_SIZE_CHARS: usize = 2_000;
const PERSISTED_OUTPUT_TAG: &str = "<persisted-output>";
const PERSISTED_OUTPUT_CLOSING_TAG: &str = "</persisted-output>";

/// Returns a short git status summary for system-reminder injection.
pub(crate) fn git_status_context() -> String {
    let branch = std::process::Command::new("git")
        .args(["branch", "--show-current"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    if branch.is_empty() {
        return String::new();
    }
    let status = std::process::Command::new("git")
        .args(["status", "--short", "--no-ahead-behind"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    let log = std::process::Command::new("git")
        .args(["log", "--oneline", "-3", "--no-decorate"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    let mut result = format!("Current branch: {branch}");
    if !status.is_empty() {
        result.push_str(&format!("\nStatus:\n{status}"));
    }
    if !log.is_empty() {
        result.push_str(&format!("\nRecent commits:\n{log}"));
    }
    result
}

/// Processes a tool result and persists oversized output to a preview file.
pub(crate) fn process_tool_result(text: &str, max_chars: usize, session_id: &uuid::Uuid) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    if let Some(message) = persist_and_preview(text, session_id) {
        return message;
    }
    truncate_tool_result(text, max_chars)
}

/// Truncates a tool result by preserving the head and tail.
pub(crate) fn truncate_tool_result(text: &str, max_chars: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max_chars {
        return text.to_string();
    }
    let head_len = max_chars / 2;
    let tail_len = max_chars - head_len;
    let head: String = chars[..head_len].iter().collect();
    let tail: String = chars[chars.len() - tail_len..].iter().collect();
    let omitted = chars.len() - max_chars;
    format!("{head}\n\n[…{omitted} chars truncated…]\n\n{tail}")
}

fn persist_and_preview(text: &str, session_id: &uuid::Uuid) -> Option<String> {
    let dir = std::env::temp_dir()
        .join(format!("puffer-{session_id}"))
        .join("tool-results");
    std::fs::create_dir_all(&dir).ok()?;
    let filename = format!("{}.txt", uuid::Uuid::new_v4());
    let filepath = dir.join(&filename);
    std::fs::write(&filepath, text).ok()?;
    Some(build_persisted_output_message(
        &filepath.to_string_lossy(),
        text,
    ))
}

/// Builds the persisted-output preview message shown to the model.
pub(crate) fn build_persisted_output_message(filepath: &str, text: &str) -> String {
    let total_chars = text.chars().count();
    let size_str = format_byte_size(text.len());
    if total_chars <= PREVIEW_SIZE_CHARS {
        return format!(
            "{PERSISTED_OUTPUT_TAG}\n\
             Output too large ({size_str}). Full output saved to: {filepath}\n\n\
             Preview:\n\
             {text}\n\
             {PERSISTED_OUTPUT_CLOSING_TAG}"
        );
    }
    if total_chars <= PREVIEW_SIZE_CHARS * 2 {
        let (preview, _) = head_preview(text, PREVIEW_SIZE_CHARS);
        let preview_size_str = format_byte_size(PREVIEW_SIZE_CHARS);
        return format!(
            "{PERSISTED_OUTPUT_TAG}\n\
             Output too large ({size_str}). Full output saved to: {filepath}\n\n\
             Preview (first {preview_size_str}):\n\
             {preview}\n...\n\
             {PERSISTED_OUTPUT_CLOSING_TAG}"
        );
    }
    let head_budget = PREVIEW_SIZE_CHARS / 2;
    let tail_budget = PREVIEW_SIZE_CHARS - head_budget;
    let (head, _) = head_preview(text, head_budget);
    let tail = tail_preview(text, tail_budget, total_chars);
    let omitted = total_chars
        .saturating_sub(head.chars().count())
        .saturating_sub(tail.chars().count());
    let head_size_str = format_byte_size(head_budget);
    let tail_size_str = format_byte_size(tail_budget);
    format!(
        "{PERSISTED_OUTPUT_TAG}\n\
         Output too large ({size_str}). Full output saved to: {filepath}\n\n\
         Preview (first {head_size_str} head + last {tail_size_str} tail):\n\
         {head}\n\n[…{omitted} chars truncated…]\n\n{tail}\n\
         {PERSISTED_OUTPUT_CLOSING_TAG}"
    )
}

fn head_preview(text: &str, max_chars: usize) -> (String, bool) {
    if text.chars().count() <= max_chars {
        return (text.to_string(), false);
    }
    let truncated: String = text.chars().take(max_chars).collect();
    let cut = truncated
        .rfind('\n')
        .filter(|&pos| pos > truncated.len() / 2)
        .unwrap_or(truncated.len());
    (truncated[..cut].to_string(), true)
}

fn tail_preview(text: &str, max_chars: usize, total_chars: usize) -> String {
    if total_chars <= max_chars {
        return text.to_string();
    }
    let skip = total_chars - max_chars;
    let suffix: String = text.chars().skip(skip).collect();
    let cut = suffix
        .find('\n')
        .filter(|&pos| pos < suffix.len() / 4)
        .map(|pos| pos + 1)
        .unwrap_or(0);
    suffix[cut..].to_string()
}

fn format_byte_size(bytes: usize) -> String {
    if bytes >= 1_000_000 {
        format!("{:.1} MB", bytes as f64 / 1_000_000.0)
    } else if bytes >= 1_000 {
        format!("{:.1} KB", bytes as f64 / 1_000.0)
    } else {
        format!("{bytes} bytes")
    }
}

/// Enforces the aggregate tool-result budget in place.
pub(crate) fn enforce_tool_result_budget(outputs: &mut [String], session_id: &uuid::Uuid) {
    let total: usize = outputs.iter().map(|o| o.len()).sum();
    if total <= MAX_TOOL_RESULTS_PER_MESSAGE_CHARS {
        return;
    }
    let mut indices: Vec<usize> = (0..outputs.len()).collect();
    indices.sort_by(|&a, &b| outputs[b].len().cmp(&outputs[a].len()));
    let mut remaining = total;
    for idx in indices {
        if remaining <= MAX_TOOL_RESULTS_PER_MESSAGE_CHARS {
            break;
        }
        let output = &outputs[idx];
        if output.contains(PERSISTED_OUTPUT_TAG) {
            continue;
        }
        if let Some(msg) = persist_and_preview(output, session_id) {
            remaining = remaining.saturating_sub(output.len()) + msg.len();
            outputs[idx] = msg;
        }
    }
}

/// Resolves max output tokens for a provider model.
pub(crate) fn resolve_max_output_tokens(provider: &ProviderDescriptor, model_id: &str) -> u32 {
    provider
        .models
        .iter()
        .find(|m| m.id == model_id)
        .map(|m| m.max_output_tokens)
        .filter(|&v| v > 0)
        .unwrap_or(16_384)
}

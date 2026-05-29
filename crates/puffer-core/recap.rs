//! Session recap (away-summary) feature.
//!
//! Mirrors Anthropic claude-code's `/recap` semantics (RE'd at v2.1.142):
//! a short, model-generated one-liner that surfaces in the UI when the
//! user comes back after stepping away. Two entrypoints:
//!
//! 1. `/recap` slash command — always runs (subject to `enabled`).
//! 2. Auto-trigger from TUI idle timer or GUI window-blur events — runs
//!    only when `auto=true` and the [`should_auto_trigger`] skip checks
//!    pass.
//!
//! The generated text is rendered as a transient system message in the
//! transcript: it is pushed into `state.transcript` for immediate display
//! but **not** written to `session.jsonl`, so `/resume` does not surface
//! historical recaps and the model never sees past recaps in its context.
//!
//! Constants are inherited from claude-code's reverse-engineered defaults
//! (`on8=180s`, `Ls3=3`, `ks3=2`).

use anyhow::Result;
use puffer_provider_registry::{AuthStore, ProviderRegistry};
use puffer_resources::LoadedResources;

use crate::state::{AppState, MessageRole};

/// Prompt sent to the model. The first paragraph is the glanceable recap
/// sentence; the optional following paragraphs are capped detail for UIs
/// that expose an expanded recap card.
pub(crate) const RECAP_PROMPT: &str = "The user stepped away and is coming back. Return plain text only in this exact shape: first paragraph is one short sentence under 22 words; then optionally up to 3 detail paragraphs. Each detail paragraph must be under 55 words. No markdown, bullets, headings, file inventories, exhaustive logs, or root-cause dumps. Focus on current goal, progress, blocker if any, and the next action.";

/// Visible-but-quiet prefix the TUI / GUI use to render the recap line
/// with a distinct symbol so users can tell auto-recap apart from a
/// regular system note. Matches claude-code's `nwq` reference symbol.
pub const RECAP_DISPLAY_PREFIX: &str = "\u{203B} recap: ";

const MAX_RECAP_SHORT_WORDS: usize = 22;
const MAX_RECAP_DETAIL_PARAGRAPHS: usize = 3;
const MAX_RECAP_DETAIL_WORDS: usize = 55;

/// Runs one synchronous recap side-turn against the currently selected
/// provider and model. Tool-filter is empty, so no tools are reachable;
/// the model sees the in-memory transcript (via `state.clone()`) and
/// returns a short summary as plain assistant text.
///
/// Returns the trimmed assistant text on success. Errors propagate so
/// callers can decide whether to surface the failure to the user
/// (slash command path) or swallow it (auto-trigger path).
pub fn run_recap_side_turn(
    state: &AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
) -> Result<String> {
    let mut side_state = state.clone();
    let filter = crate::runtime::build_request_tool_filter(&[])?;
    let turn = crate::runtime::execute_user_prompt_with_tool_filter(
        &mut side_state,
        resources,
        providers,
        auth_store,
        RECAP_PROMPT,
        filter.as_ref(),
    )?;
    Ok(normalize_recap_text(&turn.assistant_text))
}

/// Renders a successful recap into the live transcript without writing
/// it to `session.jsonl`. The recap is `MessageRole::System` so existing
/// renderers display it; the [`RECAP_DISPLAY_PREFIX`] sentinel lets TUI /
/// GUI styling pick it out for italic+dim treatment.
pub fn render_recap(state: &mut AppState, text: &str) {
    let recap = normalize_recap_text(text);
    let mut line = String::with_capacity(RECAP_DISPLAY_PREFIX.len() + recap.len());
    line.push_str(RECAP_DISPLAY_PREFIX);
    line.push_str(&recap);
    state.push_message(MessageRole::System, line);
}

fn normalize_recap_text(text: &str) -> String {
    let paragraphs = recap_paragraphs(text);
    if paragraphs.is_empty() {
        return String::new();
    }

    let first = strip_optional_label(
        &paragraphs[0],
        &["short:", "summary:", "sentence:", "recap:"],
    );
    let (short_source, first_detail) = split_first_sentence(&first);
    let short = limit_words(&short_source, MAX_RECAP_SHORT_WORDS);
    let mut details = Vec::new();
    if let Some(detail) = first_detail {
        push_detail_paragraph(&mut details, &detail);
    }
    for paragraph in paragraphs.iter().skip(1) {
        push_detail_paragraph(&mut details, paragraph);
    }

    if details.is_empty() {
        return short;
    }

    let mut output = short;
    for detail in details {
        output.push_str("\n\n");
        output.push_str(&detail);
    }
    output
}

fn recap_paragraphs(text: &str) -> Vec<String> {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let without_prefix = normalized
        .strip_prefix(RECAP_DISPLAY_PREFIX)
        .unwrap_or(&normalized);
    let mut paragraphs = Vec::new();
    let mut current = Vec::new();

    for line in without_prefix.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            flush_recap_paragraph(&mut paragraphs, &mut current);
            continue;
        }
        if let Some(rest) = strip_known_label(trimmed, &["details:", "detail:"]) {
            flush_recap_paragraph(&mut paragraphs, &mut current);
            if !rest.is_empty() {
                current.push(rest.to_string());
            }
            continue;
        }
        current.push(strip_list_marker(trimmed).to_string());
    }
    flush_recap_paragraph(&mut paragraphs, &mut current);
    paragraphs
}

fn flush_recap_paragraph(paragraphs: &mut Vec<String>, current: &mut Vec<String>) {
    if current.is_empty() {
        return;
    }
    let paragraph = collapse_recap_whitespace(&current.join(" "));
    if !paragraph.is_empty() {
        paragraphs.push(paragraph);
    }
    current.clear();
}

fn push_detail_paragraph(details: &mut Vec<String>, paragraph: &str) {
    if details.len() >= MAX_RECAP_DETAIL_PARAGRAPHS {
        return;
    }
    let cleaned = strip_optional_label(paragraph, &["details:", "detail:"]);
    let cleaned = collapse_recap_whitespace(strip_list_marker(&cleaned));
    if cleaned.is_empty() {
        return;
    }
    details.push(limit_words(&cleaned, MAX_RECAP_DETAIL_WORDS));
}

fn strip_known_label<'a>(text: &'a str, labels: &[&str]) -> Option<&'a str> {
    let trimmed = text.trim();
    let lower = trimmed.to_ascii_lowercase();
    for label in labels {
        if lower.starts_with(label) {
            return Some(trimmed[label.len()..].trim());
        }
    }
    None
}

fn strip_optional_label(text: &str, labels: &[&str]) -> String {
    strip_known_label(text, labels)
        .unwrap_or_else(|| text.trim())
        .to_string()
}

fn strip_list_marker(text: &str) -> &str {
    let trimmed = text.trim();
    if let Some(rest) = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
    {
        return rest.trim();
    }
    let mut chars = trimmed.char_indices().peekable();
    while let Some((_, ch)) = chars.peek() {
        if !ch.is_ascii_digit() {
            break;
        }
        chars.next();
    }
    if let Some((marker_index, marker)) = chars.next() {
        if (marker == '.' || marker == ')') && trimmed[marker_index + 1..].starts_with(' ') {
            return trimmed[marker_index + 2..].trim();
        }
    }
    trimmed
}

fn split_first_sentence(text: &str) -> (String, Option<String>) {
    let trimmed = text.trim();
    for (index, ch) in trimmed.char_indices() {
        if !matches!(ch, '.' | '!' | '?') {
            continue;
        }
        let end = index + ch.len_utf8();
        let next_is_boundary = trimmed[end..]
            .chars()
            .next()
            .map(|next| next.is_whitespace())
            .unwrap_or(true);
        if next_is_boundary {
            let first = trimmed[..end].trim().to_string();
            let rest = trimmed[end..].trim();
            return (
                first,
                if rest.is_empty() {
                    None
                } else {
                    Some(rest.to_string())
                },
            );
        }
    }
    (trimmed.to_string(), None)
}

fn limit_words(text: &str, max_words: usize) -> String {
    let words = text.split_whitespace().collect::<Vec<_>>();
    if words.len() <= max_words {
        return collapse_recap_whitespace(text);
    }
    let mut limited = words[..max_words].join(" ");
    limited = limited
        .trim_end_matches(|ch: char| matches!(ch, ',' | ';' | ':'))
        .to_string();
    if !limited.ends_with('.') && !limited.ends_with('!') && !limited.ends_with('?') {
        limited.push('.');
    }
    limited
}

fn collapse_recap_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Skip-check decisions for the auto-trigger path. Returns `Ok(true)` if
/// a recap should fire, `Ok(false)` if any guard rejects it. The slash
/// command path bypasses these checks.
///
/// `last_recap_user_msg_count` is the count of user messages observed at
/// the time the last recap fired (or `None` if no recap has fired this
/// session). `draft_input` is the user's unsent input buffer (TUI's
/// editor contents or GUI's input field text).
pub fn should_auto_trigger(
    state: &AppState,
    last_recap_user_msg_count: Option<usize>,
    draft_input: &str,
) -> bool {
    if !state.config.recap.enabled || !state.config.recap.auto {
        return false;
    }
    if !draft_input.trim().is_empty() {
        return false;
    }
    let user_count = count_user_messages(state);
    if user_count < state.config.recap.min_user_messages {
        return false;
    }
    if let Some(last) = last_recap_user_msg_count {
        if user_count.saturating_sub(last) < state.config.recap.cooldown_messages {
            return false;
        }
    }
    true
}

/// Counts real user messages in the in-memory transcript. Used by the
/// auto-trigger skip checks to enforce `Ls3` (min user messages) and
/// `ks3` (cooldown between recaps).
pub fn count_user_messages(state: &AppState) -> usize {
    state
        .transcript
        .iter()
        .filter(|message| matches!(message.role, MessageRole::User))
        .count()
}

/// Whether the recap feature is enabled at all (gating both the slash
/// command and the auto-trigger).
pub fn enabled(state: &AppState) -> bool {
    state.config.recap.enabled
}

/// Whether the auto-trigger specifically is enabled. Slash command runs
/// regardless when [`enabled`] is true.
pub fn auto_enabled(state: &AppState) -> bool {
    state.config.recap.enabled && state.config.recap.auto
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_config::PufferConfig;
    use puffer_session_store::SessionMetadata;
    use std::path::PathBuf;
    use uuid::Uuid;

    fn fresh_state() -> AppState {
        let session = SessionMetadata {
            id: Uuid::nil(),
            display_name: None,
            generated_title: None,
            cwd: PathBuf::from("/tmp"),
            created_at_ms: 0,
            updated_at_ms: 0,
            parent_session_id: None,
            slug: None,
            tags: Vec::new(),
            note: None,
        };
        AppState::new(PufferConfig::default(), PathBuf::from("/tmp"), session)
    }

    fn push_user(state: &mut AppState, text: &str) {
        state.push_message(MessageRole::User, text.to_string());
    }

    #[test]
    fn skip_when_disabled() {
        let mut state = fresh_state();
        state.config.recap.enabled = false;
        for _ in 0..10 {
            push_user(&mut state, "u");
        }
        assert!(!should_auto_trigger(&state, None, ""));
    }

    #[test]
    fn skip_when_auto_disabled_only() {
        let mut state = fresh_state();
        state.config.recap.enabled = true;
        state.config.recap.auto = false;
        for _ in 0..10 {
            push_user(&mut state, "u");
        }
        // enabled() stays true (slash command still works) but auto is off.
        assert!(enabled(&state));
        assert!(!auto_enabled(&state));
        assert!(!should_auto_trigger(&state, None, ""));
    }

    #[test]
    fn skip_when_draft_non_empty() {
        let mut state = fresh_state();
        for _ in 0..5 {
            push_user(&mut state, "u");
        }
        assert!(!should_auto_trigger(&state, None, "typing..."));
        // Whitespace-only is treated as empty (trim) — does NOT block.
        assert!(should_auto_trigger(&state, None, "  "));
    }

    #[test]
    fn skip_below_min_user_messages() {
        let mut state = fresh_state();
        push_user(&mut state, "one");
        push_user(&mut state, "two");
        // default min_user_messages = 3, two is below threshold
        assert!(!should_auto_trigger(&state, None, ""));
        push_user(&mut state, "three");
        assert!(should_auto_trigger(&state, None, ""));
    }

    #[test]
    fn skip_during_cooldown() {
        let mut state = fresh_state();
        for _ in 0..3 {
            push_user(&mut state, "u");
        }
        // Last recap fired when user_count == 3; cooldown = 2 default.
        // Still 3 now → 0 new messages, cooldown blocks.
        assert!(!should_auto_trigger(&state, Some(3), ""));
        // One new user message — still inside cooldown (need 2).
        push_user(&mut state, "u");
        assert!(!should_auto_trigger(&state, Some(3), ""));
        // Two new — cooldown satisfied.
        push_user(&mut state, "u");
        assert!(should_auto_trigger(&state, Some(3), ""));
    }

    #[test]
    fn render_recap_uses_prefix() {
        let mut state = fresh_state();
        render_recap(
            &mut state,
            "Summary: Hello world.\n\nDetails:\n- First useful detail.\n\nSecond useful detail.",
        );
        let last = state.transcript.last().expect("recap message pushed");
        assert!(matches!(last.role, MessageRole::System));
        assert!(last.text.starts_with(RECAP_DISPLAY_PREFIX));
        assert!(last.text.contains("Hello world.\n\nFirst useful detail."));
        assert!(last.text.ends_with("Second useful detail."));
    }

    #[test]
    fn recap_prompt_requests_a_short_sentence_with_capped_details() {
        assert!(RECAP_PROMPT.contains("one short sentence"));
        assert!(RECAP_PROMPT.contains("up to 3 detail paragraphs"));
        assert!(RECAP_PROMPT.contains("No markdown, bullets"));
    }

    #[test]
    fn normalize_recap_caps_details_and_strips_dump_formatting() {
        let text = "\
Short: The auth refresh path is being tightened before the next test run. Extra sentence should become detail.

Details:
- Provider routing is wired, but the OAuth refresh branch still needs the failing status case handled before the CLI path is re-run.

2. The desktop view should only surface the active blocker and next action rather than replaying each file inspected during the turn.

Third paragraph stays.

Fourth paragraph is dropped.";

        let normalized = normalize_recap_text(text);
        let parts = normalized.split("\n\n").collect::<Vec<_>>();
        assert_eq!(parts.len(), 4);
        assert_eq!(
            parts[0],
            "The auth refresh path is being tightened before the next test run."
        );
        assert_eq!(parts[1], "Extra sentence should become detail.");
        assert!(!parts[2].starts_with('-'));
        assert!(!parts[3].contains("Fourth paragraph"));
    }
}

use super::ToolInvocation;
use super::{ActionKind, ActionObservation, ReflectionLanguage, ValidationSnapshot};
use serde_json::Value;
use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

const PLACEHOLDER_CONTENTS: &[&str] = &[
    "[]",
    "{}",
    "\"\"",
    "''",
    "null",
    "todo",
    "tbd",
    "placeholder",
    "<empty>",
];
const VALIDATION_HINTS: &[&str] = &[
    "test",
    "tests",
    "pytest",
    "cargo test",
    "npm test",
    "pnpm test",
    "yarn test",
    "vitest",
    "jest",
    "mocha",
    "rspec",
    "go test",
    "mvn test",
    "gradle test",
    "ctest",
    "check",
    "verify",
    "verifier",
    "lint",
    "ruff",
    "clippy",
    "typecheck",
    "tsc",
    "integration",
    "e2e",
];
const ARTIFACT_HINTS: &[&str] = &[
    "write", "create", "save", "output", "artifact", "answer", "draft", "generate", "emit",
    "update",
];
const RUNTIME_PATH_MARKERS: &[&str] = &[
    "/.puffer/",
    ".puffer/",
    "/.codex/",
    ".codex/",
    "/.git/",
    ".git/",
    "/target/",
    "target/",
    "/tmp/",
    "/var/folders/",
    "/private/var/folders/",
];

pub(super) fn content_is_meaningful(content: &str) -> bool {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return false;
    }
    let normalized = normalize_text(trimmed);
    if PLACEHOLDER_CONTENTS.contains(&normalized.as_str()) {
        return false;
    }
    if trimmed
        .chars()
        .all(|ch| matches!(ch, '[' | ']' | '{' | '}' | '"' | '\'' | '`'))
    {
        return false;
    }
    trimmed
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .take(4)
        .count()
        >= 4
}

pub(super) fn looks_like_validation_command(command: &str, description: &str) -> bool {
    let combined = format!(
        "{} {}",
        normalize_text(command),
        normalize_text(description)
    );
    VALIDATION_HINTS.iter().any(|hint| combined.contains(hint))
}

pub(super) fn extract_path_candidates(text: &str) -> BTreeSet<String> {
    text.split_whitespace()
        .map(clean_path_token)
        .filter(|token| is_path_candidate(token))
        .collect()
}

pub(super) fn extract_artifact_candidates(goal: &str) -> BTreeSet<String> {
    let mut artifacts = BTreeSet::new();
    let lowered_goal = goal.to_ascii_lowercase();
    for path in extract_path_candidates(goal) {
        let lowered_path = path.to_ascii_lowercase();
        let hinted = lowered_goal.find(&lowered_path).is_some_and(|index| {
            let start = index.saturating_sub(48);
            let end = (index + lowered_path.len() + 16).min(lowered_goal.len());
            let window = &lowered_goal[start..end];
            ARTIFACT_HINTS.iter().any(|hint| window.contains(hint))
        });
        let basename = path
            .rsplit('/')
            .next()
            .unwrap_or(path.as_str())
            .to_ascii_lowercase();
        let outputish = basename.contains("out")
            || basename.contains("output")
            || basename.contains("answer")
            || basename.contains("result");
        if hinted || outputish {
            artifacts.insert(path);
        }
    }
    artifacts
}

pub(super) fn summarize_goal(goal: &str) -> String {
    let line = first_non_empty_line(goal).unwrap_or(goal).trim();
    truncate_chars(line, 240)
}

pub(super) fn path_matches_targets(path: &str, targets: &BTreeSet<String>) -> bool {
    if targets.is_empty() {
        return false;
    }
    let normalized_path = clean_path_token(path).to_ascii_lowercase();
    let path_name = normalized_path
        .rsplit('/')
        .next()
        .unwrap_or(normalized_path.as_str());
    targets.iter().any(|target| {
        let normalized_target = clean_path_token(target).to_ascii_lowercase();
        let target_name = normalized_target
            .rsplit('/')
            .next()
            .unwrap_or(normalized_target.as_str());
        normalized_path == normalized_target
            || normalized_path.ends_with(&format!("/{normalized_target}"))
            || normalized_target.ends_with(&format!("/{normalized_path}"))
            || path_name == target_name
    })
}

pub(super) fn is_runtime_path(path: &str) -> bool {
    let normalized = path.to_ascii_lowercase();
    RUNTIME_PATH_MARKERS
        .iter()
        .any(|marker| normalized.contains(marker))
}

pub(super) fn extract_count(text: &str, label: &str) -> Option<u32> {
    let normalized_text = text.to_ascii_lowercase();
    let normalized_label = label.to_ascii_lowercase();
    for (index, _) in normalized_text.match_indices(&normalized_label) {
        if let Some(value) = find_number_before(&normalized_text[..index]) {
            return Some(value);
        }
        if let Some(value) = find_number_after(&normalized_text[index + normalized_label.len()..]) {
            return Some(value);
        }
    }
    None
}

pub(super) fn count_case_insensitive(text: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }
    text.to_ascii_lowercase()
        .match_indices(&needle.to_ascii_lowercase())
        .count()
}

pub(super) fn first_non_empty_line(text: &str) -> Option<&str> {
    text.lines().find(|line| !line.trim().is_empty())
}

pub(super) fn normalize_text(text: &str) -> String {
    text.to_ascii_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn clean_path_token(token: &str) -> String {
    let mut cleaned = token
        .trim_matches(|ch: char| matches!(ch, '"' | '\'' | '`' | '(' | ')' | '[' | ']' | '{' | '}'))
        .trim_end_matches(|ch: char| matches!(ch, ',' | ';' | ':' | '!' | '?'))
        .to_string();
    if cleaned.ends_with('.') && cleaned.matches('.').count() > 1 {
        cleaned.pop();
    }
    cleaned
}

fn has_file_extension(path: &str) -> bool {
    let Some(name) = path.rsplit('/').next() else {
        return false;
    };
    let Some((stem, extension)) = name.rsplit_once('.') else {
        return false;
    };
    !stem.is_empty()
        && !extension.is_empty()
        && extension.len() <= 12
        && extension.chars().all(|ch| ch.is_ascii_alphanumeric())
}

fn is_path_candidate(token: &str) -> bool {
    if token.is_empty() || token.contains("://") || token.starts_with("--") {
        return false;
    }
    if is_scp_style_remote(token) {
        return false;
    }
    (token.starts_with('/')
        || token.starts_with("./")
        || token.starts_with("../")
        || token.contains('/'))
        && token.chars().any(|ch| ch.is_ascii_alphanumeric())
        || has_file_extension(token)
}

/// Detects scp/git-ssh style remotes such as `user@host:/git/server` so the
/// reflection tracker does not mistake them for local filesystem targets.
fn is_scp_style_remote(token: &str) -> bool {
    let Some((left, right)) = token.split_once(':') else {
        return false;
    };
    if left.is_empty() || right.is_empty() {
        return false;
    }
    // Require `user@host` on the left — plain `host:path` is ambiguous with
    // Windows drive letters and ratio-style tokens, so we keep the heuristic
    // narrow here.
    let Some((user, host)) = left.split_once('@') else {
        return false;
    };
    !user.is_empty()
        && !host.is_empty()
        && !host.contains('/')
        && !user.contains('/')
        && host.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_'))
}

fn find_number_before(text: &str) -> Option<u32> {
    text.rsplit(|ch: char| ch.is_whitespace() || matches!(ch, ',' | ';' | '(' | ')' | '[' | ']'))
        .take(3)
        .find_map(parse_numeric_token)
}

fn find_number_after(text: &str) -> Option<u32> {
    text.split(|ch: char| ch.is_whitespace() || matches!(ch, ',' | ';' | '(' | ')' | '[' | ']'))
        .take(3)
        .find_map(parse_numeric_token)
}

fn parse_numeric_token(token: &str) -> Option<u32> {
    let digits = token.trim_matches(|ch: char| !ch.is_ascii_digit());
    if digits.is_empty() {
        None
    } else {
        digits.parse::<u32>().ok()
    }
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    text.chars().take(max_chars).collect::<String>()
}

pub(super) fn classify_validation(invocation: &ToolInvocation) -> Option<ValidationSnapshot> {
    let input = serde_json::from_str::<Value>(&invocation.input).ok()?;
    let command = input
        .get("command")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let description = input
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !looks_like_validation_command(command, description) {
        return None;
    }
    let failed = extract_count(invocation.output.as_str(), "failed");
    let passed = extract_count(invocation.output.as_str(), "passed");
    let error_count = Some(count_case_insensitive(invocation.output.as_str(), "error:") as u32);
    Some(ValidationSnapshot {
        success: invocation.success,
        failed,
        passed,
        error_count,
    })
}

#[derive(Debug, Clone)]
pub(super) struct WriteProgress {
    pub(super) path: String,
    pub(super) meaningful: bool,
    pub(super) artifact: bool,
}

#[derive(Debug, Clone)]
pub(super) struct EditProgress {
    pub(super) path: String,
    pub(super) meaningful: bool,
}

pub(super) fn classify_write_progress(
    invocation: &ToolInvocation,
    artifact_paths: &BTreeSet<String>,
) -> Option<WriteProgress> {
    let input = serde_json::from_str::<Value>(&invocation.input).ok()?;
    let path = input.get("file_path")?.as_str()?.to_string();
    let content = input
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default();
    Some(WriteProgress {
        artifact: path_matches_targets(&path, artifact_paths),
        meaningful: content_is_meaningful(content),
        path,
    })
}

pub(super) fn classify_edit_progress(
    invocation: &ToolInvocation,
    target_paths: &BTreeSet<String>,
) -> Option<EditProgress> {
    let input = serde_json::from_str::<Value>(&invocation.input).ok()?;
    let path = input.get("file_path")?.as_str()?.to_string();
    let old_string = input
        .get("old_string")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let new_string = input
        .get("new_string")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let meaningful = old_string.trim() != new_string.trim()
        && (!new_string.trim().is_empty() || path_matches_targets(&path, target_paths));
    Some(EditProgress { path, meaningful })
}

pub(super) fn validation_improved(
    previous: Option<ValidationSnapshot>,
    current: ValidationSnapshot,
) -> bool {
    let Some(previous) = previous else {
        return current.success;
    };
    if current.success && !previous.success {
        return true;
    }
    if let (Some(prev_failed), Some(curr_failed)) = (previous.failed, current.failed) {
        if curr_failed < prev_failed {
            return true;
        }
    }
    if let (Some(prev_passed), Some(curr_passed)) = (previous.passed, current.passed) {
        if curr_passed > prev_passed {
            return true;
        }
    }
    if let (Some(prev_errors), Some(curr_errors)) = (previous.error_count, current.error_count) {
        if curr_errors < prev_errors {
            return true;
        }
    }
    false
}

pub(super) fn observe_invocation(invocation: &ToolInvocation) -> ActionObservation {
    let primary_path = primary_path(invocation);
    let fingerprint = normalized_fingerprint(invocation, primary_path.as_deref());
    let error_signature = if invocation.success {
        None
    } else {
        first_non_empty_line(&invocation.output).map(normalize_text)
    };
    ActionObservation {
        kind: action_kind(&invocation.tool_id),
        fingerprint,
        error_signature,
        primary_path,
    }
}

pub(super) fn render_action_preview(action: &ActionObservation) -> String {
    match &action.primary_path {
        Some(path) => format!("{:?} {}", action.kind, path),
        None => action.fingerprint.clone(),
    }
}

pub(super) fn build_prompt(
    language: ReflectionLanguage,
    goal: &str,
    summary: &str,
    signal_lines: &str,
    recent_actions: &str,
    relevant_paths: &str,
    judge_lines: &str,
) -> String {
    match language {
        ReflectionLanguage::Chinese => format!(
            "<system-reminder>\n反思检查点已触发。\n{summary}\n\n当前目标摘要：\n- {goal}\n\nJudge 结论：\n{judge_lines}\n\n最近信号：\n{signal_lines}\n\n最近动作：\n{recent_actions}\n\n相关文件：\n{relevant_paths}\n\n先在内部用中文回答下面 5 个问题，再继续执行任务。除非你决定升级处理，否则不要把这段反思原样告诉用户。\n1. 当前目标是什么？\n2. 有哪些证据说明当前方法有效或无效？\n3. 自上次 checkpoint 以来有什么变化？\n4. 现在最好的下一步动作是什么？\n5. 继续、重规划，还是升级处理？\n\n输出约束：\n- 先在内部得出一个决定：CONTINUE、REPLAN 或 ESCALATE。\n- 如果决定是 REPLAN，立刻换方法，不要重复刚才那条路径。\n- 如果决定是 ESCALATE，但当前没有用户可问，就简短说明阻塞点并采取成本最低的 fallback，而不是继续死循环。\n- 不要只停在反思；反思后要继续做事。\n</system-reminder>"
        ),
        ReflectionLanguage::English => format!(
            "<system-reminder>\nReflection checkpoint triggered.\n{summary}\n\nCurrent goal summary:\n- {goal}\n\nJudge verdict:\n{judge_lines}\n\nRecent signals:\n{signal_lines}\n\nRecent actions:\n{recent_actions}\n\nRelevant files:\n{relevant_paths}\n\nAnswer the following 5 questions internally in English before you continue. Do not echo the full reflection to the user unless you decide to escalate.\n1. What is the current goal?\n2. What evidence says the current approach is or is not working?\n3. What changed since the last checkpoint?\n4. What is the next best action?\n5. Continue, replan, or escalate?\n\nOutput constraints:\n- Decide internally: CONTINUE, REPLAN, or ESCALATE.\n- If the decision is REPLAN, switch methods immediately instead of repeating the current path.\n- If the decision is ESCALATE and no user interaction is available, state the blocker briefly and take the cheapest viable fallback instead of looping.\n- Do not stop at reflection; continue the task.\n</system-reminder>"
        ),
    }
}

pub(super) fn language_label(language: ReflectionLanguage) -> &'static str {
    match language {
        ReflectionLanguage::English => "en",
        ReflectionLanguage::Chinese => "zh",
    }
}

pub(super) fn unix_time_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn action_kind(tool_id: &str) -> ActionKind {
    match tool_id {
        "Read" => ActionKind::Read,
        "Write" => ActionKind::Write,
        "Edit" => ActionKind::Edit,
        "Bash" => ActionKind::Bash,
        _ => ActionKind::Other,
    }
}

fn primary_path(invocation: &ToolInvocation) -> Option<String> {
    let input = serde_json::from_str::<Value>(&invocation.input).ok()?;
    if let Some(path) = input.get("file_path").and_then(Value::as_str) {
        return Some(path.to_string());
    }
    if invocation.tool_id == "Bash" {
        let command = input
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or_default();
        return extract_path_candidates(command).into_iter().next();
    }
    None
}

fn normalized_fingerprint(invocation: &ToolInvocation, primary_path: Option<&str>) -> String {
    match invocation.tool_id.as_str() {
        "Read" | "Write" | "Edit" => format!(
            "{}:{}",
            invocation.tool_id.to_ascii_lowercase(),
            primary_path.unwrap_or("unknown")
        ),
        "Bash" => {
            let input = serde_json::from_str::<Value>(&invocation.input).ok();
            let command = input
                .as_ref()
                .and_then(|value| value.get("command"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            let normalized = normalize_text(command);
            let head = normalized
                .split_whitespace()
                .take(4)
                .collect::<Vec<_>>()
                .join(" ");
            match primary_path {
                Some(path) => format!("bash:{head}:{path}"),
                None => format!("bash:{head}"),
            }
        }
        _ => format!(
            "{}:{}",
            invocation.tool_id.to_ascii_lowercase(),
            normalize_text(&invocation.input)
        ),
    }
}

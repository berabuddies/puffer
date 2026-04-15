use std::collections::BTreeSet;

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
    (token.starts_with('/')
        || token.starts_with("./")
        || token.starts_with("../")
        || token.contains('/'))
        && token.chars().any(|ch| ch.is_ascii_alphanumeric())
        || has_file_extension(token)
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

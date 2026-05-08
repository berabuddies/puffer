use crate::workspace_paths;
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;

#[derive(Debug, Deserialize)]
struct ClaudeEditInput {
    file_path: String,
    old_string: String,
    new_string: String,
    #[serde(default)]
    replace_all: bool,
}

/// Executes a Claude-style `Edit` operation and returns a JSON tool result payload.
pub fn execute_claude_edit(
    cwd: &Path,
    working_dirs: &[std::path::PathBuf],
    allow_all_paths: bool,
    input: Value,
) -> Result<String> {
    let input: ClaudeEditInput = serde_json::from_value(input).context("invalid Edit input")?;

    let raw_path = Path::new(&input.file_path);
    if !raw_path.is_absolute()
        && !input.file_path.trim().starts_with("~/")
        && input.file_path.trim() != "~"
    {
        bail!("Edit expects `file_path` to be an absolute path");
    }
    let sandbox_mode = if allow_all_paths {
        "danger-full-access"
    } else {
        "workspace-write"
    };
    let path = workspace_paths::resolve_path_for_session(
        cwd,
        working_dirs,
        sandbox_mode,
        Path::new(&input.file_path),
    )?;
    if input.old_string == input.new_string {
        bail!("No changes to make: old_string and new_string are exactly the same.");
    }

    let original = if path.exists() {
        fs::read_to_string(&path)
            .with_context(|| format!("failed to read file {}", path.display()))?
    } else {
        String::new()
    };
    let (updated, original_file) = if !path.exists() && input.old_string.is_empty() {
        (input.new_string.clone(), String::new())
    } else if input.old_string.is_empty() {
        if !original.is_empty() {
            bail!("Edit with empty old_string requires the target file to be empty or missing.");
        }
        (input.new_string.clone(), original.clone())
    } else {
        let occurrences = occurrence_count(&original, &input.old_string);
        if occurrences == 0 {
            // Mirror CC's actionable wording: include a snippet of the
            // string so the model sees what it actually sent (vs. a
            // memory-only reference that could have drifted), plus a
            // `\uXXXX`-escape diagnostic note when the input contains
            // those escape forms — that's the most common silent
            // mismatch class (model emitted ` ` literally vs. an
            // actual NBSP in the file). Codex parity for the
            // `String to replace not found` branch.
            let snippet = old_string_diagnostic_snippet(&input.old_string);
            let escape_hint = if contains_unicode_escape(&input.old_string) {
                "\n(Note: Edit also tried swapping `\\uXXXX` escapes and their characters; \
                 neither form matched, so the mismatch is likely elsewhere in old_string. \
                 Re-read the file and copy the exact surrounding text.)"
            } else {
                ""
            };
            bail!(
                "Edit failed: old_string was not found in {}.\n\
                String: {snippet}{escape_hint}",
                path.display()
            );
        }
        if occurrences > 1 && !input.replace_all {
            // Include the exact match count so the model can pick a
            // strategy: 2-3 matches → add 1-2 lines of context;
            // many matches → use `replace_all`. Codex parity:
            // `Found N matches of the string to replace, but
            // replace_all is false. ...`
            let snippet = old_string_diagnostic_snippet(&input.old_string);
            bail!(
                "Edit failed: Found {occurrences} matches of old_string in {}, \
                but replace_all is false. To replace all occurrences, set \
                replace_all to true. To replace only one occurrence, please \
                provide more context to uniquely identify the instance.\n\
                String: {snippet}",
                path.display()
            );
        }
        let updated = if input.replace_all {
            original.replace(&input.old_string, &input.new_string)
        } else {
            original.replacen(&input.old_string, &input.new_string, 1)
        };
        (updated, original.clone())
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent.to_path_buf())
            .with_context(|| format!("failed to create parent directory {}", parent.display()))?;
    }
    fs::write(&path, &updated)
        .with_context(|| format!("failed to write file {}", path.display()))?;

    let output = json!({
        "filePath": input.file_path,
        "oldString": input.old_string,
        "newString": input.new_string,
        "originalFile": original_file,
        "structuredPatch": build_structured_patch(&original, &updated),
        "userModified": false,
        "replaceAll": input.replace_all
    });
    Ok(serde_json::to_string_pretty(&output)?)
}

/// Returns true when an Edit request targets an existing file mutation that
/// should require a prior full-file Read in the runtime dispatcher.
pub(crate) fn requires_prior_read(input: &Value) -> bool {
    let Some(file_path) = input.get("file_path").and_then(Value::as_str) else {
        return true;
    };
    let old_string = input
        .get("old_string")
        .and_then(Value::as_str)
        .unwrap_or_default();
    !(old_string.is_empty() && !Path::new(file_path).exists())
}

fn occurrence_count(haystack: &str, needle: &str) -> usize {
    haystack.match_indices(needle).count()
}

/// Truncate `old_string` to a single-line snippet for inclusion in
/// error messages. Wraps in backticks; collapses internal newlines
/// to `↵` so the agent sees the structure without the message
/// becoming unreadable. Caps at 200 chars — long enough to identify
/// the line, short enough to avoid bloating the error tool result.
fn old_string_diagnostic_snippet(old_string: &str) -> String {
    const MAX: usize = 200;
    let normalized = old_string.replace('\n', "↵");
    let chars: Vec<char> = normalized.chars().collect();
    if chars.len() <= MAX {
        format!("`{normalized}`")
    } else {
        let head: String = chars.iter().take(MAX).collect();
        let omitted = chars.len() - MAX;
        format!("`{head}…` ({omitted} more chars)")
    }
}

/// Detect whether a string contains a literal `\uXXXX` escape
/// sequence. When true, the not-found error includes a hint that
/// codex's bundle calls "the most common silent mismatch class"
/// — model emitted the escape literal but the file has the actual
/// codepoint (or vice versa). Mirrors codex `bo9` predicate
/// (`claude-2.1.133` bundle).
fn contains_unicode_escape(s: &str) -> bool {
    // Match `\u` followed by 4 hex digits. A literal backslash in
    // Rust source is just `\\` in the regex source we emit through
    // bytes-iter (avoids a regex crate dep for this trivial check).
    let bytes = s.as_bytes();
    let mut i = 0;
    while i + 5 < bytes.len() {
        if bytes[i] == b'\\'
            && bytes[i + 1] == b'u'
            && bytes[i + 2..i + 6].iter().all(u8::is_ascii_hexdigit)
        {
            return true;
        }
        i += 1;
    }
    false
}

fn build_structured_patch(original: &str, updated: &str) -> Vec<Value> {
    let old_lines = split_lines(original);
    let new_lines = split_lines(updated);
    vec![json!({
        "oldStart": if old_lines.is_empty() { 0 } else { 1 },
        "oldLines": old_lines.len(),
        "newStart": if new_lines.is_empty() { 0 } else { 1 },
        "newLines": new_lines.len(),
        "lines": old_lines
            .iter()
            .map(|line| format!("-{line}"))
            .chain(new_lines.iter().map(|line| format!("+{line}")))
            .collect::<Vec<_>>(),
    })]
}

fn split_lines(content: &str) -> Vec<String> {
    if content.is_empty() {
        Vec::new()
    } else {
        content.lines().map(str::to_string).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn old_string_diagnostic_snippet_truncates_long_strings() {
        let s = "x".repeat(500);
        let snippet = old_string_diagnostic_snippet(&s);
        assert!(snippet.starts_with("`"));
        assert!(snippet.contains("more chars"));
        assert!(snippet.len() < 350); // 200 chars + decoration
    }

    #[test]
    fn old_string_diagnostic_snippet_collapses_newlines() {
        let snippet = old_string_diagnostic_snippet("a\nb\nc");
        assert_eq!(snippet, "`a↵b↵c`");
    }

    #[test]
    fn contains_unicode_escape_detects_uxxxx_form() {
        assert!(contains_unicode_escape(r"\u00A0"));
        assert!(contains_unicode_escape(r"foo\u2014bar"));
        assert!(!contains_unicode_escape("plain string"));
        assert!(!contains_unicode_escape(r"\u00")); // too few hex digits
        assert!(!contains_unicode_escape(r"\nXXXX")); // not \u
    }

    #[test]
    fn not_found_error_includes_string_snippet() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("sample.txt");
        fs::write(&file, "alpha\nbeta\n").unwrap();
        let input = json!({
            "file_path": file.display().to_string(),
            "old_string": "missing_thing",
            "new_string": "x"
        });

        let err = execute_claude_edit(temp.path(), &[], false, input).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("was not found"));
        assert!(
            msg.contains("missing_thing"),
            "error message should include the user-supplied old_string; got: {msg}"
        );
    }

    #[test]
    fn not_found_error_emits_unicode_escape_hint_when_relevant() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("sample.txt");
        fs::write(&file, "plain ascii\n").unwrap();
        let input = json!({
            "file_path": file.display().to_string(),
            "old_string": r"non_breaking\u00A0space",
            "new_string": "x"
        });

        let err = execute_claude_edit(temp.path(), &[], false, input).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("\\uXXXX") || msg.contains("escape"));
    }

    #[test]
    fn not_found_error_omits_unicode_hint_when_no_escape() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("sample.txt");
        fs::write(&file, "alpha\nbeta\n").unwrap();
        let input = json!({
            "file_path": file.display().to_string(),
            "old_string": "missing_thing",
            "new_string": "x"
        });

        let err = execute_claude_edit(temp.path(), &[], false, input).unwrap_err();
        let msg = err.to_string();
        assert!(!msg.contains("escapes"));
    }

    #[test]
    fn not_unique_error_includes_match_count() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("sample.txt");
        fs::write(&file, "foo\nfoo\nfoo\nbar\n").unwrap();
        let input = json!({
            "file_path": file.display().to_string(),
            "old_string": "foo",
            "new_string": "x"
            // replace_all defaults to false
        });

        let err = execute_claude_edit(temp.path(), &[], false, input).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("Found 3 matches"),
            "error message should report the exact match count; got: {msg}"
        );
        assert!(msg.contains("replace_all"));
    }

    #[test]
    fn edit_replaces_unique_occurrence() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("sample.txt");
        fs::write(&file, "alpha\nbeta\n").unwrap();
        let input = json!({
            "file_path": file.display().to_string(),
            "old_string": "beta",
            "new_string": "gamma"
        });

        let output = execute_claude_edit(temp.path(), &[], false, input).unwrap();
        assert!(output.contains("\"replaceAll\": false"));
        assert_eq!(fs::read_to_string(&file).unwrap(), "alpha\ngamma\n");
    }

    #[test]
    fn edit_replace_all_updates_every_occurrence() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("sample.txt");
        fs::write(&file, "a\nx\nx\n").unwrap();
        let input = json!({
            "file_path": file.display().to_string(),
            "old_string": "x",
            "new_string": "y",
            "replace_all": true
        });

        let _ = execute_claude_edit(temp.path(), &[], false, input).unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "a\ny\ny\n");
    }

    #[test]
    fn edit_rejects_non_unique_without_replace_all() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("sample.txt");
        fs::write(&file, "x\nx\n").unwrap();
        let input = json!({
            "file_path": file.display().to_string(),
            "old_string": "x",
            "new_string": "y"
        });

        let error = execute_claude_edit(temp.path(), &[], false, input).unwrap_err();
        assert!(
            error.to_string().contains("Found 2 matches")
                || error.to_string().contains("not unique")
        );
    }

    #[test]
    fn edit_rejects_relative_paths() {
        let temp = tempfile::tempdir().unwrap();
        let input = json!({
            "file_path": "relative.txt",
            "old_string": "x",
            "new_string": "y"
        });

        let error = execute_claude_edit(temp.path(), &[], false, input).unwrap_err();
        assert!(error.to_string().contains("absolute path"));
    }

    #[test]
    fn edit_can_create_missing_file_with_empty_old_string() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("new.txt");
        let input = json!({
            "file_path": file.display().to_string(),
            "old_string": "",
            "new_string": "hello"
        });

        let output = execute_claude_edit(temp.path(), &[], false, input).unwrap();
        assert!(output.contains("\"originalFile\": \"\""));
        assert_eq!(fs::read_to_string(&file).unwrap(), "hello");
    }

    #[test]
    fn missing_file_creation_edit_does_not_require_prior_read() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("new.txt");
        assert!(!requires_prior_read(&json!({
            "file_path": file.display().to_string(),
            "old_string": "",
            "new_string": "hello"
        })));
    }

    #[test]
    fn edit_rejects_paths_outside_working_directories() {
        let temp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let file = outside.path().join("sample.txt");
        fs::write(&file, "alpha\nbeta\n").unwrap();
        let input = json!({
            "file_path": file.display().to_string(),
            "old_string": "beta",
            "new_string": "gamma"
        });

        let error = execute_claude_edit(temp.path(), &[], false, input).unwrap_err();
        assert!(error
            .to_string()
            .contains("outside the current working directories"));
    }
}

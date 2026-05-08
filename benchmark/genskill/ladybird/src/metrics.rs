//! Metric calculators for replay artifacts.

use crate::replay::{Outcome, ReplayArtifact, ToolCall};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

/// Computed metrics for a single replay.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayMetrics {
    pub passed: bool,
    pub total_tokens: u64,
    pub tool_calls: u64,
    pub duplicate_score: u64,
    pub file_overlap: f32,
    pub symbol_overlap: f32,
}

/// Computes metrics from one artifact and the merged-fix reference diff.
pub fn compute(artifact: &ReplayArtifact, reference_fix: &str) -> ReplayMetrics {
    let passed = matches!(artifact.outcome, Outcome::Pass);
    let duplicate_score = compute_duplicate_score(&artifact.tool_call_log);
    let agent_files = files_in_diff(&artifact.final_diff);
    let ref_files = files_in_diff(reference_fix);
    let file_overlap = jaccard(&agent_files, &ref_files);
    let agent_symbols = symbols_in_diff(&artifact.final_diff);
    let ref_symbols = symbols_in_diff(reference_fix);
    let symbol_overlap = if ref_symbols.is_empty() {
        0.0
    } else {
        agent_symbols.intersection(&ref_symbols).count() as f32 / ref_symbols.len() as f32
    };
    ReplayMetrics {
        passed,
        total_tokens: artifact.tokens.total,
        tool_calls: artifact.tool_calls,
        duplicate_score,
        file_overlap,
        symbol_overlap,
    }
}

fn compute_duplicate_score(log: &[ToolCall]) -> u64 {
    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    for call in log {
        *counts.entry(normalize_key(call)).or_default() += 1;
    }
    counts.values().map(|n| n.saturating_sub(1)).sum()
}

fn normalize_key(call: &ToolCall) -> String {
    match call.name.as_str() {
        "Read" => format!(
            "Read::{}",
            call.input
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("?")
        ),
        "Bash" => {
            let cmd = call
                .input
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            format!(
                "Bash::{}",
                cmd.trim()
                    .to_ascii_lowercase()
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            )
        }
        "Grep" => format!(
            "Grep::{}::{}",
            call.input
                .get("pattern")
                .and_then(|v| v.as_str())
                .unwrap_or(""),
            call.input
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
        ),
        "Edit" | "Write" => format!(
            "{}::{}",
            call.name,
            call.input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("?")
        ),
        other => format!("{other}::{}", call.input),
    }
}

fn files_in_diff(diff: &str) -> HashSet<String> {
    diff.lines()
        .filter_map(|l| {
            l.strip_prefix("+++ b/")
                .or_else(|| l.strip_prefix("--- a/"))
        })
        .map(|s| s.trim().to_string())
        .collect()
}

fn symbols_in_diff(diff: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    for line in diff.lines() {
        if !line.starts_with('+') && !line.starts_with('-') {
            continue;
        }
        if line.starts_with("++") || line.starts_with("--") {
            continue;
        }
        for token in line[1..].split(|c: char| !c.is_alphanumeric() && c != '_') {
            if token.len() >= 4 && !token.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                out.insert(token.to_string());
            }
        }
    }
    out
}

fn jaccard<T: Eq + std::hash::Hash>(a: &HashSet<T>, b: &HashSet<T>) -> f32 {
    let union = a.union(b).count();
    if union == 0 {
        0.0
    } else {
        a.intersection(b).count() as f32 / union as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jaccard_empty_is_zero() {
        let a: HashSet<&str> = HashSet::new();
        assert_eq!(jaccard(&a, &HashSet::new()), 0.0);
    }

    #[test]
    fn jaccard_full_overlap() {
        let a: HashSet<&str> = ["x"].into_iter().collect();
        assert_eq!(jaccard(&a, &["x"].into_iter().collect()), 1.0);
    }

    #[test]
    fn files_in_diff_extracts() {
        let d = "diff --git a/foo.cpp b/foo.cpp\n--- a/foo.cpp\n+++ b/foo.cpp\n";
        assert!(files_in_diff(d).contains("foo.cpp"));
    }

    #[test]
    fn duplicate_score_simple() {
        let log = vec![
            ToolCall {
                name: "Read".into(),
                input: serde_json::json!({"path":"x"}),
                output_size: 0,
                ts: "".into(),
            },
            ToolCall {
                name: "Read".into(),
                input: serde_json::json!({"path":"x"}),
                output_size: 0,
                ts: "".into(),
            },
            ToolCall {
                name: "Read".into(),
                input: serde_json::json!({"path":"y"}),
                output_size: 0,
                ts: "".into(),
            },
        ];
        assert_eq!(compute_duplicate_score(&log), 1);
    }

    #[test]
    fn symbols_extracts_identifiers() {
        let d = "@@ ...\n+ ResolveGridLine(line);\n- oldFunc(x);\n";
        let syms = symbols_in_diff(d);
        assert!(syms.contains("ResolveGridLine"));
        assert!(syms.contains("oldFunc"));
        assert!(!syms.contains("x"));
    }
}

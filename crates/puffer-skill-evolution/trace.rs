//! Extract a structured `ExecutionTrace` from a session transcript.

use crate::{ExecutionTrace, TraceEntry};

/// Minimal transcript event shape this module accepts.
///
/// This crate does not depend on `puffer-session-store::TranscriptEvent`
/// directly, so callers convert session events into this shape.
#[derive(Debug, Clone)]
pub struct TranscriptStep {
    /// Role: `user`, `assistant`, or `tool`.
    pub role: String,
    /// Plain text content of the step.
    pub text: String,
    /// Names of tools invoked in this step.
    pub tool_calls: Vec<String>,
    /// Whether the step indicates failure.
    pub error: bool,
}

/// Builds an `ExecutionTrace` from raw transcript steps.
///
/// Filters out empty user and assistant text turns with no tool calls, keeping
/// steps that meaningfully advanced the task. The `task_summary` is derived
/// from the first non-empty user turn.
pub fn extract_trace(steps: &[TranscriptStep]) -> ExecutionTrace {
    let task_summary = steps
        .iter()
        .find(|step| step.role == "user" && !step.text.trim().is_empty())
        .map(|step| step.text.lines().next().unwrap_or("").to_string())
        .unwrap_or_else(|| "(no user turn found)".to_string());

    let entries = steps
        .iter()
        .filter(|step| {
            !step.tool_calls.is_empty()
                || (step.role == "assistant" && !step.text.trim().is_empty())
        })
        .map(|step| TraceEntry {
            summary: summarize(&step.text),
            tool_calls: step.tool_calls.clone(),
            succeeded: !step.error,
        })
        .collect();

    ExecutionTrace {
        entries,
        task_summary,
    }
}

fn summarize(text: &str) -> String {
    let line = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    if line.len() > 200 {
        format!("{}...", &line[..197])
    } else {
        line.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_trace_empty_returns_placeholder() {
        let trace = extract_trace(&[]);
        assert_eq!(trace.task_summary, "(no user turn found)");
        assert!(trace.entries.is_empty());
    }

    #[test]
    fn extract_trace_filters_empty_turns() {
        let steps = vec![
            TranscriptStep {
                role: "user".into(),
                text: "Fix the build error".into(),
                tool_calls: vec![],
                error: false,
            },
            TranscriptStep {
                role: "assistant".into(),
                text: String::new(),
                tool_calls: vec![],
                error: false,
            },
            TranscriptStep {
                role: "assistant".into(),
                text: "Reading file".into(),
                tool_calls: vec!["read".into()],
                error: false,
            },
            TranscriptStep {
                role: "tool".into(),
                text: "error: file not found".into(),
                tool_calls: vec![],
                error: true,
            },
        ];
        let trace = extract_trace(&steps);
        assert_eq!(trace.task_summary, "Fix the build error");
        assert_eq!(trace.entries.len(), 1);
        assert_eq!(trace.entries[0].tool_calls, vec!["read".to_string()]);
    }

    #[test]
    fn summarize_truncates_long_text() {
        let text = "a".repeat(300);
        let summary = summarize(&text);
        assert_eq!(summary.len(), 200);
        assert!(summary.ends_with("..."));
    }
}

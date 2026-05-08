//! Convert puffer session JSONL transcripts to flat markdown.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs;
use std::io::Write;
use std::path::Path;

#[derive(Debug, Deserialize)]
struct TranscriptLine {
    role: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    tool_call: Option<ToolCall>,
}

#[derive(Debug, Deserialize)]
struct ToolCall {
    name: String,
    #[serde(default)]
    input: Option<serde_json::Value>,
}

/// Reads a JSONL transcript and writes a flat markdown rendering.
pub fn transcript_to_md(input: &Path, output: &Path) -> Result<()> {
    let content =
        fs::read_to_string(input).with_context(|| format!("reading {}", input.display()))?;
    let mut out =
        fs::File::create(output).with_context(|| format!("creating {}", output.display()))?;

    writeln!(out, "# Expert run transcript")?;
    writeln!(out)?;

    for (i, line) in content.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let parsed: TranscriptLine =
            serde_json::from_str(line).with_context(|| format!("parsing line {}", i + 1))?;
        writeln!(out, "## {} (line {})", parsed.role, i + 1)?;
        if let Some(t) = &parsed.text {
            writeln!(out)?;
            writeln!(out, "{}", t.trim())?;
            writeln!(out)?;
        }
        if let Some(tc) = &parsed.tool_call {
            writeln!(out, "**tool_call:** `{}`", tc.name)?;
            if let Some(input_val) = &tc.input {
                writeln!(out, "```json")?;
                writeln!(out, "{}", serde_json::to_string_pretty(input_val)?)?;
                writeln!(out, "```")?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn renders_simple_transcript() {
        let tmp = TempDir::new().unwrap();
        let input = tmp.path().join("in.jsonl");
        let output = tmp.path().join("out.md");
        fs::write(&input, "{\"role\":\"user\",\"text\":\"hi\"}\n{\"role\":\"assistant\",\"text\":\"hello\",\"tool_call\":{\"name\":\"Read\",\"input\":{\"path\":\"/x\"}}}\n").unwrap();
        transcript_to_md(&input, &output).unwrap();
        let result = fs::read_to_string(&output).unwrap();
        assert!(result.contains("## user"));
        assert!(result.contains("hi"));
        assert!(result.contains("**tool_call:** `Read`"));
    }
}

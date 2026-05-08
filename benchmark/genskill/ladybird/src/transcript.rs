//! Convert puffer session JSONL transcripts to flat markdown.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs;
use std::io::Write;
use std::path::Path;

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum TranscriptLine {
    Event(TranscriptEventLine),
    Legacy(LegacyTranscriptLine),
}

#[derive(Debug, Deserialize)]
struct LegacyTranscriptLine {
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

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum TranscriptEventLine {
    UserMessage {
        text: String,
    },
    AssistantMessage {
        text: String,
    },
    SystemMessage {
        text: String,
    },
    ToolInvocation {
        tool_id: String,
        input: String,
        output: String,
        success: bool,
    },
    #[serde(other)]
    Other,
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
        write_line(&mut out, &parsed, i + 1)?;
    }
    Ok(())
}

fn write_line(out: &mut fs::File, parsed: &TranscriptLine, line_number: usize) -> Result<()> {
    match parsed {
        TranscriptLine::Legacy(parsed) => {
            writeln!(out, "## {} (line {})", parsed.role, line_number)?;
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
        TranscriptLine::Event(TranscriptEventLine::UserMessage { text }) => {
            write_text_block(out, "user", line_number, text)?;
        }
        TranscriptLine::Event(TranscriptEventLine::AssistantMessage { text }) => {
            write_text_block(out, "assistant", line_number, text)?;
        }
        TranscriptLine::Event(TranscriptEventLine::SystemMessage { text }) => {
            write_text_block(out, "system", line_number, text)?;
        }
        TranscriptLine::Event(TranscriptEventLine::ToolInvocation {
            tool_id,
            input,
            output,
            success,
        }) => {
            writeln!(out, "## tool (line {})", line_number)?;
            writeln!(out)?;
            writeln!(out, "**tool_call:** `{}`", tool_id)?;
            writeln!(out)?;
            writeln!(out, "**success:** `{}`", success)?;
            writeln!(out)?;
            writeln!(out, "```json")?;
            writeln!(out, "{}", pretty_json_or_raw(input)?)?;
            writeln!(out, "```")?;
            writeln!(out)?;
            writeln!(out, "```text")?;
            writeln!(out, "{}", trim_for_markdown(output))?;
            writeln!(out, "```")?;
            writeln!(out)?;
        }
        TranscriptLine::Event(TranscriptEventLine::Other) => {}
    }
    Ok(())
}

fn write_text_block(out: &mut fs::File, role: &str, line_number: usize, text: &str) -> Result<()> {
    writeln!(out, "## {} (line {})", role, line_number)?;
    writeln!(out)?;
    writeln!(out, "{}", text.trim())?;
    writeln!(out)?;
    Ok(())
}

fn pretty_json_or_raw(value: &str) -> Result<String> {
    let parsed = serde_json::from_str::<serde_json::Value>(value);
    Ok(match parsed {
        Ok(parsed) => serde_json::to_string_pretty(&parsed)?,
        Err(_) => value.to_string(),
    })
}

fn trim_for_markdown(value: &str) -> String {
    const LIMIT: usize = 4_000;
    if value.len() <= LIMIT {
        value.trim().to_string()
    } else {
        let boundary = value
            .char_indices()
            .map(|(index, _)| index)
            .take_while(|index| *index <= LIMIT)
            .last()
            .unwrap_or(0);
        format!("{}\n...[truncated]", value[..boundary].trim())
    }
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

    #[test]
    fn renders_puffer_session_events() {
        let tmp = TempDir::new().unwrap();
        let input = tmp.path().join("in.jsonl");
        let output = tmp.path().join("out.md");
        fs::write(&input, "{\"type\":\"user_message\",\"text\":\"fix it\"}\n{\"type\":\"tool_invocation\",\"call_id\":\"1\",\"tool_id\":\"Read\",\"input\":\"{\\\"path\\\":\\\"foo.cpp\\\"}\",\"output\":\"contents\",\"success\":true}\n").unwrap();
        transcript_to_md(&input, &output).unwrap();
        let result = fs::read_to_string(&output).unwrap();
        assert!(result.contains("## user"));
        assert!(result.contains("fix it"));
        assert!(result.contains("**tool_call:** `Read`"));
        assert!(result.contains("\"path\": \"foo.cpp\""));
        assert!(result.contains("contents"));
    }
}

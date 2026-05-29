use crate::AppState;
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::Value;
use std::path::Path;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TouchDesignerActionInput {
    action: String,
    #[serde(default)]
    code: Option<String>,
}

/// Executes one TouchDesigner verifier action backed by Lambda Skill contracts.
pub fn execute_touchdesigner_action(
    _state: &mut AppState,
    _cwd: &Path,
    input: Value,
) -> Result<String> {
    let parsed: TouchDesignerActionInput =
        serde_json::from_value(input).context("invalid TouchDesignerAction input")?;
    match parsed.action.as_str() {
        "validateScript" => validate_script_action(parsed),
        other => bail!("unsupported TouchDesignerAction action `{other}`"),
    }
}

fn validate_script_action(input: TouchDesignerActionInput) -> Result<String> {
    let code = required_code(input.code)?;
    validate_no_hardcoded_absolute_paths(&code)?;
    validate_native_tool_preferred(&code)?;
    Ok(serde_json::to_string(&code)?)
}

fn required_code(value: Option<String>) -> Result<String> {
    let Some(code) = value else {
        bail!("TouchDesignerAction validateScript requires code");
    };
    if code.trim().is_empty() {
        bail!("TouchDesignerAction code must be non-empty");
    }
    Ok(code)
}

fn validate_no_hardcoded_absolute_paths(code: &str) -> Result<()> {
    for literal in quoted_string_literals(code) {
        if looks_like_absolute_path(&literal) {
            bail!(
                "TouchDesignerAction rejects hardcoded absolute path literal `{}`; use me.parent() or scriptOp.parent() for TouchDesigner callbacks",
                literal
            );
        }
    }
    Ok(())
}

fn validate_native_tool_preferred(code: &str) -> Result<()> {
    let normalized = code
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect::<Vec<_>>();
    let joined = normalized.join("\n");
    let complex_markers = [
        "\nfor ", "\nwhile ", "\nif ", "\ndef ", "\nclass ", " try:", "\ntry:", " with ",
        "\nwith ", "lambda ",
    ];
    let has_complex_marker = complex_markers
        .iter()
        .any(|marker| joined.starts_with(marker.trim_start()) || joined.contains(marker));
    if has_complex_marker {
        return Ok(());
    }

    let native_equivalents = [
        ".par.",
        ".create(",
        ".destroy(",
        ".outputConnectors",
        ".inputConnectors",
        ".connect(",
        "op(",
    ];
    if native_equivalents
        .iter()
        .any(|marker| joined.contains(marker))
    {
        bail!(
            "TouchDesignerAction rejects simple td_execute_python scripts that should use native TouchDesigner MCP tools first"
        );
    }
    Ok(())
}

fn looks_like_absolute_path(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.starts_with('/') || trimmed.starts_with("~/") || trimmed.starts_with("\\\\") {
        return true;
    }
    let mut chars = trimmed.chars();
    matches!(
        (chars.next(), chars.next(), chars.next()),
        (Some(drive), Some(':'), Some('\\' | '/')) if drive.is_ascii_alphabetic()
    )
}

fn quoted_string_literals(code: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = code.char_indices().peekable();
    while let Some((_, ch)) = chars.next() {
        if ch != '\'' && ch != '"' {
            continue;
        }
        let quote = ch;
        let mut triple = false;
        if matches!(chars.peek(), Some((_, next)) if *next == quote) {
            let mut probe = chars.clone();
            probe.next();
            if matches!(probe.peek(), Some((_, next)) if *next == quote) {
                chars.next();
                chars.next();
                triple = true;
            }
        }
        let mut value = String::new();
        let mut escaped = false;
        let mut close_run = 0usize;
        while let Some((_, inner)) = chars.next() {
            if escaped {
                value.push(inner);
                escaped = false;
                continue;
            }
            if inner == '\\' {
                escaped = true;
                continue;
            }
            if inner == quote {
                if !triple {
                    break;
                }
                close_run += 1;
                if close_run == 3 {
                    let keep = value.len().saturating_sub(2);
                    value.truncate(keep);
                    break;
                }
                value.push(inner);
                continue;
            }
            close_run = 0;
            value.push(inner);
        }
        out.push(value);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_complex_relative_touchdesigner_script() {
        let output = validate_script_action(TouchDesignerActionInput {
            action: "validateScript".to_string(),
            code: Some(
                "root = me.parent()\ncreated = []\nfor name in ['bg', 'out']:\n    node = root.op(name)\n    if node:\n        created.append(node.path)\nresult = {'created': created}".to_string(),
            ),
        })
        .unwrap();
        assert!(output.contains("me.parent()"));
        assert!(!output.contains("/project1"));
    }

    #[test]
    fn rejects_hardcoded_touchdesigner_paths() {
        let error = validate_script_action(TouchDesignerActionInput {
            action: "validateScript".to_string(),
            code: Some("root = op('/project1')\nresult = root.path".to_string()),
        })
        .unwrap_err();
        assert!(error.to_string().contains("absolute path"));
    }

    #[test]
    fn rejects_obvious_native_tool_replacements() {
        let error = validate_script_action(TouchDesignerActionInput {
            action: "validateScript".to_string(),
            code: Some("op('bg').par.brightness = 0.5\nresult = {'ok': True}".to_string()),
        })
        .unwrap_err();
        assert!(error.to_string().contains("native TouchDesigner MCP tools"));
    }
}

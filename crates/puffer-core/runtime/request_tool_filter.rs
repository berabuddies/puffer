use crate::tool_names::{canonical_tool_name, tool_definition_matches_selector};
use anyhow::{anyhow, Result};
use glob::Pattern;
use puffer_tools::ToolDefinition;
use serde_json::Value;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

/// Stores one request-scoped tool allowlist derived from slash-command metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RequestToolFilter {
    rules: Vec<RequestToolRule>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RequestToolRule {
    tool_id: String,
    matcher: ToolRuleMatcher,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ToolRuleMatcher {
    Any,
    BashPrefix(Vec<String>),
    PathGlob(String),
}

impl RequestToolFilter {
    pub(crate) fn empty_static() -> &'static Self {
        static EMPTY: OnceLock<RequestToolFilter> = OnceLock::new();
        EMPTY.get_or_init(|| Self { rules: Vec::new() })
    }

    /// Returns true when a tool definition should be exposed to the model.
    pub(crate) fn allows_definition(&self, definition: &ToolDefinition) -> bool {
        self.rules
            .iter()
            .any(|rule| tool_definition_matches_selector(definition, &rule.tool_id))
    }

    /// Returns true when a concrete tool invocation stays within the request scope.
    pub(crate) fn allows_call(
        &self,
        definition: &ToolDefinition,
        cwd: &Path,
        input: &Value,
    ) -> Result<bool> {
        let matching_rules = self
            .rules
            .iter()
            .filter(|rule| tool_definition_matches_selector(definition, &rule.tool_id))
            .collect::<Vec<_>>();
        if matching_rules.is_empty() {
            return Ok(false);
        }
        for rule in matching_rules {
            if matcher_matches(&rule.matcher, definition, cwd, input)? {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

/// Builds a request-scoped tool allowlist from declarative prompt metadata.
pub(crate) fn build_request_tool_filter(entries: &[String]) -> Result<Option<RequestToolFilter>> {
    let rules = entries
        .iter()
        .map(|entry| parse_rule(entry))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .filter(|rule| !rule.tool_id.is_empty())
        .collect::<Vec<_>>();
    if rules.is_empty() {
        Ok(None)
    } else {
        Ok(Some(RequestToolFilter { rules }))
    }
}

fn parse_rule(entry: &str) -> Result<RequestToolRule> {
    let trimmed = entry.trim();
    if trimmed.is_empty() {
        return Ok(RequestToolRule {
            tool_id: String::new(),
            matcher: ToolRuleMatcher::Any,
        });
    }
    if let Some((tool, rest)) = trimmed.split_once('(') {
        let inner = rest
            .strip_suffix(')')
            .ok_or_else(|| anyhow!("invalid allowed tool entry `{trimmed}`"))?;
        let tool_id = canonical_tool_name(tool);
        let matcher = if tool_id == "bash" {
            ToolRuleMatcher::BashPrefix(parse_bash_prefix(inner)?)
        } else {
            ToolRuleMatcher::PathGlob(inner.trim().to_string())
        };
        return Ok(RequestToolRule { tool_id, matcher });
    }
    Ok(RequestToolRule {
        tool_id: canonical_tool_name(trimmed),
        matcher: ToolRuleMatcher::Any,
    })
}

fn parse_bash_prefix(raw: &str) -> Result<Vec<String>> {
    let trimmed = raw.trim();
    let prefix = trimmed
        .strip_suffix(":*")
        .or_else(|| trimmed.strip_suffix('*'))
        .unwrap_or(trimmed)
        .trim();
    let tokens = prefix
        .split_whitespace()
        .map(str::to_string)
        .collect::<Vec<_>>();
    if tokens.is_empty() {
        return Err(anyhow!("invalid Bash allowed tool selector `{raw}`"));
    }
    Ok(tokens)
}

fn matcher_matches(
    matcher: &ToolRuleMatcher,
    definition: &ToolDefinition,
    cwd: &Path,
    input: &Value,
) -> Result<bool> {
    match matcher {
        ToolRuleMatcher::Any => Ok(true),
        ToolRuleMatcher::BashPrefix(prefix) => Ok(input
            .get("command")
            .and_then(Value::as_str)
            .is_some_and(|command| bash_prefix_matches(prefix, command))),
        ToolRuleMatcher::PathGlob(pattern) => {
            let Some(raw_path) = extract_tool_path(definition, input) else {
                return Ok(false);
            };
            path_matches(pattern, cwd, raw_path)
        }
    }
}

fn bash_prefix_matches(prefix: &[String], command: &str) -> bool {
    let command_tokens = command
        .split_whitespace()
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>();
    prefix.len() <= command_tokens.len()
        && prefix
            .iter()
            .map(|token| token.to_ascii_lowercase())
            .zip(command_tokens.iter())
            .all(|(expected, actual)| expected == *actual)
}

fn extract_tool_path<'a>(definition: &ToolDefinition, input: &'a Value) -> Option<&'a str> {
    match canonical_tool_name(definition.id.as_str()).as_str() {
        "read" | "edit" | "write" => input.get("file_path").and_then(Value::as_str),
        "notebookedit" => input.get("notebook_path").and_then(Value::as_str),
        "glob" | "grep" => input.get("path").and_then(Value::as_str),
        other if other == "readmcpresourcetool" => input.get("uri").and_then(Value::as_str),
        _ => input
            .get("path")
            .and_then(Value::as_str)
            .or_else(|| input.get("file_path").and_then(Value::as_str)),
    }
}

fn path_matches(pattern: &str, cwd: &Path, raw_path: &str) -> Result<bool> {
    let compiled = Pattern::new(&normalize_path_glob(pattern))?;
    let candidate = normalize_path_string(resolve_path_for_match(cwd, raw_path));
    Ok(compiled.matches(&candidate))
}

fn resolve_path_for_match(cwd: &Path, raw_path: &str) -> PathBuf {
    let expanded = expand_home(raw_path);
    let candidate = Path::new(&expanded);
    if candidate.is_absolute() {
        normalize_path(candidate)
    } else {
        normalize_path(&cwd.join(candidate))
    }
}

fn normalize_path_glob(raw: &str) -> String {
    normalize_path_string(normalize_path(Path::new(&expand_home(raw))))
}

fn normalize_path_string(path: PathBuf) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn expand_home(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed == "~" {
        return std::env::var("HOME").unwrap_or_else(|_| trimmed.to_string());
    }
    if let Some(rest) = trimmed.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{home}/{rest}");
        }
    }
    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_tools::{
        ToolDefinition, ToolDisplayHints, ToolInputSchema, ToolKind, ToolMetadata, ToolPolicyHints,
    };
    use serde_json::json;

    fn tool(id: &str) -> ToolDefinition {
        ToolDefinition {
            id: id.to_string(),
            name: id.to_string(),
            description: id.to_string(),
            handler: "runtime:test".to_string(),
            aliases: Vec::new(),
            handler_args: Vec::new(),
            kind: ToolKind::Custom,
            input_schema: ToolInputSchema::default(),
            metadata: ToolMetadata::default(),
            policy: ToolPolicyHints::default(),
            shared_lib: None,
            enabled_if: None,
            display: ToolDisplayHints::default(),
        }
    }

    #[test]
    fn filter_maps_task_and_ls_aliases() {
        let filter = build_request_tool_filter(&["Task".to_string(), "LS".to_string()])
            .unwrap()
            .unwrap();
        assert!(filter.allows_definition(&tool("Agent")));
        assert!(filter.allows_definition(&tool("Glob")));
        assert!(!filter.allows_definition(&tool("Read")));
    }

    #[test]
    fn filter_maps_provider_specific_and_legacy_aliases() {
        let cwd = Path::new("/tmp/work");
        let filter = build_request_tool_filter(&[
            "read_file(/tmp/work/**)".to_string(),
            "replace_in_file(/tmp/work/.claude/settings.json)".to_string(),
            "Brief".to_string(),
            "AgentOutputTool".to_string(),
            "KillShell".to_string(),
        ])
        .unwrap()
        .unwrap();
        assert!(filter.allows_definition(&tool("Read")));
        assert!(filter.allows_definition(&tool("Edit")));
        assert!(filter.allows_definition(&tool("SendUserMessage")));
        assert!(filter.allows_definition(&tool("TaskOutput")));
        assert!(filter.allows_definition(&tool("TaskStop")));
        assert!(!filter.allows_definition(&tool("Write")));
        assert!(filter
            .allows_call(
                &tool("Read"),
                cwd,
                &json!({"file_path": "/tmp/work/docs/guide.md"})
            )
            .unwrap());
        assert!(filter
            .allows_call(
                &tool("Edit"),
                cwd,
                &json!({"file_path": "/tmp/work/.claude/settings.json"})
            )
            .unwrap());
    }

    #[test]
    fn filter_matches_bash_prefix_tokens() {
        let filter = build_request_tool_filter(&["Bash(git diff:*)".to_string()])
            .unwrap()
            .unwrap();
        assert!(filter
            .allows_call(
                &tool("Bash"),
                Path::new("/tmp/work"),
                &json!({"command": "git diff --name-only origin/HEAD..."})
            )
            .unwrap());
        assert!(!filter
            .allows_call(
                &tool("Bash"),
                Path::new("/tmp/work"),
                &json!({"command": "git status"})
            )
            .unwrap());
    }

    #[test]
    fn filter_matches_path_globs_against_absolute_and_relative_paths() {
        let cwd = Path::new("/tmp/work");
        let filter = build_request_tool_filter(&[
            "Read(/tmp/work/**)".to_string(),
            "Edit(/tmp/work/.claude/settings.json)".to_string(),
        ])
        .unwrap()
        .unwrap();
        assert!(filter
            .allows_call(
                &tool("Read"),
                cwd,
                &json!({"file_path": "/tmp/work/docs/guide.md"})
            )
            .unwrap());
        assert!(filter
            .allows_call(
                &tool("Edit"),
                cwd,
                &json!({"file_path": ".claude/settings.json"})
            )
            .unwrap());
        assert!(!filter
            .allows_call(
                &tool("Edit"),
                cwd,
                &json!({"file_path": "/tmp/work/src/main.rs"})
            )
            .unwrap());
    }
}

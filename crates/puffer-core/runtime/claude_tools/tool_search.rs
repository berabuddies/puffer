use anyhow::{bail, Result};
use puffer_tools::{ToolDefinition, ToolRegistry};
use serde::Deserialize;
use serde_json::{json, Value};
use std::cmp::Reverse;
use std::collections::HashSet;
use std::fmt::Write as _;

const DEFAULT_MAX_RESULTS: usize = 5;
const MAX_RESULTS_LIMIT: usize = 20;
const DEFERRED_TOOL_NAMES: &[&str] = &[
    "AskUserQuestion",
    "Config",
    "CronCreate",
    "CronDelete",
    "CronList",
    "EnterPlanMode",
    "EnterWorktree",
    "ExitPlanMode",
    "ExitWorktree",
    "ListMcpResourcesTool",
    "LSP",
    "NotebookEdit",
    "ProcessControl",
    "ReadMcpResourceTool",
    "RemoteTrigger",
    "SendMessage",
    "Sleep",
    "TaskCreate",
    "TaskGet",
    "TaskList",
    "TaskOutput",
    "TaskStop",
    "TaskUpdate",
    "TeamCreate",
    "TeamDelete",
    "TodoWrite",
    "WebFetch",
    "WebSearch",
];

#[derive(Debug, Deserialize)]
struct ToolSearchInput {
    query: String,
    #[serde(default)]
    max_results: Option<u64>,
}

/// Returns true when the tool should be treated as deferred for Claude-style
/// `ToolSearch` lookup semantics.
pub fn is_claude_deferred_tool(definition: &ToolDefinition) -> bool {
    if definition.id.eq_ignore_ascii_case("ToolSearch") {
        return false;
    }
    if definition.id.starts_with("mcp__") {
        return true;
    }
    DEFERRED_TOOL_NAMES
        .iter()
        .any(|name| definition.id.eq_ignore_ascii_case(name))
}

/// Executes Claude-style `ToolSearch` over deferred tools and returns matching
/// schemas encoded inside a `<functions>` block.
pub fn execute_claude_tool_search_tool(registry: &ToolRegistry, input: Value) -> Result<String> {
    let parsed: ToolSearchInput = serde_json::from_value(input)?;
    let query = parsed.query.trim();
    if query.is_empty() {
        bail!("ToolSearch query cannot be empty");
    }
    let max_results = parsed
        .max_results
        .unwrap_or(DEFAULT_MAX_RESULTS as u64)
        .clamp(1, MAX_RESULTS_LIMIT as u64) as usize;

    let definitions = registry.definitions().collect::<Vec<_>>();
    let deferred = definitions
        .iter()
        .copied()
        .filter(|definition| is_claude_deferred_tool(definition))
        .collect::<Vec<_>>();

    let matches = if let Some(selection) = query.strip_prefix("select:") {
        resolve_select_matches(selection, &deferred, &definitions, max_results)
    } else {
        resolve_keyword_matches(query, &deferred, max_results)
    };

    let mut output = String::from("<functions>\n");
    for definition in matches {
        let payload = json!({
            "name": definition.id,
            "description": definition.description,
            "parameters": definition.input_schema.as_json_schema(),
        });
        let _ = writeln!(&mut output, "<function>{payload}</function>");
    }
    output.push_str("</functions>");
    Ok(output)
}

fn resolve_select_matches<'a>(
    selection: &str,
    deferred: &[&'a ToolDefinition],
    all: &[&'a ToolDefinition],
    max_results: usize,
) -> Vec<&'a ToolDefinition> {
    let mut selected = Vec::new();
    let mut seen = HashSet::<String>::new();
    for requested in selection
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let found = find_tool_case_insensitive(deferred, requested)
            .or_else(|| find_tool_case_insensitive(all, requested));
        if let Some(definition) = found {
            let key = definition.id.to_ascii_lowercase();
            if seen.insert(key) {
                selected.push(definition);
                if selected.len() >= max_results {
                    break;
                }
            }
        }
    }
    selected
}

fn resolve_keyword_matches<'a>(
    query: &str,
    deferred: &[&'a ToolDefinition],
    max_results: usize,
) -> Vec<&'a ToolDefinition> {
    let query_lower = query.to_ascii_lowercase();
    if let Some(exact) = find_tool_case_insensitive(deferred, &query_lower) {
        return vec![exact];
    }

    let terms = query_lower
        .split_whitespace()
        .map(str::to_string)
        .collect::<Vec<_>>();
    let mut required = Vec::new();
    let mut optional = Vec::new();
    for term in &terms {
        if let Some(stripped) = term.strip_prefix('+') {
            if !stripped.is_empty() {
                required.push(stripped.to_string());
            }
        } else {
            optional.push(term.clone());
        }
    }
    let scoring_terms = if required.is_empty() {
        terms
    } else {
        required
            .iter()
            .cloned()
            .chain(optional.iter().cloned())
            .collect()
    };

    let mut scored = deferred
        .iter()
        .copied()
        .filter_map(|definition| {
            let parts = collect_name_parts(definition);
            let description = definition.description.to_ascii_lowercase();
            let required_match = required.iter().all(|term| {
                parts.iter().any(|part| part.contains(term)) || description.contains(term)
            });
            if !required_match {
                return None;
            }
            let score = scoring_terms
                .iter()
                .map(|term| score_term(definition, &parts, &description, term))
                .sum::<u32>();
            (score > 0).then_some((score, definition))
        })
        .collect::<Vec<_>>();

    scored.sort_by_key(|(score, definition)| (Reverse(*score), definition.id.to_ascii_lowercase()));
    scored
        .into_iter()
        .take(max_results)
        .map(|(_, definition)| definition)
        .collect()
}

fn find_tool_case_insensitive<'a>(
    definitions: &[&'a ToolDefinition],
    wanted: &str,
) -> Option<&'a ToolDefinition> {
    definitions
        .iter()
        .copied()
        .find(|definition| definition.id.eq_ignore_ascii_case(wanted))
}

fn collect_name_parts(definition: &ToolDefinition) -> Vec<String> {
    split_identifier(&definition.id)
        .into_iter()
        .chain(split_identifier(&definition.name))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
}

fn split_identifier(value: &str) -> Vec<String> {
    let mut normalized = String::new();
    let mut previous_was_lower = false;
    for ch in value.chars() {
        if ch == '_' || ch == '-' {
            normalized.push(' ');
            previous_was_lower = false;
            continue;
        }
        if ch.is_ascii_uppercase() && previous_was_lower {
            normalized.push(' ');
        }
        normalized.push(ch.to_ascii_lowercase());
        previous_was_lower = ch.is_ascii_lowercase() || ch.is_ascii_digit();
    }
    normalized
        .split_whitespace()
        .map(str::to_string)
        .collect::<Vec<_>>()
}

fn score_term(definition: &ToolDefinition, parts: &[String], description: &str, term: &str) -> u32 {
    let mut score = 0;
    if definition.id.eq_ignore_ascii_case(term) || definition.name.eq_ignore_ascii_case(term) {
        score += 10;
    }
    if parts.iter().any(|part| part == term) {
        score += 6;
    } else if parts.iter().any(|part| part.contains(term)) {
        score += 3;
    }
    if description.contains(term) {
        score += 2;
    }
    score
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_resources::{LoadedItem, LoadedResources, SourceInfo, SourceKind, ToolSpec};
    use serde_json::json;

    fn sample_registry() -> ToolRegistry {
        ToolRegistry::from_resources(&LoadedResources {
            tools: vec![
                LoadedItem {
                    value: ToolSpec {
                        id: "Read".to_string(),
                        name: "Read".to_string(),
                        description: "Read file contents".to_string(),
                        handler: "builtin:read_file".to_string(),
                        ..ToolSpec::default()
                    },
                    source_info: SourceInfo {
                        path: "tools/claude_read.yaml".into(),
                        kind: SourceKind::Builtin,
                    },
                },
                LoadedItem {
                    value: ToolSpec {
                        id: "ToolSearch".to_string(),
                        name: "ToolSearch".to_string(),
                        description: "Search deferred tools".to_string(),
                        handler: "runtime:tool_search".to_string(),
                        ..ToolSpec::default()
                    },
                    source_info: SourceInfo {
                        path: "tools/tool_search.yaml".into(),
                        kind: SourceKind::Builtin,
                    },
                },
                LoadedItem {
                    value: ToolSpec {
                        id: "NotebookEdit".to_string(),
                        name: "NotebookEdit".to_string(),
                        description: "Edit notebook cells".to_string(),
                        handler: "runtime:notebook_edit".to_string(),
                        ..ToolSpec::default()
                    },
                    source_info: SourceInfo {
                        path: "tools/notebook_edit.yaml".into(),
                        kind: SourceKind::Builtin,
                    },
                },
                LoadedItem {
                    value: ToolSpec {
                        id: "WebSearch".to_string(),
                        name: "WebSearch".to_string(),
                        description: "Search the web".to_string(),
                        handler: "provider:web_search".to_string(),
                        ..ToolSpec::default()
                    },
                    source_info: SourceInfo {
                        path: "tools/web_search.yaml".into(),
                        kind: SourceKind::Builtin,
                    },
                },
            ],
            ..LoadedResources::default()
        })
    }

    #[test]
    fn select_query_can_return_non_deferred_exact_match() {
        let output =
            execute_claude_tool_search_tool(&sample_registry(), json!({"query": "select:Read"}))
                .unwrap();
        assert!(output.contains("\"name\":\"Read\""));
    }

    #[test]
    fn keyword_query_filters_to_deferred_tools() {
        let output =
            execute_claude_tool_search_tool(&sample_registry(), json!({"query": "notebook"}))
                .unwrap();
        assert!(output.contains("\"name\":\"NotebookEdit\""));
        assert!(!output.contains("\"name\":\"Read\""));
    }

    #[test]
    fn mcp_prefixed_tools_are_deferred() {
        let registry = ToolRegistry::from_resources(&LoadedResources {
            tools: vec![LoadedItem {
                value: ToolSpec {
                    id: "mcp__github__issues_list".to_string(),
                    name: "mcp__github__issues_list".to_string(),
                    description: "List GitHub issues".to_string(),
                    handler: "exec".to_string(),
                    handler_args: vec!["/bin/true".to_string()],
                    ..ToolSpec::default()
                },
                source_info: SourceInfo {
                    path: "tools/mcp_github_issues_list.yaml".into(),
                    kind: SourceKind::Builtin,
                },
            }],
            ..LoadedResources::default()
        });
        let output =
            execute_claude_tool_search_tool(&registry, json!({"query": "github issues"})).unwrap();
        assert!(output.contains("mcp__github__issues_list"));
    }

    #[test]
    fn empty_query_is_rejected() {
        let error = execute_claude_tool_search_tool(&sample_registry(), json!({"query": "   "}))
            .unwrap_err()
            .to_string();
        assert!(error.contains("cannot be empty"));
    }
}

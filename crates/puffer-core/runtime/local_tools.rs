use anyhow::{anyhow, bail, Result};
use glob::Pattern;
use puffer_resources::{plugin_mcp_servers, LoadedResources};
use puffer_tools::{ToolDefinition, ToolRegistry};
use serde::Deserialize;
use serde_json::{json, Value};
use std::cmp::Reverse;
use std::fmt::Write as _;
use std::fs;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Deserialize)]
struct SkillInput {
    skill: String,
    #[serde(default)]
    args: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ToolSearchInput {
    query: String,
    #[serde(default)]
    max_results: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct GlobInput {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NotebookEditInput {
    notebook_path: String,
    cell_number: usize,
    #[serde(default)]
    new_source: String,
    #[serde(default)]
    cell_type: Option<String>,
    #[serde(default)]
    edit_mode: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ListMcpInput {
    #[serde(default)]
    server: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ReadMcpInput {
    server: String,
    uri: String,
}

#[derive(Debug, Clone)]
struct McpResourceRecord {
    uri: String,
    server: String,
    name: String,
    description: String,
    mime_type: String,
    text: String,
}

pub(super) fn is_runtime_local_tool(definition: &ToolDefinition) -> bool {
    matches!(
        definition.handler.as_str(),
        "runtime:skill"
            | "runtime:tool_search"
            | "runtime:glob"
            | "runtime:notebook_edit"
            | "runtime:list_mcp_resources"
            | "runtime:read_mcp_resource"
    )
}

pub(super) fn execute_runtime_local_tool(
    resources: &LoadedResources,
    registry: &ToolRegistry,
    definition: &ToolDefinition,
    cwd: &Path,
    input: Value,
) -> Result<String> {
    match definition.handler.as_str() {
        "runtime:skill" => execute_skill_tool(resources, input),
        "runtime:tool_search" => execute_tool_search(registry, input),
        "runtime:glob" => execute_glob_tool(cwd, input),
        "runtime:notebook_edit" => execute_notebook_edit(cwd, input),
        "runtime:list_mcp_resources" => execute_list_mcp_resources(resources, input),
        "runtime:read_mcp_resource" => execute_read_mcp_resource(resources, input),
        other => Err(anyhow!("unsupported runtime-local tool handler {other}")),
    }
}

fn execute_skill_tool(resources: &LoadedResources, input: Value) -> Result<String> {
    let input: SkillInput = serde_json::from_value(input)?;
    let skill = resources
        .skills
        .iter()
        .find(|skill| skill.value.name == input.skill)
        .or_else(|| {
            resources
                .skills
                .iter()
                .find(|skill| skill.value.name.eq_ignore_ascii_case(&input.skill))
        })
        .ok_or_else(|| anyhow!("unknown skill `{}`", input.skill))?;

    if skill.value.disable_model_invocation {
        bail!(
            "skill `{}` disables model invocation and cannot be loaded through the Skill tool",
            skill.value.name
        );
    }

    let mut output = String::new();
    let _ = writeln!(&mut output, "Skill {}", skill.value.name);
    let _ = writeln!(&mut output, "{}", skill.value.description);
    if let Some(args) = input
        .args
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        let _ = writeln!(&mut output, "args: {}", args.trim());
    }
    let _ = writeln!(
        &mut output,
        "\n<skill name=\"{}\">\n{}\n</skill>",
        skill.value.name,
        skill.value.content.trim()
    );
    Ok(output.trim().to_string())
}

fn execute_tool_search(registry: &ToolRegistry, input: Value) -> Result<String> {
    let input: ToolSearchInput = serde_json::from_value(input)?;
    let max_results = input.max_results.unwrap_or(5).clamp(1, 20) as usize;
    let query = input.query.trim();
    if query.is_empty() {
        bail!("ToolSearch query cannot be empty");
    }

    let definitions = if let Some(selection) = query.strip_prefix("select:") {
        let wanted = selection
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .collect::<Vec<_>>();
        registry
            .definitions()
            .filter(|definition| {
                wanted
                    .iter()
                    .any(|name| definition.id.eq_ignore_ascii_case(name))
            })
            .take(max_results)
            .collect::<Vec<_>>()
    } else {
        let terms = query
            .to_ascii_lowercase()
            .split_whitespace()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let mut scored = registry
            .definitions()
            .filter(|definition| definition.id != "ToolSearch")
            .map(|definition| {
                let haystack = format!(
                    "{} {} {}",
                    definition.id, definition.name, definition.description
                )
                .to_ascii_lowercase();
                let score = terms
                    .iter()
                    .map(|term| {
                        if definition.id.eq_ignore_ascii_case(term) {
                            10
                        } else if haystack.contains(term) {
                            1
                        } else {
                            0
                        }
                    })
                    .sum::<u32>();
                (score, definition)
            })
            .filter(|(score, _)| *score > 0)
            .collect::<Vec<_>>();
        scored.sort_by_key(|(score, definition)| {
            (Reverse(*score), definition.id.to_ascii_lowercase())
        });
        scored
            .into_iter()
            .take(max_results)
            .map(|(_, definition)| definition)
            .collect::<Vec<_>>()
    };

    let mut output = String::from("<functions>\n");
    for definition in definitions {
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

fn execute_glob_tool(cwd: &Path, input: Value) -> Result<String> {
    let input: GlobInput = serde_json::from_value(input)?;
    let pattern = Pattern::new(&input.pattern)
        .map_err(|error| anyhow!("invalid glob pattern `{}`: {error}", input.pattern))?;
    let root = input
        .path
        .as_deref()
        .map(|path| resolve_workspace_path(cwd, Path::new(path)))
        .transpose()?
        .unwrap_or_else(|| cwd.to_path_buf());
    let mut matches = Vec::new();
    collect_glob_matches(cwd, &root, &pattern, &mut matches)?;
    matches.sort();
    Ok(serde_json::to_string_pretty(&matches)?)
}

fn execute_notebook_edit(cwd: &Path, input: Value) -> Result<String> {
    let input: NotebookEditInput = serde_json::from_value(input)?;
    let edit_mode = input.edit_mode.as_deref().unwrap_or("replace");
    let notebook_path = resolve_workspace_path(cwd, Path::new(&input.notebook_path))?;
    if notebook_path.extension().and_then(|ext| ext.to_str()) != Some("ipynb") {
        bail!("NotebookEdit requires a .ipynb file");
    }
    let raw = fs::read_to_string(&notebook_path)?;
    let mut notebook: Value = serde_json::from_str(&raw)?;
    let cells = notebook
        .get_mut("cells")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| anyhow!("notebook is missing a cells array"))?;

    match edit_mode {
        "replace" => {
            let cell = cells
                .get_mut(input.cell_number)
                .ok_or_else(|| anyhow!("cell {} not found", input.cell_number))?;
            let cell_type = input
                .cell_type
                .clone()
                .or_else(|| {
                    cell.get("cell_type")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .unwrap_or_else(|| "code".to_string());
            rewrite_notebook_cell(cell, &cell_type, &input.new_source)?;
        }
        "insert" => {
            let index = input.cell_number.min(cells.len());
            cells.insert(
                index,
                make_notebook_cell(input.cell_type.as_deref(), &input.new_source),
            );
        }
        "delete" => {
            if input.cell_number >= cells.len() {
                bail!("cell {} not found", input.cell_number);
            }
            cells.remove(input.cell_number);
        }
        other => bail!("unsupported notebook edit_mode `{other}`"),
    }

    let updated = serde_json::to_string_pretty(&notebook)?;
    fs::write(&notebook_path, &updated)?;
    Ok(serde_json::to_string_pretty(&json!({
        "notebook_path": notebook_path,
        "cell_number": input.cell_number,
        "edit_mode": edit_mode,
    }))?)
}

fn execute_list_mcp_resources(resources: &LoadedResources, input: Value) -> Result<String> {
    let input: ListMcpInput = serde_json::from_value(input)?;
    let records = collect_mcp_resource_records(resources);
    let filtered = records
        .into_iter()
        .filter(|record| {
            input
                .server
                .as_deref()
                .is_none_or(|server| record.server == server)
        })
        .map(|record| {
            json!({
                "uri": record.uri,
                "name": record.name,
                "mimeType": record.mime_type,
                "description": record.description,
                "server": record.server,
            })
        })
        .collect::<Vec<_>>();
    Ok(serde_json::to_string_pretty(&filtered)?)
}

fn execute_read_mcp_resource(resources: &LoadedResources, input: Value) -> Result<String> {
    let input: ReadMcpInput = serde_json::from_value(input)?;
    let record = collect_mcp_resource_records(resources)
        .into_iter()
        .find(|record| record.server == input.server && record.uri == input.uri)
        .ok_or_else(|| {
            anyhow!(
                "MCP resource `{}` for server `{}` not found",
                input.uri,
                input.server
            )
        })?;
    Ok(serde_json::to_string_pretty(&json!({
        "contents": [
            {
                "uri": record.uri,
                "mimeType": record.mime_type,
                "text": record.text,
            }
        ]
    }))?)
}

fn collect_mcp_resource_records(resources: &LoadedResources) -> Vec<McpResourceRecord> {
    let mut records = Vec::new();

    for server in &resources.mcp_servers {
        let text = if server.source_info.path.exists() {
            fs::read_to_string(&server.source_info.path)
                .unwrap_or_else(|_| serde_json::to_string_pretty(&server.value).unwrap_or_default())
        } else {
            serde_json::to_string_pretty(&server.value).unwrap_or_default()
        };
        records.push(McpResourceRecord {
            uri: format!("mcp://manifest/{}", server.value.id),
            server: server.value.id.clone(),
            name: server.value.display_name.clone(),
            description: if server.value.description.is_empty() {
                "Configured MCP server manifest".to_string()
            } else {
                server.value.description.clone()
            },
            mime_type: "application/yaml".to_string(),
            text,
        });
    }

    for (plugin, server) in plugin_mcp_servers(resources) {
        let text = serde_json::to_string_pretty(&json!({
            "plugin": plugin.id,
            "server": server,
        }))
        .unwrap_or_default();
        records.push(McpResourceRecord {
            uri: format!("mcp://plugin/{}/{}", plugin.id, server.id),
            server: server.id.clone(),
            name: server.display_name.clone(),
            description: format!("MCP server manifest embedded in plugin {}", plugin.id),
            mime_type: "application/json".to_string(),
            text,
        });
    }

    records
}

fn collect_glob_matches(
    workspace_root: &Path,
    current: &Path,
    pattern: &Pattern,
    matches: &mut Vec<String>,
) -> Result<()> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        let relative = path.strip_prefix(workspace_root).unwrap_or(&path);
        let relative_text = relative.to_string_lossy().replace('\\', "/");
        if pattern.matches(&relative_text) {
            matches.push(relative_text.clone());
        }
        if file_type.is_dir() {
            collect_glob_matches(workspace_root, &path, pattern, matches)?;
        }
    }
    Ok(())
}

fn resolve_workspace_path(cwd: &Path, path: &Path) -> Result<PathBuf> {
    let workspace_root = fs::canonicalize(cwd)?;
    let workspace_path = normalize_path(cwd);
    let candidate = if path.is_absolute() {
        normalize_path(path)
    } else {
        normalize_path(&cwd.join(path))
    };
    if !candidate.starts_with(&workspace_path) {
        bail!(
            "path {} escapes workspace {}",
            path.display(),
            cwd.display()
        );
    }
    let ancestor = nearest_existing_ancestor(&candidate)
        .ok_or_else(|| anyhow!("failed to resolve path {}", path.display()))?;
    let canonical_ancestor = fs::canonicalize(&ancestor)?;
    if !canonical_ancestor.starts_with(&workspace_root) {
        bail!(
            "path {} resolves through symlink outside workspace {}",
            path.display(),
            cwd.display()
        );
    }
    Ok(candidate)
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(
                    normalized.components().next_back(),
                    Some(Component::Normal(_))
                ) {
                    normalized.pop();
                } else if !normalized.has_root() {
                    normalized.push(component.as_os_str());
                }
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    if normalized.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        normalized
    }
}

fn nearest_existing_ancestor(path: &Path) -> Option<PathBuf> {
    let mut current = path.to_path_buf();
    loop {
        if current.exists() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

fn rewrite_notebook_cell(cell: &mut Value, cell_type: &str, new_source: &str) -> Result<()> {
    let object = cell
        .as_object_mut()
        .ok_or_else(|| anyhow!("notebook cell is not an object"))?;
    object.insert(
        "cell_type".to_string(),
        Value::String(cell_type.to_string()),
    );
    object.insert("source".to_string(), source_lines(new_source));
    if cell_type == "code" {
        object
            .entry("outputs".to_string())
            .or_insert_with(|| Value::Array(Vec::new()));
        object
            .entry("execution_count".to_string())
            .or_insert(Value::Null);
    }
    Ok(())
}

fn make_notebook_cell(cell_type: Option<&str>, new_source: &str) -> Value {
    let cell_type = cell_type.unwrap_or("code");
    let mut object = serde_json::Map::new();
    object.insert(
        "cell_type".to_string(),
        Value::String(cell_type.to_string()),
    );
    object.insert(
        "metadata".to_string(),
        Value::Object(serde_json::Map::new()),
    );
    object.insert("source".to_string(), source_lines(new_source));
    if cell_type == "code" {
        object.insert("outputs".to_string(), Value::Array(Vec::new()));
        object.insert("execution_count".to_string(), Value::Null);
    }
    Value::Object(object)
}

fn source_lines(text: &str) -> Value {
    Value::Array(
        text.lines()
            .map(|line| Value::String(format!("{line}\n")))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_resources::{
        LoadedItem, McpServerSpec, SkillSpec, SourceInfo, SourceKind, ToolSpec,
    };

    fn sample_registry() -> ToolRegistry {
        ToolRegistry::from_resources(&LoadedResources {
            tools: vec![
                LoadedItem {
                    value: ToolSpec {
                        id: "Read".to_string(),
                        name: "Read".to_string(),
                        description: "Read a file".to_string(),
                        handler: "builtin:read_file".to_string(),
                        ..ToolSpec::default()
                    },
                    source_info: SourceInfo {
                        path: "tools/read.yaml".into(),
                        kind: SourceKind::Builtin,
                    },
                },
                LoadedItem {
                    value: ToolSpec {
                        id: "ToolSearch".to_string(),
                        name: "ToolSearch".to_string(),
                        description: "Search tools".to_string(),
                        handler: "runtime:tool_search".to_string(),
                        ..ToolSpec::default()
                    },
                    source_info: SourceInfo {
                        path: "tools/tool_search.yaml".into(),
                        kind: SourceKind::Builtin,
                    },
                },
            ],
            ..LoadedResources::default()
        })
    }

    #[test]
    fn skill_tool_loads_enabled_skill() {
        let resources = LoadedResources {
            skills: vec![LoadedItem {
                value: SkillSpec {
                    name: "reviewer".to_string(),
                    description: "Review code".to_string(),
                    content: "Inspect changes".to_string(),
                    disable_model_invocation: false,
                },
                source_info: SourceInfo {
                    path: "skills/reviewer/SKILL.md".into(),
                    kind: SourceKind::Builtin,
                },
            }],
            ..LoadedResources::default()
        };
        let output = execute_skill_tool(
            &resources,
            json!({"skill": "reviewer", "args": "focus on tests"}),
        )
        .unwrap();
        assert!(output.contains("<skill name=\"reviewer\">"));
        assert!(output.contains("focus on tests"));
    }

    #[test]
    fn tool_search_returns_function_blocks() {
        let output = execute_tool_search(&sample_registry(), json!({"query": "read"})).unwrap();
        assert!(output.contains("<functions>"));
        assert!(output.contains("\"name\":\"Read\""));
    }

    #[test]
    fn list_mcp_resources_returns_manifest_entries() {
        let resources = LoadedResources {
            mcp_servers: vec![LoadedItem {
                value: McpServerSpec {
                    id: "docs".to_string(),
                    display_name: "Docs".to_string(),
                    transport: "stdio".to_string(),
                    endpoint: String::new(),
                    target: "docs-server".to_string(),
                    description: "Docs server".to_string(),
                },
                source_info: SourceInfo {
                    path: "resources/mcp_servers/docs.yaml".into(),
                    kind: SourceKind::Builtin,
                },
            }],
            ..LoadedResources::default()
        };
        let output = execute_list_mcp_resources(&resources, json!({})).unwrap();
        assert!(output.contains("\"server\": \"docs\""));
        assert!(output.contains("mcp://manifest/docs"));
    }

    #[test]
    fn read_mcp_resource_returns_contents() {
        let resources = LoadedResources {
            mcp_servers: vec![LoadedItem {
                value: McpServerSpec {
                    id: "docs".to_string(),
                    display_name: "Docs".to_string(),
                    transport: "stdio".to_string(),
                    endpoint: String::new(),
                    target: "docs-server".to_string(),
                    description: "Docs server".to_string(),
                },
                source_info: SourceInfo {
                    path: "resources/mcp_servers/docs.yaml".into(),
                    kind: SourceKind::Builtin,
                },
            }],
            ..LoadedResources::default()
        };
        let output = execute_read_mcp_resource(
            &resources,
            json!({"server": "docs", "uri": "mcp://manifest/docs"}),
        )
        .unwrap();
        assert!(output.contains("\"contents\""));
        assert!(output.contains("mcp://manifest/docs"));
    }

    #[test]
    fn glob_tool_finds_matching_files() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("src")).unwrap();
        std::fs::write(temp.path().join("src/lib.rs"), "pub fn hi() {}").unwrap();
        std::fs::write(temp.path().join("src/main.rs"), "fn main() {}").unwrap();
        let output = execute_glob_tool(temp.path(), json!({"pattern": "src/*.rs"})).unwrap();
        assert!(output.contains("src/lib.rs"));
        assert!(output.contains("src/main.rs"));
    }

    #[test]
    fn notebook_edit_replaces_cell_source() {
        let temp = tempfile::tempdir().unwrap();
        let notebook = temp.path().join("demo.ipynb");
        std::fs::write(
            &notebook,
            serde_json::to_string(&json!({
                "cells": [
                    {
                        "cell_type": "code",
                        "metadata": {},
                        "source": ["print('old')\n"],
                        "outputs": [],
                        "execution_count": null
                    }
                ],
                "metadata": {},
                "nbformat": 4,
                "nbformat_minor": 5
            }))
            .unwrap(),
        )
        .unwrap();
        let output = execute_notebook_edit(
            temp.path(),
            json!({
                "notebook_path": "demo.ipynb",
                "cell_number": 0,
                "new_source": "print('new')",
                "edit_mode": "replace"
            }),
        )
        .unwrap();
        assert!(output.contains("\"edit_mode\": \"replace\""));
        let updated = std::fs::read_to_string(&notebook).unwrap();
        assert!(updated.contains("print('new')"));
    }
}

mod sleep;

use crate::workspace_paths;
use crate::AppState;
use anyhow::{anyhow, bail, Context, Result};
use glob::Pattern;
use puffer_resources::{plugin_mcp_servers, LoadedResources};
use puffer_tools::{ToolDefinition, ToolRegistry};
use serde::Deserialize;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

use super::local_mcp_resources::{
    is_live_resource_server, list_live_mcp_resources, live_resource_server_names,
    read_live_mcp_resource,
};

#[derive(Debug, Deserialize)]
struct GlobInput {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
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
            | "runtime:sleep"
            | "runtime:list_mcp_resources"
            | "runtime:read_mcp_resource"
    )
}

pub(super) fn execute_runtime_local_tool(
    state: &AppState,
    resources: &LoadedResources,
    registry: &ToolRegistry,
    definition: &ToolDefinition,
    cwd: &Path,
    input: Value,
) -> Result<String> {
    match definition.handler.as_str() {
        "runtime:skill" => execute_skill_tool(resources, input),
        "runtime:tool_search" => execute_tool_search(registry, input),
        "runtime:glob" => execute_glob_tool(
            cwd,
            &state.working_dirs,
            workspace_paths::sandbox_allows_all_paths(&state.sandbox_mode),
            input,
        ),
        "runtime:sleep" => sleep::execute_sleep(input),
        "runtime:list_mcp_resources" => execute_list_mcp_resources(resources, cwd, input),
        "runtime:read_mcp_resource" => execute_read_mcp_resource(resources, cwd, input),
        other => Err(anyhow!("unsupported runtime-local tool handler {other}")),
    }
}

fn execute_skill_tool(resources: &LoadedResources, input: Value) -> Result<String> {
    super::claude_tools::skill::execute_claude_skill_tool(resources, input)
}

fn execute_tool_search(registry: &ToolRegistry, input: Value) -> Result<String> {
    super::claude_tools::tool_search::execute_claude_tool_search_tool(registry, input)
}

fn execute_glob_tool(
    cwd: &Path,
    working_dirs: &[PathBuf],
    allow_all_paths: bool,
    input: Value,
) -> Result<String> {
    let input: GlobInput = serde_json::from_value(input)?;
    let pattern = Pattern::new(&input.pattern)
        .map_err(|error| anyhow!("invalid glob pattern `{}`: {error}", input.pattern))?;
    let sandbox_mode = if allow_all_paths {
        "danger-full-access"
    } else {
        "workspace-write"
    };
    let root = input
        .path
        .as_deref()
        .map(|path| {
            workspace_paths::resolve_path_for_session(
                cwd,
                working_dirs,
                sandbox_mode,
                Path::new(path),
            )
        })
        .transpose()?
        .unwrap_or_else(|| cwd.to_path_buf());
    let mut matches = Vec::new();
    collect_glob_matches(&root, &root, &pattern, &mut matches)?;
    matches.sort();
    Ok(serde_json::to_string_pretty(&matches)?)
}

fn execute_list_mcp_resources(
    resources: &LoadedResources,
    cwd: &Path,
    input: Value,
) -> Result<String> {
    let input: ListMcpInput = serde_json::from_value(input)?;
    if let Some(server) = input
        .server
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let available = available_resource_servers(resources, cwd);
        if !available
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(server))
        {
            bail!(
                "Server \"{server}\" not found. Available servers: {}",
                available.join(", ")
            );
        }
    }
    let mut filtered = if live_resource_server_names(resources)
        .iter()
        .any(|candidate| matches_server_filter_name(candidate, input.server.as_deref()))
    {
        serde_json::from_str::<Vec<Value>>(&list_live_mcp_resources(
            resources,
            cwd,
            input.server.as_deref(),
        )?)
        .context("failed to decode live MCP resources")?
    } else {
        Vec::new()
    };
    filtered.extend(
        collect_mcp_resource_records(resources)
            .into_iter()
            .filter(|record| matches_server_filter(record, input.server.as_deref()))
            .map(|record| {
                json!({
                    "uri": record.uri,
                    "name": record.name,
                    "mimeType": record.mime_type,
                    "description": record.description,
                    "server": record.server,
                })
            })
            .collect::<Vec<_>>(),
    );
    filtered.sort_by(|left, right| {
        left["server"]
            .as_str()
            .cmp(&right["server"].as_str())
            .then_with(|| left["uri"].as_str().cmp(&right["uri"].as_str()))
    });
    Ok(serde_json::to_string_pretty(&filtered)?)
}

fn execute_read_mcp_resource(
    resources: &LoadedResources,
    cwd: &Path,
    input: Value,
) -> Result<String> {
    let input: ReadMcpInput = serde_json::from_value(input)?;
    let available = available_resource_servers(resources, cwd);
    if !available
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(input.server.trim()))
    {
        bail!(
            "Server \"{}\" not found. Available servers: {}",
            input.server,
            available.join(", ")
        );
    }
    if is_live_resource_server(resources, input.server.trim()) {
        let output = read_live_mcp_resource(resources, cwd, input.server.trim(), &input.uri)?;
        return Ok(serde_json::to_string_pretty(&decorate_live_read_output(
            output,
            input.server.trim(),
            &input.uri,
        ))?);
    }
    let record = collect_mcp_resource_records(resources)
        .into_iter()
        .find(|record| {
            record.uri == input.uri && record.server.eq_ignore_ascii_case(input.server.trim())
        })
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
                "name": record.name,
                "description": record.description,
                "server": record.server,
                "text": record.text,
            }
        ]
    }))?)
}

fn matches_server_filter(record: &McpResourceRecord, server: Option<&str>) -> bool {
    server
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_none_or(|value| record.server.eq_ignore_ascii_case(value))
}

fn matches_server_filter_name(server_name: &str, server: Option<&str>) -> bool {
    server
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_none_or(|value| server_name.eq_ignore_ascii_case(value))
}

fn available_resource_servers(resources: &LoadedResources, _cwd: &Path) -> Vec<String> {
    let mut servers = collect_mcp_resource_records(resources)
        .into_iter()
        .map(|record| record.server)
        .collect::<Vec<_>>();
    servers.extend(live_resource_server_names(resources));
    servers.sort_by_key(|value| value.to_ascii_lowercase());
    servers.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
    servers
}

fn decorate_live_read_output(
    output: crate::runtime::claude_tools::mcp_resources::ReadMcpResourceToolOutput,
    server: &str,
    uri: &str,
) -> Value {
    let name = filesystem_resource_name(uri);
    let description = if server.eq_ignore_ascii_case("filesystem") {
        Some("Live filesystem resource")
    } else {
        None
    };
    let contents = output
        .contents
        .into_iter()
        .map(|content| {
            let mut value = serde_json::to_value(content).unwrap_or_else(|_| json!({}));
            if let Some(object) = value.as_object_mut() {
                object.insert("server".to_string(), json!(server));
                if let Some(name) = name.as_deref() {
                    object.insert("name".to_string(), json!(name));
                }
                if let Some(description) = description {
                    object.insert("description".to_string(), json!(description));
                }
            }
            value
        })
        .collect::<Vec<_>>();
    json!({ "contents": contents })
}

fn filesystem_resource_name(uri: &str) -> Option<String> {
    uri.strip_prefix("mcp://filesystem/")
        .map(ToOwned::to_owned)
        .filter(|value| !value.is_empty())
}

fn collect_mcp_resource_records(resources: &LoadedResources) -> Vec<McpResourceRecord> {
    let mut records = Vec::new();

    for server in &resources.mcp_servers {
        if is_live_resource_server(resources, server.value.id.as_str()) {
            continue;
        }
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
        if is_live_resource_server(resources, server.id.as_str()) {
            continue;
        }
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
                    ..SkillSpec::default()
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
        assert!(output.contains("<command-name>reviewer</command-name>"));
        assert!(output.contains("<skill name=\"reviewer\">"));
        assert!(output.contains("focus on tests"));
    }

    #[test]
    fn tool_search_returns_function_blocks() {
        let output = execute_tool_search(&sample_registry(), json!({"query": "notebook"})).unwrap();
        assert!(output.contains("<functions>"));
        assert!(output.contains("\"name\":\"NotebookEdit\""));
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
        let output =
            execute_list_mcp_resources(&resources, std::env::temp_dir().as_path(), json!({}))
                .unwrap();
        assert!(output.contains("\"server\": \"docs\""));
        assert!(output.contains("mcp://manifest/docs"));
    }

    #[test]
    fn list_mcp_resources_filters_server_case_insensitively() {
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
        let output = execute_list_mcp_resources(
            &resources,
            std::env::temp_dir().as_path(),
            json!({"server": "DOCS"}),
        )
        .unwrap();
        assert!(output.contains("mcp://manifest/docs"));
    }

    #[test]
    fn list_mcp_resources_errors_for_unknown_server() {
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
        let error = execute_list_mcp_resources(
            &resources,
            std::env::temp_dir().as_path(),
            json!({"server": "missing"}),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("Server \"missing\" not found"));
        assert!(error.contains("docs"));
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
            std::env::temp_dir().as_path(),
            json!({"server": "docs", "uri": "mcp://manifest/docs"}),
        )
        .unwrap();
        assert!(output.contains("\"contents\""));
        assert!(output.contains("mcp://manifest/docs"));
        assert!(output.contains("\"server\": \"docs\""));
        assert!(output.contains("\"name\": \"Docs\""));
    }

    #[test]
    fn read_mcp_resource_errors_for_unknown_server() {
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
        let error = execute_read_mcp_resource(
            &resources,
            std::env::temp_dir().as_path(),
            json!({"server": "missing", "uri": "mcp://manifest/docs"}),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("Server \"missing\" not found"));
        assert!(error.contains("docs"));
    }

    #[test]
    fn filesystem_mcp_resources_list_and_read_live_workspace_files() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("notes")).unwrap();
        std::fs::write(temp.path().join("notes/guide.md"), "# Guide\n").unwrap();
        std::fs::write(temp.path().join("notes/data.bin"), [0xff, 0x00, 0x01]).unwrap();
        let resources = LoadedResources {
            mcp_servers: vec![LoadedItem {
                value: McpServerSpec {
                    id: "filesystem".to_string(),
                    display_name: "Filesystem".to_string(),
                    transport: "stdio".to_string(),
                    endpoint: String::new(),
                    target: "builtin:filesystem".to_string(),
                    description: "Filesystem server".to_string(),
                },
                source_info: SourceInfo {
                    path: "resources/mcp_servers/filesystem.yaml".into(),
                    kind: SourceKind::Builtin,
                },
            }],
            ..LoadedResources::default()
        };

        let listed =
            execute_list_mcp_resources(&resources, temp.path(), json!({"server": "filesystem"}))
                .unwrap();
        assert!(listed.contains("\"server\": \"filesystem\""));
        assert!(listed.contains("mcp://filesystem/notes/guide.md"));
        assert!(listed.contains("mcp://filesystem/notes/data.bin"));

        let read = execute_read_mcp_resource(
            &resources,
            temp.path(),
            json!({"server": "filesystem", "uri": "mcp://filesystem/notes/guide.md"}),
        )
        .unwrap();
        assert!(read.contains("\"mimeType\": \"text/markdown\""));
        assert!(read.contains("\"name\": \"notes/guide.md\""));
        assert!(read.contains("# Guide"));

        let binary = execute_read_mcp_resource(
            &resources,
            temp.path(),
            json!({"server": "filesystem", "uri": "mcp://filesystem/notes/data.bin"}),
        )
        .unwrap();
        assert!(binary.contains("\"blobSavedTo\""));
        assert!(binary.contains("Binary content saved"));
    }

    #[test]
    fn glob_tool_finds_matching_files() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("src")).unwrap();
        std::fs::write(temp.path().join("src/lib.rs"), "pub fn hi() {}").unwrap();
        std::fs::write(temp.path().join("src/main.rs"), "fn main() {}").unwrap();
        let output =
            execute_glob_tool(temp.path(), &[], false, json!({"pattern": "src/*.rs"})).unwrap();
        assert!(output.contains("src/lib.rs"));
        assert!(output.contains("src/main.rs"));
    }

    #[test]
    fn glob_tool_accepts_added_working_directory_paths() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().join("repo");
        let extra = temp.path().join("docs");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(extra.join("guides")).unwrap();
        std::fs::write(extra.join("guides/intro.md"), "hi").unwrap();

        let output = execute_glob_tool(
            &cwd,
            std::slice::from_ref(&extra),
            false,
            json!({
                "pattern": "guides/*.md",
                "path": extra.display().to_string()
            }),
        )
        .unwrap();

        assert!(output.contains("guides/intro.md"));
    }
}

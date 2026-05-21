mod browser;
mod sleep;

use crate::permissions::FilesystemPermissionPolicy;
use crate::workspace_paths;
use crate::AppState;
use anyhow::{anyhow, Result};
use glob::Pattern;
use puffer_resources::LoadedResources;
use puffer_runner_api::{
    FilesystemExecutionPolicy, McpResourceContentPart, McpResourceRecord, NullChunkSink,
    RunnerError, ToolRunner,
};
use puffer_tools::{ToolDefinition, ToolRegistry};
use serde::Deserialize;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

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

pub(super) fn is_runtime_local_tool(definition: &ToolDefinition) -> bool {
    matches!(
        definition.handler.as_str(),
        "runtime:skill"
            | "runtime:tool_search"
            | "runtime:browser"
            | "runtime:glob"
            | "runtime:sleep"
            | "runtime:list_mcp_resources"
            | "runtime:read_mcp_resource"
            | "runtime:mcp_call"
    )
}

pub(super) fn execute_runtime_local_tool(
    state: &AppState,
    resources: &LoadedResources,
    registry: &ToolRegistry,
    definition: &ToolDefinition,
    cwd: &Path,
    filesystem_policy: &FilesystemPermissionPolicy,
    input: Value,
) -> Result<String> {
    match definition.handler.as_str() {
        "runtime:skill" => execute_skill_tool(resources, input),
        "runtime:tool_search" => execute_tool_search(registry, input),
        "runtime:browser" => {
            browser::execute_browser_tool(cwd, &state.browser_root_session_id(), input)
        }
        "runtime:glob" => execute_glob_tool(cwd, &state.working_dirs, filesystem_policy, input),
        "runtime:sleep" => sleep::execute_sleep(input),
        "runtime:list_mcp_resources" => {
            execute_list_mcp_resources(state.tool_runner.as_ref(), input)
        }
        "runtime:read_mcp_resource" => execute_read_mcp_resource(state.tool_runner.as_ref(), input),
        "runtime:mcp_call" => execute_mcp_call(state.tool_runner.as_ref(), definition, input),
        other => Err(anyhow!("unsupported runtime-local tool handler {other}")),
    }
}

/// Routes one model-issued `mcp__<server>__<tool>` call back through the
/// tool runner. The qualified tool id is opaque to dispatch — we read the
/// original `(server, tool)` strings from `handler_args`, which the
/// registry stamped on at registration time.
fn execute_mcp_call(
    runner: &dyn ToolRunner,
    definition: &ToolDefinition,
    input: Value,
) -> Result<String> {
    let server = definition
        .handler_args
        .first()
        .ok_or_else(|| anyhow!("runtime:mcp_call missing server in handler_args"))?;
    let tool = definition
        .handler_args
        .get(1)
        .ok_or_else(|| anyhow!("runtime:mcp_call missing tool in handler_args"))?;
    let mut sink = NullChunkSink;
    let result = runner
        .call_mcp_tool(server, tool, input, &mut sink)
        .map_err(map_runner_error)?;
    if !result.success {
        let mut error = format!("MCP tool {server}/{tool} reported failure");
        if !result.stderr.is_empty() {
            error.push_str(&format!(": {}", result.stderr));
        } else if !result.stdout.is_empty() {
            error.push_str(&format!(": {}", result.stdout));
        }
        return Err(anyhow!(error));
    }
    if !result.metadata.is_null() {
        Ok(serde_json::to_string_pretty(&result.metadata)?)
    } else if !result.stdout.is_empty() {
        Ok(result.stdout)
    } else {
        Ok(String::new())
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
    filesystem_policy: &FilesystemPermissionPolicy,
    input: Value,
) -> Result<String> {
    let runner_policy: FilesystemExecutionPolicy = filesystem_policy.runner_policy();
    let input: GlobInput = serde_json::from_value(input)?;
    let pattern = Pattern::new(&input.pattern)
        .map_err(|error| anyhow!("invalid glob pattern `{}`: {error}", input.pattern))?;
    let root = input
        .path
        .as_deref()
        .map(|path| {
            workspace_paths::resolve_path_for_filesystem_policy(
                cwd,
                working_dirs,
                runner_policy.sandbox_mode,
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

fn execute_list_mcp_resources(runner: &dyn ToolRunner, input: Value) -> Result<String> {
    let input: ListMcpInput = serde_json::from_value(input)?;
    let server_filter = input
        .server
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let records = runner
        .list_mcp_resources(server_filter)
        .map_err(map_runner_error)?;
    let mut payload = records
        .into_iter()
        .map(record_to_legacy_json)
        .collect::<Vec<_>>();
    payload.sort_by(|left, right| {
        left["server"]
            .as_str()
            .cmp(&right["server"].as_str())
            .then_with(|| left["uri"].as_str().cmp(&right["uri"].as_str()))
    });
    Ok(serde_json::to_string_pretty(&payload)?)
}

fn execute_read_mcp_resource(runner: &dyn ToolRunner, input: Value) -> Result<String> {
    let input: ReadMcpInput = serde_json::from_value(input)?;
    let content = runner
        .read_mcp_resource(input.server.trim(), &input.uri)
        .map_err(map_runner_error)?;
    let server = content.server.clone();
    let display_name = filesystem_resource_name(&input.uri);
    let description = if server.eq_ignore_ascii_case("filesystem") {
        Some("Live filesystem resource".to_string())
    } else {
        None
    };
    let contents: Vec<Value> = content
        .parts
        .into_iter()
        .map(|part| {
            part_to_legacy_json(
                part,
                &server,
                display_name.as_deref(),
                description.as_deref(),
            )
        })
        .collect();
    Ok(serde_json::to_string_pretty(
        &json!({ "contents": contents }),
    )?)
}

fn record_to_legacy_json(record: McpResourceRecord) -> Value {
    json!({
        "uri": record.uri,
        "name": record.name,
        "mimeType": record.mime_type.unwrap_or_default(),
        "description": record.description.unwrap_or_default(),
        "server": record.server,
    })
}

fn part_to_legacy_json(
    part: McpResourceContentPart,
    server: &str,
    name_override: Option<&str>,
    description_override: Option<&str>,
) -> Value {
    let mut value = match part {
        McpResourceContentPart::Text {
            uri,
            mime_type,
            text,
        } => json!({
            "uri": uri,
            "mimeType": mime_type,
            "text": text,
        }),
        McpResourceContentPart::Blob {
            uri,
            mime_type,
            bytes,
        } => {
            let saved = persist_blob_to_workspace(server, &uri, mime_type.as_deref(), &bytes);
            let mut text_message =
                format!("[Resource from {server} at {uri}] Binary content saved",);
            if let Some(path) = saved.as_deref() {
                text_message.push_str(&format!(" to {} ({} bytes", path.display(), bytes.len()));
            } else {
                text_message.push_str(&format!(" ({} bytes", bytes.len()));
            }
            if let Some(mime) = mime_type.as_deref().filter(|m| !m.trim().is_empty()) {
                text_message.push_str(&format!(", mime type {mime})."));
            } else {
                text_message.push_str(").");
            }
            let mut payload = json!({
                "uri": uri,
                "mimeType": mime_type,
                "text": text_message,
            });
            if let Some(path) = saved.as_deref() {
                payload["blobSavedTo"] = json!(path.display().to_string());
            }
            payload
        }
    };
    if let Some(object) = value.as_object_mut() {
        object.insert("server".to_string(), json!(server));
        if let Some(name) = name_override {
            object.insert("name".to_string(), json!(name));
        }
        if let Some(description) = description_override {
            object.insert("description".to_string(), json!(description));
        }
    }
    value
}

fn persist_blob_to_workspace(
    server: &str,
    _uri: &str,
    mime_type: Option<&str>,
    bytes: &[u8],
) -> Option<PathBuf> {
    let workspace_root = std::env::current_dir().ok()?;
    let dir = workspace_root.join(".puffer").join("mcp-blobs");
    fs::create_dir_all(&dir).ok()?;
    let extension = match mime_type.unwrap_or("").to_ascii_lowercase().as_str() {
        "application/json" => "json",
        "application/yaml" | "text/yaml" | "application/x-yaml" => "yaml",
        "application/pdf" => "pdf",
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "text/plain" => "txt",
        "text/markdown" => "md",
        _ => "bin",
    };
    let server_fragment = sanitize_fragment(server);
    let file_name = format!(
        "mcp-resource-{server_fragment}-{}.{extension}",
        uuid::Uuid::new_v4()
    );
    let path = dir.join(file_name);
    fs::write(&path, bytes).ok()?;
    Some(path)
}

fn sanitize_fragment(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    for character in input.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
            result.push(character);
        } else {
            result.push('-');
        }
    }
    let compact = result.trim_matches('-').to_string();
    if compact.is_empty() {
        "server".to_string()
    } else {
        compact
    }
}

fn filesystem_resource_name(uri: &str) -> Option<String> {
    uri.strip_prefix("mcp://filesystem/")
        .map(ToOwned::to_owned)
        .filter(|value| !value.is_empty())
}

fn map_runner_error(err: RunnerError) -> anyhow::Error {
    match err {
        RunnerError::NotFound(message) => anyhow!(message),
        other => anyhow!(other.to_string()),
    }
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
    use crate::permissions::profile::{EffectiveApprovalPolicy, EffectiveSandboxMode};
    use crate::permissions::FilesystemPermissionPolicy;
    use crate::runner_adapter::LocalToolRunner;
    use puffer_resources::{
        LoadedItem, McpServerSpec, SkillSpec, SourceInfo, SourceKind, ToolSpec,
    };
    use puffer_runner_api::{
        ChunkSink, DirEntry, McpPrompt, McpPromptContent, McpResourceContent, McpResult,
        McpServerInfo, McpTool, RunnerCapabilities, RunnerPing, ToolRequest, ToolResult,
    };

    fn workspace_write_filesystem_policy(root: &Path) -> FilesystemPermissionPolicy {
        FilesystemPermissionPolicy {
            approval: EffectiveApprovalPolicy::Allow,
            sandbox_mode: EffectiveSandboxMode::WorkspaceWrite,
            workspace_roots: vec![root.to_path_buf()],
            session_granted: true,
        }
    }

    #[derive(Debug)]
    struct FakeMcpRunner;

    impl FakeMcpRunner {
        fn docs_record() -> McpResourceRecord {
            McpResourceRecord {
                server: "docs".to_string(),
                uri: "mcp://manifest/docs".to_string(),
                name: "Docs".to_string(),
                mime_type: Some("application/yaml".to_string()),
                description: Some("Docs server".to_string()),
            }
        }

        fn missing_server_error(server: &str) -> RunnerError {
            RunnerError::NotFound(format!(
                "MCP server `{server}` not found. Available servers: docs",
            ))
        }
    }

    impl ToolRunner for FakeMcpRunner {
        fn ping(&self) -> Result<RunnerPing, RunnerError> {
            Ok(RunnerPing {
                version: "test".to_string(),
                uptime: Default::default(),
            })
        }

        fn capabilities(&self) -> RunnerCapabilities {
            RunnerCapabilities {
                backend: "fake".to_string(),
                mcp_supported: true,
                ..RunnerCapabilities::default()
            }
        }

        fn execute_tool(
            &self,
            _req: ToolRequest,
            _sink: &mut dyn ChunkSink,
        ) -> Result<ToolResult, RunnerError> {
            Err(RunnerError::Unsupported(
                "fake runner does not execute tools".to_string(),
            ))
        }

        fn read_file(&self, _path: &Path) -> Result<Vec<u8>, RunnerError> {
            Err(RunnerError::Unsupported(
                "fake runner does not read files".to_string(),
            ))
        }

        fn list_dir(&self, _path: &Path) -> Result<Vec<DirEntry>, RunnerError> {
            Err(RunnerError::Unsupported(
                "fake runner does not list directories".to_string(),
            ))
        }

        fn glob(&self, _root: &Path, _pattern: &str) -> Result<Vec<PathBuf>, RunnerError> {
            Err(RunnerError::Unsupported(
                "fake runner does not glob files".to_string(),
            ))
        }

        fn list_mcp_servers(&self) -> Result<Vec<McpServerInfo>, RunnerError> {
            Ok(vec![McpServerInfo {
                id: "docs".to_string(),
                display_name: "Docs".to_string(),
                transport: "stdio".to_string(),
                target: "fake-docs".to_string(),
                description: "Docs server".to_string(),
            }])
        }

        fn list_mcp_tools(&self, _server: &str) -> Result<Vec<McpTool>, RunnerError> {
            Err(RunnerError::Unsupported(
                "fake runner does not list tools".to_string(),
            ))
        }

        fn call_mcp_tool(
            &self,
            _server: &str,
            _tool: &str,
            _args: Value,
            _sink: &mut dyn ChunkSink,
        ) -> Result<McpResult, RunnerError> {
            Err(RunnerError::Unsupported(
                "fake runner does not call tools".to_string(),
            ))
        }

        fn list_mcp_resources(
            &self,
            server: Option<&str>,
        ) -> Result<Vec<McpResourceRecord>, RunnerError> {
            match server.map(str::trim).filter(|value| !value.is_empty()) {
                Some(value) if value.eq_ignore_ascii_case("docs") => Ok(vec![Self::docs_record()]),
                Some(value) => Err(Self::missing_server_error(value)),
                None => Ok(vec![Self::docs_record()]),
            }
        }

        fn read_mcp_resource(
            &self,
            server: &str,
            uri: &str,
        ) -> Result<McpResourceContent, RunnerError> {
            if !server.eq_ignore_ascii_case("docs") {
                return Err(Self::missing_server_error(server));
            }
            Ok(McpResourceContent {
                server: "docs".to_string(),
                uri: uri.to_string(),
                parts: vec![McpResourceContentPart::Text {
                    uri: uri.to_string(),
                    mime_type: Some("application/yaml".to_string()),
                    text: "id: docs\n".to_string(),
                }],
            })
        }

        fn list_mcp_prompts(&self, _server: &str) -> Result<Vec<McpPrompt>, RunnerError> {
            Err(RunnerError::Unsupported(
                "fake runner does not list prompts".to_string(),
            ))
        }

        fn get_mcp_prompt(
            &self,
            _server: &str,
            _name: &str,
            _args: Value,
        ) -> Result<McpPromptContent, RunnerError> {
            Err(RunnerError::Unsupported(
                "fake runner does not render prompts".to_string(),
            ))
        }
    }

    fn filesystem_spec() -> McpServerSpec {
        McpServerSpec {
            id: "filesystem".to_string(),
            display_name: "Filesystem".to_string(),
            transport: "stdio".to_string(),
            endpoint: String::new(),
            target: "builtin:filesystem".to_string(),
            description: "Filesystem server".to_string(),
            headers: Default::default(),
            oauth: None,
        }
    }

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
        let runner = FakeMcpRunner;
        let output = execute_list_mcp_resources(&runner, json!({})).unwrap();
        assert!(output.contains("\"server\": \"docs\""));
        assert!(output.contains("mcp://manifest/docs"));
    }

    #[test]
    fn list_mcp_resources_filters_server_case_insensitively() {
        let runner = FakeMcpRunner;
        let output = execute_list_mcp_resources(&runner, json!({"server": "DOCS"})).unwrap();
        assert!(output.contains("mcp://manifest/docs"));
    }

    #[test]
    fn list_mcp_resources_errors_for_unknown_server() {
        let runner = FakeMcpRunner;
        let error = execute_list_mcp_resources(&runner, json!({"server": "missing"}))
            .unwrap_err()
            .to_string();
        assert!(error.contains("MCP server `missing` not found"));
        assert!(error.contains("docs"));
    }

    #[test]
    fn read_mcp_resource_returns_contents() {
        let runner = FakeMcpRunner;
        let output = execute_read_mcp_resource(
            &runner,
            json!({"server": "docs", "uri": "mcp://manifest/docs"}),
        )
        .unwrap();
        assert!(output.contains("\"contents\""));
        assert!(output.contains("mcp://manifest/docs"));
        assert!(output.contains("\"server\": \"docs\""));
    }

    #[test]
    fn read_mcp_resource_errors_for_unknown_server() {
        let runner = FakeMcpRunner;
        let error = execute_read_mcp_resource(
            &runner,
            json!({"server": "missing", "uri": "mcp://manifest/docs"}),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("MCP server `missing` not found"));
        assert!(error.contains("docs"));
    }

    #[test]
    fn filesystem_mcp_resources_list_and_read_live_workspace_files() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("notes")).unwrap();
        std::fs::write(temp.path().join("notes/guide.md"), "# Guide\n").unwrap();
        std::fs::write(temp.path().join("notes/data.bin"), [0xff, 0x00, 0x01]).unwrap();
        let runner = LocalToolRunner::new()
            .with_mcp_servers(vec![filesystem_spec()])
            .with_mcp_workspace_root(temp.path().to_path_buf());

        let listed = execute_list_mcp_resources(&runner, json!({"server": "filesystem"})).unwrap();
        assert!(listed.contains("\"server\": \"filesystem\""));
        assert!(listed.contains("mcp://filesystem/notes/guide.md"));
        assert!(listed.contains("mcp://filesystem/notes/data.bin"));

        let read = execute_read_mcp_resource(
            &runner,
            json!({"server": "filesystem", "uri": "mcp://filesystem/notes/guide.md"}),
        )
        .unwrap();
        assert!(read.contains("\"mimeType\": \"text/markdown\""));
        assert!(read.contains("\"name\": \"notes/guide.md\""));
        assert!(read.contains("# Guide"));

        let binary = execute_read_mcp_resource(
            &runner,
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
        let filesystem_policy = workspace_write_filesystem_policy(temp.path());
        let output = execute_glob_tool(
            temp.path(),
            &[],
            &filesystem_policy,
            json!({"pattern": "src/*.rs"}),
        )
        .unwrap();
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
            &workspace_write_filesystem_policy(&cwd),
            json!({
                "pattern": "guides/*.md",
                "path": extra.display().to_string()
            }),
        )
        .unwrap();

        assert!(output.contains("guides/intro.md"));
    }
}

//! MCP server lifecycle + resource/tool dispatch owned by [`LocalToolRunner`].
//!
//! Today's puffer ships exactly one live MCP transport — a built-in
//! filesystem server that walks the active workspace root — plus a "manifest
//! resource" view that re-exports configured `.puffer/mcp_servers/*.yaml`
//! entries as readable resources. Both used to live in the runtime
//! (`runtime::local_mcp_resources` + `runtime::local_tools`) and were called
//! directly. Phase 1 of the runner refactor moves that ownership onto
//! `LocalToolRunner` so MCP flows through the [`puffer_runner_api::ToolRunner`]
//! trait and works identically over the gRPC backend.
//!
//! `McpHost` is intentionally thin: it eagerly captures the
//! [`McpServerSpec`] list at construction (no hidden lazy spawn) and serves
//! resource lookups synchronously. Real subprocess MCP clients can grow on
//! top of this struct without touching the trait surface.

use anyhow::Context;
use puffer_resources::McpServerSpec;
use puffer_runner_api::{
    McpPrompt, McpPromptContent, McpResourceContent, McpResourceContentPart, McpResourceRecord,
    McpResult, McpServerInfo, McpTool, RunnerError,
};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

/// Owns the MCP server roster and any live transports the runner needs to
/// satisfy `ToolRunner`'s 7 MCP methods.
#[derive(Debug, Clone, Default)]
pub struct McpHost {
    servers: Vec<McpServerSpec>,
    workspace_root: Option<PathBuf>,
}

impl McpHost {
    /// Builds a host from a list of MCP manifests plus the optional workspace
    /// root used by the built-in `filesystem` server.
    pub fn new(servers: Vec<McpServerSpec>, workspace_root: Option<PathBuf>) -> Self {
        Self {
            servers,
            workspace_root,
        }
    }

    /// Returns the configured MCP servers as runner-shaped DTOs.
    pub fn list_servers(&self) -> Vec<McpServerInfo> {
        self.servers.iter().map(spec_to_info).collect()
    }

    /// Returns the workspace root used by built-in transports.
    pub fn workspace_root(&self) -> Option<&Path> {
        self.workspace_root.as_deref()
    }

    /// Consumes the host and returns the configured server specs. Useful
    /// when reconfiguring the workspace root without re-loading manifests.
    pub fn into_servers(self) -> Vec<McpServerSpec> {
        self.servers
    }

    /// Borrows the configured server specs.
    pub fn servers(&self) -> &[McpServerSpec] {
        &self.servers
    }

    /// Returns `Unsupported` until a real MCP client is wired up: today's
    /// runner only implements filesystem-style resource discovery, not tool
    /// calls.
    pub fn list_tools(&self, server: &str) -> Result<Vec<McpTool>, RunnerError> {
        self.lookup_server(server)?;
        Err(RunnerError::Unsupported(format!(
            "MCP `tools/list` is not implemented for server `{server}`",
        )))
    }

    /// Same story as `list_tools` — kept for protocol completeness.
    pub fn call_tool(
        &self,
        server: &str,
        tool: &str,
        _args: Value,
    ) -> Result<McpResult, RunnerError> {
        self.lookup_server(server)?;
        Err(RunnerError::Unsupported(format!(
            "MCP `tools/call` for `{tool}` on server `{server}` is not implemented",
        )))
    }

    /// Lists resources across one or all servers. The built-in `filesystem`
    /// transport walks `workspace_root`; every other server contributes its
    /// manifest as a single `mcp://manifest/<id>` record.
    pub fn list_resources(
        &self,
        server: Option<&str>,
    ) -> Result<Vec<McpResourceRecord>, RunnerError> {
        if let Some(filter) = server.map(str::trim).filter(|s| !s.is_empty()) {
            self.lookup_server(filter)?;
        }
        let mut out = Vec::new();
        for spec in &self.servers {
            if let Some(filter) = server.map(str::trim).filter(|s| !s.is_empty()) {
                if !spec.id.eq_ignore_ascii_case(filter) {
                    continue;
                }
            }
            if is_live_filesystem_server(&spec.id, &spec.target) {
                out.extend(self.list_filesystem_resources(&spec.id)?);
            } else {
                out.push(manifest_resource_record(spec));
            }
        }
        Ok(out)
    }

    /// Reads one resource from a server. Resolves filesystem URIs against
    /// `workspace_root` and returns manifest YAML for non-live servers.
    pub fn read_resource(
        &self,
        server: &str,
        uri: &str,
    ) -> Result<McpResourceContent, RunnerError> {
        let spec = self.lookup_server(server)?.clone();
        if is_live_filesystem_server(&spec.id, &spec.target) {
            return self.read_filesystem_resource(&spec.id, uri);
        }
        let expected = format!("mcp://manifest/{}", spec.id);
        if !uri.eq_ignore_ascii_case(&expected) {
            return Err(RunnerError::NotFound(format!(
                "MCP resource `{uri}` not found on server `{server}`"
            )));
        }
        let text = serde_json::to_string_pretty(&spec)
            .map_err(|e| RunnerError::Other(format!("serialize manifest: {e}")))?;
        Ok(McpResourceContent {
            server: spec.id.clone(),
            uri: uri.to_string(),
            parts: vec![McpResourceContentPart::Text {
                uri: uri.to_string(),
                mime_type: Some("application/yaml".to_string()),
                text,
            }],
        })
    }

    pub fn list_prompts(&self, server: &str) -> Result<Vec<McpPrompt>, RunnerError> {
        self.lookup_server(server)?;
        Err(RunnerError::Unsupported(format!(
            "MCP `prompts/list` is not implemented for server `{server}`",
        )))
    }

    pub fn get_prompt(
        &self,
        server: &str,
        name: &str,
        _args: Value,
    ) -> Result<McpPromptContent, RunnerError> {
        self.lookup_server(server)?;
        Err(RunnerError::Unsupported(format!(
            "MCP `prompts/get` for `{name}` on server `{server}` is not implemented",
        )))
    }

    fn lookup_server(&self, server: &str) -> Result<&McpServerSpec, RunnerError> {
        let trimmed = server.trim();
        self.servers
            .iter()
            .find(|spec| spec.id.eq_ignore_ascii_case(trimmed))
            .ok_or_else(|| {
                let available = self
                    .servers
                    .iter()
                    .map(|s| s.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                RunnerError::NotFound(format!(
                    "MCP server `{server}` not found. Available servers: {available}",
                ))
            })
    }

    fn list_filesystem_resources(
        &self,
        server: &str,
    ) -> Result<Vec<McpResourceRecord>, RunnerError> {
        let Some(root) = self.workspace_root.as_deref() else {
            return Ok(Vec::new());
        };
        let mut relative = Vec::new();
        collect_workspace_files(root, root, &mut relative)
            .map_err(|e| RunnerError::Mcp(format!("walk workspace {root:?}: {e}")))?;
        relative.sort();
        relative.truncate(200);
        Ok(relative
            .into_iter()
            .map(|rel| {
                let path = root.join(&rel);
                McpResourceRecord {
                    server: server.to_string(),
                    uri: format!("mcp://filesystem/{rel}"),
                    name: rel,
                    mime_type: Some(mime_type_for_path(&path)),
                    description: Some("Live filesystem resource".to_string()),
                }
            })
            .collect())
    }

    fn read_filesystem_resource(
        &self,
        server: &str,
        uri: &str,
    ) -> Result<McpResourceContent, RunnerError> {
        let root = self
            .workspace_root
            .as_deref()
            .ok_or_else(|| RunnerError::Mcp("filesystem MCP requires a workspace root".into()))?;
        let relative = uri
            .strip_prefix("mcp://filesystem/")
            .ok_or_else(|| {
                RunnerError::InvalidArgument(format!(
                    "filesystem MCP URI `{uri}` must use the `mcp://filesystem/` scheme",
                ))
            })?;
        let path = resolve_workspace_file(root, relative)
            .map_err(|e| RunnerError::Mcp(format!("resolve workspace file: {e}")))?;
        let bytes = fs::read(&path)
            .map_err(|e| RunnerError::Mcp(format!("read {}: {e}", path.display())))?;
        let mime_type = Some(mime_type_for_path(&path));
        let part = match String::from_utf8(bytes.clone()) {
            Ok(text) => McpResourceContentPart::Text {
                uri: uri.to_string(),
                mime_type,
                text,
            },
            Err(_) => McpResourceContentPart::Blob {
                uri: uri.to_string(),
                mime_type,
                bytes,
            },
        };
        Ok(McpResourceContent {
            server: server.to_string(),
            uri: uri.to_string(),
            parts: vec![part],
        })
    }
}

fn spec_to_info(spec: &McpServerSpec) -> McpServerInfo {
    McpServerInfo {
        id: spec.id.clone(),
        display_name: spec.display_name.clone(),
        transport: spec.transport.clone(),
        target: spec.target.clone(),
        description: spec.description.clone(),
    }
}

fn manifest_resource_record(spec: &McpServerSpec) -> McpResourceRecord {
    let description = if spec.description.is_empty() {
        Some("Configured MCP server manifest".to_string())
    } else {
        Some(spec.description.clone())
    };
    McpResourceRecord {
        server: spec.id.clone(),
        uri: format!("mcp://manifest/{}", spec.id),
        name: if spec.display_name.is_empty() {
            spec.id.clone()
        } else {
            spec.display_name.clone()
        },
        mime_type: Some("application/yaml".to_string()),
        description,
    }
}

/// Returns true when `spec` describes the built-in filesystem stub.
pub fn is_live_filesystem_server(id: &str, target: &str) -> bool {
    id.trim().eq_ignore_ascii_case("filesystem")
        || matches!(
            target.trim(),
            "builtin:filesystem" | "internal://filesystem" | "puffer-mcp-filesystem"
        )
}

fn collect_workspace_files(
    root: &Path,
    current: &Path,
    output: &mut Vec<String>,
) -> anyhow::Result<()> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect_workspace_files(root, &path, output)?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let relative = path.strip_prefix(root).unwrap_or(&path);
        output.push(relative.to_string_lossy().replace('\\', "/"));
    }
    Ok(())
}

fn resolve_workspace_file(root: &Path, relative: &str) -> anyhow::Result<PathBuf> {
    let candidate = root.join(relative);
    let canonical_root = fs::canonicalize(root).context("canonicalize workspace root")?;
    let ancestor = nearest_existing_ancestor(&candidate)
        .ok_or_else(|| anyhow::anyhow!("failed to resolve path {}", candidate.display()))?;
    let canonical_ancestor = fs::canonicalize(&ancestor).context("canonicalize ancestor")?;
    if !canonical_ancestor.starts_with(&canonical_root) {
        anyhow::bail!(
            "path {} resolves through symlink outside workspace {}",
            relative,
            root.display()
        );
    }
    Ok(candidate)
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

fn mime_type_for_path(path: &Path) -> String {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
    {
        "md" => "text/markdown",
        "json" => "application/json",
        "yaml" | "yml" => "application/yaml",
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "txt" => "text/plain",
        _ => "application/octet-stream",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_resources::McpServerSpec;

    fn fs_spec() -> McpServerSpec {
        McpServerSpec {
            id: "filesystem".into(),
            display_name: "Filesystem".into(),
            transport: "stdio".into(),
            endpoint: String::new(),
            target: "builtin:filesystem".into(),
            description: "Filesystem server".into(),
        }
    }

    fn manifest_spec(id: &str) -> McpServerSpec {
        McpServerSpec {
            id: id.into(),
            display_name: format!("{id} display"),
            transport: "stdio".into(),
            endpoint: String::new(),
            target: format!("{id}-target"),
            description: format!("{id} description"),
        }
    }

    #[test]
    fn list_servers_returns_runner_dtos() {
        let host = McpHost::new(vec![manifest_spec("docs"), fs_spec()], None);
        let servers = host.list_servers();
        assert_eq!(servers.len(), 2);
        assert_eq!(servers[0].id, "docs");
        assert_eq!(servers[1].id, "filesystem");
    }

    #[test]
    fn list_resources_walks_workspace_for_filesystem() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("guide.md"), "# Guide\n").unwrap();
        fs::write(temp.path().join("data.bin"), [0xff_u8, 0x00, 0x01]).unwrap();
        let host = McpHost::new(vec![fs_spec()], Some(temp.path().to_path_buf()));
        let records = host.list_resources(None).unwrap();
        let names: Vec<_> = records.iter().map(|r| r.uri.clone()).collect();
        assert!(names.iter().any(|u| u == "mcp://filesystem/guide.md"));
        assert!(names.iter().any(|u| u == "mcp://filesystem/data.bin"));
    }

    #[test]
    fn list_resources_filters_by_server_case_insensitive() {
        let host = McpHost::new(vec![manifest_spec("docs")], None);
        let records = host.list_resources(Some("DOCS")).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].uri, "mcp://manifest/docs");
    }

    #[test]
    fn list_resources_unknown_server_errors() {
        let host = McpHost::new(vec![manifest_spec("docs")], None);
        let err = host.list_resources(Some("missing")).unwrap_err();
        assert!(matches!(err, RunnerError::NotFound(_)));
    }

    #[test]
    fn read_resource_returns_manifest_for_non_live_server() {
        let host = McpHost::new(vec![manifest_spec("docs")], None);
        let content = host
            .read_resource("docs", "mcp://manifest/docs")
            .unwrap();
        assert_eq!(content.server, "docs");
        assert!(matches!(
            content.parts[0],
            McpResourceContentPart::Text { .. }
        ));
    }

    #[test]
    fn read_resource_reads_filesystem_text() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("hello.txt"), "hi").unwrap();
        let host = McpHost::new(vec![fs_spec()], Some(temp.path().to_path_buf()));
        let content = host
            .read_resource("filesystem", "mcp://filesystem/hello.txt")
            .unwrap();
        match &content.parts[0] {
            McpResourceContentPart::Text { text, .. } => assert_eq!(text, "hi"),
            other => panic!("expected text content, got {other:?}"),
        }
    }

    #[test]
    fn read_resource_returns_blob_for_binary_files() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("data.bin"), [0xff_u8, 0x00, 0x01]).unwrap();
        let host = McpHost::new(vec![fs_spec()], Some(temp.path().to_path_buf()));
        let content = host
            .read_resource("filesystem", "mcp://filesystem/data.bin")
            .unwrap();
        match &content.parts[0] {
            McpResourceContentPart::Blob { bytes, .. } => {
                assert_eq!(bytes, &vec![0xff_u8, 0x00, 0x01]);
            }
            other => panic!("expected blob, got {other:?}"),
        }
    }

    #[test]
    fn list_tools_unsupported_for_known_server() {
        let host = McpHost::new(vec![manifest_spec("docs")], None);
        let err = host.list_tools("docs").unwrap_err();
        assert!(matches!(err, RunnerError::Unsupported(_)));
    }

    #[test]
    fn list_tools_unknown_server_errors_with_not_found() {
        let host = McpHost::new(vec![manifest_spec("docs")], None);
        let err = host.list_tools("missing").unwrap_err();
        assert!(matches!(err, RunnerError::NotFound(_)));
    }
}

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
//! `McpHost` keeps the [`McpServerSpec`] roster eagerly (the resource walker
//! and manifest fallback need it for synchronous lookup) and lazily owns an
//! [`McpConnectionManager`] that drives real subprocess MCP servers via
//! `rmcp`. The connection manager is shared (`Arc`) so cloning the host is
//! cheap and clones see the same live connections.

use anyhow::Context;
use puffer_mcp_oauth::{default_token_dir, PersistedTokens};
use puffer_resources::McpServerSpec;
use puffer_runner_api::{
    ChunkSink, DeclineAllElicitations, ElicitationHandler, McpPrompt, McpPromptContent,
    McpResourceContent, McpResourceContentPart, McpResourceRecord, McpResult, McpServerInfo,
    McpTool, OAuthStatus, OAuthTokensPayload, RunnerError,
};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::connection_manager::{entry_from_spec, McpConnectionManager};

/// Owns the MCP server roster and any live transports the runner needs to
/// satisfy `ToolRunner`'s 7 MCP methods.
#[derive(Debug, Clone)]
pub struct McpHost {
    servers: Vec<McpServerSpec>,
    workspace_root: Option<PathBuf>,
    connections: Arc<McpConnectionManager>,
    /// Strategy for fielding server-initiated `elicitation/create` requests.
    /// Cloned into the connection manager when the host (re)builds it; kept
    /// here so callers can swap it via [`McpHost::with_elicitation_handler`]
    /// without losing the configured server roster.
    elicitation: Arc<dyn ElicitationHandler>,
    /// Optional override for OAuth token storage location. Tests pin this
    /// to a `TempDir`; production picks the default
    /// (`<config>/puffer/mcp-tokens`).
    oauth_token_dir: Option<PathBuf>,
}

impl Default for McpHost {
    fn default() -> Self {
        Self {
            servers: Vec::new(),
            workspace_root: None,
            connections: Arc::new(McpConnectionManager::default()),
            elicitation: Arc::new(DeclineAllElicitations),
            oauth_token_dir: None,
        }
    }
}

impl McpHost {
    /// Builds a host from a list of MCP manifests plus the optional workspace
    /// root used by the built-in `filesystem` server.
    pub fn new(servers: Vec<McpServerSpec>, workspace_root: Option<PathBuf>) -> Self {
        Self::with_elicitation(servers, workspace_root, Arc::new(DeclineAllElicitations))
    }

    /// Like [`McpHost::new`] but installs a custom elicitation handler on
    /// the underlying connection manager.
    pub fn with_elicitation(
        servers: Vec<McpServerSpec>,
        workspace_root: Option<PathBuf>,
        elicitation: Arc<dyn ElicitationHandler>,
    ) -> Self {
        Self::with_elicitation_and_oauth_dir(servers, workspace_root, elicitation, None)
    }

    /// All-args constructor that also pins the OAuth token storage dir.
    pub fn with_elicitation_and_oauth_dir(
        servers: Vec<McpServerSpec>,
        workspace_root: Option<PathBuf>,
        elicitation: Arc<dyn ElicitationHandler>,
        oauth_token_dir: Option<PathBuf>,
    ) -> Self {
        let entries = servers.iter().filter_map(entry_from_spec);
        let mut manager = McpConnectionManager::with_servers(entries)
            .with_elicitation_handler(Arc::clone(&elicitation));
        if let Some(dir) = oauth_token_dir.clone() {
            manager = manager.with_oauth_token_dir(dir);
        }
        let connections = Arc::new(manager);
        Self {
            servers,
            workspace_root,
            connections,
            elicitation,
            oauth_token_dir,
        }
    }

    /// Pin the OAuth token storage dir. Re-builds the underlying connection
    /// manager so future connects pick up the new location.
    pub fn with_oauth_token_dir(mut self, dir: PathBuf) -> Self {
        self.oauth_token_dir = Some(dir.clone());
        let entries = self.servers.iter().filter_map(entry_from_spec);
        self.connections = Arc::new(
            McpConnectionManager::with_servers(entries)
                .with_elicitation_handler(Arc::clone(&self.elicitation))
                .with_oauth_token_dir(dir),
        );
        self
    }

    /// Returns the elicitation handler currently associated with this host.
    /// Used by `LocalToolRunner` when it needs to rebuild the host with a
    /// different workspace root or server roster while keeping the same
    /// handler.
    pub fn elicitation_handler(&self) -> Arc<dyn ElicitationHandler> {
        Arc::clone(&self.elicitation)
    }

    /// Replaces the elicitation handler. Re-builds the underlying connection
    /// manager so future connections pick up the new handler; existing live
    /// connections keep the old one (drop the host to reset them).
    pub fn with_elicitation_handler(mut self, handler: Arc<dyn ElicitationHandler>) -> Self {
        let entries = self.servers.iter().filter_map(entry_from_spec);
        self.connections = Arc::new(
            McpConnectionManager::with_servers(entries)
                .with_elicitation_handler(Arc::clone(&handler)),
        );
        self.elicitation = handler;
        self
    }

    /// Returns the configured MCP servers as runner-shaped DTOs.
    pub fn list_servers(&self) -> Vec<McpServerInfo> {
        self.servers.iter().map(spec_to_info).collect()
    }

    /// Returns the workspace root used by built-in transports.
    pub fn workspace_root(&self) -> Option<&Path> {
        self.workspace_root.as_deref()
    }

    /// Returns the OAuth token storage directory, if one was pinned via
    /// [`McpHost::with_oauth_token_dir`] or
    /// [`McpHost::with_elicitation_and_oauth_dir`]. Used by the
    /// hot-reload pathway to preserve a custom token dir across an
    /// `McpHost` swap so persisted tokens stay reachable.
    pub fn oauth_token_dir(&self) -> Option<&Path> {
        self.oauth_token_dir.as_deref()
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

    /// Lists the tools advertised by `server`.
    ///
    /// For the built-in filesystem stub there are no callable tools (it only
    /// serves resources), so we keep the historical `Unsupported` reply.
    /// Every other configured server is dispatched to the connection
    /// manager, which lazily spawns the underlying subprocess on first use.
    pub fn list_tools(&self, server: &str) -> Result<Vec<McpTool>, RunnerError> {
        let spec = self.lookup_server(server)?;
        if is_live_filesystem_server(&spec.id, &spec.target) {
            return Err(RunnerError::Unsupported(format!(
                "MCP `tools/list` is not implemented for built-in server `{server}`",
            )));
        }
        self.connections.list_tools(&spec.id)
    }

    /// Calls `tool` on `server` with the supplied JSON arguments.
    ///
    /// The built-in filesystem server has no callable tools, so it keeps
    /// returning `Unsupported`. Configured subprocess servers route
    /// through the connection manager — `sink` receives any
    /// `notifications/progress` events the server emits during the call.
    pub fn call_tool(
        &self,
        server: &str,
        tool: &str,
        args: Value,
        sink: &mut dyn ChunkSink,
    ) -> Result<McpResult, RunnerError> {
        let spec = self.lookup_server(server)?;
        if is_live_filesystem_server(&spec.id, &spec.target) {
            return Err(RunnerError::Unsupported(format!(
                "MCP `tools/call` for `{tool}` on built-in server `{server}` is not implemented",
            )));
        }
        self.connections.call_tool(&spec.id, tool, args, sink)
    }

    /// Lists resources across one or all servers. The built-in `filesystem`
    /// transport walks `workspace_root`; every other server is queried over
    /// stdio through the connection manager.
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
                out.extend(self.connections.list_resources(&spec.id)?);
            }
        }
        Ok(out)
    }

    /// Reads one resource from a server. Resolves filesystem URIs against
    /// `workspace_root`; every other server is queried over stdio through
    /// the connection manager.
    pub fn read_resource(
        &self,
        server: &str,
        uri: &str,
    ) -> Result<McpResourceContent, RunnerError> {
        let spec = self.lookup_server(server)?.clone();
        if is_live_filesystem_server(&spec.id, &spec.target) {
            return self.read_filesystem_resource(&spec.id, uri);
        }
        self.connections.read_resource(&spec.id, uri)
    }

    pub fn list_prompts(&self, server: &str) -> Result<Vec<McpPrompt>, RunnerError> {
        let spec = self.lookup_server(server)?;
        if is_live_filesystem_server(&spec.id, &spec.target) {
            return Err(RunnerError::Unsupported(format!(
                "MCP `prompts/list` is not implemented for built-in server `{server}`",
            )));
        }
        self.connections.list_prompts(&spec.id)
    }

    pub fn get_prompt(
        &self,
        server: &str,
        name: &str,
        args: Value,
    ) -> Result<McpPromptContent, RunnerError> {
        let spec = self.lookup_server(server)?;
        if is_live_filesystem_server(&spec.id, &spec.target) {
            return Err(RunnerError::Unsupported(format!(
                "MCP `prompts/get` for `{name}` on built-in server `{server}` is not implemented",
            )));
        }
        self.connections.get_prompt(&spec.id, name, args)
    }

    /// Persist a token bundle minted by `puffer-cli`'s interactive OAuth
    /// flow to the on-disk credential store. The caller (the local runner's
    /// `push_oauth_tokens` impl) has already verified the server is
    /// configured; this method only handles the persistence + URL-resolution
    /// step. No network IO.
    pub fn push_oauth_tokens(
        &self,
        server: &str,
        tokens: OAuthTokensPayload,
    ) -> Result<(), RunnerError> {
        let spec = self.lookup_server(server)?;
        // Trust the server_url the caller provided when it differs from the
        // configured one (some manifests put the auth issuer on a separate
        // host from the MCP endpoint); fall back to the manifest URL when
        // the caller didn't set it.
        let server_url = if tokens.server_url.trim().is_empty() {
            http_url_for_server(spec)?
        } else {
            tokens.server_url.clone()
        };
        let server_id = if tokens.server_id.trim().is_empty() {
            spec.id.clone()
        } else {
            tokens.server_id.clone()
        };
        let payload = PersistedTokens {
            server_id,
            server_url,
            client_id: tokens.client_id,
            client_secret: tokens.client_secret,
            access_token: tokens.access_token,
            token_type: tokens.token_type,
            refresh_token: tokens.refresh_token,
            scopes: tokens.scopes,
            expires_at_ms: tokens.expires_at_ms,
        };
        let token_dir = self
            .oauth_token_dir
            .clone()
            .unwrap_or_else(default_token_dir);
        payload
            .write_to(&token_dir)
            .map_err(|e| RunnerError::Other(format!("write OAuth tokens for `{server}`: {e}")))
    }

    /// Returns the OAuth status for `server`. Used by `puffer mcp
    /// login-status`. No secret material is returned.
    pub fn oauth_status(&self, server: &str) -> Result<OAuthStatus, RunnerError> {
        let spec = self.lookup_server(server)?;
        let server_url = http_url_for_server(spec)?;
        let token_dir = self
            .oauth_token_dir
            .clone()
            .unwrap_or_else(default_token_dir);
        let persisted = PersistedTokens::read_from(&token_dir, &spec.id, &server_url)
            .map_err(|e| RunnerError::Other(format!("read OAuth tokens for `{server}`: {e}")))?;
        Ok(match persisted {
            None => OAuthStatus::Absent,
            Some(p) => OAuthStatus::Present {
                expires_at_ms: p.expires_at_ms,
                has_refresh: p.refresh_token.is_some(),
                scopes: p.scopes,
            },
        })
    }

    /// Drops any persisted OAuth tokens for `server`. Idempotent: if no
    /// tokens are stored, returns `Ok(())`.
    pub fn clear_oauth_tokens(&self, server: &str) -> Result<(), RunnerError> {
        let spec = self.lookup_server(server)?;
        let server_url = http_url_for_server(spec)?;
        let token_dir = self
            .oauth_token_dir
            .clone()
            .unwrap_or_else(default_token_dir);
        PersistedTokens::delete_from(&token_dir, &spec.id, &server_url)
            .map(|_| ())
            .map_err(|e| RunnerError::Other(format!("remove OAuth tokens for `{server}`: {e}")))
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
        let relative = uri.strip_prefix("mcp://filesystem/").ok_or_else(|| {
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

/// Resolve the HTTP URL associated with an MCP server spec, mirroring the
/// `puffer-cli` resolver. The `endpoint` field is the historical home for
/// HTTP/SSE entries (used by `puffer mcp add`); the `target` field is what
/// pass-1.5d manifests use. Stdio servers don't have a URL — those are
/// rejected as `InvalidArgument` because OAuth doesn't apply.
pub fn http_url_for_server(spec: &McpServerSpec) -> Result<String, RunnerError> {
    let raw = if !spec.endpoint.trim().is_empty() {
        spec.endpoint.trim()
    } else {
        spec.target.trim()
    };
    if raw.is_empty() {
        return Err(RunnerError::InvalidArgument(format!(
            "MCP server `{}` has no HTTP target/endpoint",
            spec.id
        )));
    }
    Ok(raw.to_string())
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
            env: Default::default(),
            inherit_env: true,
            timeout: None,
            connect_timeout: None,
            headers: Default::default(),
            oauth: None,
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
            env: Default::default(),
            inherit_env: true,
            timeout: None,
            connect_timeout: None,
            headers: Default::default(),
            oauth: None,
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
    fn list_resources_unknown_server_errors() {
        let host = McpHost::new(vec![manifest_spec("docs")], None);
        let err = host.list_resources(Some("missing")).unwrap_err();
        assert!(matches!(err, RunnerError::NotFound(_)));
    }

    #[test]
    fn list_resources_for_subprocess_server_routes_through_connection_manager() {
        // The `docs` manifest points at a binary that does not exist, so the
        // connection manager fails to spawn — the host should bubble that up
        // as an `Mcp` error instead of returning a synthetic manifest record.
        let host = McpHost::new(vec![manifest_spec("docs")], None);
        let err = host.list_resources(Some("docs")).unwrap_err();
        assert!(
            matches!(err, RunnerError::Mcp(_)),
            "expected Mcp error from failed spawn, got {err:?}"
        );
    }

    #[test]
    fn read_resource_for_subprocess_server_routes_through_connection_manager() {
        let host = McpHost::new(vec![manifest_spec("docs")], None);
        let err = host
            .read_resource("docs", "mcp://example/anything")
            .unwrap_err();
        assert!(
            matches!(err, RunnerError::Mcp(_)),
            "expected Mcp error from failed spawn, got {err:?}"
        );
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
    fn list_tools_unsupported_for_filesystem_stub() {
        // The built-in filesystem server has no callable tools; it is
        // expected to keep returning `Unsupported` for `tools/list`.
        let host = McpHost::new(vec![fs_spec()], None);
        let err = host.list_tools("filesystem").unwrap_err();
        assert!(matches!(err, RunnerError::Unsupported(_)));
    }

    #[test]
    fn list_tools_for_subprocess_server_routes_through_connection_manager() {
        // The `docs` manifest points at a binary that does not exist, so
        // the launcher fails to spawn — the host should surface that as an
        // `Mcp` error rather than the historical `Unsupported`.
        let host = McpHost::new(vec![manifest_spec("docs")], None);
        let err = host.list_tools("docs").unwrap_err();
        assert!(
            matches!(err, RunnerError::Mcp(_)),
            "expected Mcp error from failed spawn, got {err:?}"
        );
    }

    #[test]
    fn list_tools_unknown_server_errors_with_not_found() {
        let host = McpHost::new(vec![manifest_spec("docs")], None);
        let err = host.list_tools("missing").unwrap_err();
        assert!(matches!(err, RunnerError::NotFound(_)));
    }
}

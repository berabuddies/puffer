//! Conversions between the generated proto types and the
//! [`puffer_runner_api`] DTOs. Kept here so the api crate stays free of
//! tonic / prost dependencies.

use std::path::PathBuf;

use puffer_runner_api::{
    DirEntry, McpPrompt, McpPromptArgument, McpPromptContent, McpPromptMessage, McpResourceContent,
    McpResourceContentPart, McpResourceRecord, McpResult, McpServerInfo, McpTool, OAuthStatus,
    OAuthTokensPayload, ReadStateUpdate, RunnerCapabilities, RunnerError, ToolRequest, ToolResult,
};

use crate::proto;

// --- ToolRequest -------------------------------------------------------

pub(crate) fn to_proto_tool_request(req: &ToolRequest) -> proto::ToolRequest {
    proto::ToolRequest {
        tool_id: req.tool_id.clone(),
        cwd: req.cwd.display().to_string(),
        working_dirs: req
            .working_dirs
            .iter()
            .map(|p| p.display().to_string())
            .collect(),
        allow_all_paths: req.allow_all_paths,
        input_json: req.input.to_string(),
        session_id: req.session_id.clone(),
    }
}

pub(crate) fn from_proto_tool_request(req: proto::ToolRequest) -> Result<ToolRequest, RunnerError> {
    let input = if req.input_json.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_str(&req.input_json)
            .map_err(|e| RunnerError::InvalidArgument(format!("input_json: {e}")))?
    };
    Ok(ToolRequest {
        tool_id: req.tool_id,
        cwd: PathBuf::from(req.cwd),
        working_dirs: req.working_dirs.into_iter().map(PathBuf::from).collect(),
        allow_all_paths: req.allow_all_paths,
        input,
        session_id: req.session_id,
    })
}

// --- ToolResult --------------------------------------------------------

pub(crate) fn to_proto_tool_completed(result: &ToolResult) -> proto::ToolCompleted {
    proto::ToolCompleted {
        tool_id: result.tool_id.clone(),
        success: result.success,
        stdout: result.stdout.clone(),
        stderr: result.stderr.clone(),
        metadata_json: result.metadata.to_string(),
        read_state_updates: result
            .read_state_updates
            .iter()
            .map(to_proto_read_state_update)
            .collect(),
    }
}

pub(crate) fn from_proto_tool_completed(
    value: proto::ToolCompleted,
) -> Result<ToolResult, RunnerError> {
    let metadata = if value.metadata_json.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_str(&value.metadata_json)
            .map_err(|e| RunnerError::Other(format!("metadata_json: {e}")))?
    };
    let mut updates = Vec::with_capacity(value.read_state_updates.len());
    for u in value.read_state_updates {
        updates.push(from_proto_read_state_update(u)?);
    }
    Ok(ToolResult {
        tool_id: value.tool_id,
        success: value.success,
        stdout: value.stdout,
        stderr: value.stderr,
        metadata,
        read_state_updates: updates,
    })
}

pub(crate) fn to_proto_read_state_update(u: &ReadStateUpdate) -> proto::ReadStateUpdate {
    proto::ReadStateUpdate {
        path: u.path.display().to_string(),
        timestamp_ms: u.timestamp_ms.to_string(),
        is_partial_view: u.is_partial_view,
    }
}

pub(crate) fn from_proto_read_state_update(
    u: proto::ReadStateUpdate,
) -> Result<ReadStateUpdate, RunnerError> {
    let timestamp_ms: u128 = u
        .timestamp_ms
        .parse()
        .map_err(|e| RunnerError::Other(format!("timestamp_ms: {e}")))?;
    Ok(ReadStateUpdate {
        path: PathBuf::from(u.path),
        timestamp_ms,
        is_partial_view: u.is_partial_view,
    })
}

// --- Capabilities ------------------------------------------------------

pub(crate) fn to_proto_capabilities(c: &RunnerCapabilities) -> proto::RunnerCapabilities {
    proto::RunnerCapabilities {
        backend: c.backend.clone(),
        version: c.version.clone(),
        supported_tools: c.supported_tools.clone(),
        mcp_supported: c.mcp_supported,
    }
}

pub(crate) fn from_proto_capabilities(c: proto::RunnerCapabilities) -> RunnerCapabilities {
    RunnerCapabilities {
        backend: c.backend,
        version: c.version,
        supported_tools: c.supported_tools,
        mcp_supported: c.mcp_supported,
    }
}

// --- DirEntry ----------------------------------------------------------

pub(crate) fn to_proto_dir_entry(entry: &DirEntry) -> proto::DirEntry {
    proto::DirEntry {
        path: entry.path.display().to_string(),
        is_dir: entry.is_dir,
        is_file: entry.is_file,
        is_symlink: entry.is_symlink,
    }
}

pub(crate) fn from_proto_dir_entry(entry: proto::DirEntry) -> DirEntry {
    DirEntry {
        path: PathBuf::from(entry.path),
        is_dir: entry.is_dir,
        is_file: entry.is_file,
        is_symlink: entry.is_symlink,
    }
}

// --- MCP servers / tools ----------------------------------------------

pub(crate) fn to_proto_mcp_server(info: &McpServerInfo) -> proto::McpServerInfo {
    proto::McpServerInfo {
        id: info.id.clone(),
        display_name: info.display_name.clone(),
        transport: info.transport.clone(),
        target: info.target.clone(),
        description: info.description.clone(),
    }
}

pub(crate) fn from_proto_mcp_server(info: proto::McpServerInfo) -> McpServerInfo {
    McpServerInfo {
        id: info.id,
        display_name: info.display_name,
        transport: info.transport,
        target: info.target,
        description: info.description,
    }
}

pub(crate) fn to_proto_mcp_tool(tool: &McpTool) -> proto::McpTool {
    proto::McpTool {
        name: tool.name.clone(),
        description: tool.description.clone(),
        input_schema_json: tool.input_schema.as_ref().map(|v| v.to_string()),
    }
}

pub(crate) fn from_proto_mcp_tool(tool: proto::McpTool) -> Result<McpTool, RunnerError> {
    let input_schema = match tool.input_schema_json {
        Some(s) if !s.is_empty() => Some(
            serde_json::from_str(&s)
                .map_err(|e| RunnerError::Other(format!("input_schema_json: {e}")))?,
        ),
        _ => None,
    };
    Ok(McpTool {
        name: tool.name,
        description: tool.description,
        input_schema,
    })
}

pub(crate) fn to_proto_mcp_result(r: &McpResult) -> proto::McpToolResult {
    proto::McpToolResult {
        server: r.server.clone(),
        tool: r.tool.clone(),
        success: r.success,
        stdout: r.stdout.clone(),
        stderr: r.stderr.clone(),
        metadata_json: r.metadata.to_string(),
    }
}

pub(crate) fn from_proto_mcp_result(r: proto::McpToolResult) -> Result<McpResult, RunnerError> {
    let metadata = if r.metadata_json.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_str(&r.metadata_json)
            .map_err(|e| RunnerError::Other(format!("metadata_json: {e}")))?
    };
    Ok(McpResult {
        server: r.server,
        tool: r.tool,
        success: r.success,
        stdout: r.stdout,
        stderr: r.stderr,
        metadata,
    })
}

pub(crate) fn to_proto_mcp_resource_record(r: &McpResourceRecord) -> proto::McpResourceRecord {
    proto::McpResourceRecord {
        server: r.server.clone(),
        uri: r.uri.clone(),
        name: r.name.clone(),
        mime_type: r.mime_type.clone(),
        description: r.description.clone(),
    }
}

pub(crate) fn from_proto_mcp_resource_record(r: proto::McpResourceRecord) -> McpResourceRecord {
    McpResourceRecord {
        server: r.server,
        uri: r.uri,
        name: r.name,
        mime_type: r.mime_type,
        description: r.description,
    }
}

pub(crate) fn to_proto_mcp_resource_content(c: &McpResourceContent) -> proto::McpResourceContent {
    proto::McpResourceContent {
        server: c.server.clone(),
        uri: c.uri.clone(),
        parts: c.parts.iter().map(to_proto_mcp_resource_part).collect(),
    }
}

pub(crate) fn from_proto_mcp_resource_content(
    c: proto::McpResourceContent,
) -> Result<McpResourceContent, RunnerError> {
    let mut parts = Vec::with_capacity(c.parts.len());
    for p in c.parts {
        parts.push(from_proto_mcp_resource_part(p)?);
    }
    Ok(McpResourceContent {
        server: c.server,
        uri: c.uri,
        parts,
    })
}

pub(crate) fn to_proto_mcp_resource_part(
    p: &McpResourceContentPart,
) -> proto::McpResourceContentPart {
    match p {
        McpResourceContentPart::Text {
            uri,
            mime_type,
            text,
        } => proto::McpResourceContentPart {
            kind: "text".into(),
            uri: uri.clone(),
            mime_type: mime_type.clone(),
            text: text.clone(),
            bytes: Vec::new(),
        },
        McpResourceContentPart::Blob {
            uri,
            mime_type,
            bytes,
        } => proto::McpResourceContentPart {
            kind: "blob".into(),
            uri: uri.clone(),
            mime_type: mime_type.clone(),
            text: String::new(),
            bytes: bytes.clone(),
        },
    }
}

pub(crate) fn from_proto_mcp_resource_part(
    p: proto::McpResourceContentPart,
) -> Result<McpResourceContentPart, RunnerError> {
    match p.kind.as_str() {
        "text" => Ok(McpResourceContentPart::Text {
            uri: p.uri,
            mime_type: p.mime_type,
            text: p.text,
        }),
        "blob" => Ok(McpResourceContentPart::Blob {
            uri: p.uri,
            mime_type: p.mime_type,
            bytes: p.bytes,
        }),
        other => Err(RunnerError::Other(format!(
            "unknown McpResourceContentPart kind `{other}`"
        ))),
    }
}

pub(crate) fn to_proto_mcp_prompt(p: &McpPrompt) -> proto::McpPrompt {
    proto::McpPrompt {
        name: p.name.clone(),
        description: p.description.clone(),
        arguments: p
            .arguments
            .iter()
            .map(|a| proto::McpPromptArgument {
                name: a.name.clone(),
                description: a.description.clone(),
                required: a.required,
            })
            .collect(),
    }
}

pub(crate) fn from_proto_mcp_prompt(p: proto::McpPrompt) -> McpPrompt {
    McpPrompt {
        name: p.name,
        description: p.description,
        arguments: p
            .arguments
            .into_iter()
            .map(|a| McpPromptArgument {
                name: a.name,
                description: a.description,
                required: a.required,
            })
            .collect(),
    }
}

pub(crate) fn to_proto_mcp_prompt_content(c: &McpPromptContent) -> proto::McpPromptContent {
    proto::McpPromptContent {
        server: c.server.clone(),
        name: c.name.clone(),
        messages: c
            .messages
            .iter()
            .map(|m| proto::McpPromptMessage {
                role: m.role.clone(),
                text: m.text.clone(),
            })
            .collect(),
    }
}

pub(crate) fn from_proto_mcp_prompt_content(c: proto::McpPromptContent) -> McpPromptContent {
    McpPromptContent {
        server: c.server,
        name: c.name,
        messages: c
            .messages
            .into_iter()
            .map(|m| McpPromptMessage {
                role: m.role,
                text: m.text,
            })
            .collect(),
    }
}

// --- OAuth ------------------------------------------------------------

pub(crate) fn oauth_payload_to_proto(p: &OAuthTokensPayload) -> proto::OAuthTokensPayload {
    proto::OAuthTokensPayload {
        server_id: p.server_id.clone(),
        server_url: p.server_url.clone(),
        client_id: p.client_id.clone(),
        client_secret: p.client_secret.clone(),
        access_token: p.access_token.clone(),
        token_type: p.token_type.clone(),
        refresh_token: p.refresh_token.clone(),
        scopes: p.scopes.clone(),
        expires_at_ms: p.expires_at_ms,
    }
}

pub(crate) fn oauth_payload_from_proto(p: proto::OAuthTokensPayload) -> OAuthTokensPayload {
    OAuthTokensPayload {
        server_id: p.server_id,
        server_url: p.server_url,
        client_id: p.client_id,
        client_secret: p.client_secret,
        access_token: p.access_token,
        token_type: p.token_type,
        refresh_token: p.refresh_token,
        scopes: p.scopes,
        expires_at_ms: p.expires_at_ms,
    }
}

pub(crate) fn oauth_status_to_proto(status: OAuthStatus) -> proto::OAuthStatusResponse {
    let state = match status {
        OAuthStatus::Absent => proto::o_auth_status_response::State::Absent(proto::Empty {}),
        OAuthStatus::Present {
            expires_at_ms,
            has_refresh,
            scopes,
        } => proto::o_auth_status_response::State::Present(proto::OAuthStatusPresent {
            expires_at_ms,
            has_refresh,
            scopes,
        }),
    };
    proto::OAuthStatusResponse { state: Some(state) }
}

pub(crate) fn oauth_status_from_proto(resp: proto::OAuthStatusResponse) -> OAuthStatus {
    match resp.state {
        Some(proto::o_auth_status_response::State::Present(p)) => OAuthStatus::Present {
            expires_at_ms: p.expires_at_ms,
            has_refresh: p.has_refresh,
            scopes: p.scopes,
        },
        // Absent or empty: treat as Absent (the safest default).
        _ => OAuthStatus::Absent,
    }
}

// --- RunnerError ↔ tonic::Status --------------------------------------

pub(crate) fn runner_error_to_status(err: &RunnerError) -> tonic::Status {
    use tonic::Code;
    let code = match err {
        RunnerError::NotFound(_) => Code::NotFound,
        RunnerError::PermissionDenied(_) => Code::PermissionDenied,
        RunnerError::Unsupported(_) => Code::Unimplemented,
        RunnerError::InvalidArgument(_) => Code::InvalidArgument,
        RunnerError::Transport(_) => Code::Unavailable,
        RunnerError::Mcp(_) => Code::Internal,
        RunnerError::OAuthRequired { .. } => Code::FailedPrecondition,
        RunnerError::Execution(_) => Code::Internal,
        RunnerError::Other(_) => Code::Internal,
    };
    let mut status = tonic::Status::new(code, err.to_string());
    if let RunnerError::OAuthRequired {
        server_id,
        authorization_url,
    } = err
    {
        // Round-trip the typed shape via custom metadata keys so the
        // client (`status_to_runner_error`) can rebuild the variant.
        // ASCII-only values; URLs are guaranteed printable so the
        // tonic header parser won't reject them.
        if let Ok(v) = tonic::metadata::AsciiMetadataValue::try_from(server_id.as_str()) {
            status.metadata_mut().insert("x-puffer-oauth-server", v);
        }
        if let Some(url) = authorization_url {
            if let Ok(v) = tonic::metadata::AsciiMetadataValue::try_from(url.as_str()) {
                status.metadata_mut().insert("x-puffer-oauth-url", v);
            }
        }
    }
    status
}

pub(crate) fn status_to_runner_error(status: tonic::Status) -> RunnerError {
    use tonic::Code;
    let msg = status.message().to_string();
    if matches!(status.code(), Code::FailedPrecondition) {
        if let Some(server_id) = status
            .metadata()
            .get("x-puffer-oauth-server")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
        {
            let authorization_url = status
                .metadata()
                .get("x-puffer-oauth-url")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            return RunnerError::OAuthRequired {
                server_id,
                authorization_url,
            };
        }
    }
    match status.code() {
        Code::NotFound => RunnerError::NotFound(msg),
        Code::PermissionDenied => RunnerError::PermissionDenied(msg),
        Code::Unimplemented => RunnerError::Unsupported(msg),
        Code::InvalidArgument => RunnerError::InvalidArgument(msg),
        Code::Unavailable => RunnerError::Transport(msg),
        _ => RunnerError::Other(msg),
    }
}

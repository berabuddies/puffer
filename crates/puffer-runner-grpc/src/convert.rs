//! Conversions between the generated proto types and the
//! [`puffer_runner_api`] DTOs. Kept here so the api crate stays free of
//! tonic / prost dependencies.

use std::path::PathBuf;

use puffer_runner_api::{
    DirEntry, McpPrompt, McpPromptArgument, McpPromptContent, McpPromptMessage, McpResourceContent,
    McpResourceContentPart, McpResourceRecord, McpResult, McpServerInfo, McpTool,
    PermissionDecision, PermissionRequest, ReadStateUpdate, RunnerCapabilities, RunnerError,
    ToolRequest, ToolResult,
};

use crate::proto;

// --- ToolRequest -------------------------------------------------------

pub fn to_proto_tool_request(req: &ToolRequest) -> proto::ToolRequest {
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

pub fn from_proto_tool_request(req: proto::ToolRequest) -> Result<ToolRequest, RunnerError> {
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

pub fn to_proto_tool_completed(result: &ToolResult) -> proto::ToolCompleted {
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

pub fn from_proto_tool_completed(value: proto::ToolCompleted) -> Result<ToolResult, RunnerError> {
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

pub fn to_proto_read_state_update(u: &ReadStateUpdate) -> proto::ReadStateUpdate {
    proto::ReadStateUpdate {
        path: u.path.display().to_string(),
        timestamp_ms: u.timestamp_ms.to_string(),
        is_partial_view: u.is_partial_view,
    }
}

pub fn from_proto_read_state_update(
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

pub fn to_proto_capabilities(c: &RunnerCapabilities) -> proto::RunnerCapabilities {
    proto::RunnerCapabilities {
        backend: c.backend.clone(),
        version: c.version.clone(),
        supported_tools: c.supported_tools.clone(),
        mcp_supported: c.mcp_supported,
        permission_relay_supported: c.permission_relay_supported,
    }
}

pub fn from_proto_capabilities(c: proto::RunnerCapabilities) -> RunnerCapabilities {
    RunnerCapabilities {
        backend: c.backend,
        version: c.version,
        supported_tools: c.supported_tools,
        mcp_supported: c.mcp_supported,
        permission_relay_supported: c.permission_relay_supported,
    }
}

// --- DirEntry ----------------------------------------------------------

pub fn to_proto_dir_entry(entry: &DirEntry) -> proto::DirEntry {
    proto::DirEntry {
        path: entry.path.display().to_string(),
        is_dir: entry.is_dir,
        is_file: entry.is_file,
        is_symlink: entry.is_symlink,
    }
}

pub fn from_proto_dir_entry(entry: proto::DirEntry) -> DirEntry {
    DirEntry {
        path: PathBuf::from(entry.path),
        is_dir: entry.is_dir,
        is_file: entry.is_file,
        is_symlink: entry.is_symlink,
    }
}

// --- MCP servers / tools ----------------------------------------------

pub fn to_proto_mcp_server(info: &McpServerInfo) -> proto::McpServerInfo {
    proto::McpServerInfo {
        id: info.id.clone(),
        display_name: info.display_name.clone(),
        transport: info.transport.clone(),
        target: info.target.clone(),
        description: info.description.clone(),
    }
}

pub fn from_proto_mcp_server(info: proto::McpServerInfo) -> McpServerInfo {
    McpServerInfo {
        id: info.id,
        display_name: info.display_name,
        transport: info.transport,
        target: info.target,
        description: info.description,
    }
}

pub fn to_proto_mcp_tool(tool: &McpTool) -> proto::McpTool {
    proto::McpTool {
        name: tool.name.clone(),
        description: tool.description.clone(),
        input_schema_json: tool.input_schema.as_ref().map(|v| v.to_string()),
    }
}

pub fn from_proto_mcp_tool(tool: proto::McpTool) -> Result<McpTool, RunnerError> {
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

pub fn to_proto_mcp_result(r: &McpResult) -> proto::McpToolResult {
    proto::McpToolResult {
        server: r.server.clone(),
        tool: r.tool.clone(),
        success: r.success,
        stdout: r.stdout.clone(),
        stderr: r.stderr.clone(),
        metadata_json: r.metadata.to_string(),
    }
}

pub fn from_proto_mcp_result(r: proto::McpToolResult) -> Result<McpResult, RunnerError> {
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

pub fn to_proto_mcp_resource_record(r: &McpResourceRecord) -> proto::McpResourceRecord {
    proto::McpResourceRecord {
        server: r.server.clone(),
        uri: r.uri.clone(),
        name: r.name.clone(),
        mime_type: r.mime_type.clone(),
        description: r.description.clone(),
    }
}

pub fn from_proto_mcp_resource_record(r: proto::McpResourceRecord) -> McpResourceRecord {
    McpResourceRecord {
        server: r.server,
        uri: r.uri,
        name: r.name,
        mime_type: r.mime_type,
        description: r.description,
    }
}

pub fn to_proto_mcp_resource_content(c: &McpResourceContent) -> proto::McpResourceContent {
    proto::McpResourceContent {
        server: c.server.clone(),
        uri: c.uri.clone(),
        parts: c.parts.iter().map(to_proto_mcp_resource_part).collect(),
    }
}

pub fn from_proto_mcp_resource_content(
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

pub fn to_proto_mcp_resource_part(p: &McpResourceContentPart) -> proto::McpResourceContentPart {
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

pub fn from_proto_mcp_resource_part(
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

pub fn to_proto_mcp_prompt(p: &McpPrompt) -> proto::McpPrompt {
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

pub fn from_proto_mcp_prompt(p: proto::McpPrompt) -> McpPrompt {
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

pub fn to_proto_mcp_prompt_content(c: &McpPromptContent) -> proto::McpPromptContent {
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

pub fn from_proto_mcp_prompt_content(c: proto::McpPromptContent) -> McpPromptContent {
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

// --- Permission --------------------------------------------------------

pub fn to_proto_permission_request(req: &PermissionRequest, id: &str) -> proto::PermissionRequest {
    proto::PermissionRequest {
        tool_id: req.tool_id.clone(),
        cwd: req.cwd.display().to_string(),
        input_json: req.input.to_string(),
        reason: req.reason.clone(),
        id: id.to_string(),
    }
}

pub fn from_proto_permission_request(
    req: proto::PermissionRequest,
) -> Result<(String, PermissionRequest), RunnerError> {
    let input = if req.input_json.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_str(&req.input_json)
            .map_err(|e| RunnerError::InvalidArgument(format!("permission input_json: {e}")))?
    };
    Ok((
        req.id,
        PermissionRequest {
            tool_id: req.tool_id,
            cwd: PathBuf::from(req.cwd),
            input,
            reason: req.reason,
        },
    ))
}

pub fn permission_decision_to_str(d: PermissionDecision) -> &'static str {
    match d {
        PermissionDecision::AllowOnce => "allow_once",
        PermissionDecision::AllowSession => "allow_session",
        PermissionDecision::AllowAllSession => "allow_all_session",
        PermissionDecision::Deny => "deny",
    }
}

pub fn permission_decision_from_str(s: &str) -> Result<PermissionDecision, RunnerError> {
    match s {
        "allow_once" => Ok(PermissionDecision::AllowOnce),
        "allow_session" => Ok(PermissionDecision::AllowSession),
        "allow_all_session" => Ok(PermissionDecision::AllowAllSession),
        "deny" => Ok(PermissionDecision::Deny),
        other => Err(RunnerError::Other(format!(
            "unknown permission decision `{other}`"
        ))),
    }
}

// --- RunnerError ↔ tonic::Status --------------------------------------

pub fn runner_error_to_status(err: &RunnerError) -> tonic::Status {
    use tonic::Code;
    let code = match err {
        RunnerError::NotFound(_) => Code::NotFound,
        RunnerError::PermissionDenied(_) => Code::PermissionDenied,
        RunnerError::Unsupported(_) => Code::Unimplemented,
        RunnerError::InvalidArgument(_) => Code::InvalidArgument,
        RunnerError::Transport(_) => Code::Unavailable,
        RunnerError::Mcp(_) => Code::Internal,
        RunnerError::Execution(_) => Code::Internal,
        RunnerError::Other(_) => Code::Internal,
    };
    tonic::Status::new(code, err.to_string())
}

pub fn status_to_runner_error(status: tonic::Status) -> RunnerError {
    use tonic::Code;
    let msg = status.message().to_string();
    match status.code() {
        Code::NotFound => RunnerError::NotFound(msg),
        Code::PermissionDenied => RunnerError::PermissionDenied(msg),
        Code::Unimplemented => RunnerError::Unsupported(msg),
        Code::InvalidArgument => RunnerError::InvalidArgument(msg),
        Code::Unavailable => RunnerError::Transport(msg),
        _ => RunnerError::Other(msg),
    }
}

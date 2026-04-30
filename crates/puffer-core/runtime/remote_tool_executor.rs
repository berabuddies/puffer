use super::tool_stream::emit_tool_stream_delta;
use super::{ToolOutputDelta, ToolOutputStream};
use crate::AppState;
use anyhow::{anyhow, Context, Result};
use puffer_config::{RemotePathMapConfig, RemoteToolRunnerConfig};
use puffer_remote_tools::{
    execute_tool_blocking, RemoteToolChunkStream, RemoteToolExecutionContext, RemoteToolRequest,
};
use puffer_tools::ToolExecutionResult;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub(super) fn maybe_execute_remote_tool_call(
    state: &mut AppState,
    call_id: Option<&str>,
    tool_id: &str,
    input: Value,
    execution_context: Option<&RemoteToolExecutionContext>,
) -> Result<Option<ToolExecutionResult>> {
    let Some(config) = state.config.remote_tool_runner.as_ref() else {
        return Ok(None);
    };
    if !tool_enabled_for_remote(config, tool_id) {
        return Ok(None);
    }
    let endpoint = config
        .endpoint
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("remote tool runner endpoint is not configured"))?;
    let request = build_remote_request(state, config, call_id, tool_id, &input, execution_context)?;
    let auth_token = resolve_auth_token(config)?;
    let result = execute_tool_blocking(endpoint, auth_token.as_deref(), request, |chunk| {
        emit_tool_stream_delta(ToolOutputDelta {
            call_id: call_id.unwrap_or_default().to_string(),
            tool_id: tool_id.to_string(),
            stream: match chunk.stream {
                RemoteToolChunkStream::Stdout => ToolOutputStream::Stdout,
                RemoteToolChunkStream::Stderr => ToolOutputStream::Stderr,
            },
            text: chunk.text,
        });
    })?;
    Ok(Some(result))
}

pub(super) fn remote_tool_parallel_safe(state: &AppState, tool_id: &str) -> bool {
    state
        .config
        .remote_tool_runner
        .as_ref()
        .is_none_or(|config| !tool_enabled_for_remote(config, tool_id))
}

fn tool_enabled_for_remote(config: &RemoteToolRunnerConfig, tool_id: &str) -> bool {
    let _ = config;
    super::remote_tools::remote_tool_runner_supported_tools()
        .iter()
        .any(|candidate| *candidate == tool_id)
}

fn resolve_auth_token(config: &RemoteToolRunnerConfig) -> Result<Option<String>> {
    if let Some(token) = config
        .auth_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok(Some(token.to_string()));
    }
    if let Some(name) = config
        .auth_token_env
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let value = std::env::var(name)
            .with_context(|| format!("failed to read tool runner token env `{name}`"))?;
        if value.trim().is_empty() {
            return Err(anyhow!("tool runner token env `{name}` is empty"));
        }
        return Ok(Some(value));
    }
    Ok(None)
}

fn build_remote_request(
    state: &AppState,
    config: &RemoteToolRunnerConfig,
    call_id: Option<&str>,
    tool_id: &str,
    input: &Value,
    execution_context: Option<&RemoteToolExecutionContext>,
) -> Result<RemoteToolRequest> {
    let remote_cwd = resolve_remote_cwd(state, config);
    let working_dirs = if state.working_dirs.is_empty() {
        vec![remote_cwd.display().to_string()]
    } else {
        state
            .working_dirs
            .iter()
            .map(|path| map_local_path(path, state, config).display().to_string())
            .collect()
    };
    let mapped_input = map_tool_input_paths(tool_id, input.clone(), state, config);
    Ok(RemoteToolRequest {
        session_id: state.session.id.to_string(),
        call_id: call_id.unwrap_or_default().to_string(),
        tool_id: tool_id.to_string(),
        input_json: serde_json::to_string(&mapped_input)?,
        cwd: remote_cwd.display().to_string(),
        working_dirs,
        sandbox_mode: state.sandbox_mode.clone(),
        execution_context_json: execution_context.map(serde_json::to_string).transpose()?,
    })
}

fn resolve_remote_cwd(state: &AppState, config: &RemoteToolRunnerConfig) -> PathBuf {
    if let Some(explicit) = config
        .remote_cwd
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return PathBuf::from(explicit);
    }
    map_local_path(&state.cwd, state, config)
}

fn map_local_path(path: &Path, state: &AppState, config: &RemoteToolRunnerConfig) -> PathBuf {
    if let Some(mapped) = map_with_explicit_path_map(path, config.path_map.as_ref()) {
        return mapped;
    }
    if let Some(remote_cwd) = config.remote_cwd.as_deref() {
        if let Ok(relative) = path.strip_prefix(&state.cwd) {
            return PathBuf::from(remote_cwd).join(relative);
        }
    }
    path.to_path_buf()
}

fn map_with_explicit_path_map(path: &Path, map: Option<&RemotePathMapConfig>) -> Option<PathBuf> {
    let map = map?;
    let local_root = map.local_root.as_deref()?;
    let remote_root = map.remote_root.as_deref()?;
    path.strip_prefix(local_root)
        .ok()
        .map(|suffix| PathBuf::from(remote_root).join(suffix))
}

fn map_tool_input_paths(
    tool_id: &str,
    mut input: Value,
    state: &AppState,
    config: &RemoteToolRunnerConfig,
) -> Value {
    for field in tool_path_fields(tool_id) {
        if let Some(original) = input.get(field).and_then(Value::as_str) {
            let mapped = map_tool_path_string(original, state, config);
            if mapped != original {
                input[field] = Value::String(mapped);
            }
        }
    }
    input
}

fn map_tool_path_string(path: &str, state: &AppState, config: &RemoteToolRunnerConfig) -> String {
    let candidate = Path::new(path);
    if !candidate.is_absolute() {
        return path.to_string();
    }
    map_local_path(candidate, state, config)
        .display()
        .to_string()
}

fn tool_path_fields(tool_id: &str) -> &'static [&'static str] {
    match tool_id {
        "Read" | "Write" | "Edit" => &["file_path"],
        "NotebookEdit" => &["notebook_path"],
        "Glob" | "Grep" => &["path"],
        _ => &[],
    }
}

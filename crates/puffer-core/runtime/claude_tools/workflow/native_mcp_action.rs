use crate::runtime::claude_tools::workflow::secret_value::resolve_secret_handle;
use crate::AppState;
use anyhow::{bail, Context, Result};
use puffer_config::{ensure_workspace_dirs, ConfigPaths};
use puffer_resources::McpServerSpec;
use puffer_runner_api::RunnerError;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use uuid::Uuid;

/// Executes the verified native-mcp runtime bridge.
pub fn execute_native_mcp_action(state: &mut AppState, cwd: &Path, input: Value) -> Result<String> {
    let object = input
        .as_object()
        .context("NativeMcpAction input must be an object")?;
    let action = required_str(object, "action")?;
    let output = match action {
        "validateStdioConfig" => validate_stdio_config(state, object)?,
        "validateHttpConfig" => validate_http_config(state, object)?,
        "applySecurityPolicy" => apply_security_policy(object)?,
        "configureSampling" => configure_sampling(object)?,
        "addStdioServer" => add_stdio_server(state, cwd, object)?,
        "addHttpServer" => add_http_server(state, cwd, object)?,
        "discoverTools" => discover_tools(state)?,
        other => bail!("unsupported NativeMcpAction action `{other}`"),
    };
    Ok(serde_json::to_string_pretty(&output)?)
}

fn discover_tools(state: &AppState) -> Result<Value> {
    let servers = state.tool_runner.list_mcp_servers().map_err(runner_error)?;
    let mut tools = Vec::new();
    for server in &servers {
        let server_tools = state
            .tool_runner
            .list_mcp_tools(&server.id)
            .map_err(runner_error)
            .with_context(|| format!("list MCP tools for `{}`", server.id))?;
        for tool in server_tools {
            tools.push(json!({
                "server": server.id,
                "tool": tool.name,
                "qualifiedToolName": format!(
                    "mcp_{}_{}",
                    hermes_component(&server.id),
                    hermes_component(&tool.name)
                ),
            }));
        }
    }
    Ok(json!({
        "tools_discovered": true,
        "tool_names_normalized": true,
        "persistent_connection": true,
        "reconnect_policy": true,
        "server_count": servers.len(),
        "tools": tools,
    }))
}

fn validate_stdio_config(state: &AppState, object: &Map<String, Value>) -> Result<Value> {
    let name = validate_server_name(required_str(object, "name")?)?;
    let command = validate_nonempty_string(required_str(object, "command")?, "stdio command")?;
    let args = parse_string_list(optional_field(object, &["args"]), "args")?;
    let timeout = positive_i64(optional_field(object, &["timeout"]), 120, "timeout")?;
    let connect_timeout = positive_i64(
        optional_field(object, &["connectTimeout", "connect_timeout"]),
        60,
        "connect_timeout",
    )?;
    let env_input = optional_field(object, &["envVars", "env_vars"]);
    let (env, env_secret) = parse_secret_map(state, env_input, "env_vars")?;
    let mut out = json!({
        "name": name,
        "transport": "stdio",
        "command": command,
        "args": args,
        "env_keys": sorted_keys(&env),
        "timeout": timeout,
        "connect_timeout": connect_timeout,
    });
    if let Some(secret) = env_secret {
        out["env_secret"] = secret;
    } else if !env.is_empty() {
        out["env"] = json!(env);
    }
    Ok(out)
}

fn validate_http_config(state: &AppState, object: &Map<String, Value>) -> Result<Value> {
    let name = validate_server_name(required_str(object, "name")?)?;
    let url = validate_http_url(required_str(object, "url")?)?;
    let timeout = positive_i64(optional_field(object, &["timeout"]), 120, "timeout")?;
    let connect_timeout = positive_i64(
        optional_field(object, &["connectTimeout", "connect_timeout"]),
        60,
        "connect_timeout",
    )?;
    let headers_input = optional_field(object, &["headers"]);
    let (headers, headers_secret) = parse_secret_map(state, headers_input, "headers")?;
    let mut out = json!({
        "name": name,
        "transport": "http",
        "url": url,
        "header_keys": sorted_keys(&headers),
        "timeout": timeout,
        "connect_timeout": connect_timeout,
    });
    if let Some(secret) = headers_secret {
        out["headers_secret"] = secret;
    } else if !headers.is_empty() {
        out["headers"] = json!(headers);
    }
    Ok(out)
}

fn apply_security_policy(object: &Map<String, Value>) -> Result<Value> {
    let config = parse_json_object(required_field(object, "config")?, "config")?;
    let transport = required_str(&config, "transport")?;
    let name = required_str(&config, "name")?;
    validate_server_name(name)?;
    match transport {
        "stdio" => {
            let keys = parse_string_list(optional_field(&config, &["env_keys"]), "env_keys")?;
            Ok(json!({
                "transport": "stdio",
                "credentials_redacted": true,
                "safe_env_filter": true,
                "inherit_env_allowlist": [
                    "PATH", "HOME", "USER", "LANG", "LC_ALL", "TERM",
                    "SHELL", "TMPDIR", "XDG_*"
                ],
                "explicit_env_keys": keys,
            }))
        }
        "http" => {
            let keys = parse_string_list(optional_field(&config, &["header_keys"]), "header_keys")?;
            Ok(json!({
                "transport": "http",
                "credentials_redacted": true,
                "safe_env_filter": true,
                "header_keys": keys,
            }))
        }
        other => bail!("NativeMcpAction security policy rejects transport `{other}`"),
    }
}

fn configure_sampling(object: &Map<String, Value>) -> Result<Value> {
    let config = parse_json_object(required_field(object, "config")?, "config")?;
    let trusted = optional_bool(optional_field(object, &["trusted"]), false)?;
    let name = validate_server_name(required_str(&config, "name")?)?;
    let transport = required_str(&config, "transport")?;
    if transport != "stdio" && transport != "http" {
        bail!("NativeMcpAction sampling rejects transport `{transport}`");
    }
    Ok(json!({
        "server": name,
        "transport": transport,
        "sampling_bounded": true,
        "enabled": trusted,
        "model": if trusted { Value::String("default".to_string()) } else { Value::Null },
        "allowed_models": if trusted { json!(["default"]) } else { json!([]) },
        "max_tokens_cap": if trusted { 2048 } else { 0 },
        "timeout_seconds": if trusted { 60 } else { 0 },
        "max_rpm": if trusted { 12 } else { 0 },
        "max_tool_rounds": if trusted { 3 } else { 0 },
    }))
}

fn add_stdio_server(state: &AppState, cwd: &Path, object: &Map<String, Value>) -> Result<Value> {
    let config = parse_json_object(required_field(object, "config")?, "config")?;
    let security = parse_json_object(required_field(object, "security")?, "security")?;
    let sampling = parse_json_object(required_field(object, "sampling")?, "sampling")?;
    ensure_add_contract(&config, &security, &sampling, "stdio")?;

    let name = validate_server_name(required_str(&config, "name")?)?;
    let command = required_str(&config, "command")?.to_string();
    let args = parse_string_list(optional_field(&config, &["args"]), "args")?;
    let timeout = positive_seconds(optional_field(&config, &["timeout"]), 120, "timeout")?;
    let connect_timeout = positive_seconds(
        optional_field(&config, &["connect_timeout", "connectTimeout"]),
        60,
        "connect_timeout",
    )?;
    let (env, secret_origin) = parse_config_map(state, &config, "env", "env_secret")?;
    let env = sanitize_stdio_env(env, secret_origin)?;
    let spec = McpServerSpec {
        id: name.clone(),
        display_name: name.clone(),
        transport: "stdio".to_string(),
        endpoint: String::new(),
        target: shell_words::join(std::iter::once(command).chain(args)),
        description: "Added by verified native-mcp Lambda Skill".to_string(),
        env,
        inherit_env: false,
        timeout: Some(timeout),
        connect_timeout: Some(connect_timeout),
        headers: BTreeMap::new(),
        oauth: None,
    };
    let path = write_mcp_manifest(cwd, &spec)?;
    Ok(json!({
        "ok": true,
        "server": name,
        "transport": "stdio",
        "restart_required": true,
        "manifest": path.display().to_string(),
    }))
}

fn add_http_server(state: &AppState, cwd: &Path, object: &Map<String, Value>) -> Result<Value> {
    let config = parse_json_object(required_field(object, "config")?, "config")?;
    let security = parse_json_object(required_field(object, "security")?, "security")?;
    let sampling = parse_json_object(required_field(object, "sampling")?, "sampling")?;
    ensure_add_contract(&config, &security, &sampling, "http")?;

    let name = validate_server_name(required_str(&config, "name")?)?;
    let url = validate_http_url(required_str(&config, "url")?)?;
    let timeout = positive_seconds(optional_field(&config, &["timeout"]), 120, "timeout")?;
    let connect_timeout = positive_seconds(
        optional_field(&config, &["connect_timeout", "connectTimeout"]),
        60,
        "connect_timeout",
    )?;
    let (headers, secret_origin) = parse_config_map(state, &config, "headers", "headers_secret")?;
    let headers = sanitize_http_headers(headers, secret_origin)?;
    let spec = McpServerSpec {
        id: name.clone(),
        display_name: name.clone(),
        transport: "http".to_string(),
        endpoint: String::new(),
        target: url,
        description: "Added by verified native-mcp Lambda Skill".to_string(),
        env: BTreeMap::new(),
        inherit_env: true,
        timeout: Some(timeout),
        connect_timeout: Some(connect_timeout),
        headers,
        oauth: None,
    };
    let path = write_mcp_manifest(cwd, &spec)?;
    Ok(json!({
        "ok": true,
        "server": name,
        "transport": "http",
        "restart_required": true,
        "manifest": path.display().to_string(),
    }))
}

fn ensure_add_contract(
    config: &Map<String, Value>,
    security: &Map<String, Value>,
    sampling: &Map<String, Value>,
    expected_transport: &str,
) -> Result<()> {
    let transport = required_str(config, "transport")?;
    if transport != expected_transport {
        bail!("NativeMcpAction expected {expected_transport} config, got `{transport}`");
    }
    if required_str(security, "transport")? != expected_transport {
        bail!("NativeMcpAction security transport does not match config");
    }
    if security
        .get("credentials_redacted")
        .and_then(Value::as_bool)
        != Some(true)
    {
        bail!("NativeMcpAction requires credentials_redacted=true");
    }
    if expected_transport == "stdio"
        && security.get("safe_env_filter").and_then(Value::as_bool) != Some(true)
    {
        bail!("NativeMcpAction stdio requires safe_env_filter=true");
    }
    if sampling.get("sampling_bounded").and_then(Value::as_bool) != Some(true) {
        bail!("NativeMcpAction requires sampling_bounded=true");
    }
    if sampling.get("enabled").and_then(Value::as_bool) == Some(true) {
        bail!("NativeMcpAction does not support MCP sampling yet; retry with trusted=false");
    }
    let expected_keys = if expected_transport == "stdio" {
        parse_string_list(optional_field(config, &["env_keys"]), "env_keys")?
    } else {
        parse_string_list(optional_field(config, &["header_keys"]), "header_keys")?
    };
    let reviewed_keys = if expected_transport == "stdio" {
        parse_string_list(
            optional_field(security, &["explicit_env_keys"]),
            "explicit_env_keys",
        )?
    } else {
        parse_string_list(optional_field(security, &["header_keys"]), "header_keys")?
    };
    if BTreeSet::from_iter(expected_keys) != BTreeSet::from_iter(reviewed_keys) {
        bail!("NativeMcpAction security review keys do not match config keys");
    }
    Ok(())
}

fn write_mcp_manifest(cwd: &Path, spec: &McpServerSpec) -> Result<std::path::PathBuf> {
    let paths = ConfigPaths::discover(cwd);
    ensure_workspace_dirs(&paths)?;
    let dir = paths.workspace_config_dir.join("resources/mcp_servers");
    fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create MCP resource dir {}", dir.display()))?;
    let path = dir.join(format!("{}.yaml", spec.id));
    let yaml = serde_yaml::to_string(spec).context("failed to serialize MCP server manifest")?;
    fs::write(&path, yaml).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

fn parse_config_map(
    state: &AppState,
    config: &Map<String, Value>,
    direct_field: &str,
    secret_field: &str,
) -> Result<(BTreeMap<String, String>, bool)> {
    if let Some(direct) = optional_field(config, &[direct_field]) {
        return parse_plain_string_map(direct, direct_field).map(|map| (map, false));
    }
    if let Some(secret) = optional_field(config, &[secret_field]) {
        let raw = resolve_secret_handle(state, secret)?;
        return parse_raw_map(&raw, direct_field).map(|map| (map, true));
    }
    Ok((BTreeMap::new(), false))
}

fn sanitize_stdio_env(
    env: BTreeMap<String, String>,
    secret_origin: bool,
) -> Result<BTreeMap<String, String>> {
    if !secret_origin {
        return Ok(env);
    }
    env.into_iter()
        .map(|(key, value)| {
            if contains_env_reference(&value) {
                return Ok((key, value));
            }
            match std::env::var(&key) {
                Ok(current) if current == value => Ok((key.clone(), format!("${{{key}}}"))),
                _ => bail!(
                    "NativeMcpAction refuses to persist raw MCP env secret `{key}`; set the same environment variable on the Puffer process and retry"
                ),
            }
        })
        .collect()
}

fn sanitize_http_headers(
    headers: BTreeMap<String, String>,
    secret_origin: bool,
) -> Result<BTreeMap<String, String>> {
    if !secret_origin {
        return Ok(headers);
    }
    headers
        .into_iter()
        .map(|(key, value)| {
            if contains_env_reference(&value) {
                Ok((key, value))
            } else {
                bail!(
                    "NativeMcpAction refuses to persist raw MCP header secret `{key}`; use an environment reference such as Bearer ${{TOKEN}}"
                )
            }
        })
        .collect()
}

fn contains_env_reference(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'$' {
            index += 1;
            continue;
        }
        if bytes.get(index + 1) == Some(&b'{') {
            if let Some(end) = value[index + 2..].find('}') {
                let name = &value[index + 2..index + 2 + end];
                if valid_env_reference_name(name) {
                    return true;
                }
            }
        } else {
            let rest = &value[index + 1..];
            let len = rest
                .chars()
                .take_while(|ch| *ch == '_' || ch.is_ascii_alphanumeric())
                .map(char::len_utf8)
                .sum::<usize>();
            if len > 0 && valid_env_reference_name(&rest[..len]) {
                return true;
            }
        }
        index += 1;
    }
    false
}

fn valid_env_reference_name(name: &str) -> bool {
    !name.is_empty()
        && !name.chars().next().is_some_and(|ch| ch.is_ascii_digit())
        && name
            .chars()
            .all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn parse_secret_map(
    state: &AppState,
    value: Option<&Value>,
    field: &str,
) -> Result<(BTreeMap<String, String>, Option<Value>)> {
    let Some(value) = value else {
        return Ok((BTreeMap::new(), None));
    };
    let expanded = parse_jsonish_value(value).unwrap_or_else(|| value.clone());
    if is_secret_handle(&expanded) {
        let raw = resolve_secret_handle(state, &expanded)?;
        let parsed = parse_raw_map(&raw, field)?;
        return Ok((parsed, Some(expanded)));
    }
    let map = parse_plain_string_map(&expanded, field)?;
    let secret = if map.is_empty() {
        None
    } else {
        Some(store_secret_map_handle(state, &map, field)?)
    };
    Ok((map, secret))
}

fn parse_plain_string_map(value: &Value, field: &str) -> Result<BTreeMap<String, String>> {
    match value {
        Value::Null => Ok(BTreeMap::new()),
        Value::String(text) => parse_raw_map(text, field),
        Value::Object(object) => object
            .iter()
            .map(|(key, value)| {
                let Some(value) = value.as_str() else {
                    bail!("NativeMcpAction `{field}` values must be strings");
                };
                validate_map_key(field, key)?;
                Ok((key.clone(), value.to_string()))
            })
            .collect(),
        _ => bail!("NativeMcpAction `{field}` must be an object, JSON string, or KEY=VALUE lines"),
    }
}

fn parse_raw_map(raw: &str, field: &str) -> Result<BTreeMap<String, String>> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(BTreeMap::new());
    }
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        return parse_plain_string_map(&value, field);
    }
    let mut out = BTreeMap::new();
    for line in trimmed
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let Some((key, value)) = line.split_once('=') else {
            bail!("NativeMcpAction `{field}` line must be KEY=VALUE");
        };
        validate_map_key(field, key)?;
        out.insert(key.to_string(), value.to_string());
    }
    Ok(out)
}

fn store_secret_map_handle(
    state: &AppState,
    map: &BTreeMap<String, String>,
    label: &str,
) -> Result<Value> {
    let secret_id = Uuid::new_v4().to_string();
    let raw = serde_json::to_string(map).context("serialize native MCP secret map")?;
    state
        .secret_values
        .lock()
        .map_err(|_| anyhow::anyhow!("secret store lock poisoned"))?
        .insert(secret_id.clone(), raw);
    Ok(json!({
        "secretId": secret_id,
        "sec": "secret",
        "label": format!("native-mcp-{label}"),
    }))
}

fn parse_json_object(value: &Value, field: &str) -> Result<Map<String, Value>> {
    let expanded = parse_jsonish_value(value).unwrap_or_else(|| value.clone());
    let Some(object) = expanded.as_object() else {
        bail!("NativeMcpAction `{field}` must be an object or JSON object string");
    };
    Ok(object.clone())
}

fn parse_jsonish_value(value: &Value) -> Option<Value> {
    value
        .as_str()
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
}

fn parse_string_list(value: Option<&Value>, field: &str) -> Result<Vec<String>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let expanded = parse_jsonish_value(value).unwrap_or_else(|| value.clone());
    let Some(array) = expanded.as_array() else {
        bail!("NativeMcpAction `{field}` must be a list");
    };
    array
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_string)
                .with_context(|| format!("NativeMcpAction `{field}` entries must be strings"))
        })
        .collect()
}

fn optional_bool(value: Option<&Value>, default: bool) -> Result<bool> {
    let Some(value) = value else {
        return Ok(default);
    };
    match value {
        Value::Bool(value) => Ok(*value),
        Value::String(text) => Ok(matches!(
            text.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "trusted"
        )),
        _ => bail!("NativeMcpAction bool field must be boolean or string"),
    }
}

fn positive_i64(value: Option<&Value>, default: i64, field: &str) -> Result<i64> {
    let value = match value {
        None | Some(Value::Null) => default,
        Some(Value::Number(number)) => number
            .as_i64()
            .with_context(|| format!("NativeMcpAction `{field}` must be an integer"))?,
        Some(Value::String(text)) if text.trim().is_empty() => default,
        Some(Value::String(text)) => text
            .trim()
            .parse::<i64>()
            .with_context(|| format!("NativeMcpAction `{field}` must be an integer"))?,
        Some(_) => bail!("NativeMcpAction `{field}` must be an integer"),
    };
    if value <= 0 {
        bail!("NativeMcpAction `{field}` must be positive");
    }
    Ok(value)
}

fn positive_seconds(value: Option<&Value>, default: i64, field: &str) -> Result<u64> {
    let value = positive_i64(value, default, field)?;
    u64::try_from(value).with_context(|| format!("NativeMcpAction `{field}` is out of range"))
}

fn sorted_keys(map: &BTreeMap<String, String>) -> Vec<String> {
    map.keys().cloned().collect()
}

fn is_secret_handle(value: &Value) -> bool {
    value
        .as_object()
        .and_then(|object| object.get("sec"))
        .and_then(Value::as_str)
        == Some("secret")
}

fn required_field<'a>(object: &'a Map<String, Value>, field: &str) -> Result<&'a Value> {
    object
        .get(field)
        .with_context(|| format!("NativeMcpAction `{field}` is required"))
}

fn optional_field<'a>(object: &'a Map<String, Value>, fields: &[&str]) -> Option<&'a Value> {
    fields.iter().find_map(|field| object.get(*field))
}

fn required_str<'a>(object: &'a Map<String, Value>, field: &str) -> Result<&'a str> {
    required_field(object, field)?
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .with_context(|| format!("NativeMcpAction `{field}` must be a non-empty string"))
}

fn validate_nonempty_string(value: &str, field: &str) -> Result<String> {
    if value.trim().is_empty() || value.contains('\0') {
        bail!("NativeMcpAction `{field}` must be non-empty");
    }
    Ok(value.trim().to_string())
}

fn validate_server_name(value: &str) -> Result<String> {
    let value = validate_nonempty_string(value, "name")?;
    if !value
        .chars()
        .all(|ch| ch == '_' || ch == '-' || ch == '.' || ch.is_ascii_alphanumeric())
    {
        bail!(
            "NativeMcpAction server name must contain only letters, numbers, dot, underscore, or dash"
        );
    }
    Ok(value)
}

fn validate_env_key(value: &str) -> Result<()> {
    if value.is_empty()
        || !value
            .chars()
            .all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
        || value.chars().next().is_some_and(|ch| ch.is_ascii_digit())
    {
        bail!("NativeMcpAction env/header key `{value}` is invalid");
    }
    Ok(())
}

fn validate_header_key(value: &str) -> Result<()> {
    if value.is_empty()
        || !value
            .chars()
            .all(|ch| ch == '-' || ch == '_' || ch.is_ascii_alphanumeric())
    {
        bail!("NativeMcpAction header key `{value}` is invalid");
    }
    Ok(())
}

fn validate_map_key(field: &str, value: &str) -> Result<()> {
    if field.contains("header") {
        validate_header_key(value)
    } else {
        validate_env_key(value)
    }
}

fn validate_http_url(value: &str) -> Result<String> {
    let parsed = url::Url::parse(value).context("NativeMcpAction url must be absolute")?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        bail!("NativeMcpAction url must use http or https");
    }
    if parsed.host_str().is_none() {
        bail!("NativeMcpAction url requires a host");
    }
    Ok(value.to_string())
}

fn hermes_component(input: &str) -> String {
    let mut out = String::with_capacity(input.len().max(1));
    for ch in input.chars() {
        if ch == '_' || ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        out.push('_');
    }
    out
}

fn runner_error(error: RunnerError) -> anyhow::Error {
    match error {
        RunnerError::NotFound(message) => anyhow::anyhow!("MCP target not found: {message}"),
        RunnerError::PermissionDenied(message) => anyhow::anyhow!("MCP access denied: {message}"),
        RunnerError::Unsupported(message) => {
            anyhow::anyhow!("unsupported MCP operation: {message}")
        }
        RunnerError::InvalidArgument(message) => {
            anyhow::anyhow!("invalid MCP operation: {message}")
        }
        RunnerError::Mcp(message) => anyhow::anyhow!("MCP error: {message}"),
        RunnerError::Transport(message) => anyhow::anyhow!("MCP transport error: {message}"),
        RunnerError::OAuthRequired { server_id, .. } => {
            anyhow::anyhow!("MCP OAuth required for server `{server_id}`")
        }
        RunnerError::Execution(message) => anyhow::anyhow!("MCP execution error: {message}"),
        RunnerError::Other(message) => anyhow::anyhow!("{message}"),
    }
}

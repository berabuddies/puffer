use super::*;

#[test]
fn native_mcp_add_stdio_server_writes_safe_manifest() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let handle = crate::runtime::claude_tools::workflow::secret_value::execute_secret_value(
        &mut state,
        &cwd,
        json!({
            "action": "prepare",
            "value": "NATIVE_MCP_TOKEN=${NATIVE_MCP_TOKEN}",
            "label": "native-mcp"
        }),
    )
    .unwrap();
    let handle: Value = serde_json::from_str(&handle).unwrap();

    let config = native_mcp(
        &mut state,
        &cwd,
        json!({
            "action": "validateStdioConfig",
            "name": "time",
            "command": "uvx",
            "args": ["mcp-server-time"],
            "envVars": handle,
            "timeout": 120,
            "connectTimeout": 60
        }),
    );
    assert!(!config.contains("NATIVE_MCP_TOKEN=${NATIVE_MCP_TOKEN}"));
    let config: Value = serde_json::from_str(&config).unwrap();
    let security = native_mcp(
        &mut state,
        &cwd,
        json!({"action": "applySecurityPolicy", "config": config.clone()}),
    );
    let security: Value = serde_json::from_str(&security).unwrap();
    let sampling = native_mcp(
        &mut state,
        &cwd,
        json!({"action": "configureSampling", "config": config.clone(), "trusted": false}),
    );
    let sampling: Value = serde_json::from_str(&sampling).unwrap();

    let added = native_mcp(
        &mut state,
        &cwd,
        json!({
            "action": "addStdioServer",
            "config": config,
            "security": security,
            "sampling": sampling
        }),
    );
    let added: Value = serde_json::from_str(&added).unwrap();
    assert_eq!(added["restart_required"], json!(true));

    let manifest = cwd.join(".puffer/resources/mcp_servers/time.yaml");
    let yaml = fs::read_to_string(&manifest).unwrap();
    assert!(yaml.contains("inherit_env: false"));
    assert!(yaml.contains("${NATIVE_MCP_TOKEN}"));
    assert!(!yaml.contains("NATIVE_MCP_TOKEN=${NATIVE_MCP_TOKEN}"));
    let spec: puffer_resources::McpServerSpec = serde_yaml::from_str(&yaml).unwrap();
    assert_eq!(spec.env["NATIVE_MCP_TOKEN"], "${NATIVE_MCP_TOKEN}");
    assert!(!spec.inherit_env);
    assert_eq!(spec.timeout, Some(120));
    assert_eq!(spec.connect_timeout, Some(60));
}

#[test]
fn native_mcp_add_rejects_raw_secret_manifest_values() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let handle = crate::runtime::claude_tools::workflow::secret_value::execute_secret_value(
        &mut state,
        &cwd,
        json!({
            "action": "prepare",
            "value": "PUFFER_NATIVE_MCP_SHOULD_NOT_EXIST=raw-secret-value",
            "label": "native-mcp"
        }),
    )
    .unwrap();
    let handle: Value = serde_json::from_str(&handle).unwrap();

    let config = native_mcp(
        &mut state,
        &cwd,
        json!({
            "action": "validateStdioConfig",
            "name": "secreted",
            "command": "uvx",
            "args": ["mcp-server-time"],
            "envVars": handle
        }),
    );
    assert!(!config.contains("raw-secret-value"));
    let config: Value = serde_json::from_str(&config).unwrap();
    let security: Value = serde_json::from_str(&native_mcp(
        &mut state,
        &cwd,
        json!({"action": "applySecurityPolicy", "config": config.clone()}),
    ))
    .unwrap();
    let sampling: Value = serde_json::from_str(&native_mcp(
        &mut state,
        &cwd,
        json!({"action": "configureSampling", "config": config.clone(), "trusted": false}),
    ))
    .unwrap();

    let error =
        crate::runtime::claude_tools::workflow::native_mcp_action::execute_native_mcp_action(
            &mut state,
            &cwd,
            json!({
                "action": "addStdioServer",
                "config": config,
                "security": security,
                "sampling": sampling
            }),
        )
        .unwrap_err();

    assert!(error
        .to_string()
        .contains("refuses to persist raw MCP env secret"));
    assert!(!cwd
        .join(".puffer/resources/mcp_servers/secreted.yaml")
        .exists());
}

#[test]
fn native_mcp_add_http_server_preserves_timeout_and_header_refs() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let handle = crate::runtime::claude_tools::workflow::secret_value::execute_secret_value(
        &mut state,
        &cwd,
        json!({
            "action": "prepare",
            "value": "{\"Authorization\":\"Bearer ${NATIVE_MCP_HTTP_TOKEN}\"}",
            "label": "native-mcp-http"
        }),
    )
    .unwrap();
    let handle: Value = serde_json::from_str(&handle).unwrap();

    let config: Value = serde_json::from_str(&native_mcp(
        &mut state,
        &cwd,
        json!({
            "action": "validateHttpConfig",
            "name": "remote",
            "url": "https://mcp.example.test/mcp",
            "headers": handle,
            "timeout": 180,
            "connectTimeout": 30
        }),
    ))
    .unwrap();
    let security: Value = serde_json::from_str(&native_mcp(
        &mut state,
        &cwd,
        json!({"action": "applySecurityPolicy", "config": config.clone()}),
    ))
    .unwrap();
    let sampling: Value = serde_json::from_str(&native_mcp(
        &mut state,
        &cwd,
        json!({"action": "configureSampling", "config": config.clone(), "trusted": false}),
    ))
    .unwrap();

    let added: Value = serde_json::from_str(&native_mcp(
        &mut state,
        &cwd,
        json!({
            "action": "addHttpServer",
            "config": config,
            "security": security,
            "sampling": sampling
        }),
    ))
    .unwrap();
    assert_eq!(added["restart_required"], json!(true));

    let yaml = fs::read_to_string(cwd.join(".puffer/resources/mcp_servers/remote.yaml")).unwrap();
    let spec: puffer_resources::McpServerSpec = serde_yaml::from_str(&yaml).unwrap();
    assert_eq!(
        spec.headers["Authorization"],
        "Bearer ${NATIVE_MCP_HTTP_TOKEN}"
    );
    assert_eq!(spec.timeout, Some(180));
    assert_eq!(spec.connect_timeout, Some(30));
}

fn native_mcp(state: &mut AppState, cwd: &Path, input: Value) -> String {
    crate::runtime::claude_tools::workflow::native_mcp_action::execute_native_mcp_action(
        state, cwd, input,
    )
    .unwrap()
}

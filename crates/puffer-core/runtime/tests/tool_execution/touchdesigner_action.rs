use super::*;
use crate::runtime::lambda_gate::{LambdaGateState, LambdaHostEnv};

#[test]
fn touchdesigner_script_guard_commits_safe_script_fact() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let host = LambdaHostEnv::from_json_str(
        r#"{"effects":["proc","eval","net_w"],"domains":[],"tools":[{"name":"make_safe_td_script","params":[{"name":"code","ty":"str"}],"result":"SafeScript{no_absolute_paths(s) && native_tool_preferred(s)}","effects":["proc"],"concreteTools":["TouchDesignerAction"],"concreteInputContracts":{"TouchDesignerAction":{"action":"validateScript","code":{"$arg":"code"}}}},{"name":"td_execute_python","params":[{"name":"script","ty":"SafeScript{no_absolute_paths(s) && native_tool_preferred(s)}"}],"result":"PyResult","effects":["eval","net_w"],"concreteTools":["McpToolCall"],"concreteInputContracts":{"McpToolCall":{"server":"twozero_td","tool":"td_execute_python","args":{"code":{"$arg":"script"}}}}}]}"#,
    )
    .unwrap();
    state.lambda_gate = Some(LambdaGateState::with_host_caps(host));
    let resources = LoadedResources {
        tools: vec![
            loaded_tool(
                "LambdaHostCall",
                "Admit Lambda host call",
                "runtime:lambda_host_call",
            ),
            loaded_tool(
                "TouchDesignerAction",
                "TouchDesigner action",
                "runtime:workflow:touchdesigner_action",
            ),
            loaded_tool(
                "McpToolCall",
                "Call MCP tool",
                "runtime:workflow:mcp_tool_call",
            ),
        ],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let providers = empty_providers();
    let request_config = test_openai_request_config();
    let code = "root = me.parent()\ncreated = []\nfor name in ['bg', 'out']:\n    node = root.op(name)\n    if node:\n        created.append(node.path)\nresult = {'created': created}";

    let admitted = execute_tool_call(
        &mut state,
        &resources,
        &providers,
        &mut AuthStore::default(),
        &registry,
        "gpt-5",
        &cwd,
        ToolExecutionBackend::OpenAi {
            request_config: &request_config,
            structured_output: None,
        },
        None,
        "LambdaHostCall",
        json!({
            "host_tool": "make_safe_td_script",
            "args": {"code": code},
            "tool": "TouchDesignerAction",
            "input": {"action": "validateScript", "code": code},
        }),
    )
    .unwrap();
    assert!(admitted.success);

    let guarded = execute_tool_call(
        &mut state,
        &resources,
        &providers,
        &mut AuthStore::default(),
        &registry,
        "gpt-5",
        &cwd,
        ToolExecutionBackend::OpenAi {
            request_config: &request_config,
            structured_output: None,
        },
        None,
        "TouchDesignerAction",
        json!({"action": "validateScript", "code": code}),
    )
    .unwrap();
    assert!(guarded.success);
    assert!(state.pending_lambda_host_call.is_none());

    let execute_admitted = execute_tool_call(
        &mut state,
        &resources,
        &providers,
        &mut AuthStore::default(),
        &registry,
        "gpt-5",
        &cwd,
        ToolExecutionBackend::OpenAi {
            request_config: &request_config,
            structured_output: None,
        },
        None,
        "LambdaHostCall",
        json!({
            "host_tool": "td_execute_python",
            "args": {"script": code},
            "tool": "McpToolCall",
            "input": {"server": "twozero_td", "tool": "td_execute_python", "args": {"code": code}},
        }),
    )
    .unwrap();
    assert!(
        execute_admitted.success,
        "admit failed: stdout={} stderr={}",
        execute_admitted.output.stdout, execute_admitted.output.stderr
    );
    assert_eq!(
        state
            .pending_lambda_host_call
            .as_ref()
            .unwrap()
            .concrete_tool(),
        "McpToolCall"
    );
}

#[test]
fn touchdesigner_script_guard_blocks_unvalidated_script_fact() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let host = LambdaHostEnv::from_json_str(
        r#"{"effects":["proc","eval","net_w"],"domains":[],"tools":[{"name":"td_execute_python","params":[{"name":"script","ty":"SafeScript{no_absolute_paths(s) && native_tool_preferred(s)}"}],"result":"PyResult","effects":["eval","net_w"],"concreteTools":["McpToolCall"],"concreteInputContracts":{"McpToolCall":{"server":"twozero_td","tool":"td_execute_python","args":{"code":{"$arg":"script"}}}}}]}"#,
    )
    .unwrap();
    state.lambda_gate = Some(LambdaGateState::with_host_caps(host));
    let resources = LoadedResources {
        tools: vec![
            loaded_tool(
                "LambdaHostCall",
                "Admit Lambda host call",
                "runtime:lambda_host_call",
            ),
            loaded_tool(
                "McpToolCall",
                "Call MCP tool",
                "runtime:workflow:mcp_tool_call",
            ),
        ],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let providers = empty_providers();
    let request_config = test_openai_request_config();
    let result = execute_tool_call(
        &mut state,
        &resources,
        &providers,
        &mut AuthStore::default(),
        &registry,
        "gpt-5",
        &cwd,
        ToolExecutionBackend::OpenAi {
            request_config: &request_config,
            structured_output: None,
        },
        None,
        "LambdaHostCall",
        json!({
            "host_tool": "td_execute_python",
            "args": {"script": "root = op('/project1')\nresult = root.path"},
            "tool": "McpToolCall",
            "input": {
                "server": "twozero_td",
                "tool": "td_execute_python",
                "args": {"code": "root = op('/project1')\nresult = root.path"}
            },
        }),
    )
    .unwrap();

    assert!(!result.success);
    assert!(
        result.output.stdout.contains("does not match SafeScript"),
        "unexpected stdout={} stderr={}",
        result.output.stdout,
        result.output.stderr
    );
    assert!(state.pending_lambda_host_call.is_none());
}

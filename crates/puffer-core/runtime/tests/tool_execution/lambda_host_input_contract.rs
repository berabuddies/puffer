use super::*;
use crate::runtime::lambda_gate::{LambdaGateState, LambdaHostEnv};

#[test]
fn lambda_host_call_rejects_concrete_input_that_breaks_contract() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let host = LambdaHostEnv::from_json_str(
        r#"{"effects":[],"domains":[],"tools":[{"name":"formal_search","concreteTools":["ToolSearch"],"concreteInputContracts":{"ToolSearch":{"query":{"$arg":"query"}}},"params":[{"name":"query","ty":"str"}],"effects":[]}]}"#,
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
                "ToolSearch",
                "Search available tools",
                "runtime:tool_search",
            ),
        ],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let providers = empty_providers();
    let request_config = test_openai_request_config();

    let rejected = execute_tool_call(
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
            "host_tool": "formal_search",
            "args": {"query": "Hanzhi Liu"},
            "tool": "ToolSearch",
            "input": {"query": "different author"},
        }),
    )
    .unwrap();

    assert!(!rejected.success);
    assert!(rejected
        .output
        .stdout
        .contains("does not match the precompiled ToolSearch contract"));
    assert!(state.pending_lambda_host_call.is_none());
}

use super::*;
use crate::runtime::lambda_gate::{LambdaGateState, LambdaHostEnv};

#[test]
fn secret_value_prepare_returns_handle_without_raw_secret() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();

    let output = crate::runtime::claude_tools::workflow::secret_value::execute_secret_value(
        &mut state,
        &cwd,
        json!({
            "action": "prepare",
            "value": "HF_TOKEN=hf_private",
            "label": "modal"
        }),
    )
    .unwrap();

    assert!(!output.contains("hf_private"));
    let value: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(value["sec"], "secret");
    assert_eq!(value["label"], "modal");
    let secret_id = value["secretId"].as_str().unwrap();
    assert_eq!(
        state.secret_values.lock().unwrap().get(secret_id).unwrap(),
        "HF_TOKEN=hf_private"
    );
}

#[test]
fn modal_secret_create_rejects_raw_value() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();

    let error = crate::runtime::claude_tools::workflow::modal_action::execute_modal_action(
        &mut state,
        &cwd,
        json!({
            "action": "secretCreate",
            "name": "huggingface",
            "value": "HF_TOKEN=hf_private"
        }),
    )
    .unwrap_err();

    assert!(error.to_string().contains("SecretValue handle"));
}

#[test]
fn modal_secret_create_validates_assignment_before_cli() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let handle = crate::runtime::claude_tools::workflow::secret_value::execute_secret_value(
        &mut state,
        &cwd,
        json!({
            "action": "prepare",
            "value": "hf_private"
        }),
    )
    .unwrap();
    let handle: Value = serde_json::from_str(&handle).unwrap();

    let error = crate::runtime::claude_tools::workflow::modal_action::execute_modal_action(
        &mut state,
        &cwd,
        json!({
            "action": "secretCreate",
            "name": "huggingface",
            "value": handle
        }),
    )
    .unwrap_err();

    assert!(error.to_string().contains("KEY=VALUE"));
}

#[test]
fn modal_define_function_writes_definition_file() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();

    let output = crate::runtime::claude_tools::workflow::modal_action::execute_modal_action(
        &mut state,
        &cwd,
        json!({
            "action": "defineFunction",
            "gpu": "A100",
            "image": "modal.Image.debian_slim().pip_install(\"torch\")",
            "schedule": "modal.Cron(\"0 0 * * *\")"
        }),
    )
    .unwrap();
    let value: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(value["app_defined"], json!(true));
    assert_eq!(value["kind"], json!("function"));
    let path = PathBuf::from(value["path"].as_str().unwrap());
    let body = fs::read_to_string(path).unwrap();
    assert!(body.contains("@app.function"));
    assert!(body.contains("gpu=\"A100\""));
    assert!(body.contains("modal.Image.debian_slim().pip_install(\"torch\")"));
    assert!(body.contains("modal.Cron(\"0 0 * * *\")"));
}

#[test]
fn modal_define_class_rejects_execution_tokens() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();

    let error = crate::runtime::claude_tools::workflow::modal_action::execute_modal_action(
        &mut state,
        &cwd,
        json!({
            "action": "defineClass",
            "gpu": "H100",
            "image": "modal.Image.debian_slim(); import os"
        }),
    )
    .unwrap_err();

    assert!(error.to_string().contains("single expression"));
}

#[test]
fn lambda_host_call_redacts_secret_schema_fields_in_gate_metadata() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let host = LambdaHostEnv::from_json_str(
        r#"{"effects":["proc"],"domains":[],"tools":[{"name":"prepare_secret","concreteTools":["SecretValue"],"concreteInputContracts":{"SecretValue":{"action":"prepare","value":{"$arg":"value"}}},"params":[{"name":"value","ty":"str"}],"result":"SecretValue{sec = secret}","effects":["proc"]}]}"#,
    )
    .unwrap();
    state.lambda_gate = Some(LambdaGateState::with_host_caps(host));
    let mut secret_tool = loaded_tool(
        "SecretValue",
        "Prepare secret",
        "runtime:workflow:secret_value",
    );
    secret_tool.value.input_schema = Some(json!({
        "type": "object",
        "properties": {
            "action": { "type": "string" },
            "value": {
                "type": "string",
                "x-puffer-secret": true
            }
        },
        "required": ["action", "value"]
    }));
    let resources = LoadedResources {
        tools: vec![
            loaded_tool(
                "LambdaHostCall",
                "Admit Lambda host call",
                "runtime:lambda_host_call",
            ),
            secret_tool,
        ],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let providers = empty_providers();
    let request_config = test_openai_request_config();

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
            "host_tool": "prepare_secret",
            "args": {"value": "HF_TOKEN=hf_private"},
            "tool": "SecretValue",
            "input": {"action": "prepare", "value": "HF_TOKEN=hf_private"},
        }),
    )
    .unwrap();

    assert!(admitted.success);
    let metadata = admitted.output.metadata.to_string();
    assert!(!metadata.contains("hf_private"));
    assert!(metadata.contains("[redacted]"));
}

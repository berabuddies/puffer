use super::*;
use crate::runtime::lambda_gate::{LambdaFact, LambdaGateState, LambdaHostEnv};
use puffer_resources::{LoadedItem, SkillSpec, SkillVerificationSpec, SourceInfo, SourceKind};

#[test]
fn task_output_reads_runtime_background_agent_output_file() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let agent_output_dir = ConfigPaths::discover(&cwd)
        .workspace_config_dir
        .join("runtime")
        .join("agent_outputs");
    fs::create_dir_all(&agent_output_dir).unwrap();
    let output_file = agent_output_dir.join("agent-demo.json");
    fs::write(
        &output_file,
        serde_json::to_string_pretty(&json!({
            "status": "completed",
            "result": "background ok"
        }))
        .unwrap(),
    )
    .unwrap();

    let output = super::super::claude_tools::workflow::task_output::execute_task_output(
        &mut state,
        &cwd,
        json!({
            "task_id": "agent-demo",
            "block": false,
            "timeout": 50
        }),
    )
    .unwrap();
    let parsed: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(parsed["retrieval_status"], "success");
    assert_eq!(parsed["task"]["task_type"], "agent");
    assert_eq!(parsed["task"]["status"], "completed");
    assert_eq!(parsed["task"]["output"], "background ok");
    assert_eq!(parsed["task"]["result"], "background ok");
    assert_eq!(
        parsed["task"]["outputFile"].as_str(),
        Some(output_file.display().to_string().as_str())
    );
}

#[test]
fn send_user_message_stores_resolved_attachment_paths() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let attachment = cwd.join("note.txt");
    fs::write(&attachment, "hello").unwrap();

    let output =
        super::super::claude_tools::workflow::send_user_message::execute_send_user_message(
            &mut state,
            &cwd,
            json!({
                "message": "hello",
                "attachments": ["note.txt"],
                "status": "normal"
            }),
        )
        .unwrap();
    let parsed: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(
        parsed["message"]["attachments"][0].as_str(),
        Some(attachment.display().to_string().as_str())
    );
}

#[test]
fn send_user_message_ignores_workspace_ask_permissions() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let permissions_dir = ConfigPaths::discover(&cwd).workspace_config_dir;
    fs::create_dir_all(&permissions_dir).unwrap();
    fs::write(
        permissions_dir.join("permissions.toml"),
        "[tools]\nsend_user_message = \"ask\"\nbrief = \"ask\"\n",
    )
    .unwrap();

    let mut tool = loaded_tool(
        "SendUserMessage",
        "Send a message to the user",
        "runtime:workflow:send_user_message",
    );
    tool.value.approval_policy = Some("auto".to_string());
    tool.value.sandbox_policy = Some("read-only".to_string());
    let resources = LoadedResources {
        tools: vec![tool],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let mut providers = ProviderRegistry::new();
    providers.register(openai_provider("http://127.0.0.1".to_string()));
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
        "SendUserMessage",
        json!({
            "message": "hi",
            "status": "normal"
        }),
    )
    .unwrap();

    assert!(result.success);
    assert!(result.output.stdout.contains("\"message\": \"hi\""));
}

#[test]
fn lambda_gate_rejects_direct_concrete_tool_without_host_bridge() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let host = LambdaHostEnv::from_json_str(
        r#"{"effects":[],"domains":[],"tools":[{"name":"ToolSearch","effects":[]}]}"#,
    )
    .unwrap();
    state.lambda_gate = Some(LambdaGateState::with_host_caps(host));
    let resources = LoadedResources {
        tools: vec![loaded_tool(
            "ToolSearch",
            "Search available tools",
            "runtime:tool_search",
        )],
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
        "ToolSearch",
        json!({"query": "ToolSearch"}),
    )
    .unwrap();

    assert!(!result.success);
    assert!(result
        .output
        .stdout
        .contains("requires LambdaHostCall before concrete tool calls"));
    assert!(result
        .output
        .stdout
        .contains("Recoverable: Retry by calling LambdaHostCall before this concrete tool call"));
    assert_eq!(
        result.output.metadata["lambda_skill"]["event"],
        json!("gate_rejected")
    );
    assert_eq!(
        result.output.metadata["lambda_skill"]["recoverable"],
        json!(true)
    );
    assert_eq!(
        result.output.metadata["lambda_skill"]["retry_tool"],
        json!("LambdaHostCall")
    );
}

#[test]
fn active_lambda_gate_allows_tools_outside_verified_scope_for_exploration() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    fs::write(cwd.join("note.txt"), "hello").unwrap();
    let host = LambdaHostEnv::from_json_str(
        r#"{"effects":[],"domains":[],"tools":[{"name":"formal_search","concreteTools":["ToolSearch"],"concreteInputContracts":{"ToolSearch":{"query":{"$arg":"query"}}},"params":[{"name":"query","ty":"str"}],"effects":[]}]}"#,
    )
    .unwrap();
    let mut gate = LambdaGateState::with_host_caps(host);
    gate.set_request_tool_filter(
        super::super::build_request_tool_filter(&[
            "ToolSearch".to_string(),
            "LambdaHostCall".to_string(),
            "LambdaInternal".to_string(),
        ])
        .unwrap()
        .unwrap(),
    );
    state.lambda_gate = Some(gate);
    let resources = LoadedResources {
        tools: vec![
            loaded_tool("Glob", "Find files", "runtime:claude_glob"),
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
        "Glob",
        json!({"pattern": "*.txt"}),
    )
    .unwrap();

    assert!(result.success, "{}", result.output.stdout);
    assert!(result.output.stdout.contains("note.txt"));
    assert!(state.lambda_gate.is_some());
    assert!(state.pending_lambda_host_call.is_none());
}

#[test]
fn lambda_gate_does_not_commit_facts_for_direct_concrete_tool() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let host = LambdaHostEnv::from_json_str(
        r#"{"effects":[],"domains":[],"tools":[{"name":"ToolSearch","effects":[],"registers":[{"pred":"searched","args":[]}]}]}"#,
    )
    .unwrap();
    state.lambda_gate = Some(LambdaGateState::with_host_caps(host));
    let resources = LoadedResources {
        tools: vec![loaded_tool(
            "ToolSearch",
            "Search available tools",
            "runtime:tool_search",
        )],
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
        "ToolSearch",
        json!({"query": "ToolSearch"}),
    )
    .unwrap();

    assert!(!result.success);
    assert!(result
        .output
        .stdout
        .contains("requires LambdaHostCall before concrete tool calls"));
    let gate = state.lambda_gate.as_ref().expect("gate remains installed");
    assert!(!gate
        .facts()
        .contains(&LambdaFact::new("searched", Vec::new())));
}

#[test]
fn lambda_host_call_bridges_formal_tool_to_declared_concrete_tool() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let host = LambdaHostEnv::from_json_str(
        r#"{"effects":[],"domains":[],"tools":[{"name":"formal_search","concreteTools":["ToolSearch"],"concreteInputContracts":{"ToolSearch":{"query":{"$template":"${query} ${limit}"}}},"params":[{"name":"query","ty":"str"},{"name":"limit","ty":"int"}],"effects":[],"registers":[{"pred":"searched","args":[]}]}]}"#,
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
    let concrete_input = json!({"query": "ToolSearch 1"});

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
            "host_tool": "formal_search",
            "args": {"query": "ToolSearch", "limit": 1},
            "tool": "ToolSearch",
            "input": concrete_input,
        }),
    )
    .unwrap();

    assert!(admitted.success);
    assert_eq!(
        admitted.output.metadata["lambda_skill"]["event"],
        json!("host_call_admitted")
    );
    assert_eq!(
        admitted.output.metadata["lambda_skill"]["host_tool"],
        json!("formal_search")
    );
    assert_eq!(
        admitted.output.metadata["lambda_skill"]["host_args"],
        json!({"query": "ToolSearch", "limit": 1})
    );
    assert_eq!(
        admitted.output.metadata["lambda_skill"]["concrete_tool"],
        json!("ToolSearch")
    );
    assert!(state.pending_lambda_host_call.is_some());
    let gate = state.lambda_gate.as_ref().expect("gate remains installed");
    assert!(!gate
        .facts()
        .contains(&LambdaFact::new("searched", Vec::new())));

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
        "ToolSearch",
        json!({"query": "ToolSearch 1"}),
    )
    .unwrap();

    assert!(result.success);
    assert_eq!(
        result.output.metadata["lambda_skill"]["event"],
        json!("host_call_committed")
    );
    assert_eq!(
        result.output.metadata["lambda_skill"]["host_tool"],
        json!("formal_search")
    );
    assert_eq!(
        result.output.metadata["lambda_skill"]["host_args"],
        json!({"query": "ToolSearch", "limit": 1})
    );
    assert_eq!(
        result.output.metadata["lambda_skill"]["concrete_tool"],
        json!("ToolSearch")
    );
    assert_eq!(
        result.output.metadata["lambda_skill"]["registered_facts"][0]["pred"],
        json!("searched")
    );
    assert!(state.pending_lambda_host_call.is_none());
    let gate = state.lambda_gate.as_ref().expect("gate remains installed");
    assert!(gate
        .facts()
        .contains(&LambdaFact::new("searched", Vec::new())));
}

#[test]
fn lambda_host_call_rejects_mismatched_concrete_input() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let host = LambdaHostEnv::from_json_str(
        r#"{"effects":[],"domains":[],"tools":[{"name":"formal_search","concreteTools":["ToolSearch"],"concreteInputContracts":{"ToolSearch":{"query":"ToolSearch"}},"effects":[]}]}"#,
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
            "host_tool": "formal_search",
            "args": {},
            "tool": "ToolSearch",
            "input": {"query": "ToolSearch"},
        }),
    )
    .unwrap();
    assert!(admitted.success);

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
        "ToolSearch",
        json!({"query": "different"}),
    )
    .unwrap();

    assert!(!rejected.success);
    assert!(rejected
        .output
        .stdout
        .contains("pending formal host call formal_search requires next concrete tool ToolSearch"));
    assert!(rejected
        .output
        .stdout
        .contains("Recoverable: A LambdaHostCall bridge is already pending"));
    assert_eq!(
        rejected.output.metadata["lambda_skill"]["event"],
        json!("gate_rejected")
    );
    assert_eq!(
        rejected.output.metadata["lambda_skill"]["recoverable"],
        json!(true)
    );
    assert_eq!(
        rejected.output.metadata["lambda_skill"]["retry_tool"],
        json!("ToolSearch")
    );
    assert!(state.pending_lambda_host_call.is_some());
}

#[test]
fn lambda_host_call_rejects_bad_formal_args() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let host = LambdaHostEnv::from_json_str(
        r#"{"effects":[],"domains":[],"tools":[{"name":"formal_search","concreteTools":["ToolSearch"],"concreteInputContracts":{"ToolSearch":{"query":{"$template":"${query} ${limit}"}}},"params":[{"name":"query","ty":"str"},{"name":"limit","ty":"int"}],"effects":[]}]}"#,
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
            "args": {"query": "ToolSearch", "limit": "many"},
            "tool": "ToolSearch",
            "input": {"query": "ToolSearch"},
        }),
    )
    .unwrap();

    assert!(!rejected.success);
    assert!(rejected
        .output
        .stdout
        .contains("formal arg limit for formal_search does not match int"));
    assert!(state.pending_lambda_host_call.is_none());
}

#[test]
fn lambda_host_call_rejects_unknown_concrete_tool() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let host = LambdaHostEnv::from_json_str(
        r#"{"effects":[],"domains":[],"tools":[{"name":"formal_search","concreteTools":["ToolSearch"],"concreteInputContracts":{"ToolSearch":{"query":"ToolSearch"}},"effects":[]}]}"#,
    )
    .unwrap();
    state.lambda_gate = Some(LambdaGateState::with_host_caps(host));
    let resources = LoadedResources {
        tools: vec![loaded_tool(
            "LambdaHostCall",
            "Admit Lambda host call",
            "runtime:lambda_host_call",
        )],
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
            "args": {},
            "tool": "MissingTool",
            "input": {},
        }),
    )
    .unwrap();

    assert!(!rejected.success);
    assert!(rejected
        .output
        .stdout
        .contains("LambdaHostCall target tool MissingTool is not available"));
    assert!(state.pending_lambda_host_call.is_none());
}

#[test]
fn model_invoked_verified_lambda_skill_installs_gate_and_tool_scope() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let host_path = cwd.join("verified-host.json");
    fs::write(
        &host_path,
        r#"{"effects":[],"domains":[],"tools":[{"name":"formal_search","concreteTools":["ToolSearch","Bash"],"concreteInputContracts":{"ToolSearch":{"query":{"$arg":"query"}},"Bash":{"command":{"$template":"printf ${json:query}"}}},"params":[{"name":"query","ty":"str"}],"effects":[],"registers":[{"pred":"searched","args":[]}]}]}"#,
    )
    .unwrap();
    let mut skill_tool = loaded_tool("Skill", "Load a skill", "runtime:skill");
    skill_tool.value.approval_policy = Some("auto".to_string());
    skill_tool.value.sandbox_policy = Some("read-only".to_string());
    let resources = LoadedResources {
        tools: vec![
            skill_tool,
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
            loaded_tool("Bash", "Run shell", "bash"),
        ],
        skills: vec![LoadedItem {
            value: SkillSpec {
                name: "issue-triage".to_string(),
                description: "Triage issues".to_string(),
                content: "Use LambdaHostCall before concrete tools.".to_string(),
                allowed_tools: vec!["ToolSearch".to_string()],
                disable_model_invocation: false,
                verification: Some(SkillVerificationSpec {
                    system: "lambda-skill".to_string(),
                    source_path: Some("skills/issue-triage/skill.lskill".to_string()),
                    generated_path: Some("skills/issue-triage/out/GENERATED.SKILL.md".to_string()),
                    host_catalogue_path: Some(host_path.display().to_string()),
                    compiler_path: None,
                    host_tool_bindings: Default::default(),
                    tools: Some(1),
                    actions: Some(1),
                }),
                ..SkillSpec::default()
            },
            source_info: SourceInfo {
                path: "skills/issue-triage/skill.lskill".into(),
                kind: SourceKind::Workspace,
            },
        }],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let providers = empty_providers();
    let request_config = test_openai_request_config();

    let loaded = execute_tool_call(
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
        "Skill",
        json!({"skill": "issue-triage"}),
    )
    .unwrap();
    assert!(loaded.success);
    assert!(loaded
        .output
        .stdout
        .contains("<command-name>issue-triage</command-name>"));
    assert!(loaded
        .output
        .stdout
        .contains("allowed-tools: ToolSearch, LambdaHostCall, LambdaInternal"));
    assert!(state.lambda_gate.is_some());

    let denied_target = execute_tool_call(
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
            "args": {"query": "issues"},
            "tool": "Bash",
        }),
    )
    .unwrap();
    assert!(!denied_target.success);
    assert!(denied_target
        .output
        .stdout
        .contains("outside the active skill tool scope"));
    assert!(state.pending_lambda_host_call.is_none());

    let concrete_input = json!({"query": "issues"});
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
            "host_tool": "formal_search",
            "args": {"query": "issues"},
            "tool": "ToolSearch",
            "input": concrete_input,
        }),
    )
    .unwrap();
    assert!(admitted.success);
    assert!(state.pending_lambda_host_call.is_some());

    let committed = execute_tool_call(
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
        "ToolSearch",
        json!({"query": "issues"}),
    )
    .unwrap();
    assert!(committed.success);
    assert_eq!(
        committed.output.metadata["lambda_skill"]["event"],
        json!("host_call_committed")
    );
    assert!(state.pending_lambda_host_call.is_none());
}

#[test]
fn execute_openai_tool_calls_respect_request_tool_filter() {
    let resources = LoadedResources {
        tools: vec![loaded_tool("bash", "Run shell", "bash")],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let mut providers = ProviderRegistry::new();
    providers.register(openai_provider("http://127.0.0.1".to_string()));
    let tool_calls = vec![OpenAIResponseToolCall {
        item_id: Some("fc_1".to_string()),
        status: Some("completed".to_string()),
        call_id: "call_1".to_string(),
        name: "bash".to_string(),
        arguments: json!({ "command": "printf hi" }),
    }];
    let request_config = test_openai_request_config();
    let filter = super::super::build_request_tool_filter(&["Read".to_string()])
        .unwrap()
        .unwrap();
    let mut state = temp_state();
    let cwd = state.cwd.clone();

    let result = execute_openai_tool_calls(
        &mut state,
        &resources,
        &providers,
        &mut AuthStore::default(),
        &tool_calls,
        &registry,
        &cwd,
        &request_config,
        "gpt-5",
        None,
        Some(&filter),
    )
    .unwrap();

    assert!(!result.invocations[0].success);
    assert!(result.outputs[0]
        .output
        .contains("slash command tool scope denied this tool call"));
}

#[test]
fn execute_anthropic_tool_calls_respect_request_tool_filter() {
    let resources = LoadedResources {
        tools: vec![loaded_tool("bash", "Run shell", "bash")],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let mut providers = ProviderRegistry::new();
    providers.register(provider());
    let request_config = test_anthropic_request_config();
    let response = json!({
        "content": [
            {
                "type": "tool_use",
                "id": "toolu_1",
                "name": "bash",
                "input": {
                    "command": "printf hi"
                }
            }
        ]
    });
    let filter = super::super::build_request_tool_filter(&["Read".to_string()])
        .unwrap()
        .unwrap();
    let mut state = temp_state();
    let cwd = state.cwd.clone();

    let result = execute_anthropic_tool_calls(
        &mut state,
        &resources,
        &providers,
        &mut AuthStore::default(),
        &response,
        &registry,
        &cwd,
        &request_config,
        "claude-sonnet-4-5",
        None,
        Some(&filter),
    )
    .unwrap()
    .expect("anthropic tool results");

    assert!(!result.invocations[0].success);
    assert!(result.invocations[0]
        .output
        .contains("slash command tool scope denied this tool call"));
}

#[test]
fn request_scoped_write_path_filters_allow_only_matching_paths() {
    let mut tool = loaded_tool("Write", "Write file", "runtime:claude_write");
    tool.value.approval_policy = Some("on-request".to_string());
    tool.value.sandbox_policy = Some("workspace-write".to_string());
    let resources = LoadedResources {
        tools: vec![tool],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let mut providers = ProviderRegistry::new();
    providers.register(openai_provider("http://127.0.0.1".to_string()));
    let request_config = test_openai_request_config();
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let allowed_dir = cwd.join("allowed");
    let denied_dir = cwd.join("other");
    fs::create_dir_all(&allowed_dir).unwrap();
    fs::create_dir_all(&denied_dir).unwrap();
    let filter =
        super::super::build_request_tool_filter(&[format!("Write({}/**)", allowed_dir.display())])
            .unwrap()
            .unwrap();
    let permission_context = crate::permissions::load_runtime_permission_context_with_inputs(
        &cwd,
        &resources,
        &state,
        crate::permissions::RuntimePermissionInputs {
            request_tool_filter: Some(filter.clone()),
        },
    )
    .unwrap();
    let definition = registry.definition("Write").unwrap();
    let allowed_path = allowed_dir.join("file.txt");
    let denied_path = denied_dir.join("file.txt");

    assert!(permission_context.tool_visible_to_model(definition));
    assert_eq!(
        permission_context
            .decision_for_tool_call(
                definition,
                &json!({"file_path": allowed_path, "content": "allowed"})
            )
            .behavior,
        crate::permissions::ToolPermissionBehavior::Allow
    );
    let denied_decision = permission_context.decision_for_tool_call(
        definition,
        &json!({"file_path": denied_path, "content": "denied"}),
    );
    assert_eq!(
        denied_decision.behavior,
        crate::permissions::ToolPermissionBehavior::Deny
    );
    assert_eq!(
        denied_decision.reason.as_deref(),
        Some("slash command tool scope denied this tool call")
    );

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
        Some(&filter),
        "Write",
        json!({"file_path": denied_path, "content": "denied"}),
    )
    .unwrap();

    assert!(!result.success);
    assert!(result
        .output
        .stdout
        .contains("slash command tool scope denied this tool call"));
}

#[test]
fn request_scoped_edit_path_filters_allow_only_the_selected_file() {
    let mut tool = loaded_tool("Edit", "Edit file", "runtime:claude_edit");
    tool.value.approval_policy = Some("on-request".to_string());
    tool.value.sandbox_policy = Some("workspace-write".to_string());
    let resources = LoadedResources {
        tools: vec![tool],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let mut providers = ProviderRegistry::new();
    providers.register(openai_provider("http://127.0.0.1".to_string()));
    let request_config = test_openai_request_config();
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let allowed_dir = cwd.join("allowed");
    let denied_dir = cwd.join("other");
    fs::create_dir_all(&allowed_dir).unwrap();
    fs::create_dir_all(&denied_dir).unwrap();
    let allowed_path = allowed_dir.join("file.txt");
    let denied_path = denied_dir.join("file.txt");
    fs::write(&allowed_path, "alpha").unwrap();
    fs::write(&denied_path, "alpha").unwrap();
    mark_file_fully_read(&mut state, &allowed_path);
    mark_file_fully_read(&mut state, &denied_path);

    let filter =
        super::super::build_request_tool_filter(&[format!("Edit({})", allowed_path.display())])
            .unwrap()
            .unwrap();
    let permission_context = crate::permissions::load_runtime_permission_context_with_inputs(
        &cwd,
        &resources,
        &state,
        crate::permissions::RuntimePermissionInputs {
            request_tool_filter: Some(filter.clone()),
        },
    )
    .unwrap();
    let definition = registry.definition("Edit").unwrap();

    assert!(permission_context.tool_visible_to_model(definition));
    assert_eq!(
        permission_context
            .decision_for_tool_call(
                definition,
                &json!({"file_path": allowed_path, "old_string": "alpha", "new_string": "beta"})
            )
            .behavior,
        crate::permissions::ToolPermissionBehavior::Allow
    );
    let denied_decision = permission_context.decision_for_tool_call(
        definition,
        &json!({"file_path": denied_path, "old_string": "alpha", "new_string": "beta"}),
    );
    assert_eq!(
        denied_decision.behavior,
        crate::permissions::ToolPermissionBehavior::Deny
    );
    assert_eq!(
        denied_decision.reason.as_deref(),
        Some("slash command tool scope denied this tool call")
    );

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
        Some(&filter),
        "Edit",
        json!({"file_path": denied_path, "old_string": "alpha", "new_string": "beta"}),
    )
    .unwrap();

    assert!(!result.success);
    assert!(result
        .output
        .stdout
        .contains("slash command tool scope denied this tool call"));
}

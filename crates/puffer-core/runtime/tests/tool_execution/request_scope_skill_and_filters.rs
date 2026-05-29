use super::*;
use puffer_resources::{LoadedItem, SkillSpec, SkillVerificationSpec, SourceInfo, SourceKind};

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
                    require_approval: false,
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

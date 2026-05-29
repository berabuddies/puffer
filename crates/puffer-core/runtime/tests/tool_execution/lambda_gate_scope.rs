use super::*;
use crate::runtime::lambda_gate::{LambdaGateState, LambdaHostEnv};
use crate::runtime::{with_permission_prompt_handler, PermissionPromptAction};
use puffer_resources::{LoadedItem, SkillSpec, SkillVerificationSpec, SourceInfo, SourceKind};
use std::sync::{Arc, Mutex};

#[test]
fn model_invoked_plain_skill_cannot_escape_active_lambda_gate() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let host_path = cwd.join("verified-host.json");
    fs::write(
        &host_path,
        r#"{"effects":[],"domains":[],"tools":[{"name":"formal_search","concreteTools":["ToolSearch"],"concreteInputContracts":{"ToolSearch":{"query":{"$arg":"query"}}},"params":[{"name":"query","ty":"str"}],"effects":[]}]}"#,
    )
    .unwrap();
    let mut skill_tool = loaded_tool("Skill", "Load a skill", "runtime:skill");
    skill_tool.value.approval_policy = Some("auto".to_string());
    skill_tool.value.sandbox_policy = Some("read-only".to_string());
    let resources = LoadedResources {
        tools: vec![skill_tool],
        skills: vec![
            LoadedItem {
                value: SkillSpec {
                    name: "issue-triage".to_string(),
                    description: "Triage issues".to_string(),
                    content: "Use LambdaHostCall before concrete tools.".to_string(),
                    allowed_tools: vec!["ToolSearch".to_string()],
                    disable_model_invocation: false,
                    verification: Some(SkillVerificationSpec {
                        system: "lambda-skill".to_string(),
                        source_path: Some("skills/issue-triage/skill.lskill".to_string()),
                        generated_path: Some(
                            "skills/issue-triage/out/GENERATED.SKILL.md".to_string(),
                        ),
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
            },
            LoadedItem {
                value: SkillSpec {
                    name: "reviewer".to_string(),
                    description: "Review changes".to_string(),
                    content: "Review normal changes.".to_string(),
                    disable_model_invocation: false,
                    ..SkillSpec::default()
                },
                source_info: SourceInfo {
                    path: "skills/reviewer/SKILL.md".into(),
                    kind: SourceKind::Workspace,
                },
            },
        ],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let providers = empty_providers();
    let request_config = test_openai_request_config();

    let loaded_lambda = execute_tool_call(
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
    assert!(loaded_lambda.success);
    assert!(state.lambda_gate.is_some());

    let error = execute_tool_call(
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
        json!({"skill": "reviewer"}),
    )
    .unwrap_err()
    .to_string();

    assert!(error.contains("active Lambda Skill gate cannot switch to unverified skill `reviewer`"));
    assert!(state.lambda_gate.is_some());
    assert!(state.pending_lambda_host_call.is_none());
}

#[test]
fn lambda_host_call_can_load_bound_verified_skill() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let parent_host_path = cwd.join("paper-host.json");
    let child_host_path = cwd.join("arxiv-host.json");
    fs::write(
        &parent_host_path,
        r#"{"effects":["proc"],"domains":[],"tools":[{"name":"load_arxiv_skill","concreteTools":["Skill"],"concreteInputContracts":{"Skill":{"skill":"arxiv","args":""}},"params":[],"effects":["proc"],"registers":[{"pred":"arxiv_loaded","args":[]}]}]}"#,
    )
    .unwrap();
    fs::write(
        &child_host_path,
        r#"{"effects":["proc"],"domains":[],"tools":[{"name":"lookup","concreteTools":["ToolSearch"],"concreteInputContracts":{"ToolSearch":{"query":{"$arg":"query"}}},"params":[{"name":"query","ty":"str"}],"effects":["proc"]}]}"#,
    )
    .unwrap();
    let mut skill_tool = loaded_tool("Skill", "Load a skill", "runtime:skill");
    skill_tool.value.approval_policy = Some("auto".to_string());
    skill_tool.value.sandbox_policy = Some("read-only".to_string());
    let mut lambda_host_call = loaded_tool(
        "LambdaHostCall",
        "Admit Lambda host call",
        "runtime:lambda_host_call",
    );
    lambda_host_call.value.approval_policy = Some("auto".to_string());
    lambda_host_call.value.sandbox_policy = Some("read-only".to_string());
    let tool_search = loaded_tool("ToolSearch", "Search tools", "runtime:tool_search");
    let resources = LoadedResources {
        tools: vec![skill_tool, lambda_host_call, tool_search],
        skills: vec![
            LoadedItem {
                value: SkillSpec {
                    name: "paper".to_string(),
                    description: "Write research papers".to_string(),
                    content: "Use load_arxiv_skill before arXiv lookup.".to_string(),
                    allowed_tools: vec!["Skill".to_string()],
                    verification: Some(SkillVerificationSpec {
                        system: "lambda-skill".to_string(),
                        source_path: Some("skills/paper/skill.lskill".to_string()),
                        generated_path: Some("skills/paper/out/GENERATED.SKILL.md".to_string()),
                        host_catalogue_path: Some(parent_host_path.display().to_string()),
                        compiler_path: None,
                        host_tool_bindings: Default::default(),
                        require_approval: false,
                        tools: Some(1),
                        actions: Some(1),
                    }),
                    ..SkillSpec::default()
                },
                source_info: SourceInfo {
                    path: "skills/paper/skill.lskill".into(),
                    kind: SourceKind::Workspace,
                },
            },
            LoadedItem {
                value: SkillSpec {
                    name: "arxiv".to_string(),
                    description: "Search arXiv".to_string(),
                    content: "Use lookup for paper search.".to_string(),
                    allowed_tools: vec!["ToolSearch".to_string()],
                    verification: Some(SkillVerificationSpec {
                        system: "lambda-skill".to_string(),
                        source_path: Some("skills/arxiv/skill.lskill".to_string()),
                        generated_path: Some("skills/arxiv/out/GENERATED.SKILL.md".to_string()),
                        host_catalogue_path: Some(child_host_path.display().to_string()),
                        compiler_path: None,
                        host_tool_bindings: Default::default(),
                        require_approval: false,
                        tools: Some(1),
                        actions: Some(1),
                    }),
                    ..SkillSpec::default()
                },
                source_info: SourceInfo {
                    path: "skills/arxiv/skill.lskill".into(),
                    kind: SourceKind::Workspace,
                },
            },
        ],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let providers = empty_providers();
    let request_config = test_openai_request_config();

    let loaded_parent = execute_tool_call(
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
        json!({"skill": "paper"}),
    )
    .unwrap();
    assert!(loaded_parent.success);

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
            "host_tool": "load_arxiv_skill",
            "args": {},
            "tool": "Skill",
            "input": {"skill": "arxiv", "args": ""},
        }),
    )
    .unwrap();
    assert!(admitted.success, "{:?}", admitted.output);
    assert!(state.pending_lambda_host_call.is_some());

    let loaded_child = execute_tool_call(
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
        json!({"skill": "arxiv", "args": ""}),
    )
    .unwrap();
    assert!(loaded_child.success);
    assert!(loaded_child
        .output
        .stdout
        .contains("<command-name>arxiv</command-name>"));
    assert!(state.pending_lambda_host_call.is_none());

    let child_gate_admit = execute_tool_call(
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
            "host_tool": "lookup",
            "args": {"query": "lambda skills"},
            "tool": "ToolSearch",
            "input": {"query": "lambda skills"},
        }),
    )
    .unwrap();
    assert!(child_gate_admit.success);
}

#[test]
fn lambda_bridge_skips_concrete_tool_approval_by_default() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let host = LambdaHostEnv::from_json_str(
        r#"{"effects":[],"domains":[],"tools":[{"name":"formal_search","concreteTools":["ToolSearch"],"concreteInputContracts":{"ToolSearch":{"query":{"$arg":"query"}}},"params":[{"name":"query","ty":"str"}],"effects":[]}]}"#,
    )
    .unwrap();
    state.lambda_gate = Some(LambdaGateState::with_host_caps(host));

    let mut tool_search = loaded_tool(
        "ToolSearch",
        "Search available tools",
        "runtime:tool_search",
    );
    tool_search.value.approval_policy = Some("ask".to_string());
    tool_search.value.sandbox_policy = Some("read-only".to_string());
    let resources = LoadedResources {
        tools: vec![
            loaded_tool(
                "LambdaHostCall",
                "Admit Lambda host call",
                "runtime:lambda_host_call",
            ),
            tool_search,
        ],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let providers = empty_providers();
    let request_config = test_openai_request_config();
    let concrete_input = json!({"query": "ToolSearch"});

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
            "args": {"query": "ToolSearch"},
            "tool": "ToolSearch",
            "input": concrete_input,
        }),
    )
    .unwrap();
    assert!(admitted.success);
    assert!(state.pending_lambda_host_call.is_some());

    let prompts = Arc::new(Mutex::new(Vec::<String>::new()));
    let prompt_log = prompts.clone();
    let executed = with_permission_prompt_handler(
        move |request| {
            prompt_log.lock().unwrap().push(request.tool_id);
            PermissionPromptAction::Deny
        },
        || {
            execute_tool_call(
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
        },
    )
    .unwrap();

    assert!(executed.success);
    assert!(prompts.lock().unwrap().is_empty());
    assert!(state.pending_lambda_host_call.is_none());
}

#[test]
fn lambda_bridge_preserves_concrete_tool_approval_prompt_when_configured() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let host = LambdaHostEnv::from_json_str(
        r#"{"effects":[],"domains":[],"tools":[{"name":"formal_search","concreteTools":["ToolSearch"],"concreteInputContracts":{"ToolSearch":{"query":{"$arg":"query"}}},"params":[{"name":"query","ty":"str"}],"effects":[]}]}"#,
    )
    .unwrap();
    let mut gate = LambdaGateState::with_host_caps(host);
    gate.set_require_concrete_tool_approval(true);
    state.lambda_gate = Some(gate);

    let mut tool_search = loaded_tool(
        "ToolSearch",
        "Search available tools",
        "runtime:tool_search",
    );
    tool_search.value.approval_policy = Some("ask".to_string());
    tool_search.value.sandbox_policy = Some("read-only".to_string());
    let resources = LoadedResources {
        tools: vec![
            loaded_tool(
                "LambdaHostCall",
                "Admit Lambda host call",
                "runtime:lambda_host_call",
            ),
            tool_search,
        ],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let providers = empty_providers();
    let request_config = test_openai_request_config();
    let concrete_input = json!({"query": "ToolSearch"});

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
            "args": {"query": "ToolSearch"},
            "tool": "ToolSearch",
            "input": concrete_input,
        }),
    )
    .unwrap();
    assert!(admitted.success);
    assert!(state.pending_lambda_host_call.is_some());

    let prompts = Arc::new(Mutex::new(Vec::<String>::new()));
    let prompt_log = prompts.clone();
    let executed = with_permission_prompt_handler(
        move |request| {
            prompt_log.lock().unwrap().push(request.tool_id);
            PermissionPromptAction::AllowOnce
        },
        || {
            execute_tool_call(
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
        },
    )
    .unwrap();

    assert!(executed.success);
    assert_eq!(*prompts.lock().unwrap(), vec!["ToolSearch".to_string()]);
    assert!(state.pending_lambda_host_call.is_none());
}

#[test]
fn lambda_host_call_rejects_unbound_concrete_tool() {
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
            loaded_tool("Bash", "Run shell", "runtime:claude_bash"),
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
            "args": {"query": "ToolSearch"},
            "tool": "Bash",
            "input": {"command": "printf hi"},
        }),
    )
    .unwrap();

    assert!(!rejected.success);
    assert!(rejected
        .output
        .stdout
        .contains("host tool formal_search is not bound to concrete tool Bash"));
    assert!(state.pending_lambda_host_call.is_none());
}

#[test]
fn lambda_gate_commits_facts_only_after_successful_concrete_tool() {
    let mut state = temp_state();
    state.grant_all_tools_for_session();
    let cwd = state.cwd.clone();
    let host = LambdaHostEnv::from_json_str(
        r#"{"effects":["proc"],"domains":[],"tools":[{"name":"formal_run","concreteTools":["Bash"],"concreteInputContracts":{"Bash":{"command":"false"}},"effects":["proc"],"registers":[{"pred":"ran","args":[]}]}]}"#,
    )
    .unwrap();
    state.lambda_gate = Some(LambdaGateState::with_host_caps(host));

    let mut bash = loaded_tool("Bash", "Run shell", "runtime:claude_bash");
    bash.value.approval_policy = Some("auto".to_string());
    bash.value.sandbox_policy = Some("workspace-write".to_string());
    let resources = LoadedResources {
        tools: vec![
            loaded_tool(
                "LambdaHostCall",
                "Admit Lambda host call",
                "runtime:lambda_host_call",
            ),
            bash,
        ],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let providers = empty_providers();
    let request_config = test_openai_request_config();
    let concrete_input = json!({"command": "false"});

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
            "host_tool": "formal_run",
            "args": {},
            "tool": "Bash",
            "input": concrete_input,
        }),
    )
    .unwrap();
    assert!(admitted.success);

    let failed = execute_tool_call(
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
        "Bash",
        json!({"command": "false"}),
    )
    .unwrap();

    assert!(!failed.success);
    assert!(state.pending_lambda_host_call.is_some());
    let gate = state.lambda_gate.as_ref().expect("gate remains installed");
    assert!(!gate
        .facts()
        .contains(&crate::runtime::lambda_gate::LambdaFact::new(
            "ran",
            Vec::new(),
        )));
    assert!(failed.output.metadata.get("lambda_skill").is_none());
}

#[test]
fn lambda_pending_bash_bridge_ignores_description_and_default_tty() {
    let mut state = temp_state();
    state.grant_all_tools_for_session();
    let cwd = state.cwd.clone();
    let host = LambdaHostEnv::from_json_str(
        r#"{"effects":["proc"],"domains":[],"tools":[{"name":"formal_run","concreteTools":["Bash"],"concreteInputContracts":{"Bash":{"command":"printf lambda-ok","run_in_background":false,"timeout":60000}},"effects":["proc"],"registers":[{"pred":"ran","args":[]}]}]}"#,
    )
    .unwrap();
    state.lambda_gate = Some(LambdaGateState::with_host_caps(host));

    let mut bash = loaded_tool("Bash", "Run shell", "runtime:claude_bash");
    bash.value.approval_policy = Some("auto".to_string());
    bash.value.sandbox_policy = Some("workspace-write".to_string());
    let resources = LoadedResources {
        tools: vec![
            loaded_tool(
                "LambdaHostCall",
                "Admit Lambda host call",
                "runtime:lambda_host_call",
            ),
            bash,
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
            "host_tool": "formal_run",
            "args": {},
            "tool": "Bash",
        }),
    )
    .unwrap();
    assert!(admitted.success, "{}", admitted.output.stdout);
    assert_eq!(
        admitted.output.metadata["lambda_skill"]["concrete_input"],
        json!({
            "command": "printf lambda-ok",
            "run_in_background": false,
            "timeout": 60000,
            "tty": false,
        })
    );

    let completed = execute_tool_call(
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
        "Bash",
        json!({
            "command": "printf lambda-ok",
            "description": "Run the verified command",
            "run_in_background": false,
            "timeout": 60000,
            "tty": false,
        }),
    )
    .unwrap();

    assert!(completed.success, "{}", completed.output.stdout);
    assert_eq!(
        completed.output.metadata["lambda_skill"]["event"],
        json!("host_call_committed")
    );
    assert!(state.pending_lambda_host_call.is_none());
}

#[test]
fn lambda_bash_result_check_uses_inner_stdout_payload() {
    let mut state = temp_state();
    state.grant_all_tools_for_session();
    let cwd = state.cwd.clone();
    let host = LambdaHostEnv::from_json_str(
        r#"{"effects":["proc"],"domains":["Paper"],"tools":[{"name":"parse","concreteTools":["Bash"],"concreteInputContracts":{"Bash":{"command":"printf '[{\"title\":\"x\",\"arxiv_id\":\"2605.13044\"}]'","run_in_background":false,"timeout":60000}},"effects":["proc"],"result":"[Paper]{parsed_ok(p)}"}]}"#,
    )
    .unwrap();
    state.lambda_gate = Some(LambdaGateState::with_host_caps(host));

    let mut bash = loaded_tool("Bash", "Run shell", "runtime:claude_bash");
    bash.value.approval_policy = Some("auto".to_string());
    bash.value.sandbox_policy = Some("workspace-write".to_string());
    let resources = LoadedResources {
        tools: vec![
            loaded_tool(
                "LambdaHostCall",
                "Admit Lambda host call",
                "runtime:lambda_host_call",
            ),
            bash,
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
            "host_tool": "parse",
            "args": {},
            "tool": "Bash",
        }),
    )
    .unwrap();
    assert!(admitted.success, "{}", admitted.output.stdout);

    let completed = execute_tool_call(
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
        "Bash",
        json!({
            "command": "printf '[{\"title\":\"x\",\"arxiv_id\":\"2605.13044\"}]'",
            "run_in_background": false,
            "timeout": 60000,
            "tty": false,
        }),
    )
    .unwrap();

    assert!(completed.success, "{}", completed.output.stdout);
    assert_eq!(
        completed.output.metadata["lambda_skill"]["event"],
        json!("host_call_committed")
    );
    assert!(state.pending_lambda_host_call.is_none());
}

#[test]
fn lambda_host_call_bridges_internal_skill_step() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let host = LambdaHostEnv::from_json_str(
        r#"{"effects":["proc"],"domains":["Ticket"],"tools":[{"name":"parse_context","params":[{"name":"ticket","ty":"str"}],"result":"Ticket","effects":["proc"],"registers":[{"pred":"parsed","args":["ticket"]}],"concreteTools":["LambdaInternal"],"concreteInputContracts":{"LambdaInternal":{"step":"support-ticket-triage.parse_context","args":{"ticket":{"$arg":"ticket"}},"result":{"$template":"parsed ${ticket}"}}}}]}"#,
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
                "LambdaInternal",
                "Record Lambda internal step",
                "runtime:workflow:lambda_internal",
            ),
        ],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let providers = empty_providers();
    let request_config = test_openai_request_config();
    let concrete_input = json!({
        "step": "support-ticket-triage.parse_context",
        "args": {"ticket": "refund request"},
        "result": "parsed refund request",
    });

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
            "host_tool": "parse_context",
            "args": {"ticket": "refund request"},
            "tool": "LambdaInternal",
            "input": concrete_input,
        }),
    )
    .unwrap();
    assert!(admitted.success);

    let completed = execute_tool_call(
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
        "LambdaInternal",
        json!({
            "step": "support-ticket-triage.parse_context",
            "args": {"ticket": "refund request"},
            "result": "parsed refund request",
        }),
    )
    .unwrap();

    assert!(completed.success);
    assert_eq!(completed.output.stdout, "parsed refund request");
    assert!(state.pending_lambda_host_call.is_none());
    let gate = state.lambda_gate.as_ref().expect("gate remains installed");
    assert!(gate
        .facts()
        .contains(&crate::runtime::lambda_gate::LambdaFact::new(
            "parsed",
            vec!["\"refund request\"".to_string()],
        )));
}

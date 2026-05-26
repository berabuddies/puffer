use super::*;
use puffer_resources::{LoadedItem, SkillSpec, SkillVerificationSpec, SourceInfo, SourceKind};

#[test]
fn openai_skill_batch_serializes_before_parallel_safe_concrete_tools() {
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
        tools: vec![
            skill_tool,
            loaded_tool(
                "ToolSearch",
                "Search available tools",
                "runtime:tool_search",
            ),
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
    let tool_calls = vec![
        OpenAIResponseToolCall {
            item_id: Some("fc_skill".to_string()),
            status: Some("completed".to_string()),
            call_id: "call_skill".to_string(),
            name: "Skill".to_string(),
            arguments: json!({"skill": "issue-triage"}),
        },
        OpenAIResponseToolCall {
            item_id: Some("fc_search".to_string()),
            status: Some("completed".to_string()),
            call_id: "call_search".to_string(),
            name: "ToolSearch".to_string(),
            arguments: json!({"query": "ToolSearch"}),
        },
        OpenAIResponseToolCall {
            item_id: Some("fc_search_2".to_string()),
            status: Some("completed".to_string()),
            call_id: "call_search_2".to_string(),
            name: "ToolSearch".to_string(),
            arguments: json!({"query": "Bash"}),
        },
    ];

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
        None,
    )
    .unwrap();

    assert!(result.invocations[0].success);
    assert_eq!(result.invocations[0].tool_id, "Skill");
    assert!(!result.invocations[1].success);
    assert_eq!(
        result.invocations[1].metadata["lambda_skill"]["retry_tool"],
        json!("LambdaHostCall")
    );
    assert!(result.invocations[1]
        .output
        .contains("requires LambdaHostCall before concrete tool calls"));
    assert!(!result.invocations[2].success);
    assert_eq!(
        result.invocations[2].metadata["lambda_skill"]["retry_tool"],
        json!("LambdaHostCall")
    );
}

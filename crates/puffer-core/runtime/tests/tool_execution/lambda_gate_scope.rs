use super::*;
use puffer_resources::{LoadedItem, SkillSpec, SkillVerificationSpec, SourceInfo, SourceKind};

#[test]
fn model_invoked_plain_skill_clears_active_lambda_gate() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let host_path = cwd.join("verified-host.json");
    fs::write(
        &host_path,
        r#"{"effects":[],"domains":[],"tools":[{"name":"formal_search","params":[{"name":"query","ty":"str"}],"effects":[]}]}"#,
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

    let loaded_plain = execute_tool_call(
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
    .unwrap();

    assert!(loaded_plain.success);
    assert!(
        loaded_plain
            .output
            .stdout
            .contains("<command-name>reviewer</command-name>")
    );
    assert!(state.lambda_gate.is_none());
    assert!(state.pending_lambda_host_call.is_none());
}

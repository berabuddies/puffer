use super::*;

#[test]
fn legacy_search_text_external_path_uses_manual_approval() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let outside = outside_workspace_tempdir(&cwd);
    fs::write(outside.path().join("skills.md"), "idr skill needle\n").unwrap();
    let resources = LoadedResources {
        tools: vec![loaded_tool("search_text", "Search text", "search_text")],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let mut providers = ProviderRegistry::new();
    providers.register(openai_provider("http://127.0.0.1".to_string()));
    let request_config = test_openai_request_config();
    let prompts = Arc::new(Mutex::new(Vec::<String>::new()));
    let prompt_log = prompts.clone();
    let outside_path = outside.path().display().to_string();

    let result = with_permission_prompt_handler(
        move |request| {
            prompt_log.lock().unwrap().push(format!(
                "{}\n{}",
                request.summary,
                request.reason.unwrap_or_default()
            ));
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
                "search_text",
                json!({
                    "query": "needle",
                    "path": outside_path
                }),
            )
        },
    )
    .unwrap();

    assert!(result.success);
    assert!(result.output.stdout.contains("skills.md"));
    assert!(result.output.stdout.contains("needle"));
    let prompts = prompts.lock().unwrap();
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].contains("Allow search_text to access"));
    assert!(prompts[0].contains("outside the current working directories"));
    assert!(state
        .session_permission_state()
        .grants()
        .profile_view(false)
        .path_prefix_grants
        .is_empty());
}

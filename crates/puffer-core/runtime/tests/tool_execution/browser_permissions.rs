use super::*;
use crate::runtime::browser_auto_review::{
    with_browser_auto_review_test_handler, BrowserAutoReviewRequest,
};
use crate::runtime::{
    BrowserAutoReviewActionSet, BrowserAutoReviewRawAction, BrowserAutoReviewRuntimeResult,
    BrowserAutoReviewSessionTargeting, BrowserAutoReviewSource,
    BrowserAutoReviewSuggestedGrantScope, BrowserAutoReviewTargetClass, BrowserAutoReviewUrlSource,
    BrowserPermissionPromptActionSet, BrowserPermissionPromptSource,
    BrowserPermissionPromptTargetClass, PermissionPromptRequest,
};
use puffer_tools::internal_permissions::InternalToolPermissionRequest;

fn browser_resources() -> LoadedResources {
    let mut tool = loaded_tool("Browser", "Managed browser", "runtime:browser");
    tool.value.approval_policy = Some("auto".to_string());
    tool.value.sandbox_policy = Some("workspace-write".to_string());
    LoadedResources {
        tools: vec![tool.clone()],
        internal_tools: vec![tool],
        ..LoadedResources::default()
    }
}

fn openai_browser_providers() -> ProviderRegistry {
    let mut providers = ProviderRegistry::new();
    providers.register(openai_provider("http://127.0.0.1".to_string()));
    providers
}

fn capture_permission_prompts<R>(
    prompts: Arc<Mutex<Vec<PermissionPromptRequest>>>,
    action: PermissionPromptAction,
    run: impl FnOnce() -> R,
) -> R {
    with_permission_prompt_handler(
        move |request| {
            prompts.lock().unwrap().push(request);
            action
        },
        run,
    )
}

#[test]
fn allow_session_browser_approval_does_not_expand_without_target_context() {
    let resources = browser_resources();
    let registry = ToolRegistry::from_resources(&resources);
    let providers = openai_browser_providers();
    let mut auth_store = AuthStore::default();
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let session_id = state.session.id.to_string();
    let prompts = Arc::new(Mutex::new(Vec::<String>::new()));
    let prompt_log = Arc::clone(&prompts);

    crate::runtime::local_tools::with_browser_daemon_test_handler(
        move |method, params| {
            assert_eq!(method, "browser_current_tab");
            let request_session = params["sessionId"].as_str().unwrap_or_default();
            let payload = if request_session == session_id {
                json!({
                    "status": "available",
                    "tabId": "t1",
                    "url": "https://docs.example.com/page",
                    "origin": "https://docs.example.com",
                    "host": "docs.example.com",
                    "port": 443,
                    "title": "Docs"
                })
            } else {
                json!({
                    "status": "available",
                    "tabId": "t9",
                    "url": "https://other.example.com/page",
                    "origin": "https://other.example.com",
                    "host": "other.example.com",
                    "port": 443,
                    "title": "Other"
                })
            };
            Ok(payload)
        },
        || {
            with_permission_prompt_handler(
                move |request| {
                    prompt_log.lock().unwrap().push(
                        request
                            .reason
                            .clone()
                            .unwrap_or_else(|| request.tool_id.clone()),
                    );
                    PermissionPromptAction::AllowSession
                },
                || {
                    let first = resolve_tool_permission(
                        &mut state,
                        &resources,
                        &providers,
                        &mut auth_store,
                        &registry,
                        &cwd,
                        "Browser",
                        &json!({"action": "evaluate", "script": "document.title"}),
                        None,
                    )
                    .unwrap();
                    assert!(matches!(first, PermissionOutcome::Allowed(_)));

                    let second = resolve_tool_permission(
                        &mut state,
                        &resources,
                        &providers,
                        &mut auth_store,
                        &registry,
                        &cwd,
                        "Browser",
                        &json!({"action": "evaluate", "script": "window.location.href"}),
                        None,
                    )
                    .unwrap();
                    assert!(matches!(second, PermissionOutcome::Allowed(_)));

                    let third = resolve_tool_permission(
                        &mut state,
                        &resources,
                        &providers,
                        &mut auth_store,
                        &registry,
                        &cwd,
                        "Browser",
                        &json!({"action": "navigate", "url": "https://example.com"}),
                        None,
                    )
                    .unwrap();
                    assert!(matches!(third, PermissionOutcome::Allowed(_)));

                    let fourth = resolve_tool_permission(
                        &mut state,
                        &resources,
                        &providers,
                        &mut auth_store,
                        &registry,
                        &cwd,
                        "Browser",
                        &json!({
                            "action": "evaluate",
                            "sessionId": "b4f239fd-1493-4be7-a3a1-9e58fe612576",
                            "script": "document.title"
                        }),
                        None,
                    )
                    .unwrap();
                    assert!(matches!(fourth, PermissionOutcome::Allowed(_)));
                },
            )
        },
    );

    let prompts = prompts.lock().unwrap();
    assert_eq!(prompts.len(), 2);
    assert!(prompts[0].contains("executes page JavaScript"));
    assert!(prompts[1].contains("navigation and interaction require approval"));
    assert!(state.session_permission_state().has_browser_grant());
}

#[test]
fn allow_session_browser_approval_reuses_domain_scope_without_enabling_all_tools() {
    let resources = browser_resources();
    let registry = ToolRegistry::from_resources(&resources);
    let providers = empty_providers();
    let mut auth_store = AuthStore::default();
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let prompts = Arc::new(Mutex::new(Vec::<String>::new()));
    let prompt_log = Arc::clone(&prompts);

    with_permission_prompt_handler(
        move |request| {
            prompt_log.lock().unwrap().push(
                request
                    .reason
                    .clone()
                    .unwrap_or_else(|| request.tool_id.clone()),
            );
            PermissionPromptAction::AllowSession
        },
        || {
            let first = resolve_tool_permission(
                &mut state,
                &resources,
                &providers,
                &mut auth_store,
                &registry,
                &cwd,
                "Browser",
                &json!({"action":"navigate","url":"https://docs.example.com/a"}),
                None,
            )
            .unwrap();
            assert!(matches!(first, PermissionOutcome::Allowed(_)));

            let second = resolve_tool_permission(
                &mut state,
                &resources,
                &providers,
                &mut auth_store,
                &registry,
                &cwd,
                "Browser",
                &json!({"action":"click","url":"https://docs.example.com/b"}),
                None,
            )
            .unwrap();
            assert!(matches!(second, PermissionOutcome::Allowed(_)));

            let third = resolve_tool_permission(
                &mut state,
                &resources,
                &providers,
                &mut auth_store,
                &registry,
                &cwd,
                "Browser",
                &json!({"action":"click","url":"https://api.example.com/b"}),
                None,
            )
            .unwrap();
            assert!(matches!(third, PermissionOutcome::Allowed(_)));
        },
    );

    let prompts = prompts.lock().unwrap();
    assert_eq!(prompts.len(), 1);
    assert!(!state.session_permission_state().allow_all_tools());
    assert!(state.session_permission_state().has_browser_grant());
}

#[test]
fn browser_permission_prompt_carries_context_without_scope_choices() {
    let resources = browser_resources();
    let registry = ToolRegistry::from_resources(&resources);
    let providers = empty_providers();
    let mut auth_store = AuthStore::default();
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let seen_browser = Arc::new(Mutex::new(None));
    let seen_browser_clone = Arc::clone(&seen_browser);

    with_permission_prompt_handler(
        move |request| {
            *seen_browser_clone.lock().unwrap() = request.browser;
            PermissionPromptAction::Deny
        },
        || {
            let outcome = resolve_tool_permission(
                &mut state,
                &resources,
                &providers,
                &mut auth_store,
                &registry,
                &cwd,
                "Browser",
                &json!({"action":"navigate","url":"https://docs.example.com/a"}),
                None,
            )
            .unwrap();
            assert!(matches!(outcome, PermissionOutcome::Denied(_)));
        },
    );

    let browser = seen_browser
        .lock()
        .unwrap()
        .clone()
        .expect("browser payload");
    assert_eq!(browser.source, BrowserPermissionPromptSource::BrowserTool);
    assert_eq!(
        browser.action_set,
        BrowserPermissionPromptActionSet::Navigate
    );
    assert_eq!(browser.origin.as_deref(), Some("https://docs.example.com"));
    assert_eq!(browser.host.as_deref(), Some("docs.example.com"));
    assert_eq!(
        browser.target_class,
        BrowserPermissionPromptTargetClass::OpenWeb
    );
}

#[test]
fn browser_permission_prompt_summary_uses_action_and_target() {
    let resources = browser_resources();
    let registry = ToolRegistry::from_resources(&resources);
    let providers = empty_providers();
    let mut auth_store = AuthStore::default();
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let seen_request = Arc::new(Mutex::new(None));
    let seen_request_clone = Arc::clone(&seen_request);

    with_permission_prompt_handler(
        move |request| {
            *seen_request_clone.lock().unwrap() = Some(request);
            PermissionPromptAction::Deny
        },
        || {
            let outcome = resolve_tool_permission(
                &mut state,
                &resources,
                &providers,
                &mut auth_store,
                &registry,
                &cwd,
                "Browser",
                &json!({"action":"navigate","url":"https://docs.example.com/a"}),
                None,
            )
            .unwrap();
            assert!(matches!(outcome, PermissionOutcome::Denied(_)));
        },
    );

    let request = seen_request
        .lock()
        .unwrap()
        .clone()
        .expect("permission request");
    assert_eq!(request.summary, "Open https://docs.example.com/a");
    assert_eq!(
        request.reason.as_deref(),
        Some("browser navigation and interaction require approval")
    );
}

#[test]
fn browser_tool_session_grant_is_reused_by_internal_browser_route() {
    let resources = browser_resources();
    let registry = ToolRegistry::from_resources(&resources);
    let providers = empty_providers();
    let mut auth_store = AuthStore::default();
    let mut state = temp_state();
    let cwd = state.cwd.clone();

    with_permission_prompt_handler(
        move |_request| PermissionPromptAction::AllowSession,
        || {
            let first = resolve_tool_permission(
                &mut state,
                &resources,
                &providers,
                &mut auth_store,
                &registry,
                &cwd,
                "Browser",
                &json!({"action":"navigate","url":"https://docs.example.com/a"}),
                None,
            )
            .unwrap();
            assert!(matches!(first, PermissionOutcome::Allowed(_)));

            let second =
                crate::runtime::internal_tool_permissions::resolve_internal_tool_permission(
                    &mut state,
                    &resources,
                    &registry,
                    &cwd,
                    InternalToolPermissionRequest {
                        tool_id: "browser".to_string(),
                        input: json!({"action":"navigate","url":"https://docs.example.com/b"}),
                    },
                );
            assert!(second.is_allowed(), "{second:?}");
        },
    );
}

#[test]
fn internal_browser_session_grant_is_reused_by_browser_tool_route() {
    let resources = browser_resources();
    let registry = ToolRegistry::from_resources(&resources);
    let providers = empty_providers();
    let mut auth_store = AuthStore::default();
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let prompts = Arc::new(Mutex::new(Vec::<PermissionPromptRequest>::new()));

    capture_permission_prompts(
        Arc::clone(&prompts),
        PermissionPromptAction::AllowSession,
        || {
            let first = crate::runtime::internal_tool_permissions::resolve_internal_tool_permission(
                &mut state,
                &resources,
                &registry,
                &cwd,
                InternalToolPermissionRequest {
                    tool_id: "browser".to_string(),
                    input: json!({"action":"navigate","url":"https://docs.example.com/a"}),
                },
            );
            assert!(first.is_allowed(), "{first:?}");

            let second = resolve_tool_permission(
                &mut state,
                &resources,
                &providers,
                &mut auth_store,
                &registry,
                &cwd,
                "Browser",
                &json!({"action":"navigate","url":"https://docs.example.com/b"}),
                None,
            )
            .unwrap();
            assert!(matches!(second, PermissionOutcome::Allowed(_)));
        },
    );

    let prompts = prompts.lock().unwrap();
    let browser = prompts[0].browser.as_ref().expect("browser prompt");
    assert_eq!(
        browser.source,
        BrowserPermissionPromptSource::BrowserInternalTool
    );
}

#[test]
fn foreign_root_browser_evaluate_can_be_denied_at_prompt_time() {
    let resources = browser_resources();
    let registry = ToolRegistry::from_resources(&resources);
    let providers = empty_providers();
    let mut auth_store = AuthStore::default();
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let other_session = "b4f239fd-1493-4be7-a3a1-9e58fe612576";

    crate::runtime::local_tools::with_browser_daemon_test_handler(
        move |method, params| {
            assert_eq!(method, "browser_current_tab");
            assert_eq!(params["sessionId"], other_session);
            Ok(json!({
                "status": "available",
                "tabId": "t9",
                "url": "https://other.example.com/page",
                "origin": "https://other.example.com",
                "host": "other.example.com",
                "port": 443,
                "title": "Other"
            }))
        },
        || {
            with_permission_prompt_handler(
                move |_request| PermissionPromptAction::Deny,
                || {
                    let first = resolve_tool_permission(
                        &mut state,
                        &resources,
                        &providers,
                        &mut auth_store,
                        &registry,
                        &cwd,
                        "Browser",
                        &json!({
                            "action": "evaluate",
                            "sessionId": other_session,
                            "script": "document.title"
                        }),
                        None,
                    )
                    .unwrap();
                    let PermissionOutcome::Denied(result) = first else {
                        panic!("expected denied browser permission outcome");
                    };
                    assert!(result.output.stdout.contains("permission denied by user"));
                },
            )
        },
    );
}

#[test]
fn browser_permission_enriches_missing_url_from_daemon_context_before_decision() {
    let resources = browser_resources();
    let registry = ToolRegistry::from_resources(&resources);
    let providers = empty_providers();
    let mut auth_store = AuthStore::default();
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let session_id = state.session.id.to_string();
    let prompts = Arc::new(Mutex::new(Vec::<String>::new()));
    let prompt_log = Arc::clone(&prompts);
    let outcome = crate::runtime::local_tools::with_browser_daemon_test_handler(
        move |method, params| {
            assert_eq!(method, "browser_current_tab");
            assert_eq!(params["sessionId"], session_id);
            Ok(json!({
                "status": "available",
                "tabId": "t4",
                "url": "https://docs.example.com/page",
                "origin": "https://docs.example.com",
                "host": "docs.example.com",
                "port": 443,
                "title": "Docs"
            }))
        },
        || {
            with_permission_prompt_handler(
                move |request| {
                    prompt_log
                        .lock()
                        .unwrap()
                        .push(request.reason.unwrap_or_else(|| request.tool_id));
                    PermissionPromptAction::Deny
                },
                || {
                    resolve_tool_permission(
                        &mut state,
                        &resources,
                        &providers,
                        &mut auth_store,
                        &registry,
                        &cwd,
                        "Browser",
                        &json!({
                            "action": "evaluate",
                            "script": "document.title"
                        }),
                        None,
                    )
                    .unwrap()
                },
            )
        },
    );
    let prompts = prompts.lock().unwrap();
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].contains("executes page JavaScript"));
    assert!(!prompts[0].contains("no target URL"));
    let PermissionOutcome::Denied(result) = outcome else {
        panic!("expected denied outcome from prompt handler");
    };
    assert!(result.output.stdout.contains("permission denied by user"));
}

#[test]
fn browser_ask_reviewer_allow_once_skips_human_prompt() {
    let resources = browser_resources();
    let registry = ToolRegistry::from_resources(&resources);
    let providers = empty_providers();
    let mut auth_store = AuthStore::default();
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let prompts = Arc::new(Mutex::new(Vec::<PermissionPromptRequest>::new()));
    let review_requests = Arc::new(Mutex::new(Vec::new()));
    let review_log = Arc::clone(&review_requests);

    with_browser_auto_review_test_handler(
        move |request: &BrowserAutoReviewRequest| {
            review_log.lock().unwrap().push(request.clone());
            BrowserAutoReviewRuntimeResult::AllowOnce
        },
        || {
            capture_permission_prompts(Arc::clone(&prompts), PermissionPromptAction::Deny, || {
                let outcome = resolve_tool_permission(
                    &mut state,
                    &resources,
                    &providers,
                    &mut auth_store,
                    &registry,
                    &cwd,
                    "Browser",
                    &json!({"action":"navigate","url":"https://docs.example.com/a"}),
                    None,
                )
                .unwrap();
                assert!(matches!(outcome, PermissionOutcome::Allowed(_)));
            });
        },
    );

    assert!(prompts.lock().unwrap().is_empty());
    assert_eq!(review_requests.lock().unwrap().len(), 1);
    assert!(!state.session_permission_state().has_browser_grant());
}

#[test]
fn browser_ask_reviewer_allow_session_reuses_grant_without_human_prompt() {
    let resources = browser_resources();
    let registry = ToolRegistry::from_resources(&resources);
    let providers = empty_providers();
    let mut auth_store = AuthStore::default();
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let prompts = Arc::new(Mutex::new(Vec::<PermissionPromptRequest>::new()));
    let review_requests = Arc::new(Mutex::new(Vec::new()));
    let review_log = Arc::clone(&review_requests);

    with_browser_auto_review_test_handler(
        move |request: &BrowserAutoReviewRequest| {
            review_log.lock().unwrap().push(request.clone());
            BrowserAutoReviewRuntimeResult::AllowSession
        },
        || {
            capture_permission_prompts(Arc::clone(&prompts), PermissionPromptAction::Deny, || {
                let first = resolve_tool_permission(
                    &mut state,
                    &resources,
                    &providers,
                    &mut auth_store,
                    &registry,
                    &cwd,
                    "Browser",
                    &json!({"action":"navigate","url":"https://docs.example.com/a"}),
                    None,
                )
                .unwrap();
                assert!(matches!(first, PermissionOutcome::Allowed(_)));

                let second = resolve_tool_permission(
                    &mut state,
                    &resources,
                    &providers,
                    &mut auth_store,
                    &registry,
                    &cwd,
                    "Browser",
                    &json!({"action":"click","url":"https://docs.example.com/b"}),
                    None,
                )
                .unwrap();
                assert!(matches!(second, PermissionOutcome::Allowed(_)));
            });
        },
    );

    assert!(prompts.lock().unwrap().is_empty());
    let reviews = review_requests.lock().unwrap();
    assert_eq!(reviews.len(), 1);
    assert_eq!(reviews[0].source, BrowserAutoReviewSource::BrowserTool);
    assert_eq!(reviews[0].action_set, BrowserAutoReviewActionSet::Navigate);
    assert_eq!(reviews[0].raw_action, BrowserAutoReviewRawAction::Navigate);
    assert_eq!(
        reviews[0].target_class,
        BrowserAutoReviewTargetClass::OpenWeb
    );
    assert_eq!(reviews[0].url_source, BrowserAutoReviewUrlSource::Explicit);
    assert_eq!(
        reviews[0].requested_url.as_deref(),
        Some("https://docs.example.com/a")
    );
    assert_eq!(reviews[0].current_tab_url, None);
    assert!(!reviews[0].tab_management);
    assert_eq!(
        reviews[0].session_targeting,
        BrowserAutoReviewSessionTargeting::CurrentSession
    );
    assert_eq!(
        reviews[0].suggested_grant_scope,
        BrowserAutoReviewSuggestedGrantScope::AllowDomainSession
    );
    assert!(state.session_permission_state().has_browser_grant());
}

#[test]
fn browser_ask_reviewer_deny_returns_denied_without_human_prompt() {
    let resources = browser_resources();
    let registry = ToolRegistry::from_resources(&resources);
    let providers = empty_providers();
    let mut auth_store = AuthStore::default();
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let prompts = Arc::new(Mutex::new(Vec::<PermissionPromptRequest>::new()));

    with_browser_auto_review_test_handler(
        move |_request| BrowserAutoReviewRuntimeResult::Deny,
        || {
            capture_permission_prompts(
                Arc::clone(&prompts),
                PermissionPromptAction::AllowOnce,
                || {
                    let outcome = resolve_tool_permission(
                        &mut state,
                        &resources,
                        &providers,
                        &mut auth_store,
                        &registry,
                        &cwd,
                        "Browser",
                        &json!({"action":"navigate","url":"https://docs.example.com/a"}),
                        None,
                    )
                    .unwrap();
                    let PermissionOutcome::Denied(result) = outcome else {
                        panic!("expected denied result");
                    };
                    assert!(result.output.stdout.contains("permission denied by user"));
                },
            );
        },
    );

    assert!(prompts.lock().unwrap().is_empty());
}

#[test]
fn browser_ask_reviewer_needs_user_falls_back_to_human_prompt() {
    let resources = browser_resources();
    let registry = ToolRegistry::from_resources(&resources);
    let providers = empty_providers();
    let mut auth_store = AuthStore::default();
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let prompts = Arc::new(Mutex::new(Vec::<PermissionPromptRequest>::new()));

    with_browser_auto_review_test_handler(
        move |_request| BrowserAutoReviewRuntimeResult::NeedsUser,
        || {
            capture_permission_prompts(
                Arc::clone(&prompts),
                PermissionPromptAction::AllowSession,
                || {
                    let outcome = resolve_tool_permission(
                        &mut state,
                        &resources,
                        &providers,
                        &mut auth_store,
                        &registry,
                        &cwd,
                        "Browser",
                        &json!({"action":"navigate","url":"https://docs.example.com/a"}),
                        None,
                    )
                    .unwrap();
                    assert!(matches!(outcome, PermissionOutcome::Allowed(_)));
                },
            );
        },
    );

    let prompts = prompts.lock().unwrap();
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].review, None);
    assert!(state.session_permission_state().has_browser_grant());
}

#[test]
fn browser_ask_reviewer_unavailable_falls_back_to_human_prompt() {
    let resources = browser_resources();
    let registry = ToolRegistry::from_resources(&resources);
    let providers = empty_providers();
    let mut auth_store = AuthStore::default();
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let prompts = Arc::new(Mutex::new(Vec::<PermissionPromptRequest>::new()));

    with_browser_auto_review_test_handler(
        move |_request| BrowserAutoReviewRuntimeResult::Unavailable,
        || {
            capture_permission_prompts(
                Arc::clone(&prompts),
                PermissionPromptAction::AllowOnce,
                || {
                    let outcome = resolve_tool_permission(
                        &mut state,
                        &resources,
                        &providers,
                        &mut auth_store,
                        &registry,
                        &cwd,
                        "Browser",
                        &json!({"action":"navigate","url":"https://docs.example.com/a"}),
                        None,
                    )
                    .unwrap();
                    assert!(matches!(outcome, PermissionOutcome::Allowed(_)));
                },
            );
        },
    );

    assert_eq!(prompts.lock().unwrap().len(), 1);
    assert!(!state.session_permission_state().has_browser_grant());
}

#[test]
fn browser_evaluator_allow_does_not_enter_reviewer() {
    let resources = browser_resources();
    let registry = ToolRegistry::from_resources(&resources);
    let providers = empty_providers();
    let mut auth_store = AuthStore::default();
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let permissions_dir = ConfigPaths::discover(&cwd).workspace_config_dir;
    fs::create_dir_all(&permissions_dir).unwrap();
    fs::write(
        permissions_dir.join("permissions.toml"),
        "[browser]\nallow_domains = [\"example.com\"]\n",
    )
    .unwrap();
    let review_count = Arc::new(Mutex::new(0_usize));
    let review_log = Arc::clone(&review_count);
    let prompts = Arc::new(Mutex::new(Vec::<PermissionPromptRequest>::new()));

    with_browser_auto_review_test_handler(
        move |_request| {
            *review_log.lock().unwrap() += 1;
            BrowserAutoReviewRuntimeResult::Deny
        },
        || {
            capture_permission_prompts(Arc::clone(&prompts), PermissionPromptAction::Deny, || {
                let outcome = resolve_tool_permission(
                    &mut state,
                    &resources,
                    &providers,
                    &mut auth_store,
                    &registry,
                    &cwd,
                    "Browser",
                    &json!({"action":"navigate","url":"https://docs.example.com/a"}),
                    None,
                )
                .unwrap();
                assert!(matches!(outcome, PermissionOutcome::Allowed(_)));
            });
        },
    );

    assert_eq!(*review_count.lock().unwrap(), 0);
    assert!(prompts.lock().unwrap().is_empty());
}

#[test]
fn browser_tab_management_low_risk_actions_can_be_auto_reviewed_without_user_prompt() {
    let resources = browser_resources();
    let registry = ToolRegistry::from_resources(&resources);
    let providers = empty_providers();
    let mut auth_store = AuthStore::default();
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let session_id = state.session.id.to_string();
    let prompts = Arc::new(Mutex::new(Vec::<PermissionPromptRequest>::new()));
    let review_requests = Arc::new(Mutex::new(Vec::<BrowserAutoReviewRequest>::new()));
    let review_log = Arc::clone(&review_requests);

    crate::runtime::local_tools::with_browser_daemon_test_handler(
        move |method, params| {
            assert_eq!(method, "browser_current_tab");
            assert_eq!(params["sessionId"], session_id);
            Ok(json!({
                "status": "available",
                "tabId": "t1",
                "url": "https://docs.example.com/page",
                "origin": "https://docs.example.com",
                "host": "docs.example.com",
                "port": 443,
                "title": "Docs"
            }))
        },
        || {
            with_browser_auto_review_test_handler(
                move |request| {
                    review_log.lock().unwrap().push(request.clone());
                    assert!(request.tab_management);
                    assert_eq!(
                        request.session_targeting,
                        BrowserAutoReviewSessionTargeting::CurrentSession
                    );
                    match request.raw_action {
                        BrowserAutoReviewRawAction::New
                        | BrowserAutoReviewRawAction::Open
                        | BrowserAutoReviewRawAction::Focus => {
                            assert_eq!(request.action_set, BrowserAutoReviewActionSet::Navigate);
                            BrowserAutoReviewRuntimeResult::AllowOnce
                        }
                        BrowserAutoReviewRawAction::List => {
                            assert_eq!(request.action_set, BrowserAutoReviewActionSet::Inspect);
                            BrowserAutoReviewRuntimeResult::AllowOnce
                        }
                        other => panic!("unexpected raw action {other:?}"),
                    }
                },
                || {
                    capture_permission_prompts(
                        Arc::clone(&prompts),
                        PermissionPromptAction::Deny,
                        || {
                            for input in [
                                json!({"action":"new"}),
                                json!({"action":"open"}),
                                json!({"action":"focus","tabId":"t2"}),
                                json!({"action":"list"}),
                            ] {
                                let outcome = resolve_tool_permission(
                                    &mut state,
                                    &resources,
                                    &providers,
                                    &mut auth_store,
                                    &registry,
                                    &cwd,
                                    "Browser",
                                    &input,
                                    None,
                                )
                                .unwrap();
                                assert!(matches!(outcome, PermissionOutcome::Allowed(_)));
                            }
                        },
                    );
                },
            );
        },
    );

    assert!(prompts.lock().unwrap().is_empty());
    let reviews = review_requests.lock().unwrap();
    assert_eq!(reviews.len(), 4);
    assert_eq!(reviews[0].raw_action, BrowserAutoReviewRawAction::New);
    assert_eq!(
        reviews[0].url_source,
        BrowserAutoReviewUrlSource::CurrentTab
    );
    assert_eq!(
        reviews[0].current_tab_url.as_deref(),
        Some("https://docs.example.com/page")
    );
    assert_eq!(reviews[1].raw_action, BrowserAutoReviewRawAction::Open);
    assert_eq!(reviews[2].raw_action, BrowserAutoReviewRawAction::Focus);
    assert_eq!(reviews[3].raw_action, BrowserAutoReviewRawAction::List);
}

#[test]
fn browser_open_new_target_from_existing_tab_can_be_auto_reviewed_without_user_prompt() {
    let resources = browser_resources();
    let registry = ToolRegistry::from_resources(&resources);
    let providers = empty_providers();
    let mut auth_store = AuthStore::default();
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let session_id = state.session.id.to_string();
    let prompts = Arc::new(Mutex::new(Vec::<PermissionPromptRequest>::new()));
    let review_requests = Arc::new(Mutex::new(Vec::<BrowserAutoReviewRequest>::new()));
    let review_log = Arc::clone(&review_requests);

    crate::runtime::local_tools::with_browser_daemon_test_handler(
        move |method, params| {
            assert_eq!(method, "browser_current_tab");
            assert_eq!(params["sessionId"], session_id);
            Ok(json!({
                "status": "available",
                "tabId": "t1",
                "url": "https://docs.example.com/page",
                "origin": "https://docs.example.com",
                "host": "docs.example.com",
                "port": 443,
                "title": "Docs"
            }))
        },
        || {
            with_browser_auto_review_test_handler(
                move |request| {
                    review_log.lock().unwrap().push(request.clone());
                    assert_eq!(request.raw_action, BrowserAutoReviewRawAction::Open);
                    assert_eq!(request.url_source, BrowserAutoReviewUrlSource::Explicit);
                    assert_eq!(
                        request.requested_url.as_deref(),
                        Some("https://news.example.org")
                    );
                    assert_eq!(
                        request.current_tab_url.as_deref(),
                        Some("https://docs.example.com/page")
                    );
                    assert!(request.tab_management);
                    BrowserAutoReviewRuntimeResult::AllowOnce
                },
                || {
                    capture_permission_prompts(
                        Arc::clone(&prompts),
                        PermissionPromptAction::Deny,
                        || {
                            let outcome = resolve_tool_permission(
                                &mut state,
                                &resources,
                                &providers,
                                &mut auth_store,
                                &registry,
                                &cwd,
                                "Browser",
                                &json!({"action":"open","url":"https://news.example.org"}),
                                None,
                            )
                            .unwrap();
                            assert!(matches!(outcome, PermissionOutcome::Allowed(_)));
                        },
                    );
                },
            );
        },
    );

    assert!(prompts.lock().unwrap().is_empty());
    assert_eq!(review_requests.lock().unwrap().len(), 1);
}

#[test]
fn browser_open_session_grant_reuses_registrable_domain_across_hosts() {
    let resources = browser_resources();
    let registry = ToolRegistry::from_resources(&resources);
    let providers = empty_providers();
    let mut auth_store = AuthStore::default();
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let prompts = Arc::new(Mutex::new(Vec::<PermissionPromptRequest>::new()));

    capture_permission_prompts(
        Arc::clone(&prompts),
        PermissionPromptAction::AllowSession,
        || {
            let first = resolve_tool_permission(
                &mut state,
                &resources,
                &providers,
                &mut auth_store,
                &registry,
                &cwd,
                "Browser",
                &json!({"action":"open","url":"https://google.com"}),
                None,
            )
            .unwrap();
            assert!(matches!(first, PermissionOutcome::Allowed(_)));

            let second = resolve_tool_permission(
                &mut state,
                &resources,
                &providers,
                &mut auth_store,
                &registry,
                &cwd,
                "Browser",
                &json!({"action":"open","url":"https://www.google.com"}),
                None,
            )
            .unwrap();
            assert!(matches!(second, PermissionOutcome::Allowed(_)));
        },
    );

    assert_eq!(prompts.lock().unwrap().len(), 1);
}

#[test]
fn browser_evaluator_deny_does_not_enter_reviewer() {
    let resources = browser_resources();
    let registry = ToolRegistry::from_resources(&resources);
    let providers = empty_providers();
    let mut auth_store = AuthStore::default();
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let permissions_dir = ConfigPaths::discover(&cwd).workspace_config_dir;
    fs::create_dir_all(&permissions_dir).unwrap();
    fs::write(
        permissions_dir.join("permissions.toml"),
        "[browser]\ndeny_domains = [\"example.com\"]\n",
    )
    .unwrap();
    let review_count = Arc::new(Mutex::new(0_usize));
    let review_log = Arc::clone(&review_count);
    let prompts = Arc::new(Mutex::new(Vec::<PermissionPromptRequest>::new()));

    with_browser_auto_review_test_handler(
        move |_request| {
            *review_log.lock().unwrap() += 1;
            BrowserAutoReviewRuntimeResult::AllowSession
        },
        || {
            capture_permission_prompts(
                Arc::clone(&prompts),
                PermissionPromptAction::AllowOnce,
                || {
                    let outcome = resolve_tool_permission(
                        &mut state,
                        &resources,
                        &providers,
                        &mut auth_store,
                        &registry,
                        &cwd,
                        "Browser",
                        &json!({"action":"navigate","url":"https://docs.example.com/a"}),
                        None,
                    )
                    .unwrap();
                    let PermissionOutcome::Denied(result) = outcome else {
                        panic!("expected denied result");
                    };
                    assert!(result.output.stdout.contains("denies domain"));
                },
            );
        },
    );

    assert_eq!(*review_count.lock().unwrap(), 0);
    assert!(prompts.lock().unwrap().is_empty());
}

#[test]
fn resolve_and_execute_browser_permission_stay_in_sync() {
    let resources = browser_resources();
    let registry = ToolRegistry::from_resources(&resources);
    let providers = openai_browser_providers();
    let request_config = test_openai_request_config();
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let session_id = state.session.id.to_string();
    let mut execute_state = state.clone();
    let input = json!({
        "action": "navigate",
        "url": "https://docs.example.com/a",
        "sessionId": session_id
    });
    let resolve_reviews = Arc::new(Mutex::new(0_usize));
    let execute_reviews = Arc::new(Mutex::new(0_usize));
    let resolve_prompts = Arc::new(Mutex::new(Vec::<PermissionPromptRequest>::new()));
    let execute_prompts = Arc::new(Mutex::new(Vec::<PermissionPromptRequest>::new()));
    let daemon_dir = ConfigPaths::discover(&cwd).workspace_config_dir;
    fs::create_dir_all(&daemon_dir).unwrap();
    fs::write(
        daemon_dir.join("daemon.handshake"),
        serde_json::to_string(&json!({
            "url": "ws://127.0.0.1:1/ws",
            "token": "test-token",
            "workspaceRoot": cwd.display().to_string(),
        }))
        .unwrap(),
    )
    .unwrap();

    let resolve_outcome = with_browser_auto_review_test_handler(
        {
            let review_count = Arc::clone(&resolve_reviews);
            move |_request| {
                *review_count.lock().unwrap() += 1;
                BrowserAutoReviewRuntimeResult::NeedsUser
            }
        },
        || {
            capture_permission_prompts(
                Arc::clone(&resolve_prompts),
                PermissionPromptAction::AllowSession,
                || {
                    resolve_tool_permission(
                        &mut state,
                        &resources,
                        &providers,
                        &mut AuthStore::default(),
                        &registry,
                        &cwd,
                        "Browser",
                        &input,
                        None,
                    )
                    .unwrap()
                },
            )
        },
    );
    assert!(matches!(resolve_outcome, PermissionOutcome::Allowed(_)));

    let execution = crate::runtime::local_tools::with_browser_daemon_test_handler(
        move |method, params| {
            assert_eq!(method, "browser_agent");
            assert_eq!(params["action"], json!("navigate"));
            Ok(json!({
                "status": "ok",
                "sessionId": params["sessionId"].clone(),
                "url": params["url"].clone()
            }))
        },
        || {
            with_browser_auto_review_test_handler(
                {
                    let review_count = Arc::clone(&execute_reviews);
                    move |_request| {
                        *review_count.lock().unwrap() += 1;
                        BrowserAutoReviewRuntimeResult::NeedsUser
                    }
                },
                || {
                    capture_permission_prompts(
                        Arc::clone(&execute_prompts),
                        PermissionPromptAction::AllowSession,
                        || {
                            execute_tool_call(
                                &mut execute_state,
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
                                "Browser",
                                input.clone(),
                            )
                            .unwrap()
                        },
                    )
                },
            )
        },
    );

    assert!(execution.success);
    assert_eq!(*resolve_reviews.lock().unwrap(), 1);
    assert_eq!(*execute_reviews.lock().unwrap(), 0);
    let resolve_prompts = resolve_prompts.lock().unwrap();
    let execute_prompts = execute_prompts.lock().unwrap();
    assert_eq!(resolve_prompts.len(), 1);
    assert!(execute_prompts.is_empty());
}

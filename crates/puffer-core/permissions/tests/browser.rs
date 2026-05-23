use super::*;
use crate::permissions::browser_policy::BrowserPolicySettings;
use crate::permissions::browser_target::browser_permission_context_for_tool;

#[test]
fn browser_and_bridge_session_grants_feed_profile_categories() {
    let mut grants = SessionPermissionGrants::default();
    let session_id = uuid::Uuid::parse_str("2ba8b01d-5e7a-46b6-b747-7bfe5f6fa36a").unwrap();
    grants.grant_tool_call(
        &tool_definition("Browser", "on-request"),
        &serde_json::json!({
            "action": "evaluate",
            "url": "https://docs.example.com/page",
            "sessionId": "b4f239fd-1493-4be7-a3a1-9e58fe612576",
            "script": "1+1"
        }),
        &session_id,
    );
    grants.grant_tool_call(
        &tool_definition("SendMessage", "auto"),
        &serde_json::json!({"to":"bridge:session-123","message":"hi"}),
        &session_id,
    );
    let profile = EffectivePermissionProfile::from_session_state(
        PathBuf::from("/repo").as_path(),
        &[],
        &PermissionsSettings::default(),
        &SandboxSettings::from_mode("workspace-write"),
        &session_id,
        &SessionPermissionState::new(false, grants),
        false,
        None,
        None,
    );

    assert!(profile
        .grants
        .surface_grants
        .contains(&PermissionSurface::Browser));
    assert!(profile
        .grants
        .surface_grants
        .contains(&PermissionSurface::Workflow));
    assert!(!profile.grants.tool_overrides.contains_key("browser"));
    assert_eq!(
        profile
            .grants
            .tool_overrides
            .get("send_message")
            .copied()
            .or_else(|| profile.grants.tool_overrides.get("sendmessage").copied()),
        Some(EffectiveApprovalPolicy::Allow)
    );
    assert!(profile.grants.category_grants.iter().any(|grant| matches!(
        grant,
        PermissionGrantCategory::Browser(BrowserGrantCategory::AllowActionOnOriginSession {
            action: BrowserActionCategory::Evaluate,
            target_origin,
            ..
        }) if target_origin == "https://docs.example.com"
    )));
    assert!(profile.grants.category_grants.iter().any(|grant| matches!(
        grant,
        PermissionGrantCategory::Workflow(
            crate::permissions::profile::WorkflowGrantCategory::CrossSessionBridge
        )
    )));
}

#[test]
fn browser_context_defaults_to_current_session_and_inspect() {
    let context = runtime_context(
        PermissionsSettings::default(),
        SandboxSettings::from_mode("workspace-write"),
        false,
        None,
        PathBuf::from("/tmp"),
        Vec::new(),
        SessionPermissionState::default(),
    );

    let browser_context = context
        .effective_profile()
        .browser_context(&serde_json::json!({"action":"snapshot"}));

    assert_eq!(browser_context.action, Some(BrowserActionCategory::Inspect));
    assert_eq!(
        browser_context.root_session_id,
        "2ba8b01d-5e7a-46b6-b747-7bfe5f6fa36a".to_string()
    );
    assert!(browser_context.target.is_none());
}

#[test]
fn generic_browser_permission_entries_do_not_affect_browser_decisions() {
    let context = runtime_context(
        PermissionsSettings {
            tools: BTreeMap::from([("browser".to_string(), "allow".to_string())]),
        },
        SandboxSettings::from_mode("workspace-write"),
        false,
        None,
        PathBuf::from("/tmp"),
        Vec::new(),
        SessionPermissionState::default(),
    );

    let decision = context.decision_for_tool_call(
        &tool_definition("Browser", "auto"),
        &serde_json::json!({
            "action":"evaluate",
            "url":"https://docs.example.com/page",
            "script":"document.title"
        }),
    );

    assert_eq!(decision.behavior, ToolPermissionBehavior::Ask);
    assert!(decision
        .reason
        .unwrap_or_default()
        .contains("executes page JavaScript"));
}

#[test]
fn browser_target_classifies_local_dev_url() {
    let context = runtime_context(
        PermissionsSettings::default(),
        SandboxSettings::from_mode("workspace-write"),
        false,
        None,
        PathBuf::from("/tmp"),
        Vec::new(),
        SessionPermissionState::default(),
    );

    let browser_context = context
        .effective_profile()
        .browser_context(&serde_json::json!({
            "action":"open",
            "url":"localhost:3000"
        }));

    assert_eq!(
        browser_context.target.as_ref().unwrap().target_class,
        BrowserTargetClass::LocalDev
    );
    assert_eq!(browser_context.target.as_ref().unwrap().scheme, "http");
    assert_eq!(
        browser_context.target.as_ref().unwrap().host.as_deref(),
        Some("localhost")
    );
    assert_eq!(
        browser_context.target.as_ref().unwrap().origin.as_deref(),
        Some("http://localhost:3000")
    );
    assert_eq!(browser_context.target.as_ref().unwrap().port, Some(3000));
}

#[test]
fn browser_target_classifies_workspace_file_url() {
    let (_temp, cwd, nested, outside) = temp_workspace_with_context();
    let workspace_file = nested.join("index.html");
    let outside_file = outside.join("other.html");
    std::fs::write(&workspace_file, "<h1>workspace</h1>").unwrap();
    std::fs::write(&outside_file, "<h1>outside</h1>").unwrap();
    let context = runtime_context(
        PermissionsSettings::default(),
        SandboxSettings::from_mode("workspace-write"),
        false,
        None,
        cwd.clone(),
        vec![nested.clone()],
        SessionPermissionState::default(),
    );

    let browser_context = context
        .effective_profile()
        .browser_context(&serde_json::json!({
            "action":"open",
            "url":to_file_url(&workspace_file)
        }));

    assert_eq!(
        browser_context.target.as_ref().unwrap().target_class,
        BrowserTargetClass::WorkspaceFile
    );
    assert_eq!(browser_context.target.as_ref().unwrap().scheme, "file");
    assert_eq!(browser_context.target.as_ref().unwrap().origin, None);
}

#[test]
fn browser_target_classifies_non_workspace_file_url() {
    let (_temp, cwd, nested, outside) = temp_workspace_with_context();
    let outside_file = outside.join("other.html");
    std::fs::write(nested.join("index.html"), "<h1>workspace</h1>").unwrap();
    std::fs::write(&outside_file, "<h1>outside</h1>").unwrap();
    let context = runtime_context(
        PermissionsSettings::default(),
        SandboxSettings::from_mode("workspace-write"),
        false,
        None,
        cwd,
        vec![nested],
        SessionPermissionState::default(),
    );

    let browser_context = context
        .effective_profile()
        .browser_context(&serde_json::json!({
            "action":"open",
            "url":to_file_url(&outside_file)
        }));

    assert_eq!(
        browser_context.target.as_ref().unwrap().target_class,
        BrowserTargetClass::NonWorkspaceFile
    );
    assert_eq!(browser_context.target.as_ref().unwrap().scheme, "file");
}

#[test]
fn browser_target_classifies_data_url() {
    let context = runtime_context(
        PermissionsSettings::default(),
        SandboxSettings::from_mode("workspace-write"),
        false,
        None,
        PathBuf::from("/tmp"),
        Vec::new(),
        SessionPermissionState::default(),
    );

    let browser_context = context
        .effective_profile()
        .browser_context(&serde_json::json!({
            "action":"open",
            "url":"data:text/html,<h1>demo</h1>"
        }));

    assert_eq!(
        browser_context.target.as_ref().unwrap().target_class,
        BrowserTargetClass::DataUrl
    );
    assert_eq!(browser_context.target.as_ref().unwrap().scheme, "data");
    assert_eq!(browser_context.target.as_ref().unwrap().origin, None);
}

#[test]
fn browser_target_classifies_open_web_url() {
    let context = runtime_context(
        PermissionsSettings::default(),
        SandboxSettings::from_mode("workspace-write"),
        false,
        None,
        PathBuf::from("/tmp"),
        Vec::new(),
        SessionPermissionState::default(),
    );

    let browser_context = context
        .effective_profile()
        .browser_context(&serde_json::json!({
            "action":"open",
            "url":"https://docs.example.co.uk/path"
        }));

    assert_eq!(
        browser_context.target.as_ref().unwrap().target_class,
        BrowserTargetClass::OpenWeb
    );
    assert_eq!(browser_context.target.as_ref().unwrap().scheme, "https");
    assert_eq!(
        browser_context.target.as_ref().unwrap().host.as_deref(),
        Some("docs.example.co.uk")
    );
    assert_eq!(
        browser_context
            .target
            .as_ref()
            .unwrap()
            .registrable_domain
            .as_deref(),
        Some("example.co.uk")
    );
    assert_eq!(
        browser_context.target.as_ref().unwrap().origin.as_deref(),
        Some("https://docs.example.co.uk")
    );
}

#[test]
fn browser_target_uses_psl_for_private_suffixes() {
    let context = runtime_context(
        PermissionsSettings::default(),
        SandboxSettings::from_mode("workspace-write"),
        false,
        None,
        PathBuf::from("/tmp"),
        Vec::new(),
        SessionPermissionState::default(),
    );

    let browser_context = context
        .effective_profile()
        .browser_context(&serde_json::json!({
            "action":"open",
            "url":"https://foo.github.io/demo"
        }));

    assert_eq!(
        browser_context
            .target
            .as_ref()
            .unwrap()
            .registrable_domain
            .as_deref(),
        Some("foo.github.io")
    );
}

#[test]
fn browser_target_uses_psl_for_multi_label_suffixes_beyond_hardcoded_list() {
    let context = runtime_context(
        PermissionsSettings::default(),
        SandboxSettings::from_mode("workspace-write"),
        false,
        None,
        PathBuf::from("/tmp"),
        Vec::new(),
        SessionPermissionState::default(),
    );

    let browser_context = context
        .effective_profile()
        .browser_context(&serde_json::json!({
            "action":"open",
            "url":"https://a.b.example.uk.com/path"
        }));

    assert_eq!(
        browser_context
            .target
            .as_ref()
            .unwrap()
            .registrable_domain
            .as_deref(),
        Some("example.uk.com")
    );
}

#[test]
fn browser_inspect_without_target_context_asks_but_targeted_calls_still_gate_by_action() {
    let context = runtime_context(
        PermissionsSettings::default(),
        SandboxSettings::from_mode("workspace-write"),
        false,
        None,
        PathBuf::from("/tmp"),
        Vec::new(),
        SessionPermissionState::default(),
    );

    let inspect = context.decision_for_tool_call(
        &tool_definition("Browser", "auto"),
        &serde_json::json!({
            "action":"snapshot"
        }),
    );
    let navigate = context.decision_for_tool_call(
        &tool_definition("Browser", "auto"),
        &serde_json::json!({
            "action":"navigate",
            "url":"https://example.com"
        }),
    );
    let evaluate = context.decision_for_tool_call(
        &tool_definition("Browser", "auto"),
        &serde_json::json!({
            "action":"evaluate",
            "script":"document.title"
        }),
    );

    assert_eq!(inspect.behavior, ToolPermissionBehavior::Ask);
    assert_eq!(navigate.behavior, ToolPermissionBehavior::Ask);
    assert_eq!(evaluate.behavior, ToolPermissionBehavior::Ask);
    let evaluate_reason = evaluate.reason.unwrap_or_default();
    assert!(evaluate_reason.contains("no target URL") || evaluate_reason.contains("page context"));
}

#[test]
fn browser_targeted_evaluate_uses_javascript_execution_reason() {
    let context = runtime_context(
        PermissionsSettings::default(),
        SandboxSettings::from_mode("workspace-write"),
        false,
        None,
        PathBuf::from("/tmp"),
        Vec::new(),
        SessionPermissionState::default(),
    );

    let evaluate = context.decision_for_tool_call(
        &tool_definition("Browser", "auto"),
        &serde_json::json!({
            "action":"evaluate",
            "url":"https://docs.example.com/page",
            "script":"document.title"
        }),
    );

    assert_eq!(evaluate.behavior, ToolPermissionBehavior::Ask);
    assert!(evaluate
        .reason
        .unwrap_or_default()
        .contains("executes page JavaScript"));
}

#[test]
fn browser_evaluator_allows_explicit_allow_without_reviewer_path() {
    let profile = runtime_context(
        PermissionsSettings::default(),
        SandboxSettings::from_mode("workspace-write"),
        false,
        None,
        PathBuf::from("/tmp"),
        Vec::new(),
        SessionPermissionState::default(),
    )
    .effective_profile()
    .clone();
    let policy = BrowserPolicySettings {
        allow_domains: vec!["example.com".to_string()],
        ..BrowserPolicySettings::default()
    };
    let context = profile.browser_context(&serde_json::json!({
        "action":"navigate",
        "url":"https://docs.example.com/page"
    }));

    let decision = crate::permissions::browser_evaluator::evaluate_browser_permission(
        &profile, &policy, &context,
    );

    assert_eq!(
        decision.decision,
        crate::permissions::browser_evaluator::BrowserPermissionDecision::Allow
    );
    assert_eq!(decision.reason, None);
}

#[test]
fn browser_policy_deny_short_circuits_before_reviewer() {
    let profile = runtime_context(
        PermissionsSettings::default(),
        SandboxSettings::from_mode("workspace-write"),
        false,
        None,
        PathBuf::from("/tmp"),
        Vec::new(),
        SessionPermissionState::default(),
    )
    .effective_profile()
    .clone();
    let policy = BrowserPolicySettings {
        deny_domains: vec!["example.com".to_string()],
        ..BrowserPolicySettings::default()
    };
    let context = profile.browser_context(&serde_json::json!({
        "action":"navigate",
        "url":"https://docs.example.com/page"
    }));
    let decision = crate::permissions::browser_evaluator::evaluate_browser_permission(
        &profile, &policy, &context,
    );

    assert_eq!(
        decision.decision,
        crate::permissions::browser_evaluator::BrowserPermissionDecision::Deny
    );
    assert!(decision
        .reason
        .unwrap_or_default()
        .contains("denies domain"));
}

#[test]
fn browser_evaluator_reuses_origin_session_grant_before_policy_allow() {
    let session_id = Uuid::parse_str("2ba8b01d-5e7a-46b6-b747-7bfe5f6fa36a").unwrap();
    let mut grants = SessionPermissionGrants::default();
    grants.grant_browser_tool_call(
        &tool_definition("Browser", "auto"),
        &serde_json::json!({
            "action":"navigate",
            "url":"https://docs.example.com/a"
        }),
        &session_id,
        crate::permissions::browser_grants::BrowserGrantScopeKind::AllowOriginSession,
    );
    let profile = EffectivePermissionProfile::from_session_state(
        PathBuf::from("/repo").as_path(),
        &[],
        &PermissionsSettings::default(),
        &SandboxSettings::from_mode("workspace-write"),
        &session_id,
        &SessionPermissionState::new(false, grants),
        false,
        None,
        None,
    );
    let policy = BrowserPolicySettings::default();
    let context = profile.browser_context(&serde_json::json!({
        "action":"click",
        "url":"https://docs.example.com/b"
    }));

    let decision = crate::permissions::browser_evaluator::evaluate_browser_permission(
        &profile, &policy, &context,
    );

    assert_eq!(
        decision.decision,
        crate::permissions::browser_evaluator::BrowserPermissionDecision::Allow
    );
    assert_eq!(decision.reason, None);
}

#[test]
fn browser_evaluator_asks_when_neither_policy_nor_grant_allows() {
    let profile = runtime_context(
        PermissionsSettings::default(),
        SandboxSettings::from_mode("workspace-write"),
        false,
        None,
        PathBuf::from("/tmp"),
        Vec::new(),
        SessionPermissionState::default(),
    )
    .effective_profile()
    .clone();
    let policy = BrowserPolicySettings::default();
    let context = profile.browser_context(&serde_json::json!({
        "action":"navigate",
        "url":"https://docs.example.com/page"
    }));

    let decision = crate::permissions::browser_evaluator::evaluate_browser_permission(
        &profile, &policy, &context,
    );

    assert_eq!(
        decision.decision,
        crate::permissions::browser_evaluator::BrowserPermissionDecision::Ask
    );
    assert_eq!(
        decision.reason.as_deref(),
        Some("browser navigation and interaction require approval")
    );
}

#[test]
fn browser_policy_explicit_allow_can_allow_target_class() {
    let profile = runtime_context(
        PermissionsSettings::default(),
        SandboxSettings::from_mode("workspace-write"),
        false,
        None,
        PathBuf::from("/tmp"),
        Vec::new(),
        SessionPermissionState::default(),
    )
    .effective_profile()
    .clone();
    let policy = BrowserPolicySettings {
        allow_target_classes: vec!["local_dev".to_string()],
        ..BrowserPolicySettings::default()
    };
    let context = profile.browser_context(&serde_json::json!({
        "action":"navigate",
        "url":"http://localhost:3000"
    }));
    let decision = crate::permissions::browser_evaluator::evaluate_browser_permission(
        &profile, &policy, &context,
    );

    assert_eq!(
        decision.decision,
        crate::permissions::browser_evaluator::BrowserPermissionDecision::Allow
    );
}

#[test]
fn browser_permission_context_keeps_explicit_foreign_root_without_relabeling_reason() {
    let context = runtime_context(
        PermissionsSettings::default(),
        SandboxSettings::from_mode("workspace-write"),
        false,
        None,
        PathBuf::from("/tmp"),
        Vec::new(),
        SessionPermissionState::default(),
    );

    let decision = context.decision_for_tool_call(
        &tool_definition("Browser", "auto"),
        &serde_json::json!({
            "action":"snapshot",
            "sessionId":"b4f239fd-1493-4be7-a3a1-9e58fe612576",
            "url":"https://docs.example.com/page"
        }),
    );

    assert_eq!(decision.behavior, ToolPermissionBehavior::Ask);
    assert_eq!(
        decision.reason.as_deref(),
        Some("browser inspection requires approval")
    );
}

#[test]
fn browser_session_grant_is_scoped_to_action_category_and_root_session() {
    let session_id = Uuid::parse_str("2ba8b01d-5e7a-46b6-b747-7bfe5f6fa36a").unwrap();
    let mut grants = SessionPermissionGrants::default();
    grants.grant_tool_call(
        &tool_definition("Browser", "auto"),
        &serde_json::json!({
            "action":"evaluate",
            "script":"document.title"
        }),
        &session_id,
    );
    let profile = EffectivePermissionProfile::from_session_state(
        PathBuf::from("/repo").as_path(),
        &[],
        &PermissionsSettings::default(),
        &SandboxSettings::from_mode("workspace-write"),
        &session_id,
        &SessionPermissionState::new(false, grants),
        false,
        None,
        None,
    );

    assert!(!profile.browser_session_grant_allows(&serde_json::json!({
        "action":"evaluate",
        "script":"window.location.href"
    })));
    assert!(!profile.browser_session_grant_allows(&serde_json::json!({
        "action":"snapshot"
    })));
    assert!(!profile.browser_session_grant_allows(&serde_json::json!({
        "action":"evaluate",
        "sessionId":"b4f239fd-1493-4be7-a3a1-9e58fe612576",
        "script":"window.location.href"
    })));
}

#[test]
fn browser_origin_session_grant_does_not_cross_origins() {
    let session_id = Uuid::parse_str("2ba8b01d-5e7a-46b6-b747-7bfe5f6fa36a").unwrap();
    let mut grants = SessionPermissionGrants::default();
    grants.grant_browser_tool_call(
        &tool_definition("Browser", "auto"),
        &serde_json::json!({
            "action":"navigate",
            "url":"https://docs.example.com/a"
        }),
        &session_id,
        crate::permissions::browser_grants::BrowserGrantScopeKind::AllowOriginSession,
    );
    let profile = EffectivePermissionProfile::from_session_state(
        PathBuf::from("/repo").as_path(),
        &[],
        &PermissionsSettings::default(),
        &SandboxSettings::from_mode("workspace-write"),
        &session_id,
        &SessionPermissionState::new(false, grants),
        false,
        None,
        None,
    );

    assert!(profile.browser_session_grant_allows(&serde_json::json!({
        "action":"click",
        "url":"https://docs.example.com/b"
    })));
    assert!(!profile.browser_session_grant_allows(&serde_json::json!({
        "action":"click",
        "url":"https://api.example.com/b"
    })));
    assert!(!profile.browser_session_grant_allows(&serde_json::json!({
        "action":"click",
        "url":"https://docs.other.com/b"
    })));
}

#[test]
fn browser_domain_session_grant_does_not_cross_domains() {
    let session_id = Uuid::parse_str("2ba8b01d-5e7a-46b6-b747-7bfe5f6fa36a").unwrap();
    let mut grants = SessionPermissionGrants::default();
    grants.grant_browser_tool_call(
        &tool_definition("Browser", "auto"),
        &serde_json::json!({
            "action":"navigate",
            "url":"https://docs.example.co.uk/a"
        }),
        &session_id,
        crate::permissions::browser_grants::BrowserGrantScopeKind::AllowDomainSession,
    );
    let profile = EffectivePermissionProfile::from_session_state(
        PathBuf::from("/repo").as_path(),
        &[],
        &PermissionsSettings::default(),
        &SandboxSettings::from_mode("workspace-write"),
        &session_id,
        &SessionPermissionState::new(false, grants),
        false,
        None,
        None,
    );

    assert!(profile.browser_session_grant_allows(&serde_json::json!({
        "action":"hover",
        "url":"https://api.example.co.uk/b"
    })));
    assert!(!profile.browser_session_grant_allows(&serde_json::json!({
        "action":"hover",
        "url":"https://example.com/b"
    })));
}

#[test]
fn browser_tab_session_grant_does_not_cross_tabs() {
    let session_id = Uuid::parse_str("2ba8b01d-5e7a-46b6-b747-7bfe5f6fa36a").unwrap();
    let mut grants = SessionPermissionGrants::default();
    grants.grant_browser_tool_call(
        &tool_definition("Browser", "auto"),
        &serde_json::json!({
            "action":"evaluate",
            "tabId":"t7",
            "script":"document.title"
        }),
        &session_id,
        crate::permissions::browser_grants::BrowserGrantScopeKind::AllowTabSession,
    );
    let profile = EffectivePermissionProfile::from_session_state(
        PathBuf::from("/repo").as_path(),
        &[],
        &PermissionsSettings::default(),
        &SandboxSettings::from_mode("workspace-write"),
        &session_id,
        &SessionPermissionState::new(false, grants),
        false,
        None,
        None,
    );

    assert!(profile.browser_session_grant_allows(&serde_json::json!({
        "action":"snapshot",
        "tabId":"t7"
    })));
    assert!(!profile.browser_session_grant_allows(&serde_json::json!({
        "action":"snapshot",
        "tabId":"t8"
    })));
}

#[test]
fn suggested_scope_prefers_origin_for_normal_same_origin_interaction() {
    let context = browser_permission_context_for_tool(
        "Browser",
        &serde_json::json!({
            "action":"click",
            "tabId":"t7",
            "url":"https://docs.example.com/page"
        }),
        "2ba8b01d-5e7a-46b6-b747-7bfe5f6fa36a",
        &[],
    );

    assert_eq!(
        crate::permissions::browser_grants::suggested_browser_grant_scope(&context),
        crate::permissions::browser_grants::BrowserGrantScopeKind::AllowOriginSession
    );
}

#[test]
fn suggested_scope_keeps_tab_for_high_risk_interaction_targets() {
    let context = browser_permission_context_for_tool(
        "Browser",
        &serde_json::json!({
            "action":"click",
            "tabId":"t7",
            "url":"http://203.0.113.10:8080/page"
        }),
        "2ba8b01d-5e7a-46b6-b747-7bfe5f6fa36a",
        &[],
    );

    assert_eq!(
        crate::permissions::browser_grants::suggested_browser_grant_scope(&context),
        crate::permissions::browser_grants::BrowserGrantScopeKind::AllowTabSession
    );
}

#[test]
fn browser_surface_only_session_grant_does_not_allow_evaluate() {
    let mut grants = SessionPermissionGrants::default();
    grants.grant_surface_for_test(PermissionSurface::Browser);

    let context = runtime_context_with_session_grants(
        PermissionsSettings::default(),
        SandboxSettings::from_mode("workspace-write"),
        false,
        None,
        PathBuf::from("/tmp"),
        Vec::new(),
        false,
        grants,
        RuntimePermissionInputs::default(),
    );

    let decision = context.decision_for_tool_call(
        &tool_definition("Browser", "auto"),
        &serde_json::json!({
            "action":"evaluate",
            "script":"document.title"
        }),
    );

    assert_eq!(decision.behavior, ToolPermissionBehavior::Ask);
}

#[test]
fn allow_all_tools_allows_shell_browser_commands_as_bash() {
    let context = runtime_context(
        PermissionsSettings::default(),
        SandboxSettings::from_mode("workspace-write"),
        false,
        None,
        PathBuf::from("/tmp"),
        Vec::new(),
        SessionPermissionState::new(true, SessionPermissionGrants::default()),
    );

    let decision = context.decision_for_tool_call(
        &tool_definition("Bash", "on-request"),
        &serde_json::json!({
            "command":"puffer browser evaluate document.title"
        }),
    );

    assert_eq!(decision.behavior, ToolPermissionBehavior::Allow);
}

#[test]
fn bash_browser_command_does_not_map_into_browser_permission_context() {
    let session_id = "2ba8b01d-5e7a-46b6-b747-7bfe5f6fa36a";
    let browser = browser_permission_context_for_tool(
        "Browser",
        &serde_json::json!({
            "action":"screenshot",
            "sessionId":"root-7",
            "tabId":"t3"
        }),
        session_id,
        &[],
    );
    let shell = browser_permission_context_for_tool(
        "Bash",
        &serde_json::json!({
            "command":"puffer browser screenshot --session-id root-7 --tab-id t3"
        }),
        session_id,
        &[],
    );

    assert_eq!(browser.action, Some(BrowserActionCategory::Inspect));
    assert_eq!(shell.action, None);
    assert_eq!(shell.target, None);
    assert_eq!(shell.tab_id, None);
    assert_eq!(shell.root_session_id, session_id);
}

#[test]
fn bash_browser_tab_focus_is_not_browser_classified() {
    let context = browser_permission_context_for_tool(
        "Bash",
        &serde_json::json!({
            "command":"puffer browser tab focus t4 --session-id root-9"
        }),
        "2ba8b01d-5e7a-46b6-b747-7bfe5f6fa36a",
        &[],
    );

    assert_eq!(context.action, None);
    assert_eq!(context.tab_id, None);
    assert_eq!(
        context.root_session_id,
        "2ba8b01d-5e7a-46b6-b747-7bfe5f6fa36a"
    );
}

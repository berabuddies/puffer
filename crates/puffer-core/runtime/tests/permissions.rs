use super::*;
use crate::permissions::{
    load_runtime_permission_context, load_runtime_permission_context_with_inputs,
    RuntimePermissionInputs, ToolPermissionBehavior,
};
use crate::plans::plan_file_path;
use serde_json::json;

#[test]
fn workspace_deny_rules_filter_tools_from_model_visibility() {
    let temp = tempfile::tempdir().unwrap();
    let paths = ConfigPaths::discover(temp.path());
    ensure_workspace_dirs(&paths).unwrap();
    std::fs::write(
        paths.workspace_config_dir.join("permissions.toml"),
        "[tools]\nbash = \"deny\"\n",
    )
    .unwrap();

    let mut state = state();
    state.cwd = temp.path().to_path_buf();
    let resources = LoadedResources {
        tools: vec![
            loaded_tool("Bash", "Run shell", "runtime:claude_bash"),
            loaded_tool("Read", "Read file", "runtime:claude_read"),
        ],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let permission_context =
        load_runtime_permission_context(&state.cwd, &resources, &state).unwrap();

    let openai_tools =
        super::super::structured_output_support::openai_tool_definitions_for_request(
            &registry,
            None,
            false,
            Some(&permission_context),
        )
        .unwrap();

    assert_eq!(openai_tools.len(), 1);
    assert_eq!(openai_tools[0].name, "Read");
}

#[test]
fn browser_policy_is_loaded_from_permissions_file_browser_section() {
    let temp = tempfile::tempdir().unwrap();
    let paths = ConfigPaths::discover(temp.path());
    ensure_workspace_dirs(&paths).unwrap();
    std::fs::write(
        paths.workspace_config_dir.join("permissions.toml"),
        "[tools]\nread = \"allow\"\n\n[browser]\ndeny_domains = [\"example.com\"]\n",
    )
    .unwrap();

    let mut state = state();
    state.cwd = temp.path().to_path_buf();
    let resources = LoadedResources {
        internal_tools: vec![loaded_tool("Browser", "Browser", "runtime:browser")],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let definition = registry.internal_definition("Browser").unwrap();
    let permission_context =
        load_runtime_permission_context(&state.cwd, &resources, &state).unwrap();

    let decision = permission_context.decision_for_tool_call(
        definition,
        &json!({
            "action":"navigate",
            "url":"https://docs.example.com/page"
        }),
    );

    assert_eq!(decision.behavior, ToolPermissionBehavior::Deny);
    assert!(decision
        .reason
        .unwrap_or_default()
        .contains("denies domain"));
}

#[test]
fn request_tool_filter_limits_openai_tool_visibility_with_aliases() {
    let resources = LoadedResources {
        tools: vec![
            loaded_tool("Agent", "Delegate", "runtime:agent"),
            loaded_tool("Glob", "List files", "runtime:claude_glob"),
            loaded_tool("Read", "Read file", "runtime:claude_read"),
        ],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let filter = super::super::build_request_tool_filter(&["Task".to_string(), "LS".to_string()])
        .unwrap()
        .unwrap();

    let permission_context = load_runtime_permission_context_with_inputs(
        std::path::Path::new("/tmp/work"),
        &resources,
        &state(),
        RuntimePermissionInputs {
            request_tool_filter: Some(filter),
        },
    )
    .unwrap();
    let tools = super::super::structured_output_support::openai_tool_definitions_for_request(
        &registry,
        None,
        false,
        Some(&permission_context),
    )
    .unwrap();

    let tool_names = tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(tool_names, vec!["Agent", "Glob"]);
}

#[test]
fn request_tool_filters_apply_consistently_to_openai_and_anthropic_tool_lists() {
    let resources = LoadedResources {
        tools: vec![
            loaded_tool("Agent", "Delegate work", "runtime:agent"),
            loaded_tool("Glob", "List files", "runtime:claude_glob"),
            loaded_tool("Read", "Read file", "runtime:claude_read"),
        ],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let filter = build_request_tool_filter(&["Task".to_string(), "LS".to_string()])
        .unwrap()
        .unwrap();

    let state = state();
    let permission_context = load_runtime_permission_context_with_inputs(
        std::path::Path::new("/tmp/work"),
        &resources,
        &state,
        RuntimePermissionInputs {
            request_tool_filter: Some(filter),
        },
    )
    .unwrap();
    let openai_tools =
        super::super::structured_output_support::openai_tool_definitions_for_request(
            &registry,
            None,
            false,
            Some(&permission_context),
        )
        .unwrap();
    let anthropic_tools =
        super::super::structured_output_support::anthropic_tool_definitions_for_request(
            &registry,
            None,
            Some(&permission_context),
        )
        .unwrap();

    let openai_names = openai_tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    let anthropic_names = anthropic_tools
        .iter()
        .filter_map(|tool| tool.get("name").and_then(serde_json::Value::as_str))
        .collect::<Vec<_>>();

    assert_eq!(openai_names, vec!["Agent", "Glob"]);
    assert_eq!(anthropic_names, vec!["Agent", "Glob"]);
}

#[test]
fn plan_mode_requires_approval_for_mutating_on_request_tools() {
    let temp = tempfile::tempdir().unwrap();
    let paths = ConfigPaths::discover(temp.path());
    ensure_workspace_dirs(&paths).unwrap();

    let mut state = state();
    state.cwd = temp.path().to_path_buf();
    state.plan_mode = true;
    let mut write_tool = loaded_tool("Write", "Write file", "runtime:claude_write");
    write_tool.value.approval_policy = Some("on-request".to_string());
    write_tool.value.sandbox_policy = Some("workspace-write".to_string());
    let resources = LoadedResources {
        tools: vec![write_tool],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let definition = registry.definition("Write").unwrap();
    let permission_context =
        load_runtime_permission_context(&state.cwd, &resources, &state).unwrap();

    let error = permission_context
        .enforce_tool_call(
            definition,
            &json!({"file_path": "note.txt", "content": "hello"}),
        )
        .unwrap_err();

    assert!(error
        .to_string()
        .contains("plan mode requires approval for mutating tools"));
}

#[test]
fn plan_mode_allows_writing_the_active_plan_file() {
    let temp = tempfile::tempdir().unwrap();
    let paths = ConfigPaths::discover(temp.path());
    ensure_workspace_dirs(&paths).unwrap();

    let mut state = state();
    state.cwd = temp.path().to_path_buf();
    state.plan_mode = true;
    let mut write_tool = loaded_tool("Write", "Write file", "runtime:claude_write");
    write_tool.value.approval_policy = Some("on-request".to_string());
    write_tool.value.sandbox_policy = Some("workspace-write".to_string());
    let resources = LoadedResources {
        tools: vec![write_tool],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let definition = registry.definition("Write").unwrap();
    let permission_context =
        load_runtime_permission_context(&state.cwd, &resources, &state).unwrap();

    permission_context
        .enforce_tool_call(
            definition,
            &json!({"file_path": plan_file_path(&state).unwrap(), "content": "# Plan\n"}),
        )
        .unwrap();
}

#[test]
fn plan_mode_allows_ask_user_question() {
    let temp = tempfile::tempdir().unwrap();
    let paths = ConfigPaths::discover(temp.path());
    ensure_workspace_dirs(&paths).unwrap();

    let mut state = state();
    state.cwd = temp.path().to_path_buf();
    state.plan_mode = true;
    let resources = LoadedResources {
        tools: vec![loaded_tool(
            "AskUserQuestion",
            "Ask a question",
            "runtime:workflow:ask_user_question",
        )],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let definition = registry.definition("AskUserQuestion").unwrap();
    let permission_context =
        load_runtime_permission_context(&state.cwd, &resources, &state).unwrap();

    let decision = permission_context.decision_for_tool_call(
        definition,
        &json!({"questions":[{"question":"Pick one","header":"Choice","options":[{"label":"A","description":"A"},{"label":"B","description":"B"}]}]}),
    );
    assert_eq!(decision.behavior, ToolPermissionBehavior::Allow);
}

#[test]
fn sandboxed_shell_commands_still_run_by_default() {
    let temp = tempfile::tempdir().unwrap();
    let paths = ConfigPaths::discover(temp.path());
    ensure_workspace_dirs(&paths).unwrap();

    let mut state = state();
    state.cwd = temp.path().to_path_buf();
    let mut bash_tool = loaded_tool("Bash", "Run shell", "runtime:claude_bash");
    bash_tool.value.approval_policy = Some("on-request".to_string());
    bash_tool.value.sandbox_policy = Some("workspace-write".to_string());
    let resources = LoadedResources {
        tools: vec![bash_tool],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let definition = registry.definition("Bash").unwrap();
    let permission_context =
        load_runtime_permission_context(&state.cwd, &resources, &state).unwrap();

    permission_context
        .enforce_tool_call(definition, &json!({"command": "pwd"}))
        .unwrap();
}

#[test]
fn destructive_shell_command_requires_approval_even_without_unsandboxed_override() {
    let temp = tempfile::tempdir().unwrap();
    let paths = ConfigPaths::discover(temp.path());
    ensure_workspace_dirs(&paths).unwrap();

    let mut state = state();
    state.cwd = temp.path().to_path_buf();
    let mut bash_tool = loaded_tool("Bash", "Run shell", "runtime:claude_bash");
    bash_tool.value.approval_policy = Some("on-request".to_string());
    bash_tool.value.sandbox_policy = Some("workspace-write".to_string());
    let resources = LoadedResources {
        tools: vec![bash_tool],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let definition = registry.definition("Bash").unwrap();
    let permission_context =
        load_runtime_permission_context(&state.cwd, &resources, &state).unwrap();

    let error = permission_context
        .enforce_tool_call(definition, &json!({"command": "rm -rf /"}))
        .unwrap_err();

    assert!(error.to_string().contains("dangerously destructive"));
}

#[test]
fn shell_browser_commands_use_shell_permission_backend() {
    let temp = tempfile::tempdir().unwrap();
    let paths = ConfigPaths::discover(temp.path());
    ensure_workspace_dirs(&paths).unwrap();

    let mut state = state();
    state.cwd = temp.path().to_path_buf();
    let mut bash_tool = loaded_tool("Bash", "Run shell", "runtime:claude_bash");
    bash_tool.value.approval_policy = Some("on-request".to_string());
    bash_tool.value.sandbox_policy = Some("workspace-write".to_string());
    let resources = LoadedResources {
        tools: vec![bash_tool],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let definition = registry.definition("Bash").unwrap();
    let permission_context =
        load_runtime_permission_context(&state.cwd, &resources, &state).unwrap();

    let decision = permission_context.decision_for_tool_call(
        definition,
        &json!({
            "command":"puffer browser evaluate document.title"
        }),
    );

    assert_eq!(decision.behavior, ToolPermissionBehavior::Ask);
    assert_eq!(
        decision.reason.as_deref(),
        Some("shell command `puffer` requires approval")
    );
}

#[test]
fn browser_grants_do_not_authorize_shell_browser_commands() {
    let temp = tempfile::tempdir().unwrap();
    let paths = ConfigPaths::discover(temp.path());
    ensure_workspace_dirs(&paths).unwrap();

    let mut state = state();
    state.cwd = temp.path().to_path_buf();
    let mut bash_tool = loaded_tool("Bash", "Run shell", "runtime:claude_bash");
    bash_tool.value.approval_policy = Some("on-request".to_string());
    bash_tool.value.sandbox_policy = Some("workspace-write".to_string());
    let browser_tool = loaded_tool("Browser", "Browser", "runtime:browser");
    let resources = LoadedResources {
        tools: vec![bash_tool],
        internal_tools: vec![browser_tool],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let bash_definition = registry.definition("Bash").unwrap();
    let browser_definition = registry.internal_definition("Browser").unwrap();
    state.allow_browser_permission_for_tool_call(
        browser_definition,
        &json!({
            "action":"snapshot",
            "tabId":"t7"
        }),
        crate::permissions::browser_grants::BrowserGrantScopeKind::AllowTabSession,
    );
    let permission_context =
        load_runtime_permission_context(&state.cwd, &resources, &state).unwrap();

    assert!(!permission_context
        .effective_profile()
        .grants
        .tool_overrides
        .contains_key("bash"));

    let decision = permission_context.decision_for_tool_call(
        bash_definition,
        &json!({
            "command":"puffer browser snapshot --tab-id t7"
        }),
    );

    assert_eq!(decision.behavior, ToolPermissionBehavior::Ask);
    assert_eq!(
        decision.reason.as_deref(),
        Some("shell command `puffer` requires approval")
    );
}

#[test]
fn allow_all_tools_still_allows_shell_browser_commands_as_bash() {
    let temp = tempfile::tempdir().unwrap();
    let paths = ConfigPaths::discover(temp.path());
    ensure_workspace_dirs(&paths).unwrap();

    let mut state = state();
    state.cwd = temp.path().to_path_buf();
    state.grant_all_tools_for_session();
    let mut bash_tool = loaded_tool("Bash", "Run shell", "runtime:claude_bash");
    bash_tool.value.approval_policy = Some("on-request".to_string());
    bash_tool.value.sandbox_policy = Some("workspace-write".to_string());
    let resources = LoadedResources {
        tools: vec![bash_tool],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let definition = registry.definition("Bash").unwrap();
    let permission_context =
        load_runtime_permission_context(&state.cwd, &resources, &state).unwrap();

    let decision = permission_context.decision_for_tool_call(
        definition,
        &json!({
            "command":"puffer browser evaluate document.title"
        }),
    );

    assert_eq!(decision.behavior, ToolPermissionBehavior::Allow);
}

#[test]
fn compound_shell_browser_commands_use_shell_control_operator_reason() {
    let temp = tempfile::tempdir().unwrap();
    let paths = ConfigPaths::discover(temp.path());
    ensure_workspace_dirs(&paths).unwrap();

    let mut state = state();
    state.cwd = temp.path().to_path_buf();
    let mut bash_tool = loaded_tool("Bash", "Run shell", "runtime:claude_bash");
    bash_tool.value.approval_policy = Some("on-request".to_string());
    bash_tool.value.sandbox_policy = Some("workspace-write".to_string());
    let resources = LoadedResources {
        tools: vec![bash_tool],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let definition = registry.definition("Bash").unwrap();
    let permission_context =
        load_runtime_permission_context(&state.cwd, &resources, &state).unwrap();

    let decision = permission_context.decision_for_tool_call(
        definition,
        &json!({
            "command":"puffer browser navigate https://example.com && puffer browser snapshot"
        }),
    );

    assert_eq!(decision.behavior, ToolPermissionBehavior::Ask);
    assert_eq!(
        decision.reason.as_deref(),
        Some("shell command contains shell control or redirection operators")
    );
}

#[test]
fn non_browser_compound_shell_commands_require_approval() {
    let temp = tempfile::tempdir().unwrap();
    let paths = ConfigPaths::discover(temp.path());
    ensure_workspace_dirs(&paths).unwrap();

    let mut state = state();
    state.cwd = temp.path().to_path_buf();
    let mut bash_tool = loaded_tool("Bash", "Run shell", "runtime:claude_bash");
    bash_tool.value.approval_policy = Some("on-request".to_string());
    bash_tool.value.sandbox_policy = Some("workspace-write".to_string());
    let resources = LoadedResources {
        tools: vec![bash_tool],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let definition = registry.definition("Bash").unwrap();
    let permission_context =
        load_runtime_permission_context(&state.cwd, &resources, &state).unwrap();

    let decision = permission_context.decision_for_tool_call(
        definition,
        &json!({
            "command":"echo hi && pwd"
        }),
    );

    assert_eq!(decision.behavior, ToolPermissionBehavior::Ask);
    assert!(decision
        .reason
        .unwrap_or_default()
        .contains("control or redirection"));
}

#[test]
fn session_allow_all_is_applied_inside_permission_profile() {
    let temp = tempfile::tempdir().unwrap();
    let paths = ConfigPaths::discover(temp.path());
    ensure_workspace_dirs(&paths).unwrap();
    std::fs::write(
        paths.workspace_config_dir.join("permissions.toml"),
        "[tools]\nbash = \"deny\"\n",
    )
    .unwrap();

    let mut state = state();
    state.cwd = temp.path().to_path_buf();
    state.grant_all_tools_for_session();
    let mut bash_tool = loaded_tool("Bash", "Run shell", "runtime:claude_bash");
    bash_tool.value.approval_policy = Some("on-request".to_string());
    let resources = LoadedResources {
        tools: vec![bash_tool],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let definition = registry.definition("Bash").unwrap();
    let permission_context =
        load_runtime_permission_context(&state.cwd, &resources, &state).unwrap();

    let decision = permission_context.decision_for_tool_call(definition, &json!({"command":"pwd"}));
    assert_eq!(decision.behavior, ToolPermissionBehavior::Allow);
}

#[test]
fn runtime_permission_context_derives_typed_executor_policy_from_effective_profile() {
    let temp = tempfile::tempdir().unwrap();
    let paths = ConfigPaths::discover(temp.path());
    ensure_workspace_dirs(&paths).unwrap();
    std::fs::write(
        paths.workspace_config_dir.join("permissions.toml"),
        "[tools]\nread = \"ask\"\nbash = \"deny\"\n",
    )
    .unwrap();
    std::fs::write(
        paths.workspace_config_dir.join("sandbox.toml"),
        "mode = \"danger-full-access\"\nauto_allow = false\nallow_unsandboxed_fallback = true\nexcluded_commands = [\"sudo\"]\n",
    )
    .unwrap();

    let mut state = state();
    state.cwd = temp.path().to_path_buf();
    state.working_dirs.push(temp.path().join("extra"));
    let resources = LoadedResources::default();
    let permission_context =
        load_runtime_permission_context(&state.cwd, &resources, &state).unwrap();
    let derived = permission_context.derived_policy();

    assert_eq!(
        derived.filesystem().approval,
        crate::permissions::profile::EffectiveApprovalPolicy::Ask
    );
    assert_eq!(
        derived.filesystem().sandbox_mode,
        crate::permissions::profile::EffectiveSandboxMode::WorkspaceWrite
    );
    assert_eq!(
        derived.process().approval,
        crate::permissions::profile::EffectiveApprovalPolicy::Deny
    );
    assert!(derived.process().allow_unsandboxed_fallback);
    assert_eq!(
        derived.process().excluded_commands,
        vec!["sudo".to_string()]
    );
}

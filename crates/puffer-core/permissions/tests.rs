use super::profile::{
    build_request_tool_filter, classify_tool_permission_surface, BrowserActionCategory,
    BrowserGrantCategory, EffectiveApprovalPolicy, EffectivePermissionProfile,
    EffectiveSandboxMode, PermissionGrantCategory, PermissionSurface, SessionPermissionGrants,
    SurfaceEnforcement,
};
use super::*;
use puffer_resources::{LoadedItem, SourceInfo, SourceKind, ToolSpec};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use uuid::Uuid;

fn tool_definition(id: &str, approval_policy: &str) -> ToolDefinition {
    ToolDefinition {
        id: id.to_string(),
        name: id.to_string(),
        description: id.to_string(),
        handler: id.to_string(),
        aliases: Vec::new(),
        handler_args: Vec::new(),
        kind: puffer_tools::ToolKind::Custom,
        input_schema: puffer_tools::ToolInputSchema::default(),
        metadata: puffer_tools::ToolMetadata {
            may_spawn_processes: id == "Bash" || id == "PowerShell",
            may_read_files: false,
            may_write_files: id == "Write",
        },
        policy: puffer_tools::ToolPolicyHints {
            approval_policy: Some(approval_policy.to_string()),
            sandbox_policy: Some("workspace-write".to_string()),
        },
        shared_lib: None,
        enabled_if: None,
        display: puffer_tools::ToolDisplayHints::default(),
    }
}

fn home_env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn runtime_context(
    permissions: PermissionsSettings,
    sandbox: SandboxSettings,
    plan_mode: bool,
    active_plan_path: Option<PathBuf>,
    cwd: PathBuf,
    working_dirs: Vec<PathBuf>,
    session_allow_all: bool,
    session_tool_permissions: HashMap<String, String>,
) -> RuntimePermissionContext {
    runtime_context_with_inputs(
        permissions,
        sandbox,
        plan_mode,
        active_plan_path,
        cwd,
        working_dirs,
        session_allow_all,
        session_tool_permissions,
        RuntimePermissionInputs::default(),
    )
}

fn runtime_context_with_inputs(
    permissions: PermissionsSettings,
    sandbox: SandboxSettings,
    plan_mode: bool,
    active_plan_path: Option<PathBuf>,
    cwd: PathBuf,
    working_dirs: Vec<PathBuf>,
    session_allow_all: bool,
    session_tool_permissions: HashMap<String, String>,
    inputs: RuntimePermissionInputs,
) -> RuntimePermissionContext {
    let session_id = Uuid::parse_str("2ba8b01d-5e7a-46b6-b747-7bfe5f6fa36a").unwrap();
    let profile = EffectivePermissionProfile::from_legacy_sources(
        &cwd,
        &working_dirs,
        &permissions,
        &sandbox,
        &session_id,
        session_allow_all,
        &SessionPermissionGrants::from_legacy_tool_permissions(&session_tool_permissions),
        plan_mode,
        active_plan_path.clone(),
        inputs.request_tool_filter,
    );
    RuntimePermissionContext {
        derived_policy: profile.derived_policy(),
        profile,
        permissions,
        sandbox,
    }
}

fn runtime_context_with_session_grants(
    permissions: PermissionsSettings,
    sandbox: SandboxSettings,
    plan_mode: bool,
    active_plan_path: Option<PathBuf>,
    cwd: PathBuf,
    working_dirs: Vec<PathBuf>,
    session_allow_all: bool,
    session_grants: SessionPermissionGrants,
    inputs: RuntimePermissionInputs,
) -> RuntimePermissionContext {
    let session_id = Uuid::parse_str("2ba8b01d-5e7a-46b6-b747-7bfe5f6fa36a").unwrap();
    let profile = EffectivePermissionProfile::from_legacy_sources(
        &cwd,
        &working_dirs,
        &permissions,
        &sandbox,
        &session_id,
        session_allow_all,
        &session_grants,
        plan_mode,
        active_plan_path.clone(),
        inputs.request_tool_filter,
    );
    RuntimePermissionContext {
        derived_policy: profile.derived_policy(),
        profile,
        permissions,
        sandbox,
    }
}

#[test]
fn default_permissions_contents_follow_declared_policy() {
    let contents = default_permissions_contents(&LoadedResources {
        tools: vec![
            LoadedItem {
                value: ToolSpec {
                    id: "Bash".to_string(),
                    name: "Bash".to_string(),
                    description: "Bash".to_string(),
                    handler: "bash".to_string(),
                    aliases: Vec::new(),
                    handler_args: Vec::new(),
                    approval_policy: Some("on-request".to_string()),
                    sandbox_policy: None,
                    shared_lib: None,
                    enabled_if: None,
                    input_schema: None,
                    metadata: Default::default(),
                    display: Default::default(),
                },
                source_info: SourceInfo {
                    path: "bash.yaml".into(),
                    kind: SourceKind::Builtin,
                },
            },
            LoadedItem {
                value: ToolSpec {
                    id: "Read".to_string(),
                    name: "Read".to_string(),
                    description: "Read".to_string(),
                    handler: "read".to_string(),
                    aliases: Vec::new(),
                    handler_args: Vec::new(),
                    approval_policy: Some("auto".to_string()),
                    sandbox_policy: None,
                    shared_lib: None,
                    enabled_if: None,
                    input_schema: None,
                    metadata: Default::default(),
                    display: Default::default(),
                },
                source_info: SourceInfo {
                    path: "read.yaml".into(),
                    kind: SourceKind::Builtin,
                },
            },
        ],
        ..LoadedResources::default()
    });
    assert!(contents.contains("bash = \"on-request\""));
    assert!(contents.contains("read = \"auto\""));
}

#[test]
fn plan_mode_marks_mutating_on_request_tools_as_ask() {
    let context = runtime_context(
        PermissionsSettings::default(),
        SandboxSettings::from_mode("workspace-write"),
        true,
        None,
        PathBuf::from("/tmp"),
        Vec::new(),
        false,
        HashMap::new(),
    );
    let decision =
        context.decision_for_tool_call(&tool_definition("Write", "on-request"), &Value::Null);
    assert_eq!(decision.behavior, ToolPermissionBehavior::Ask);
    assert!(decision.reason.unwrap_or_default().contains("ExitPlanMode"));
}

#[test]
fn plan_mode_allows_writes_and_edits_for_the_active_plan_file() {
    let active_plan_path = PathBuf::from("/tmp/.puffer/plans/session.md");
    let context = runtime_context(
        PermissionsSettings::default(),
        SandboxSettings::from_mode("workspace-write"),
        true,
        Some(active_plan_path.clone()),
        PathBuf::from("/tmp"),
        Vec::new(),
        false,
        HashMap::new(),
    );
    let write = context.decision_for_tool_call(
        &tool_definition("Write", "on-request"),
        &serde_json::json!({"file_path": active_plan_path, "content": "# Plan"}),
    );
    let edit = context.decision_for_tool_call(
        &tool_definition("Edit", "on-request"),
        &serde_json::json!({"file_path": "/tmp/.puffer/plans/./session.md", "old_string": "#", "new_string": "##"}),
    );

    assert_eq!(write.behavior, ToolPermissionBehavior::Allow);
    assert_eq!(edit.behavior, ToolPermissionBehavior::Allow);
}

#[test]
fn config_reads_allow_but_writes_ask() {
    let context = runtime_context(
        PermissionsSettings::default(),
        SandboxSettings::from_mode("workspace-write"),
        false,
        None,
        PathBuf::from("/tmp"),
        Vec::new(),
        false,
        HashMap::new(),
    );
    let config = tool_definition("Config", "auto");
    let read = context.decision_for_tool_call(&config, &serde_json::json!({"setting":"theme"}));
    let write = context.decision_for_tool_call(
        &config,
        &serde_json::json!({"setting":"theme","value":"dark"}),
    );
    assert_eq!(read.behavior, ToolPermissionBehavior::Allow);
    assert_eq!(write.behavior, ToolPermissionBehavior::Ask);
}

#[test]
fn ask_user_question_runs_without_permission_gate() {
    let context = runtime_context(
        PermissionsSettings::default(),
        SandboxSettings::from_mode("workspace-write"),
        false,
        None,
        PathBuf::from("/tmp"),
        Vec::new(),
        false,
        HashMap::new(),
    );
    let question = tool_definition("AskUserQuestion", "auto");
    let decision = context.decision_for_tool_call(
        &question,
        &serde_json::json!({"questions":[{"question":"Pick one","header":"Choice","options":[{"label":"A","description":"A"},{"label":"B","description":"B"}]}]}),
    );
    assert_eq!(decision.behavior, ToolPermissionBehavior::Allow);
}

#[test]
fn web_search_requires_permission_by_default() {
    let context = runtime_context(
        PermissionsSettings::default(),
        SandboxSettings::from_mode("workspace-write"),
        false,
        None,
        PathBuf::from("/tmp"),
        Vec::new(),
        false,
        HashMap::new(),
    );
    let search = tool_definition("WebSearch", "auto");
    let decision =
        context.decision_for_tool_call(&search, &serde_json::json!({"query":"rust latest"}));
    assert_eq!(decision.behavior, ToolPermissionBehavior::Ask);
}

#[test]
fn send_message_allows_local_targets_but_asks_for_bridge_targets() {
    let context = runtime_context(
        PermissionsSettings::default(),
        SandboxSettings::from_mode("workspace-write"),
        false,
        None,
        PathBuf::from("/tmp"),
        Vec::new(),
        false,
        HashMap::new(),
    );
    let send = tool_definition("SendMessage", "auto");
    let local =
        context.decision_for_tool_call(&send, &serde_json::json!({"to":"alice","message":"hi"}));
    let bridge = context.decision_for_tool_call(
        &send,
        &serde_json::json!({"to":"bridge:session-123","message":"hi"}),
    );
    assert_eq!(local.behavior, ToolPermissionBehavior::Allow);
    assert_eq!(bridge.behavior, ToolPermissionBehavior::Ask);
}

#[test]
fn todo_write_and_agent_are_allowed_without_extra_gate() {
    let context = runtime_context(
        PermissionsSettings::default(),
        SandboxSettings::from_mode("workspace-write"),
        true,
        None,
        PathBuf::from("/tmp"),
        Vec::new(),
        false,
        HashMap::new(),
    );
    let todo = tool_definition("TodoWrite", "auto");
    let agent = tool_definition("Agent", "auto");
    let todo_decision = context.decision_for_tool_call(
        &todo,
        &serde_json::json!({"todos":[{"content":"x","status":"pending","activeForm":"Doing x"}]}),
    );
    let agent_decision = context.decision_for_tool_call(
        &agent,
        &serde_json::json!({"description":"Task","prompt":"Do it"}),
    );
    assert_eq!(todo_decision.behavior, ToolPermissionBehavior::Allow);
    assert_eq!(agent_decision.behavior, ToolPermissionBehavior::Allow);
}

#[test]
fn disabled_tool_is_hidden_from_model_pool() {
    let mut definition = tool_definition("Bash", "on-request");
    definition.policy.approval_policy = Some("disabled".to_string());
    let context = runtime_context(
        PermissionsSettings::default(),
        SandboxSettings::from_mode("workspace-write"),
        false,
        None,
        PathBuf::from("/tmp"),
        Vec::new(),
        false,
        HashMap::new(),
    );
    assert!(!context.tool_visible_to_model(&definition));
}

#[test]
fn send_user_message_ignores_workspace_ask_rules() {
    let context = runtime_context(
        PermissionsSettings {
            tools: BTreeMap::from([
                ("sendusermessage".to_string(), "ask".to_string()),
                ("brief".to_string(), "deny".to_string()),
            ]),
        },
        SandboxSettings::from_mode("workspace-write"),
        true,
        None,
        PathBuf::from("/tmp"),
        Vec::new(),
        false,
        HashMap::new(),
    );
    let send_user_message = ToolDefinition {
        id: "SendUserMessage".to_string(),
        name: "SendUserMessage".to_string(),
        description: String::new(),
        handler: "runtime:workflow:send_user_message".to_string(),
        aliases: vec!["Brief".to_string()],
        handler_args: Vec::new(),
        kind: puffer_tools::ToolKind::Custom,
        input_schema: puffer_tools::ToolInputSchema::default(),
        metadata: puffer_tools::ToolMetadata::default(),
        policy: puffer_tools::ToolPolicyHints {
            approval_policy: Some("auto".to_string()),
            sandbox_policy: Some("read-only".to_string()),
        },
        shared_lib: None,
        enabled_if: None,
        display: puffer_tools::ToolDisplayHints::default(),
    };
    let brief = ToolDefinition {
        id: "Brief".to_string(),
        ..send_user_message.clone()
    };

    let send_decision =
        context.decision_for_tool_call(&send_user_message, &serde_json::json!({"message": "hi"}));
    let brief_decision =
        context.decision_for_tool_call(&brief, &serde_json::json!({"message": "hi"}));

    assert_eq!(send_decision.behavior, ToolPermissionBehavior::Allow);
    assert_eq!(brief_decision.behavior, ToolPermissionBehavior::Allow);
    assert!(context.tool_visible_to_model(&send_user_message));
    assert!(context.tool_visible_to_model(&brief));
}

#[test]
fn legacy_provider_tool_keys_apply_to_claude_style_tool_ids() {
    let context = runtime_context(
        PermissionsSettings {
            tools: BTreeMap::from([
                ("read_file".to_string(), "deny".to_string()),
                ("replace_in_file".to_string(), "ask".to_string()),
                ("list_dir".to_string(), "allow".to_string()),
                ("search_text".to_string(), "deny".to_string()),
            ]),
        },
        SandboxSettings::from_mode("workspace-write"),
        false,
        None,
        PathBuf::from("/tmp"),
        Vec::new(),
        false,
        HashMap::new(),
    );
    let read = tool_definition("Read", "auto");
    let edit = tool_definition("Edit", "auto");
    let glob = tool_definition("Glob", "auto");
    let grep = tool_definition("Grep", "auto");

    assert_eq!(
        context
            .decision_for_tool_call(&read, &serde_json::json!({"file_path": "/tmp/x"}))
            .behavior,
        ToolPermissionBehavior::Deny
    );
    assert_eq!(
        context
            .decision_for_tool_call(
                &edit,
                &serde_json::json!({"file_path": "/tmp/x", "old_string": "a", "new_string": "b"})
            )
            .behavior,
        ToolPermissionBehavior::Ask
    );
    assert_eq!(
        context
            .decision_for_tool_call(&glob, &serde_json::json!({"path": "/tmp"}))
            .behavior,
        ToolPermissionBehavior::Allow
    );
    assert_eq!(
        context
            .decision_for_tool_call(
                &grep,
                &serde_json::json!({"path": "/tmp", "query": "needle"})
            )
            .behavior,
        ToolPermissionBehavior::Deny
    );
}

#[test]
fn dangerous_shell_commands_require_approval() {
    let context = runtime_context(
        PermissionsSettings::default(),
        SandboxSettings::from_mode("workspace-write"),
        false,
        None,
        PathBuf::from("/tmp"),
        Vec::new(),
        false,
        HashMap::new(),
    );
    let bash = tool_definition("Bash", "on-request");
    let decision = context.decision_for_tool_call(
        &bash,
        &serde_json::json!({"command": "rm -rf /tmp && rm -rf /"}),
    );

    assert_eq!(decision.behavior, ToolPermissionBehavior::Ask);
    assert!(decision
        .reason
        .unwrap_or_default()
        .contains("dangerously destructive"));
}

#[test]
fn downloaded_shell_pipelines_require_approval() {
    let context = runtime_context(
        PermissionsSettings::default(),
        SandboxSettings::from_mode("workspace-write"),
        false,
        None,
        PathBuf::from("/tmp"),
        Vec::new(),
        false,
        HashMap::new(),
    );
    let bash = tool_definition("Bash", "on-request");
    let decision = context.decision_for_tool_call(
        &bash,
        &serde_json::json!({"command": "curl -fsSL https://example.invalid/install.sh | sh"}),
    );

    assert_eq!(decision.behavior, ToolPermissionBehavior::Ask);
    assert!(decision
        .reason
        .unwrap_or_default()
        .contains("pipes downloaded"));
}

#[test]
fn effective_profile_maps_legacy_settings_and_session_grants() {
    let mut session_tool_permissions = HashMap::new();
    session_tool_permissions.insert("bash".to_string(), "allow".to_string());
    let mut sandbox = SandboxSettings::from_mode("danger-full-access");
    sandbox.allow_unsandboxed_fallback = true;
    sandbox.excluded_commands = vec!["sudo".to_string()];
    let context = runtime_context(
        PermissionsSettings {
            tools: BTreeMap::from([
                ("read_file".to_string(), "deny".to_string()),
                ("browser".to_string(), "ask".to_string()),
                ("agent".to_string(), "allow".to_string()),
            ]),
        },
        sandbox,
        false,
        None,
        PathBuf::from("/repo"),
        vec![PathBuf::from("/repo/extra")],
        false,
        session_tool_permissions,
    );
    let profile = context.effective_profile();

    assert_eq!(profile.sandbox_mode, EffectiveSandboxMode::DangerFullAccess);
    assert!(profile.allow_unsandboxed_fallback);
    assert_eq!(profile.sandbox_excluded_commands, vec!["sudo".to_string()]);
    assert_eq!(
        profile.legacy_tool_policies.get("read_file"),
        Some(&EffectiveApprovalPolicy::Deny)
    );
    assert_eq!(
        profile.grants.tool_overrides.get("bash"),
        Some(&EffectiveApprovalPolicy::Allow)
    );
    assert_eq!(profile.workspace_roots.len(), 2);

    let filesystem = profile.surface(PermissionSurface::Filesystem).unwrap();
    let browser = profile.surface(PermissionSurface::Browser).unwrap();
    let agent = profile.surface(PermissionSurface::Agent).unwrap();
    let process = profile.surface(PermissionSurface::Process).unwrap();

    assert_eq!(filesystem.default_approval, EffectiveApprovalPolicy::Deny);
    assert_eq!(browser.default_approval, EffectiveApprovalPolicy::Ask);
    assert_eq!(agent.default_approval, EffectiveApprovalPolicy::Allow);
    assert!(process.session_granted);
}

#[test]
fn derived_policy_keeps_approval_and_sandbox_axes_separate() {
    let mut sandbox = SandboxSettings::from_mode("danger-full-access");
    sandbox.allow_unsandboxed_fallback = true;
    sandbox.excluded_commands = vec!["sudo".to_string(), "docker".to_string()];
    let context = runtime_context(
        PermissionsSettings {
            tools: BTreeMap::from([
                ("read".to_string(), "ask".to_string()),
                ("bash".to_string(), "deny".to_string()),
                ("web_search".to_string(), "ask".to_string()),
            ]),
        },
        sandbox,
        false,
        None,
        PathBuf::from("/repo"),
        vec![PathBuf::from("/repo/extra")],
        false,
        HashMap::new(),
    );

    let derived = context.derived_policy();
    let legacy = context.legacy_executor_bridge();

    assert_eq!(derived.filesystem().approval, EffectiveApprovalPolicy::Ask);
    assert!(derived.filesystem().allow_all_paths());
    assert_eq!(
        derived.filesystem().workspace_roots,
        vec![PathBuf::from("/repo"), PathBuf::from("/repo/extra")]
    );
    assert_eq!(derived.process().approval, EffectiveApprovalPolicy::Deny);
    assert!(derived.process().allow_unsandboxed_fallback);
    assert_eq!(
        derived.process().excluded_commands,
        vec!["sudo".to_string(), "docker".to_string()]
    );
    assert_eq!(derived.network().approval, EffectiveApprovalPolicy::Ask);
    assert!(legacy.allow_all_paths);
    assert_eq!(legacy.filesystem_sandbox_mode, "danger-full-access");
    assert!(legacy.allow_unsandboxed_fallback);
    assert_eq!(
        legacy.excluded_commands,
        vec!["sudo".to_string(), "docker".to_string()]
    );
}

#[test]
fn effective_profile_tracks_surface_taxonomy_and_enforcement() {
    assert_eq!(
        classify_tool_permission_surface("Read"),
        PermissionSurface::Filesystem
    );
    assert_eq!(
        classify_tool_permission_surface("Bash"),
        PermissionSurface::Process
    );
    assert_eq!(
        classify_tool_permission_surface("WebSearch"),
        PermissionSurface::Network
    );
    assert_eq!(
        classify_tool_permission_surface("Browser"),
        PermissionSurface::Browser
    );
    assert_eq!(
        classify_tool_permission_surface("ListMcpResourcesTool"),
        PermissionSurface::Mcp
    );
    assert_eq!(
        classify_tool_permission_surface("TodoWrite"),
        PermissionSurface::Workflow
    );
    assert_eq!(
        classify_tool_permission_surface("Agent"),
        PermissionSurface::Agent
    );

    let profile = EffectivePermissionProfile::from_legacy_sources(
        PathBuf::from("/repo").as_path(),
        &[],
        &PermissionsSettings::default(),
        &SandboxSettings::from_mode("workspace-write"),
        &Uuid::nil(),
        false,
        &SessionPermissionGrants::default(),
        false,
        None,
        None,
    );
    assert_eq!(
        profile
            .surface(PermissionSurface::Filesystem)
            .unwrap()
            .enforcement,
        SurfaceEnforcement::ExecutionEnforced
    );
    assert_eq!(
        profile
            .surface(PermissionSurface::Process)
            .unwrap()
            .enforcement,
        SurfaceEnforcement::PolicyOnly
    );
    assert_eq!(
        profile
            .surface(PermissionSurface::Browser)
            .unwrap()
            .enforcement,
        SurfaceEnforcement::PolicyOnly
    );
    assert_eq!(
        profile.surface(PermissionSurface::Mcp).unwrap().enforcement,
        SurfaceEnforcement::ExecutionEnforced
    );
}

#[test]
fn request_scope_deny_is_part_of_profile_evaluation() {
    let context = runtime_context_with_inputs(
        PermissionsSettings::default(),
        SandboxSettings::from_mode("workspace-write"),
        false,
        None,
        PathBuf::from("/tmp/work"),
        Vec::new(),
        false,
        HashMap::new(),
        RuntimePermissionInputs {
            request_tool_filter: build_request_tool_filter(&["Read(/tmp/work/**)".to_string()])
                .unwrap(),
        },
    );
    let decision = context.decision_for_tool_call(
        &tool_definition("Bash", "on-request"),
        &serde_json::json!({"command": "pwd"}),
    );
    assert_eq!(decision.behavior, ToolPermissionBehavior::Deny);
    assert_eq!(
        decision.reason.as_deref(),
        Some("slash command tool scope denied this tool call")
    );
    assert!(!context.tool_visible_to_model(&tool_definition("Bash", "on-request")));
    assert!(context.tool_visible_to_model(&tool_definition("Read", "on-request")));
}

#[test]
fn request_tool_filter_matches_bash_command_prefix_constraints() {
    let filter = build_request_tool_filter(&["Bash(git diff:*)".to_string()])
        .unwrap()
        .unwrap();
    let bash = tool_definition("Bash", "on-request");

    assert!(filter
        .allows_call(
            &bash,
            PathBuf::from("/tmp/work").as_path(),
            &serde_json::json!({"command":"git diff --name-only origin/HEAD..."})
        )
        .unwrap());
    assert!(!filter
        .allows_call(
            &bash,
            PathBuf::from("/tmp/work").as_path(),
            &serde_json::json!({"command":"git status"})
        )
        .unwrap());
    let npm_filter = build_request_tool_filter(&["Bash(npm:*)".to_string()])
        .unwrap()
        .unwrap();

    assert!(!npm_filter
        .allows_call(
            &bash,
            PathBuf::from("/tmp/work").as_path(),
            &serde_json::json!({"command":"npmx install"})
        )
        .unwrap());
}

#[test]
fn request_tool_filter_allows_tool_aliases_for_call_matching() {
    let filter = build_request_tool_filter(&["Brief".to_string()])
        .unwrap()
        .unwrap();
    let mut send_user_message = tool_definition("SendUserMessage", "on-request");
    send_user_message.aliases = vec!["Brief".to_string()];

    assert!(filter
        .allows_call(
            &send_user_message,
            PathBuf::from("/tmp/work").as_path(),
            &serde_json::json!({"message":"hi"})
        )
        .unwrap());
}

#[test]
fn request_tool_filter_expands_home_in_path_constraints() {
    let _guard = home_env_lock().lock().unwrap();
    let old_home = std::env::var_os("HOME");
    std::env::set_var("HOME", "/tmp/request-filter-home");

    let result = (|| {
        let filter = build_request_tool_filter(&["Read(~/src/**)".to_string()])?
            .expect("filter must be built");
        Ok::<_, anyhow::Error>(filter.allows_call(
            &tool_definition("Read", "on-request"),
            PathBuf::from("/tmp/work").as_path(),
            &serde_json::json!({"file_path":"~/src/lib.rs"}),
        )?)
    })();

    match old_home {
        Some(value) => std::env::set_var("HOME", value),
        None => std::env::remove_var("HOME"),
    }

    assert!(result.unwrap());
}

#[test]
fn request_tool_filter_matches_non_prefix_glob_patterns() {
    let filter = build_request_tool_filter(&["Read(/tmp/work/src/*.rs)".to_string()])
        .unwrap()
        .unwrap();
    let read = tool_definition("Read", "on-request");

    assert!(filter
        .allows_call(
            &read,
            PathBuf::from("/tmp/work").as_path(),
            &serde_json::json!({"file_path":"/tmp/work/src/lib.rs"})
        )
        .unwrap());
    assert!(!filter
        .allows_call(
            &read,
            PathBuf::from("/tmp/work").as_path(),
            &serde_json::json!({"file_path":"/tmp/work/src/nested/mod.rs"})
        )
        .unwrap());
}

#[test]
fn session_allow_all_bypasses_workspace_deny_in_profile_decision() {
    let context = runtime_context(
        PermissionsSettings {
            tools: BTreeMap::from([("bash".to_string(), "deny".to_string())]),
        },
        SandboxSettings::from_mode("workspace-write"),
        false,
        None,
        PathBuf::from("/tmp"),
        Vec::new(),
        true,
        HashMap::new(),
    );
    let decision = context.decision_for_tool_call(
        &tool_definition("Bash", "on-request"),
        &serde_json::json!({"command": "pwd"}),
    );
    assert_eq!(decision.behavior, ToolPermissionBehavior::Allow);
}

#[test]
fn session_grants_keep_legacy_tool_allow_projection() {
    let mut grants = SessionPermissionGrants::default();
    let session_id = uuid::Uuid::nil();
    grants.grant_tool_call(
        &tool_definition("Bash", "on-request"),
        &serde_json::json!({"command": "pwd"}),
        &session_id,
    );
    let legacy = grants.legacy_tool_permissions();
    assert_eq!(legacy.get("bash").map(String::as_str), Some("allow"));
}

#[test]
fn session_grants_flow_into_derived_policies_without_flipping_sandbox() {
    let mut grants = SessionPermissionGrants::default();
    let session_id = uuid::Uuid::nil();
    grants.grant_tool_call(
        &tool_definition("Read", "on-request"),
        &serde_json::json!({"file_path": "/repo/src/lib.rs"}),
        &session_id,
    );
    let profile = EffectivePermissionProfile::from_legacy_sources(
        PathBuf::from("/repo").as_path(),
        &[],
        &PermissionsSettings::default(),
        &SandboxSettings::from_mode("workspace-write"),
        &session_id,
        false,
        &grants,
        false,
        None,
        None,
    );
    let derived = profile.derived_policy();
    let legacy = derived.legacy_bridge();

    assert!(derived.filesystem().session_granted);
    assert!(!derived.filesystem().allow_all_paths());
    assert!(!legacy.allow_all_paths);
    assert_eq!(legacy.filesystem_sandbox_mode, "workspace-write");
}

#[test]
fn browser_and_bridge_session_grants_feed_profile_categories() {
    let mut grants = SessionPermissionGrants::default();
    let session_id = uuid::Uuid::parse_str("2ba8b01d-5e7a-46b6-b747-7bfe5f6fa36a").unwrap();
    grants.grant_tool_call(
        &tool_definition("Browser", "on-request"),
        &serde_json::json!({
            "action": "evaluate",
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
    let profile = EffectivePermissionProfile::from_legacy_sources(
        PathBuf::from("/repo").as_path(),
        &[],
        &PermissionsSettings::default(),
        &SandboxSettings::from_mode("workspace-write"),
        &session_id,
        false,
        &grants,
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
        PermissionGrantCategory::Browser(BrowserGrantCategory::Action(
            BrowserActionCategory::Evaluate
        ))
    )));
    assert!(profile.grants.category_grants.iter().any(|grant| matches!(
        grant,
        PermissionGrantCategory::Browser(BrowserGrantCategory::CrossSessionAccess)
    )));
    assert!(profile.grants.category_grants.iter().any(|grant| matches!(
        grant,
        PermissionGrantCategory::Workflow(
            super::profile::WorkflowGrantCategory::CrossSessionBridge
        )
    )));
}

#[test]
fn browser_scope_defaults_to_current_session_and_inspect() {
    let context = runtime_context(
        PermissionsSettings::default(),
        SandboxSettings::from_mode("workspace-write"),
        false,
        None,
        PathBuf::from("/tmp"),
        Vec::new(),
        false,
        HashMap::new(),
    );

    let scope = context
        .effective_profile()
        .browser_scope(&serde_json::json!({"action":"snapshot"}));

    assert_eq!(scope.action, Some(BrowserActionCategory::Inspect));
    assert_eq!(
        scope.root_session_id,
        "2ba8b01d-5e7a-46b6-b747-7bfe5f6fa36a".to_string()
    );
    assert!(!scope.is_cross_session);
}

#[test]
fn browser_inspect_is_allowed_but_navigation_and_evaluate_require_approval() {
    let context = runtime_context(
        PermissionsSettings::default(),
        SandboxSettings::from_mode("workspace-write"),
        false,
        None,
        PathBuf::from("/tmp"),
        Vec::new(),
        false,
        HashMap::new(),
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

    assert_eq!(inspect.behavior, ToolPermissionBehavior::Allow);
    assert_eq!(navigate.behavior, ToolPermissionBehavior::Ask);
    assert_eq!(evaluate.behavior, ToolPermissionBehavior::Ask);
    assert!(evaluate
        .reason
        .unwrap_or_default()
        .contains("executes page JavaScript"));
}

#[test]
fn browser_cross_session_access_requires_explicit_approval() {
    let context = runtime_context(
        PermissionsSettings::default(),
        SandboxSettings::from_mode("workspace-write"),
        false,
        None,
        PathBuf::from("/tmp"),
        Vec::new(),
        false,
        HashMap::new(),
    );

    let decision = context.decision_for_tool_call(
        &tool_definition("Browser", "auto"),
        &serde_json::json!({
            "action":"snapshot",
            "sessionId":"b4f239fd-1493-4be7-a3a1-9e58fe612576"
        }),
    );

    assert_eq!(decision.behavior, ToolPermissionBehavior::Ask);
    assert!(decision
        .reason
        .unwrap_or_default()
        .contains("cross-session browser access"));
}

#[test]
fn browser_session_grant_is_scoped_to_action_category_and_cross_session_flag() {
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
    let profile = EffectivePermissionProfile::from_legacy_sources(
        PathBuf::from("/repo").as_path(),
        &[],
        &PermissionsSettings::default(),
        &SandboxSettings::from_mode("workspace-write"),
        &session_id,
        false,
        &grants,
        false,
        None,
        None,
    );

    assert!(profile.browser_session_grant_allows(&serde_json::json!({
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

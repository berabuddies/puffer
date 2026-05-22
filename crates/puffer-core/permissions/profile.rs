use super::{
    browser_action::browser_permission_value_for_tool_call,
    browser_grants::{
        browser_context_matches_grants, browser_grant_categories, BrowserGrantCategory,
    },
    browser_target::{
        browser_permission_context, browser_permission_context_for_tool, BrowserPermissionContext,
    },
    canonical_tool_name, normalize_policy_value, normalize_tool_id, PermissionsSettings,
    SandboxSettings,
};
use anyhow::{anyhow, Result};
use glob::MatchOptions;
use puffer_tools::ToolDefinition;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use uuid::Uuid;

/// Enumerates the normalized permission surfaces exposed by `puffer-core`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum PermissionSurface {
    Filesystem,
    Process,
    Network,
    Browser,
    Mcp,
    Workflow,
    Agent,
}

/// Identifies whether a capability is enforced in execution code or only in policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SurfaceEnforcement {
    PolicyOnly,
    ExecutionEnforced,
}

/// Normalized approval modes used by the effective permission profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EffectiveApprovalPolicy {
    Allow,
    Ask,
    Deny,
    OnRequest,
}

impl EffectiveApprovalPolicy {
    /// Converts a legacy approval string into the normalized effective policy.
    pub(crate) fn from_legacy_value(value: &str) -> Self {
        match normalize_policy_value(value).as_str() {
            "allow" | "auto" => Self::Allow,
            "ask" => Self::Ask,
            "deny" | "disabled" => Self::Deny,
            "on-request" => Self::OnRequest,
            _ => Self::Allow,
        }
    }
}

/// Normalized sandbox modes used by the effective permission profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EffectiveSandboxMode {
    ReadOnly,
    WorkspaceWrite,
    DangerFullAccess,
    Custom,
}

impl EffectiveSandboxMode {
    /// Converts a legacy sandbox mode string into the normalized mode.
    pub(crate) fn from_legacy_mode(mode: &str) -> Self {
        match mode.trim() {
            "read-only" => Self::ReadOnly,
            "workspace-write" => Self::WorkspaceWrite,
            "danger-full-access" => Self::DangerFullAccess,
            _ => Self::Custom,
        }
    }
}

/// Summarizes the effective approval and execution state for one surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EffectiveSurfaceProfile {
    pub(crate) surface: PermissionSurface,
    pub(crate) enforcement: SurfaceEnforcement,
    pub(crate) default_approval: EffectiveApprovalPolicy,
    pub(crate) session_granted: bool,
    pub(crate) notes: Vec<String>,
}

/// Groups workflow grants that need more detail than the tool id alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum WorkflowGrantCategory {
    CrossSessionBridge,
}

/// Carries one category-level session grant derived from a tool approval.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum PermissionGrantCategory {
    Browser(BrowserGrantCategory),
    Workflow(WorkflowGrantCategory),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum SessionGrantTarget {
    Tool(String),
    Surface(PermissionSurface),
    Category(PermissionGrantCategory),
    PathPrefix(PathBuf),
}

/// Stores the normalized session-scoped grants accumulated from permission prompts.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct SessionPermissionGrants {
    granted: BTreeSet<SessionGrantTarget>,
}

/// Canonical in-memory session permission state for one runtime session.
///
/// This is the approval-bearing state used for current-turn permission
/// evaluation and worker/UI round-trips.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionPermissionState {
    allow_all_tools: bool,
    grants: SessionPermissionGrants,
}

impl SessionPermissionGrants {
    /// Rebuilds typed grants from the legacy whole-tool permission map.
    pub(crate) fn from_legacy_tool_permissions(tool_permissions: &HashMap<String, String>) -> Self {
        let mut grants = Self::default();
        for (tool, level) in tool_permissions {
            if canonical_tool_name(tool) == "browser" {
                continue;
            }
            if EffectiveApprovalPolicy::from_legacy_value(level) == EffectiveApprovalPolicy::Allow {
                grants
                    .granted
                    .insert(SessionGrantTarget::Tool(normalize_tool_id(tool)));
            }
        }
        grants
    }

    pub(crate) fn grant_tool_call(
        &mut self,
        definition: &ToolDefinition,
        input: &Value,
        current_session_id: &Uuid,
    ) {
        let carries_browser_permission =
            browser_permission_value_for_tool_call(&definition.id, input).is_some();
        let categories = grant_categories_for_tool_call(definition, input, current_session_id);
        if canonical_tool_name(&definition.id) != "browser" && !carries_browser_permission {
            self.granted
                .insert(SessionGrantTarget::Tool(normalize_tool_id(&definition.id)));
            self.granted.insert(SessionGrantTarget::Surface(
                classify_tool_permission_surface(&definition.id),
            ));
        }
        for category in categories {
            self.granted.insert(SessionGrantTarget::Category(category));
        }
    }

    /// Inserts Browser grants for an approved Browser call using an explicit Browser scope.
    pub(crate) fn grant_browser_tool_call(
        &mut self,
        definition: &ToolDefinition,
        input: &Value,
        current_session_id: &Uuid,
        scope: super::browser_grants::BrowserGrantScopeKind,
    ) {
        if canonical_tool_name(&definition.id) != "browser" {
            if browser_permission_value_for_tool_call(&definition.id, input).is_none() {
                self.grant_tool_call(definition, input, current_session_id);
                return;
            }
        }
        let context = browser_permission_context_for_tool(
            &definition.id,
            input,
            &current_session_id.to_string(),
            &[],
        );
        for category in super::browser_grants::browser_grant_categories_for_scope(&context, scope) {
            self.granted.insert(SessionGrantTarget::Category(
                PermissionGrantCategory::Browser(category),
            ));
        }
    }

    /// Grants access to a filesystem path prefix for the current session.
    pub(crate) fn grant_path_prefix(&mut self, path: PathBuf) {
        self.granted.insert(SessionGrantTarget::PathPrefix(path));
    }

    /// Returns legacy whole-tool allow entries for compatibility snapshots.
    pub(crate) fn legacy_tool_permissions(&self) -> HashMap<String, String> {
        self.granted
            .iter()
            .filter_map(|grant| match grant {
                SessionGrantTarget::Tool(tool) => Some((tool.clone(), "allow".to_string())),
                _ => None,
            })
            .collect()
    }

    pub(crate) fn profile_view(&self, allow_all_tools: bool) -> SessionGrantProfile {
        let mut profile = SessionGrantProfile {
            allow_all_tools,
            ..SessionGrantProfile::default()
        };
        for grant in &self.granted {
            match grant {
                SessionGrantTarget::Tool(tool) => {
                    profile
                        .tool_overrides
                        .insert(tool.clone(), EffectiveApprovalPolicy::Allow);
                    profile
                        .surface_grants
                        .insert(classify_tool_permission_surface(tool));
                }
                SessionGrantTarget::Surface(surface) => {
                    profile.surface_grants.insert(*surface);
                }
                SessionGrantTarget::Category(category) => {
                    profile.category_grants.insert(category.clone());
                    profile.surface_grants.insert(category_surface(category));
                }
                SessionGrantTarget::PathPrefix(path) => {
                    profile.path_prefix_grants.push(path.clone());
                    profile.surface_grants.insert(PermissionSurface::Filesystem);
                }
            }
        }
        profile.path_prefix_grants.sort();
        profile.path_prefix_grants.dedup();
        profile
    }

    fn touches_surface(&self, surface: PermissionSurface) -> bool {
        self.granted
            .iter()
            .any(|grant| grant_target_matches_surface(grant, surface))
    }

    /// Inserts a surface-scoped session grant for tests without adding tool or
    /// category approvals.
    #[cfg(test)]
    pub(crate) fn grant_surface_for_test(&mut self, surface: PermissionSurface) {
        self.granted.insert(SessionGrantTarget::Surface(surface));
    }
}

impl SessionPermissionState {
    /// Builds a canonical session permission state from explicit allow-all and
    /// grant values.
    pub(crate) fn new(allow_all_tools: bool, grants: SessionPermissionGrants) -> Self {
        Self {
            allow_all_tools,
            grants,
        }
    }

    /// Returns true when the canonical state is in allow-all mode.
    pub fn allow_all_tools(&self) -> bool {
        self.allow_all_tools
    }

    /// Replaces the session-wide allow-all flag.
    pub(crate) fn set_allow_all_tools(&mut self, allow_all_tools: bool) {
        self.allow_all_tools = allow_all_tools;
    }

    /// Returns the accumulated typed session grants.
    pub(crate) fn grants(&self) -> &SessionPermissionGrants {
        &self.grants
    }

    /// Returns the accumulated typed session grants mutably.
    pub(crate) fn grants_mut(&mut self) -> &mut SessionPermissionGrants {
        &mut self.grants
    }

    /// Returns true when this session currently carries any Browser typed grant.
    pub fn has_browser_grant(&self) -> bool {
        self.grants.touches_surface(PermissionSurface::Browser)
    }

    /// Builds a legacy-compatible projection that can round-trip through
    /// durable snapshots and older resume semantics.
    pub fn legacy_snapshot_projection(&self) -> (bool, HashMap<String, String>) {
        (self.allow_all_tools, self.grants.legacy_tool_permissions())
    }

    /// Rebuilds typed session permission state from a legacy-compatible
    /// snapshot projection.
    pub fn from_legacy_snapshot_projection(
        allow_all_tools: bool,
        tool_permissions: &HashMap<String, String>,
    ) -> Self {
        Self::new(
            allow_all_tools,
            SessionPermissionGrants::from_legacy_tool_permissions(tool_permissions),
        )
    }
}

impl SessionGrantProfile {
    fn browser_context_is_granted(&self, context: &BrowserPermissionContext) -> bool {
        browser_context_matches_grants(context, |grant| {
            self.category_grants
                .contains(&PermissionGrantCategory::Browser(grant.clone()))
        })
    }
}

/// Summarizes session-scoped grants folded into the effective profile.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct SessionGrantProfile {
    pub(crate) allow_all_tools: bool,
    pub(crate) tool_overrides: BTreeMap<String, EffectiveApprovalPolicy>,
    pub(crate) surface_grants: BTreeSet<PermissionSurface>,
    pub(crate) category_grants: BTreeSet<PermissionGrantCategory>,
    pub(crate) path_prefix_grants: Vec<PathBuf>,
}

/// Effective permission abstraction derived from config and session state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EffectivePermissionProfile {
    pub(crate) approval_default: EffectiveApprovalPolicy,
    pub(crate) sandbox_mode: EffectiveSandboxMode,
    pub(crate) allow_unsandboxed_fallback: bool,
    pub(crate) sandbox_excluded_commands: Vec<String>,
    pub(crate) current_session_id: String,
    pub(crate) workspace_roots: Vec<PathBuf>,
    pub(crate) surfaces: BTreeMap<PermissionSurface, EffectiveSurfaceProfile>,
    pub(crate) grants: SessionGrantProfile,
    pub(crate) legacy_tool_policies: BTreeMap<String, EffectiveApprovalPolicy>,
    pub(crate) plan_mode: bool,
    pub(crate) active_plan_path: Option<PathBuf>,
    request_tool_filter: Option<RequestToolFilter>,
}

impl EffectivePermissionProfile {
    /// Builds the effective profile from the current runtime permission sources.
    pub(crate) fn from_session_state(
        cwd: &Path,
        working_dirs: &[PathBuf],
        permissions: &PermissionsSettings,
        sandbox: &SandboxSettings,
        current_session_id: &Uuid,
        session_state: &SessionPermissionState,
        plan_mode: bool,
        active_plan_path: Option<PathBuf>,
        request_tool_filter: Option<RequestToolFilter>,
    ) -> Self {
        let session_allow_all = session_state.allow_all_tools();
        let session_grants = session_state.grants();
        let legacy_tool_policies = collect_legacy_tool_policies(permissions);
        let approval_default = if session_allow_all {
            EffectiveApprovalPolicy::Allow
        } else {
            EffectiveApprovalPolicy::OnRequest
        };
        let grants = session_grants.profile_view(session_allow_all);
        let surfaces =
            build_surface_profiles(permissions, sandbox, session_allow_all, session_grants);
        Self {
            approval_default,
            sandbox_mode: EffectiveSandboxMode::from_legacy_mode(&sandbox.mode),
            allow_unsandboxed_fallback: sandbox.allow_unsandboxed_fallback,
            sandbox_excluded_commands: sandbox.excluded_commands.clone(),
            current_session_id: current_session_id.to_string(),
            workspace_roots: std::iter::once(cwd.to_path_buf())
                .chain(working_dirs.iter().cloned())
                .chain(grants.path_prefix_grants.iter().cloned())
                .map(canonical_workspace_root)
                .collect(),
            surfaces,
            grants,
            legacy_tool_policies,
            plan_mode,
            active_plan_path,
            request_tool_filter,
        }
    }

    /// Returns the normalized profile for one permission surface.
    pub(crate) fn surface(&self, surface: PermissionSurface) -> Option<&EffectiveSurfaceProfile> {
        self.surfaces.get(&surface)
    }

    /// Returns true when the request-scoped filter still exposes the tool definition.
    pub(crate) fn request_allows_definition(&self, definition: &ToolDefinition) -> bool {
        self.request_tool_filter
            .as_ref()
            .map(|filter| filter.allows_definition(definition))
            .unwrap_or(true)
    }

    /// Returns true when the request-scoped filter allows the concrete tool call.
    pub(crate) fn request_allows_call(
        &self,
        definition: &ToolDefinition,
        cwd: &Path,
        input: &Value,
    ) -> Result<bool> {
        self.request_tool_filter
            .as_ref()
            .map(|filter| filter.allows_call(definition, cwd, input))
            .unwrap_or(Ok(true))
    }

    /// Returns the structured Browser permission context for one tool call.
    pub(crate) fn browser_context(&self, input: &Value) -> BrowserPermissionContext {
        browser_permission_context(input, &self.current_session_id, &self.workspace_roots)
    }

    /// Returns the structured Browser permission context for one permission-bearing tool call.
    pub(crate) fn browser_context_for_tool(
        &self,
        tool_id: &str,
        input: &Value,
    ) -> BrowserPermissionContext {
        browser_permission_context_for_tool(
            tool_id,
            input,
            &self.current_session_id,
            &self.workspace_roots,
        )
    }

    /// Returns true when the accumulated session grants explicitly allow the Browser context.
    pub(crate) fn browser_context_is_session_granted(
        &self,
        context: &BrowserPermissionContext,
    ) -> bool {
        self.grants.browser_context_is_granted(context)
    }

    /// Returns true when the accumulated session grants explicitly allow this Browser call.
    pub(crate) fn browser_session_grant_allows(&self, input: &Value) -> bool {
        self.browser_context_is_session_granted(&self.browser_context(input))
    }
}

/// Describes a request-scoped filter built from prompt-backed allowed-tools selectors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RequestToolFilter {
    rules: Vec<RequestToolRule>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RequestToolRule {
    tool_names: BTreeSet<String>,
    constraint: Option<RequestToolConstraint>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RequestToolConstraint {
    PathPattern(String),
    CommandPrefix(String),
}

const REQUEST_TOOL_PATH_MATCH_OPTIONS: MatchOptions = MatchOptions {
    case_sensitive: true,
    require_literal_separator: true,
    require_literal_leading_dot: false,
};

/// Builds a request-scoped tool filter from prompt-backed allowed-tools entries.
pub(crate) fn build_request_tool_filter(selectors: &[String]) -> Result<Option<RequestToolFilter>> {
    if selectors.is_empty() {
        return Ok(None);
    }
    let mut rules = Vec::new();
    for selector in selectors {
        let selector = selector.trim();
        if selector.is_empty() {
            continue;
        }
        let (tool, constraint) = parse_request_tool_selector(selector)?;
        rules.push(RequestToolRule {
            tool_names: request_tool_names(&tool),
            constraint,
        });
    }
    Ok(Some(RequestToolFilter { rules }))
}

impl RequestToolFilter {
    pub(crate) fn empty_static() -> &'static Self {
        static EMPTY: OnceLock<RequestToolFilter> = OnceLock::new();
        EMPTY.get_or_init(|| Self { rules: Vec::new() })
    }

    pub(crate) fn allows_definition(&self, definition: &ToolDefinition) -> bool {
        let names = request_tool_names(&definition.id);
        self.rules.iter().any(|rule| {
            !rule.tool_names.is_disjoint(&names)
                || definition
                    .aliases
                    .iter()
                    .map(|alias| canonical_tool_name(alias))
                    .any(|name| rule.tool_names.contains(&name))
        })
    }

    pub(crate) fn allows_call(
        &self,
        definition: &ToolDefinition,
        cwd: &Path,
        input: &Value,
    ) -> Result<bool> {
        let mut names = request_tool_names(&definition.id);
        for alias in &definition.aliases {
            names.extend(request_tool_names(alias));
        }
        for rule in &self.rules {
            if rule.tool_names.is_disjoint(&names) {
                continue;
            }
            match &rule.constraint {
                Some(RequestToolConstraint::PathPattern(path_pattern)) => {
                    let call_path = call_path_for_filter(input)
                        .map(|path| absolutize_filter_path(cwd, path))
                        .transpose()?;
                    if let Some(call_path) = call_path {
                        if path_matches_pattern(cwd, path_pattern, &call_path)? {
                            return Ok(true);
                        }
                    }
                }
                Some(RequestToolConstraint::CommandPrefix(command_prefix)) => {
                    if input
                        .get("command")
                        .and_then(Value::as_str)
                        .is_some_and(|command| command_matches_prefix(command, command_prefix))
                    {
                        return Ok(true);
                    }
                }
                None => return Ok(true),
            }
        }
        Ok(false)
    }
}

/// Returns the primary permission surface for one model-visible tool id.
pub(crate) fn classify_tool_permission_surface(tool_id: &str) -> PermissionSurface {
    match canonical_tool_name(tool_id).as_str() {
        "bash" | "powershell" => PermissionSurface::Process,
        "read" | "write" | "edit" | "glob" | "grep" | "notebookedit" | "config" => {
            PermissionSurface::Filesystem
        }
        "websearch" | "webfetch" => PermissionSurface::Network,
        "browser" => PermissionSurface::Browser,
        "listmcpresourcestool" | "readmcpresourcetool" => PermissionSurface::Mcp,
        "agent" => PermissionSurface::Agent,
        "askuserquestion" | "sendmessage" | "todowrite" => PermissionSurface::Workflow,
        _ => PermissionSurface::Workflow,
    }
}

fn collect_legacy_tool_policies(
    permissions: &PermissionsSettings,
) -> BTreeMap<String, EffectiveApprovalPolicy> {
    permissions
        .tools
        .iter()
        .filter(|(tool, _)| canonical_tool_name(tool) != "browser")
        .map(|(tool, level)| {
            (
                normalize_tool_id(tool),
                EffectiveApprovalPolicy::from_legacy_value(level),
            )
        })
        .collect()
}

fn canonical_workspace_root(path: PathBuf) -> PathBuf {
    std::fs::canonicalize(&path).unwrap_or(path)
}

fn build_surface_profiles(
    permissions: &PermissionsSettings,
    sandbox: &SandboxSettings,
    session_allow_all: bool,
    session_grants: &SessionPermissionGrants,
) -> BTreeMap<PermissionSurface, EffectiveSurfaceProfile> {
    let policies = collect_legacy_tool_policies(permissions);
    let mut surfaces = BTreeMap::new();
    for surface in [
        PermissionSurface::Filesystem,
        PermissionSurface::Process,
        PermissionSurface::Network,
        PermissionSurface::Browser,
        PermissionSurface::Mcp,
        PermissionSurface::Workflow,
        PermissionSurface::Agent,
    ] {
        let tool_keys = surface_tool_keys(surface);
        let default_approval = if session_allow_all {
            if surface == PermissionSurface::Browser {
                surface_default_policy(surface)
            } else {
                EffectiveApprovalPolicy::Allow
            }
        } else {
            tool_keys
                .iter()
                .filter_map(|key| policies.get(key).copied())
                .find(|policy| *policy != EffectiveApprovalPolicy::Allow)
                .unwrap_or(surface_default_policy(surface))
        };
        let session_granted = if surface == PermissionSurface::Browser {
            session_grants.touches_surface(surface)
        } else {
            session_allow_all || session_grants.touches_surface(surface)
        };
        surfaces.insert(
            surface,
            EffectiveSurfaceProfile {
                surface,
                enforcement: surface_enforcement(surface),
                default_approval,
                session_granted,
                notes: surface_notes(surface, sandbox),
            },
        );
    }
    surfaces
}

fn surface_default_policy(surface: PermissionSurface) -> EffectiveApprovalPolicy {
    match surface {
        PermissionSurface::Filesystem => EffectiveApprovalPolicy::OnRequest,
        PermissionSurface::Process => EffectiveApprovalPolicy::OnRequest,
        PermissionSurface::Network => EffectiveApprovalPolicy::Ask,
        PermissionSurface::Browser => EffectiveApprovalPolicy::Ask,
        PermissionSurface::Mcp => EffectiveApprovalPolicy::OnRequest,
        PermissionSurface::Workflow => EffectiveApprovalPolicy::Allow,
        PermissionSurface::Agent => EffectiveApprovalPolicy::Allow,
    }
}

fn surface_enforcement(surface: PermissionSurface) -> SurfaceEnforcement {
    match surface {
        PermissionSurface::Filesystem | PermissionSurface::Mcp => {
            SurfaceEnforcement::ExecutionEnforced
        }
        PermissionSurface::Process
        | PermissionSurface::Network
        | PermissionSurface::Browser
        | PermissionSurface::Workflow
        | PermissionSurface::Agent => SurfaceEnforcement::PolicyOnly,
    }
}

fn surface_notes(surface: PermissionSurface, _sandbox: &SandboxSettings) -> Vec<String> {
    match surface {
        PermissionSurface::Filesystem => vec![
            "Path access is bounded by workspace roots on the legacy Claude tool path.".to_string(),
            "Runner sandbox roots are execution-enforced when a LocalToolRunner is configured with roots."
                .to_string(),
        ],
        PermissionSurface::Process => {
            vec![
                "Shell approval and dangerous-command checks are policy-level in puffer-core.".to_string(),
                "Process execution itself is not sandbox-enforced by the Bash tool implementation.".to_string(),
            ]
        }
        PermissionSurface::Network => vec![
            "WebSearch is approval-gated in policy.".to_string(),
            "WebFetch and provider HTTP access are not centrally execution-constrained by this profile."
                .to_string(),
        ],
        PermissionSurface::Browser => vec![
            "Browser access is evaluated by the `[browser]` section in permissions.toml and typed session grants."
                .to_string(),
        ],
        PermissionSurface::Mcp => vec![
            "MCP calls are policy-classified here.".to_string(),
            "Built-in filesystem MCP resources inherit runner sandbox enforcement when sandbox roots are configured."
                .to_string(),
        ],
        PermissionSurface::Workflow => vec![
            "Workflow tools mostly persist session-scoped state under the workspace runtime directory."
                .to_string(),
            "Current gating is policy-level, except where workflow handlers delegate into filesystem tools."
                .to_string(),
        ],
        PermissionSurface::Agent => vec![
            "Agent delegation is permission-classified at policy level.".to_string(),
            "Subagent execution inherits the surrounding runtime rather than a distinct execution sandbox."
                .to_string(),
        ],
    }
}

fn surface_tool_keys(surface: PermissionSurface) -> BTreeSet<String> {
    let tool_ids = match surface {
        PermissionSurface::Filesystem => &[
            "Read",
            "Write",
            "Edit",
            "Glob",
            "Grep",
            "NotebookEdit",
            "Config",
        ][..],
        PermissionSurface::Process => &["Bash", "PowerShell"],
        PermissionSurface::Network => &["WebSearch", "WebFetch"],
        PermissionSurface::Browser => &[],
        PermissionSurface::Mcp => &["ListMcpResourcesTool", "ReadMcpResourceTool"],
        PermissionSurface::Workflow => &["AskUserQuestion", "SendMessage", "TodoWrite"],
        PermissionSurface::Agent => &["Agent"],
    };
    let mut keys = BTreeSet::new();
    for tool_id in tool_ids {
        keys.extend(tool_keys_for_id(tool_id));
    }
    keys
}

fn tool_keys_for_id(tool_id: &str) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    keys.insert(normalize_tool_id(tool_id));
    keys.insert(canonical_tool_name(tool_id));
    for alias in legacy_aliases_for_tool_id(tool_id) {
        keys.insert(normalize_tool_id(alias));
        keys.insert(canonical_tool_name(alias));
    }
    keys.retain(|value| !value.is_empty());
    keys
}

fn legacy_aliases_for_tool_id(tool_id: &str) -> &'static [&'static str] {
    match canonical_tool_name(tool_id).as_str() {
        "agent" => &["task"],
        "edit" => &["replace_in_file"],
        "glob" => &["list_dir"],
        "grep" => &["search_text"],
        "listmcpresourcestool" => &["list_mcp_resources"],
        "read" => &["read_file"],
        "readmcpresourcetool" => &["read_mcp_resource"],
        "taskoutput" => &["agent_output_tool", "bash_output_tool"],
        "taskstop" => &["kill_shell"],
        "write" => &["write_file"],
        _ => &[],
    }
}

fn grant_categories_for_tool_call(
    definition: &ToolDefinition,
    input: &Value,
    current_session_id: &Uuid,
) -> Vec<PermissionGrantCategory> {
    match canonical_tool_name(&definition.id).as_str() {
        "browser" | "bash" | "powershell" => {
            browser_permission_value_for_tool_call(&definition.id, input)
                .map(|_| {
                    let context = browser_permission_context_for_tool(
                        &definition.id,
                        input,
                        &current_session_id.to_string(),
                        &[],
                    );
                    browser_grant_categories(&context)
                        .into_iter()
                        .map(PermissionGrantCategory::Browser)
                        .collect()
                })
                .unwrap_or_default()
        }
        "sendmessage" => workflow_grant_categories(input)
            .into_iter()
            .map(PermissionGrantCategory::Workflow)
            .collect(),
        _ => Vec::new(),
    }
}

fn workflow_grant_categories(input: &Value) -> Vec<WorkflowGrantCategory> {
    string_field(input, "to")
        .filter(|target| target.starts_with("bridge:"))
        .map(|_| vec![WorkflowGrantCategory::CrossSessionBridge])
        .unwrap_or_default()
}

fn string_field<'a>(input: &'a Value, key: &str) -> Option<&'a str> {
    input.get(key).and_then(Value::as_str)
}

fn category_surface(category: &PermissionGrantCategory) -> PermissionSurface {
    match category {
        PermissionGrantCategory::Browser(_) => PermissionSurface::Browser,
        PermissionGrantCategory::Workflow(_) => PermissionSurface::Workflow,
    }
}

fn grant_target_matches_surface(grant: &SessionGrantTarget, surface: PermissionSurface) -> bool {
    match grant {
        SessionGrantTarget::Tool(tool) => classify_tool_permission_surface(tool) == surface,
        SessionGrantTarget::Surface(candidate) => *candidate == surface,
        SessionGrantTarget::Category(category) => category_surface(category) == surface,
        SessionGrantTarget::PathPrefix(_) => surface == PermissionSurface::Filesystem,
    }
}

fn parse_request_tool_selector(selector: &str) -> Result<(String, Option<RequestToolConstraint>)> {
    let Some(open_paren) = selector.find('(') else {
        return Ok((selector.to_string(), None));
    };
    let Some(close_paren) = selector.rfind(')') else {
        return Err(anyhow!("invalid allowed tool selector `{selector}`"));
    };
    let tool = selector[..open_paren].trim();
    if tool.is_empty() {
        return Err(anyhow!("invalid allowed tool selector `{selector}`"));
    }
    let raw_constraint = selector[open_paren + 1..close_paren].trim();
    let constraint = if raw_constraint.is_empty() {
        None
    } else if command_constraint_supported(tool) && raw_constraint.ends_with(":*") {
        Some(RequestToolConstraint::CommandPrefix(
            raw_constraint.trim_end_matches(":*").trim_end().to_string(),
        ))
    } else {
        Some(RequestToolConstraint::PathPattern(
            raw_constraint.trim().to_string(),
        ))
    };
    Ok((tool.to_string(), constraint))
}

fn request_tool_names(raw: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let normalized = normalize_tool_id(raw);
    if !normalized.is_empty() {
        names.insert(normalized);
    }
    let canonical = canonical_tool_name(raw);
    if !canonical.is_empty() {
        names.insert(canonical);
    }
    match canonical_tool_name(raw).as_str() {
        "agent" => {
            names.insert("task".to_string());
        }
        "glob" => {
            names.insert("ls".to_string());
            names.insert("list_dir".to_string());
        }
        "read" => {
            names.insert("read_file".to_string());
        }
        "edit" => {
            names.insert("replace_in_file".to_string());
        }
        "write" => {
            names.insert("write_file".to_string());
        }
        "grep" => {
            names.insert("search_text".to_string());
        }
        _ => {}
    }
    names
}

fn call_path_for_filter(input: &Value) -> Option<&str> {
    ["file_path", "path", "notebook_path"]
        .into_iter()
        .find_map(|field| input.get(field).and_then(Value::as_str))
}

fn command_matches_prefix(command: &str, command_prefix: &str) -> bool {
    let mut command_tokens = command.split_whitespace();
    let mut prefix_tokens = command_prefix.split_whitespace();
    let mut matched_any = false;
    loop {
        match prefix_tokens.next() {
            Some(prefix_token) => {
                matched_any = true;
                if command_tokens.next() != Some(prefix_token) {
                    return false;
                }
            }
            None => return matched_any,
        }
    }
}

fn absolutize_filter_path(cwd: &Path, raw: &str) -> Result<PathBuf> {
    let path = expand_home_path(raw).unwrap_or_else(|| PathBuf::from(raw));
    let absolute = if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    };
    Ok(normalize_path(&absolute))
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                let _ = normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn expand_home_path(raw: &str) -> Option<PathBuf> {
    if raw == "~" {
        std::env::var_os("HOME").map(PathBuf::from)
    } else if let Some(suffix) = raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~\\")) {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join(suffix))
    } else {
        None
    }
}

fn command_constraint_supported(tool: &str) -> bool {
    matches!(canonical_tool_name(tool).as_str(), "bash" | "powershell")
}

fn path_matches_pattern(cwd: &Path, pattern: &str, path: &Path) -> Result<bool> {
    let path_pattern = absolutize_filter_path(cwd, pattern)?;
    let path_pattern = path_pattern
        .to_str()
        .ok_or_else(|| anyhow!("invalid allowed tool path pattern `{pattern}`"))?;
    let pattern = glob::Pattern::new(path_pattern)
        .map_err(|error| anyhow!("invalid allowed tool path pattern `{pattern}`: {error}"))?;
    Ok(pattern.matches_path_with(path, REQUEST_TOOL_PATH_MATCH_OPTIONS))
}

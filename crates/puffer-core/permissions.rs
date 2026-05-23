use crate::plans::plan_file_path;
use crate::tool_names::canonical_tool_name;
use crate::AppState;
use anyhow::{bail, Result};
pub(crate) mod acl;
pub(crate) mod browser_action;
pub(crate) mod browser_evaluator;
pub(crate) mod browser_grants;
pub(crate) mod browser_policy;
pub(crate) mod browser_target;
pub(crate) mod execution;
pub(crate) mod profile;
#[cfg(test)]
mod tests;

use puffer_config::ConfigPaths;
use puffer_resources::LoadedResources;
use puffer_tools::ToolDefinition;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Component, Path, PathBuf};

pub use self::browser_action::{browser_action_set_for_action, BrowserActionSet};
pub(crate) use self::execution::{DerivedPermissionPolicy, FilesystemPermissionPolicy};
pub use profile::SessionPermissionState;
pub(crate) use profile::{
    build_request_tool_filter, EffectivePermissionProfile, RequestToolFilter,
};

/// Stores persisted workspace permission overrides for tool ids.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct PermissionsSettings {
    #[serde(default)]
    pub(crate) tools: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PermissionsFile {
    #[serde(default)]
    tools: BTreeMap<String, String>,
    #[serde(default)]
    browser: browser_policy::BrowserPolicySettings,
}

/// Stores persisted workspace sandbox preferences.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SandboxSettings {
    pub(crate) mode: String,
    #[serde(default)]
    pub(crate) auto_allow: bool,
    #[serde(default)]
    pub(crate) allow_unsandboxed_fallback: bool,
    #[serde(default)]
    pub(crate) excluded_commands: Vec<String>,
}

impl SandboxSettings {
    /// Builds the default sandbox settings for the active session.
    pub(crate) fn from_mode(mode: &str) -> Self {
        Self {
            mode: mode.to_string(),
            auto_allow: false,
            allow_unsandboxed_fallback: false,
            excluded_commands: Vec::new(),
        }
    }
}

/// Describes how the runtime should handle one tool invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolPermissionBehavior {
    Allow,
    Ask,
    Deny,
}

/// Carries the chosen permission behavior plus an optional explanation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolPermissionDecision {
    pub(crate) behavior: ToolPermissionBehavior,
    pub(crate) reason: Option<String>,
}

/// Carries the effective runtime permission state for one model turn.
#[derive(Debug, Clone)]
pub(crate) struct RuntimePermissionContext {
    permissions: PermissionsSettings,
    acl: acl::ProjectPermissionAcl,
    browser_policy: browser_policy::BrowserPolicySettings,
    sandbox: SandboxSettings,
    profile: EffectivePermissionProfile,
    derived_policy: DerivedPermissionPolicy,
}

/// Aggregates the immutable permission-profile inputs for one model turn.
#[derive(Debug, Clone, Default)]
pub(crate) struct RuntimePermissionInputs {
    pub(crate) request_tool_filter: Option<profile::RequestToolFilter>,
}

impl RuntimePermissionContext {
    /// Returns the normalized permission profile derived from legacy settings.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn effective_profile(&self) -> &profile::EffectivePermissionProfile {
        &self.profile
    }

    /// Returns the executor-facing permission policies derived from the effective profile.
    pub(crate) fn derived_policy(&self) -> &DerivedPermissionPolicy {
        &self.derived_policy
    }

    /// Returns true when the tool should stay visible in the provider tool list.
    pub(crate) fn tool_visible_to_model(&self, definition: &ToolDefinition) -> bool {
        if tool_skips_permission_enforcement(definition) {
            return true;
        }
        if !self.profile.request_allows_definition(definition) {
            return false;
        }
        self.base_decision_for_tool(definition, &Value::Null)
            .behavior
            != ToolPermissionBehavior::Deny
    }

    /// Computes the effective permission decision for one tool invocation.
    pub(crate) fn decision_for_tool_call(
        &self,
        definition: &ToolDefinition,
        input: &Value,
    ) -> ToolPermissionDecision {
        if !self
            .profile
            .request_allows_call(definition, &self.profile.workspace_roots[0], input)
            .unwrap_or(false)
        {
            return ToolPermissionDecision {
                behavior: ToolPermissionBehavior::Deny,
                reason: Some("slash command tool scope denied this tool call".to_string()),
            };
        }
        self.base_decision_for_tool(definition, input)
    }

    fn base_decision_for_tool(
        &self,
        definition: &ToolDefinition,
        input: &Value,
    ) -> ToolPermissionDecision {
        if tool_skips_permission_enforcement(definition) {
            return ToolPermissionDecision {
                behavior: ToolPermissionBehavior::Allow,
                reason: None,
            };
        }
        if definition
            .policy
            .approval_policy
            .as_deref()
            .is_some_and(policy_value_disables_tool)
        {
            return ToolPermissionDecision {
                behavior: ToolPermissionBehavior::Deny,
                reason: Some("tool metadata marks it disabled".to_string()),
            };
        }
        if definition
            .enabled_if
            .as_deref()
            .is_some_and(enabled_if_value_disables_tool)
        {
            return ToolPermissionDecision {
                behavior: ToolPermissionBehavior::Deny,
                reason: Some("tool metadata currently disables it".to_string()),
            };
        }
        if self.profile.grants.allow_all_tools {
            return ToolPermissionDecision {
                behavior: ToolPermissionBehavior::Allow,
                reason: None,
            };
        }
        if let Some(decision) = self.browser_decision_for_tool(&definition.id, input) {
            return decision;
        }
        if let Some(decision) = self.bash_decision_for_tool(definition, input) {
            return decision;
        }
        if self
            .profile
            .grants
            .tool_overrides
            .contains_key(&normalize_tool_id(&definition.id))
            && canonical_tool_name(&definition.id) != "browser"
        {
            return ToolPermissionDecision {
                behavior: ToolPermissionBehavior::Allow,
                reason: None,
            };
        }

        if let Some(policy) = tool_permission_override(&self.permissions, definition) {
            return self.policy_decision(definition, input, policy);
        }

        if let Some(decision) = self.tool_specific_decision(definition, input) {
            return decision;
        }

        let policy = definition
            .policy
            .approval_policy
            .as_deref()
            .unwrap_or("auto");
        self.policy_decision(definition, input, policy)
    }

    fn browser_decision_for_tool(
        &self,
        tool_id: &str,
        input: &Value,
    ) -> Option<ToolPermissionDecision> {
        if browser_action::browser_permission_value_for_tool_call(tool_id, input).is_none() {
            return None;
        }
        let context = self.profile.browser_context_for_tool(tool_id, input);
        if let Some(decision) = self.acl.decision_for_browser_context(&context) {
            return Some(tool_decision_from_acl(decision));
        }
        let evaluation = browser_evaluator::evaluate_browser_permission(
            &self.profile,
            &self.browser_policy,
            &context,
        );
        Some(match evaluation.decision {
            browser_evaluator::BrowserPermissionDecision::Allow => ToolPermissionDecision {
                behavior: ToolPermissionBehavior::Allow,
                reason: None,
            },
            browser_evaluator::BrowserPermissionDecision::Deny => ToolPermissionDecision {
                behavior: ToolPermissionBehavior::Deny,
                reason: evaluation.reason,
            },
            browser_evaluator::BrowserPermissionDecision::Ask => ToolPermissionDecision {
                behavior: ToolPermissionBehavior::Ask,
                reason: evaluation.reason,
            },
        })
    }

    fn bash_decision_for_tool(
        &self,
        definition: &ToolDefinition,
        input: &Value,
    ) -> Option<ToolPermissionDecision> {
        if !tool_matches_any_name(definition, &["Bash", "PowerShell"]) {
            return None;
        }
        let command = input.get("command").and_then(Value::as_str)?.trim();
        if command.is_empty() {
            return Some(ToolPermissionDecision {
                behavior: ToolPermissionBehavior::Ask,
                reason: Some("shell command cannot be empty".to_string()),
            });
        }
        if let Some(decision) = acl::bash_decision_for_input(&self.acl, input) {
            if matches!(decision, acl::AclDecision::Allow(_))
                && acl::bash_command_has_control_operator(command)
            {
                return Some(ToolPermissionDecision {
                    behavior: ToolPermissionBehavior::Ask,
                    reason: Some(
                        "shell command contains shell control or redirection operators".to_string(),
                    ),
                });
            }
            return Some(tool_decision_from_acl(decision));
        }
        if let Some(reason) = shell_sandbox_reason(input, &self.sandbox) {
            return Some(ToolPermissionDecision {
                behavior: ToolPermissionBehavior::Ask,
                reason: Some(reason),
            });
        }
        if let Some(reason) = shell_command_reason(definition, input) {
            return Some(ToolPermissionDecision {
                behavior: ToolPermissionBehavior::Ask,
                reason: Some(reason),
            });
        }
        if acl::bash_command_has_control_operator(command) {
            return Some(ToolPermissionDecision {
                behavior: ToolPermissionBehavior::Ask,
                reason: Some(
                    "shell command contains shell control or redirection operators".to_string(),
                ),
            });
        }
        let argv0 = acl::effective_bash_argv0(command).unwrap_or_else(|| "<unknown>".to_string());
        Some(ToolPermissionDecision {
            behavior: ToolPermissionBehavior::Ask,
            reason: Some(format!("shell command `{argv0}` requires approval")),
        })
    }

    fn policy_decision(
        &self,
        definition: &ToolDefinition,
        input: &Value,
        policy: &str,
    ) -> ToolPermissionDecision {
        match normalize_policy_value(policy).as_str() {
            "deny" | "disabled" => ToolPermissionDecision {
                behavior: ToolPermissionBehavior::Deny,
                reason: Some("workspace permission rule set this tool to deny".to_string()),
            },
            "ask" => ToolPermissionDecision {
                behavior: ToolPermissionBehavior::Ask,
                reason: Some("workspace permission rule requires approval".to_string()),
            },
            "on-request" => {
                if let Some(reason) = self.approval_reason(definition, input) {
                    ToolPermissionDecision {
                        behavior: ToolPermissionBehavior::Ask,
                        reason: Some(reason),
                    }
                } else {
                    ToolPermissionDecision {
                        behavior: ToolPermissionBehavior::Allow,
                        reason: None,
                    }
                }
            }
            _ => ToolPermissionDecision {
                behavior: ToolPermissionBehavior::Allow,
                reason: None,
            },
        }
    }

    fn tool_specific_decision(
        &self,
        definition: &ToolDefinition,
        input: &Value,
    ) -> Option<ToolPermissionDecision> {
        match definition.id.as_str() {
            "Config" => Some(if input.get("value").is_some() {
                ToolPermissionDecision {
                    behavior: ToolPermissionBehavior::Ask,
                    reason: Some("config writes require approval".to_string()),
                }
            } else {
                ToolPermissionDecision {
                    behavior: ToolPermissionBehavior::Allow,
                    reason: None,
                }
            }),
            "AskUserQuestion" => Some(ToolPermissionDecision {
                behavior: ToolPermissionBehavior::Allow,
                reason: None,
            }),
            "WebSearch" => Some(ToolPermissionDecision {
                behavior: ToolPermissionBehavior::Ask,
                reason: Some("web search requires permission".to_string()),
            }),
            "SendMessage" => {
                let target = input.get("to").and_then(Value::as_str).unwrap_or_default();
                if target.starts_with("bridge:") {
                    Some(ToolPermissionDecision {
                        behavior: ToolPermissionBehavior::Ask,
                        reason: Some(
                            "cross-session bridge messages require explicit approval".to_string(),
                        ),
                    })
                } else {
                    Some(ToolPermissionDecision {
                        behavior: ToolPermissionBehavior::Allow,
                        reason: None,
                    })
                }
            }
            "TodoWrite" => Some(ToolPermissionDecision {
                behavior: ToolPermissionBehavior::Allow,
                reason: None,
            }),
            "Agent" => Some(ToolPermissionDecision {
                behavior: ToolPermissionBehavior::Allow,
                reason: None,
            }),
            _ => None,
        }
    }

    /// Enforces the effective permission decision for one tool invocation.
    pub(crate) fn enforce_tool_call(
        &self,
        definition: &ToolDefinition,
        input: &Value,
    ) -> Result<()> {
        let decision = self.decision_for_tool_call(definition, input);
        match decision.behavior {
            ToolPermissionBehavior::Allow => Ok(()),
            ToolPermissionBehavior::Deny => bail!(
                "tool `{}` is denied by permission policy: {}",
                definition.id,
                decision
                    .reason
                    .unwrap_or_else(|| "workspace rule denied it".to_string())
            ),
            ToolPermissionBehavior::Ask => {
                let mut message = format!(
                    "tool `{}` requires approval before execution",
                    definition.id
                );
                if let Some(reason) = decision.reason {
                    let _ = write!(&mut message, ": {reason}");
                }
                let _ = write!(
                    &mut message,
                    ". Use `/permissions allow {}` to allow it for this workspace.",
                    definition.id
                );
                bail!(message)
            }
        }
    }

    fn approval_reason(&self, definition: &ToolDefinition, input: &Value) -> Option<String> {
        if let Some(reason) = shell_sandbox_reason(input, &self.sandbox) {
            return Some(reason);
        }
        if let Some(reason) = shell_command_reason(definition, input) {
            return Some(reason);
        }
        if self.plan_mode_allows_mutation(definition, input) {
            return None;
        }
        if self.profile.plan_mode && tool_mutates_workspace(definition) {
            return Some(format!(
                "plan mode requires approval for mutating tools. Use `ExitPlanMode` before retrying `{}`.",
                definition.id
            ));
        }
        None
    }

    fn plan_mode_allows_mutation(&self, definition: &ToolDefinition, input: &Value) -> bool {
        if !self.profile.plan_mode {
            return false;
        }
        matches!(
            canonical_tool_name(&definition.id).as_str(),
            "write" | "edit"
        ) && tool_targets_active_plan_file(input, self.profile.active_plan_path.as_deref())
    }
}

fn tool_decision_from_acl(decision: acl::AclDecision) -> ToolPermissionDecision {
    match decision {
        acl::AclDecision::Allow(reason) => ToolPermissionDecision {
            behavior: ToolPermissionBehavior::Allow,
            reason: Some(reason),
        },
        acl::AclDecision::Deny(reason) => ToolPermissionDecision {
            behavior: ToolPermissionBehavior::Deny,
            reason: Some(reason),
        },
    }
}

/// Normalizes a tool id so workspace settings can key tools consistently.
pub(crate) fn normalize_tool_id(tool: &str) -> String {
    let trimmed = tool.trim();
    let mut normalized = String::new();
    let mut previous_was_separator = false;
    let mut previous_was_lower_or_digit = false;

    for ch in trimmed.chars() {
        if ch.is_ascii_alphanumeric() {
            if ch.is_ascii_uppercase() && previous_was_lower_or_digit && !normalized.ends_with('_')
            {
                normalized.push('_');
            }
            normalized.push(ch.to_ascii_lowercase());
            previous_was_separator = false;
            previous_was_lower_or_digit = ch.is_ascii_lowercase() || ch.is_ascii_digit();
            continue;
        }

        if !normalized.is_empty() && !previous_was_separator {
            normalized.push('_');
            previous_was_separator = true;
        }
        previous_was_lower_or_digit = false;
    }

    normalized.trim_matches('_').to_string()
}

/// Returns true when the supplied selector names the Browser tool.
pub fn is_browser_tool_selector(tool: &str) -> bool {
    canonical_tool_name(tool) == "browser"
}

fn tool_permission_override<'a>(
    permissions: &'a PermissionsSettings,
    definition: &ToolDefinition,
) -> Option<&'a str> {
    let keys = tool_permission_keys(definition).collect::<Vec<_>>();
    keys.iter()
        .find_map(|key| permissions.tools.get(key).map(String::as_str))
        .or_else(|| {
            permissions.tools.iter().find_map(|(tool, level)| {
                let normalized = normalize_tool_id(tool);
                keys.iter()
                    .any(|key| *key == normalized)
                    .then_some(level.as_str())
            })
        })
}

fn tool_permission_keys(definition: &ToolDefinition) -> impl Iterator<Item = String> + '_ {
    let mut keys = BTreeSet::new();
    for raw in
        std::iter::once(definition.id.as_str()).chain(definition.aliases.iter().map(String::as_str))
    {
        collect_permission_keys(&mut keys, raw);
    }
    for legacy in legacy_permission_aliases(definition) {
        collect_permission_keys(&mut keys, legacy);
    }
    keys.into_iter()
}

fn collect_permission_keys(keys: &mut BTreeSet<String>, raw: &str) {
    let normalized = normalize_tool_id(raw);
    if !normalized.is_empty() {
        keys.insert(normalized);
    }
    let canonical = canonical_tool_name(raw);
    if !canonical.is_empty() {
        keys.insert(canonical);
    }
}

fn legacy_permission_aliases(definition: &ToolDefinition) -> &'static [&'static str] {
    match canonical_tool_name(&definition.id).as_str() {
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

fn tool_matches_any_name(definition: &ToolDefinition, names: &[&str]) -> bool {
    names
        .iter()
        .any(|name| definition.id == *name || definition.aliases.iter().any(|alias| alias == name))
}

/// Renders the default permissions file contents for the loaded tool surface.
pub(crate) fn default_permissions_contents(resources: &LoadedResources) -> String {
    let mut tools = BTreeMap::new();
    for tool in &resources.tools {
        if canonical_tool_name(&tool.value.id) == "browser" {
            continue;
        }
        let key = normalize_tool_id(&tool.value.id);
        let value = tool
            .value
            .approval_policy
            .as_deref()
            .unwrap_or("auto")
            .trim();
        let value = if value.is_empty() { "auto" } else { value };
        tools.insert(key, value.to_string());
    }
    if resources.tools.is_empty() {
        tools.insert("bash".to_string(), "on-request".to_string());
    }
    toml::to_string_pretty(&PermissionsFile {
        tools,
        browser: browser_policy::BrowserPolicySettings::default(),
    })
    .expect("default permissions.toml should serialize")
}

/// Loads or initializes the workspace permissions file.
pub(crate) fn load_or_initialize_permissions(
    path: &Path,
    resources: &LoadedResources,
) -> Result<PermissionsSettings> {
    if path.exists() {
        return load_permissions_settings(path);
    }
    fs::write(path, default_permissions_contents(resources))?;
    load_permissions_settings(path)
}

/// Loads the sandbox settings for runtime evaluation without creating files on disk.
pub(crate) fn load_runtime_sandbox_settings(
    cwd: &Path,
    state: &AppState,
) -> Result<SandboxSettings> {
    let paths = ConfigPaths::discover(cwd);
    let sandbox_path = paths.workspace_config_dir.join("sandbox.toml");
    if sandbox_path.exists() {
        return Ok(toml::from_str(&fs::read_to_string(sandbox_path)?)?);
    }
    Ok(SandboxSettings::from_mode(&state.sandbox_mode))
}

/// Loads the effective permission context for one model turn or tool invocation.
pub(crate) fn load_runtime_permission_context(
    cwd: &Path,
    _resources: &LoadedResources,
    state: &AppState,
) -> Result<RuntimePermissionContext> {
    load_runtime_permission_context_with_inputs(
        cwd,
        _resources,
        state,
        RuntimePermissionInputs::default(),
    )
}

/// Loads the effective permission context for one model turn with request-scoped
/// inputs.
pub(crate) fn load_runtime_permission_context_with_inputs(
    cwd: &Path,
    _resources: &LoadedResources,
    state: &AppState,
    inputs: RuntimePermissionInputs,
) -> Result<RuntimePermissionContext> {
    let paths = ConfigPaths::discover(cwd);
    let permissions_path = paths.workspace_config_dir.join("permissions.toml");
    let active_plan_path = state.plan_mode.then(|| plan_file_path(state)).transpose()?;
    let loaded_permissions = if permissions_path.exists() {
        load_permissions_file(&permissions_path)?
    } else {
        PermissionsFile::default()
    };
    let permissions = PermissionsSettings {
        tools: loaded_permissions.tools,
    };
    let browser_policy = loaded_permissions.browser;
    let sandbox = load_runtime_sandbox_settings(cwd, state)?;
    let acl = acl::ProjectPermissionAcl::load(cwd)?;
    let browser_root_session_id = state.browser_root_session_id();
    let profile = EffectivePermissionProfile::from_session_state(
        cwd,
        &state.working_dirs,
        &permissions,
        &sandbox,
        &browser_root_session_id,
        state.session_permission_state(),
        state.plan_mode,
        active_plan_path.clone(),
        inputs.request_tool_filter,
    );
    let derived_policy = profile.derived_policy();
    Ok(RuntimePermissionContext {
        permissions,
        acl,
        browser_policy,
        sandbox,
        profile,
        derived_policy,
    })
}
fn tool_targets_active_plan_file(input: &Value, active_plan_path: Option<&Path>) -> bool {
    let Some(active_plan_path) = active_plan_path else {
        return false;
    };
    let Some(raw_path) = input.get("file_path").and_then(Value::as_str) else {
        return false;
    };
    normalize_permission_path(raw_path)
        .is_some_and(|tool_path| tool_path == normalize_filesystem_path(active_plan_path))
}
fn normalize_permission_path(raw_path: &str) -> Option<PathBuf> {
    let expanded = if raw_path == "~" {
        std::env::var_os("HOME").map(PathBuf::from)?
    } else if let Some(suffix) = raw_path
        .strip_prefix("~/")
        .or_else(|| raw_path.strip_prefix("~\\"))
    {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join(suffix))?
    } else {
        PathBuf::from(raw_path)
    };
    Some(normalize_filesystem_path(&expanded))
}
fn normalize_filesystem_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                let _ = normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

/// Writes the permissions file to disk.
pub(crate) fn write_permissions(path: &Path, settings: &PermissionsSettings) -> Result<()> {
    let browser = if path.exists() {
        load_permissions_file(path)?.browser
    } else {
        browser_policy::BrowserPolicySettings::default()
    };
    write_permissions_with_browser(path, settings, &browser)
}

fn normalize_policy_value(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn load_permissions_settings(path: &Path) -> Result<PermissionsSettings> {
    Ok(PermissionsSettings {
        tools: load_permissions_file(path)?.tools,
    })
}

fn load_permissions_file(path: &Path) -> Result<PermissionsFile> {
    let loaded: PermissionsFile = toml::from_str(&fs::read_to_string(path)?)?;
    Ok(PermissionsFile {
        tools: loaded
            .tools
            .into_iter()
            .filter(|(tool, _)| canonical_tool_name(tool) != "browser")
            .map(|(tool, level)| (normalize_tool_id(&tool), normalize_policy_value(&level)))
            .collect(),
        browser: loaded.browser.normalized(),
    })
}

fn write_permissions_with_browser(
    path: &Path,
    settings: &PermissionsSettings,
    browser: &browser_policy::BrowserPolicySettings,
) -> Result<()> {
    fs::write(
        path,
        toml::to_string_pretty(&PermissionsFile {
            tools: settings.tools.clone(),
            browser: browser.clone(),
        })?,
    )?;
    Ok(())
}

fn policy_value_disables_tool(value: &str) -> bool {
    matches!(normalize_policy_value(value).as_str(), "disabled" | "deny")
}

fn enabled_if_value_disables_tool(value: &str) -> bool {
    matches!(
        normalize_policy_value(value).as_str(),
        "0" | "disabled" | "deny" | "false" | "never" | "off"
    )
}

fn tool_mutates_workspace(definition: &ToolDefinition) -> bool {
    definition.metadata.may_write_files
        || definition.metadata.may_spawn_processes
        || definition.policy.sandbox_policy.as_deref() == Some("workspace-write")
}

fn tool_skips_permission_enforcement(definition: &ToolDefinition) -> bool {
    tool_matches_any_name(definition, &["SendUserMessage", "Brief"])
}

fn shell_sandbox_reason(input: &Value, sandbox: &SandboxSettings) -> Option<String> {
    let command = input.get("command").and_then(Value::as_str)?;
    if let Some(pattern) = sandbox
        .excluded_commands
        .iter()
        .find(|pattern| !pattern.trim().is_empty() && command.contains(pattern.as_str()))
    {
        if sandbox.allow_unsandboxed_fallback {
            return None;
        }
        return Some(format!(
            "shell command matches project shell exclusion `{}`",
            pattern.trim()
        ));
    }
    None
}

fn shell_command_reason(definition: &ToolDefinition, input: &Value) -> Option<String> {
    if !tool_matches_any_name(definition, &["Bash", "PowerShell"]) {
        return None;
    }
    let command = input.get("command").and_then(Value::as_str)?.trim();
    if command.is_empty() {
        return Some("shell command cannot be empty".to_string());
    }
    let normalized = command.to_ascii_lowercase();
    if normalized.contains("rm -rf /")
        || normalized.contains("rm -fr /")
        || normalized.contains("rm -rf ~")
        || normalized.contains("rm -fr ~")
        || normalized.contains("rm -rf \"$home\"")
        || normalized.contains("rm -rf $home")
        || normalized.contains("mkfs.")
        || normalized.contains("shutdown ")
        || normalized == "shutdown"
        || normalized.contains("reboot")
        || normalized.contains("poweroff")
        || normalized.contains("halt")
    {
        return Some("shell command looks dangerously destructive".to_string());
    }
    if (normalized.contains("curl ") || normalized.contains("wget "))
        && (normalized.contains("| sh")
            || normalized.contains("| bash")
            || normalized.contains("| zsh")
            || normalized.contains("| pwsh")
            || normalized.contains("| powershell"))
    {
        return Some("shell command pipes downloaded content directly into a shell".to_string());
    }
    None
}

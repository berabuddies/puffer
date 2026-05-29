use super::browser_target::{BrowserActionCategory, BrowserPermissionContext};
use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use serde_json::Value;
use std::cmp::Ordering;
use std::fs;
use std::path::{Component, Path, PathBuf};
use url::Url;

const ACL_FILE_NAME: &str = "permissions.acl";
const DEFAULT_ALLOW_PRIORITY: i32 = 100;
const ALWAYS_ALLOW_PRIORITY: i32 = 900;

/// Describes one ACL decision after project and priority matching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AclDecision {
    Allow(String),
    Deny(String),
}

/// Describes whether a filesystem rule is for reads or writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FilesystemAccessKind {
    Read,
    Write,
}

/// Stores project-scoped ACL rules loaded from `.puffer/permissions.acl`.
#[derive(Debug, Clone)]
pub(crate) struct ProjectPermissionAcl {
    cwd: PathBuf,
    rules: Vec<AclRule>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AclRule {
    priority: i32,
    order: usize,
    project_root: PathBuf,
    effect: AclEffect,
    scope: AclScope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AclEffect {
    Allow,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AclScope {
    All,
    Filesystem {
        access: FilesystemAccessKind,
        path_kind: AclPathKind,
        path: PathBuf,
    },
    BashArgv(String),
    BrowserDomain(String),
    BrowserAction {
        action: BrowserActionCategory,
        domain: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AclPathKind {
    File,
    Dir,
}

impl ProjectPermissionAcl {
    /// Loads the project ACL for the current workspace without creating it.
    pub(crate) fn load(cwd: &Path) -> Result<Self> {
        let path = acl_path(cwd);
        let contents = if path.exists() {
            fs::read_to_string(&path)
                .with_context(|| format!("failed to read permission ACL {}", path.display()))?
        } else {
            String::new()
        };
        Ok(Self::parse(cwd, &contents))
    }

    /// Builds an in-memory ACL from raw file contents.
    pub(crate) fn parse(cwd: &Path, contents: &str) -> Self {
        let cwd = normalize_path(cwd);
        let mut project_root = cwd.clone();
        let mut rules = Vec::new();
        let mut order = 0usize;
        for raw_line in contents.lines() {
            let line = strip_comment(raw_line).trim();
            if line.is_empty() {
                continue;
            }
            if let Some(section) = parse_project_section(line, &cwd) {
                project_root = section;
                continue;
            }
            if let Some(mut rule) = parse_rule_line(line, &project_root) {
                rule.order = order;
                order += 1;
                rules.push(rule);
            }
        }
        Self { cwd, rules }
    }

    /// Evaluates a filesystem path against the ACL.
    pub(crate) fn decision_for_path(
        &self,
        access: FilesystemAccessKind,
        path: &Path,
    ) -> Option<AclDecision> {
        let path = normalize_path(path);
        self.best_matching_rule(|rule| match &rule.scope {
            AclScope::All => true,
            AclScope::Filesystem {
                access: rule_access,
                path_kind,
                path: rule_path,
            } => {
                *rule_access == access
                    && match path_kind {
                        AclPathKind::File => path == *rule_path,
                        AclPathKind::Dir => path == *rule_path || path.starts_with(rule_path),
                    }
            }
            _ => false,
        })
    }

    /// Evaluates a shell command against argv[0] ACL rules and built-in safe commands.
    pub(crate) fn decision_for_bash_command(&self, command: &str) -> Option<AclDecision> {
        let argv0 = effective_bash_argv0(command)?;
        if let Some(decision) = self.best_matching_rule(|rule| match &rule.scope {
            AclScope::All => true,
            AclScope::BashArgv(candidate) => candidate == &argv0,
            _ => false,
        }) {
            return Some(decision);
        }
        bash_argv_is_preapproved(&argv0).then(|| {
            AclDecision::Allow(format!(
                "shell command `{argv0}` is preapproved for this project"
            ))
        })
    }

    /// Evaluates a Browser context against domain/action ACL rules.
    pub(crate) fn decision_for_browser_context(
        &self,
        context: &BrowserPermissionContext,
    ) -> Option<AclDecision> {
        let target = context.target.as_ref()?;
        let action = context.action?;
        let domain = target
            .registrable_domain
            .clone()
            .or_else(|| target.host.clone())
            .map(|value| value.trim_end_matches('.').to_ascii_lowercase())?;
        self.best_matching_rule(|rule| match &rule.scope {
            AclScope::All => true,
            AclScope::BrowserDomain(candidate) => domain_matches(&domain, candidate),
            AclScope::BrowserAction {
                action: rule_action,
                domain: candidate,
            } => *rule_action == action && domain_matches(&domain, candidate),
            _ => false,
        })
    }

    fn best_matching_rule(&self, matches: impl Fn(&AclRule) -> bool) -> Option<AclDecision> {
        self.rules
            .iter()
            .filter(|rule| self.cwd.starts_with(&rule.project_root) && matches(rule))
            .max_by(compare_rules)
            .map(|rule| match rule.effect {
                AclEffect::Allow => AclDecision::Allow(rule.describe()),
                AclEffect::Deny => AclDecision::Deny(rule.describe()),
            })
    }
}

impl AclRule {
    fn describe(&self) -> String {
        match &self.scope {
            AclScope::All => "project ACL applies to all actions".to_string(),
            AclScope::Filesystem {
                access,
                path_kind,
                path,
            } => format!(
                "project ACL {:?} {:?} rule matches {}",
                access,
                path_kind,
                path.display()
            ),
            AclScope::BashArgv(argv0) => {
                format!("project ACL shell command `{argv0}` rule matches")
            }
            AclScope::BrowserDomain(domain) => {
                format!("project ACL browser domain `{domain}` rule matches")
            }
            AclScope::BrowserAction { action, domain } => {
                format!(
                    "project ACL browser {:?} on `{domain}` rule matches",
                    action
                )
            }
        }
    }
}

/// Returns the ACL file path for the current workspace.
pub(crate) fn acl_path(cwd: &Path) -> PathBuf {
    ConfigPaths::discover(cwd)
        .workspace_config_dir
        .join(ACL_FILE_NAME)
}

/// Appends an always-allow filesystem ACL rule for a file or directory.
pub(crate) fn append_allow_path_rule(
    cwd: &Path,
    access: FilesystemAccessKind,
    path: &Path,
    path_kind: FilesystemAclPathKind,
) -> Result<()> {
    let kind = match path_kind {
        FilesystemAclPathKind::File => "file",
        FilesystemAclPathKind::Dir => "dir",
    };
    append_acl_line(
        cwd,
        &format!(
            "{DEFAULT_ALLOW_PRIORITY} Allow {} {kind} {}",
            access.rule_token(),
            quote_acl_token(&normalize_path(path).display().to_string())
        ),
    )
}

/// Appends an always-allow shell command ACL rule for the effective command.
pub(crate) fn append_allow_bash_rule(cwd: &Path, command: &str) -> Result<()> {
    let Some(argv0) = effective_bash_argv0(command) else {
        return Ok(());
    };
    append_acl_line(
        cwd,
        &format!(
            "{DEFAULT_ALLOW_PRIORITY} Allow bash command {}",
            quote_acl_token(&argv0)
        ),
    )
}

/// Appends an always-allow Browser ACL rule for the target domain and action.
pub(crate) fn append_allow_browser_rule(
    cwd: &Path,
    context: &BrowserPermissionContext,
) -> Result<()> {
    let Some(target) = &context.target else {
        return Ok(());
    };
    let Some(domain) = target
        .registrable_domain
        .as_deref()
        .or(target.host.as_deref())
        .map(|value| value.trim_end_matches('.').to_ascii_lowercase())
    else {
        return Ok(());
    };
    if let Some(action) = context.action {
        append_acl_line(
            cwd,
            &format!(
                "{DEFAULT_ALLOW_PRIORITY} Allow browser action {} {}",
                browser_action_token(action),
                quote_acl_token(&domain)
            ),
        )
    } else {
        append_acl_line(
            cwd,
            &format!(
                "{DEFAULT_ALLOW_PRIORITY} Allow browser domain {}",
                quote_acl_token(&domain)
            ),
        )
    }
}

/// Appends an always-allow ACL rule for every project action.
pub(crate) fn append_allow_all_rule(cwd: &Path) -> Result<()> {
    append_acl_line(cwd, &format!("{ALWAYS_ALLOW_PRIORITY} Allow all"))
}

/// Classifies whether a persisted filesystem ACL rule targets a file or directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FilesystemAclPathKind {
    File,
    Dir,
}

impl FilesystemAccessKind {
    fn rule_token(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
        }
    }
}

/// Returns the effective argv[0] for a shell command, unwrapping known shell `-c` forms.
pub(crate) fn effective_bash_argv0(command: &str) -> Option<String> {
    let tokens = shell_words::split(command).ok()?;
    effective_argv0_from_tokens(&tokens, 0)
}

/// Returns true when a command contains shell control or redirection operators.
pub(crate) fn bash_command_has_control_operator(command: &str) -> bool {
    shell_words::split(command).map_or(true, |tokens| {
        tokens.iter().any(|token| {
            matches!(
                token.as_str(),
                "|" | "||"
                    | "&&"
                    | ";"
                    | "&"
                    | ">"
                    | ">>"
                    | "<"
                    | "<<"
                    | "2>"
                    | "2>>"
                    | "&>"
                    | ">&"
            )
        })
    })
}

/// Returns true when the argv[0] is safe enough to run by default.
pub(crate) fn bash_argv_is_preapproved(argv0: &str) -> bool {
    matches!(
        argv0,
        "awk"
            | "basename"
            | "cat"
            | "cut"
            | "dirname"
            | "echo"
            | "false"
            | "file"
            | "grep"
            | "head"
            | "ls"
            | "printf"
            | "pwd"
            | "rg"
            | "sed"
            | "sort"
            | "stat"
            | "tail"
            | "test"
            | "tr"
            | "true"
            | "uniq"
            | "wc"
    )
}

fn append_acl_line(cwd: &Path, rule: &str) -> Result<()> {
    let path = acl_path(cwd);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create ACL directory {}", parent.display()))?;
    }
    let mut contents = if path.exists() {
        fs::read_to_string(&path)
            .with_context(|| format!("failed to read permission ACL {}", path.display()))?
    } else {
        format!(
            "# Puffer project permission ACL\n# Format: <priority> Allow|Deny <action> <params>\n[project workdir]\n"
        )
    };
    if !contents.ends_with('\n') {
        contents.push('\n');
    }
    if !contents.contains("[project workdir]") {
        contents.push_str("[project workdir]\n");
    }
    contents.push_str(rule);
    contents.push('\n');
    fs::write(&path, contents)
        .with_context(|| format!("failed to write permission ACL {}", path.display()))
}

fn parse_rule_line(line: &str, project_root: &Path) -> Option<AclRule> {
    let tokens = shell_words::split(line).ok()?;
    let (priority, effect, rest) = parse_rule_prefix(&tokens)?;
    let scope = parse_acl_scope(rest, project_root)?;
    Some(AclRule {
        priority,
        order: 0,
        project_root: normalize_path(project_root),
        effect,
        scope,
    })
}

fn parse_rule_prefix(tokens: &[String]) -> Option<(i32, AclEffect, &[String])> {
    let priority = tokens.first()?.parse().ok()?;
    let effect = match tokens.get(1)?.to_ascii_lowercase().as_str() {
        "allow" => AclEffect::Allow,
        "deny" => AclEffect::Deny,
        _ => return None,
    };
    Some((priority, effect, &tokens[2..]))
}

fn parse_acl_scope(tokens: &[String], project_root: &Path) -> Option<AclScope> {
    let action = tokens.first()?.to_ascii_lowercase();
    match action.as_str() {
        "all" => Some(AclScope::All),
        "read" | "write" => parse_filesystem_scope(tokens, project_root),
        "bash" => parse_bash_scope(tokens),
        "browser" => parse_browser_scope(tokens),
        _ => None,
    }
}

fn parse_filesystem_scope(tokens: &[String], project_root: &Path) -> Option<AclScope> {
    let access = match tokens.first()?.to_ascii_lowercase().as_str() {
        "read" => FilesystemAccessKind::Read,
        "write" => FilesystemAccessKind::Write,
        _ => return None,
    };
    let path_kind = match tokens.get(1)?.to_ascii_lowercase().as_str() {
        "file" => AclPathKind::File,
        "dir" | "directory" => AclPathKind::Dir,
        _ => return None,
    };
    let path = resolve_acl_path(project_root, tokens.get(2)?);
    Some(AclScope::Filesystem {
        access,
        path_kind,
        path,
    })
}

fn parse_bash_scope(tokens: &[String]) -> Option<AclScope> {
    let argv = match tokens.get(1).map(|value| value.to_ascii_lowercase()) {
        Some(token) if token == "argv" || token == "command" => tokens.get(2)?,
        Some(_) => tokens.get(1)?,
        None => return None,
    };
    let argv0 = command_basename(argv)?.to_ascii_lowercase();
    Some(AclScope::BashArgv(argv0))
}

fn parse_browser_scope(tokens: &[String]) -> Option<AclScope> {
    match tokens
        .get(1)
        .map(|value| value.to_ascii_lowercase())
        .as_deref()
    {
        Some("domain" | "site") => Some(AclScope::BrowserDomain(normalize_domain(tokens.get(2)?))),
        Some("action") => {
            let action = parse_browser_action(tokens.get(2)?)?;
            Some(AclScope::BrowserAction {
                action,
                domain: normalize_domain(tokens.get(3)?),
            })
        }
        Some(_) => Some(AclScope::BrowserDomain(normalize_domain(tokens.get(1)?))),
        None => None,
    }
}

fn parse_project_section(line: &str, cwd: &Path) -> Option<PathBuf> {
    let inner = line.strip_prefix('[')?.strip_suffix(']')?.trim();
    let mut parts = inner.split_whitespace();
    if !parts.next()?.eq_ignore_ascii_case("project") {
        return None;
    }
    let root = parts.collect::<Vec<_>>().join(" ");
    if root.eq_ignore_ascii_case("workdir") || root.is_empty() {
        return Some(normalize_path(cwd));
    }
    Some(resolve_acl_path(cwd, &root))
}

fn parse_browser_action(action: &str) -> Option<BrowserActionCategory> {
    match action
        .trim()
        .replace(['-', '_'], "")
        .to_ascii_lowercase()
        .as_str()
    {
        "inspect" | "list" | "snapshot" | "screenshot" | "dominspect" | "networkidle"
        | "waitnetworkidle" => Some(BrowserActionCategory::Inspect),
        "navigate" | "open" | "new" | "focus" | "close" | "reload" | "back" | "forward" => {
            Some(BrowserActionCategory::Navigate)
        }
        "interact" | "click" | "dblclick" | "hover" | "type" | "fill" | "select" | "upload"
        | "check" | "uncheck" | "press" | "scroll" | "scrollintoview" => {
            Some(BrowserActionCategory::Interact)
        }
        "evaluate" | "eval" => Some(BrowserActionCategory::Evaluate),
        _ => None,
    }
}

fn browser_action_token(action: BrowserActionCategory) -> &'static str {
    match action {
        BrowserActionCategory::Inspect => "inspect",
        BrowserActionCategory::Navigate => "navigate",
        BrowserActionCategory::Interact => "interact",
        BrowserActionCategory::Evaluate => "evaluate",
    }
}

fn effective_argv0_from_tokens(tokens: &[String], depth: usize) -> Option<String> {
    if depth > 8 {
        return None;
    }
    let command = tokens.first()?;
    let argv0 = command_basename(command)?.to_ascii_lowercase();
    if is_env_wrapper(&argv0) {
        return effective_argv0_from_tokens(skip_env_assignments(&tokens[1..]), depth + 1);
    }
    if matches!(argv0.as_str(), "command" | "exec") {
        return effective_argv0_from_tokens(&tokens[1..], depth + 1);
    }
    if is_known_shell(&argv0) {
        if let Some(script) = shell_inline_script(tokens) {
            return effective_bash_argv0(script);
        }
    }
    Some(argv0)
}

fn shell_inline_script(tokens: &[String]) -> Option<&str> {
    let mut index = 1usize;
    while index < tokens.len() {
        let token = tokens[index].as_str();
        let normalized = token.to_ascii_lowercase();
        if matches!(normalized.as_str(), "-c" | "--command" | "/c") {
            return tokens.get(index + 1).map(String::as_str);
        }
        if normalized.starts_with('-') && normalized.contains('c') {
            return tokens.get(index + 1).map(String::as_str);
        }
        if normalized.starts_with('-') {
            index += 1;
            continue;
        }
        break;
    }
    None
}

fn is_known_shell(argv0: &str) -> bool {
    matches!(
        argv0,
        "bash"
            | "cmd"
            | "cmd.exe"
            | "dash"
            | "fish"
            | "ksh"
            | "pwsh"
            | "powershell"
            | "powershell.exe"
            | "sh"
            | "zsh"
    )
}

fn is_env_wrapper(argv0: &str) -> bool {
    argv0 == "env" || argv0 == "usr/bin/env"
}

fn skip_env_assignments(tokens: &[String]) -> &[String] {
    let mut index = 0usize;
    while index < tokens.len() {
        let token = tokens[index].as_str();
        if token.starts_with('-') {
            index += if env_option_takes_value(token) { 2 } else { 1 };
            continue;
        }
        if token.contains('=') && !token.starts_with('=') {
            index += 1;
            continue;
        }
        break;
    }
    &tokens[index.min(tokens.len())..]
}

fn env_option_takes_value(option: &str) -> bool {
    matches!(option, "-S" | "-u" | "--unset")
}

fn command_basename(command: &str) -> Option<&str> {
    Path::new(command)
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
}

fn resolve_acl_path(project_root: &Path, raw: &str) -> PathBuf {
    let expanded = expand_tilde(raw).unwrap_or_else(|| PathBuf::from(raw));
    let joined = if expanded.is_absolute() {
        expanded
    } else {
        project_root.join(expanded)
    };
    normalize_path(&joined)
}

fn expand_tilde(raw: &str) -> Option<PathBuf> {
    if raw == "~" {
        return std::env::var_os("HOME").map(PathBuf::from);
    }
    raw.strip_prefix("~/")
        .or_else(|| raw.strip_prefix("~\\"))
        .and_then(|suffix| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(suffix)))
}

fn normalize_path(path: &Path) -> PathBuf {
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

fn normalize_domain(raw: &str) -> String {
    let trimmed = raw.trim().trim_end_matches('.').to_ascii_lowercase();
    if let Ok(parsed) = Url::parse(&trimmed) {
        if let Some(host) = parsed.host_str() {
            return registrable_or_host(host);
        }
    }
    if let Ok(parsed) = Url::parse(&format!("https://{trimmed}")) {
        if let Some(host) = parsed.host_str() {
            return registrable_or_host(host);
        }
    }
    trimmed
}

fn registrable_or_host(host: &str) -> String {
    if host.eq_ignore_ascii_case("localhost")
        || host.ends_with(".localhost")
        || host.parse::<std::net::IpAddr>().is_ok()
    {
        return host.trim_end_matches('.').to_ascii_lowercase();
    }
    psl::domain_str(host)
        .map(|domain| domain.trim_end_matches('.').to_ascii_lowercase())
        .unwrap_or_else(|| host.trim_end_matches('.').to_ascii_lowercase())
}

fn domain_matches(actual: &str, rule: &str) -> bool {
    actual.eq_ignore_ascii_case(rule) || actual.ends_with(&format!(".{rule}"))
}

fn strip_comment(line: &str) -> &str {
    line.split_once('#')
        .map(|(prefix, _)| prefix)
        .unwrap_or(line)
}

fn quote_acl_token(value: &str) -> String {
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | ':'))
    {
        value.to_string()
    } else {
        format!("{value:?}")
    }
}

fn compare_rules(left: &&AclRule, right: &&AclRule) -> Ordering {
    left.priority
        .cmp(&right.priority)
        .then_with(|| left.order.cmp(&right.order))
}

fn parse_bash_decision(command: &str, acl: &ProjectPermissionAcl) -> Option<AclDecision> {
    acl.decision_for_bash_command(command)
}

/// Returns the project ACL decision for a tool-call JSON command payload.
pub(crate) fn bash_decision_for_input(
    acl: &ProjectPermissionAcl,
    input: &Value,
) -> Option<AclDecision> {
    input
        .get("command")
        .and_then(Value::as_str)
        .and_then(|command| parse_bash_decision(command, acl))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::browser_target::browser_permission_context_for_tool;
    use serde_json::json;

    #[test]
    fn unwraps_known_shell_c_forms_to_effective_argv0() {
        assert_eq!(
            effective_bash_argv0("bash -c 'ls -la'").as_deref(),
            Some("ls")
        );
        assert_eq!(
            effective_bash_argv0("zsh -lc 'git status'").as_deref(),
            Some("git")
        );
        assert_eq!(
            effective_bash_argv0("pwsh -Command 'cat Cargo.toml'").as_deref(),
            Some("cat")
        );
        assert_eq!(
            effective_bash_argv0("cmd.exe /C echo hi").as_deref(),
            Some("echo")
        );
    }

    #[test]
    fn bash_defaults_allow_only_preapproved_argv0s() {
        let acl = ProjectPermissionAcl::parse(Path::new("/repo"), "");
        assert!(matches!(
            acl.decision_for_bash_command("ls -la"),
            Some(AclDecision::Allow(_))
        ));
        assert_eq!(acl.decision_for_bash_command("git status"), None);
    }

    #[test]
    fn acl_rule_priority_and_later_tie_breaker_apply() {
        let acl = ProjectPermissionAcl::parse(
            Path::new("/repo"),
            "[project workdir]\n100 Allow bash command git\n100 Deny bash command git\n",
        );
        assert!(matches!(
            acl.decision_for_bash_command("git status"),
            Some(AclDecision::Deny(_))
        ));
    }

    #[test]
    fn path_rules_match_file_and_directory_scopes() {
        let acl = ProjectPermissionAcl::parse(
            Path::new("/repo"),
            "[project workdir]\n100 Allow read dir ../shared\n100 Deny write file /repo/locked.txt\n",
        );
        assert!(matches!(
            acl.decision_for_path(FilesystemAccessKind::Read, Path::new("/shared/a.txt")),
            Some(AclDecision::Allow(_))
        ));
        assert!(matches!(
            acl.decision_for_path(FilesystemAccessKind::Write, Path::new("/repo/locked.txt")),
            Some(AclDecision::Deny(_))
        ));
    }

    #[test]
    fn browser_rules_match_current_target_domain_and_action() {
        let acl = ProjectPermissionAcl::parse(
            Path::new("/repo"),
            "[project workdir]\n100 Allow browser action navigate google.com\n100 Deny browser action interact baidu.com\n",
        );
        let google = browser_permission_context_for_tool(
            "Browser",
            &json!({"action":"navigate","url":"https://www.google.com/search?q=x"}),
            "current",
            &[PathBuf::from("/repo")],
        );
        let baidu = browser_permission_context_for_tool(
            "Browser",
            &json!({"action":"click","url":"https://www.baidu.com/"}),
            "current",
            &[PathBuf::from("/repo")],
        );
        assert!(matches!(
            acl.decision_for_browser_context(&google),
            Some(AclDecision::Allow(_))
        ));
        assert!(matches!(
            acl.decision_for_browser_context(&baidu),
            Some(AclDecision::Deny(_))
        ));
    }
}

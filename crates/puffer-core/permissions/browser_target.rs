use super::browser_action::{
    browser_action_set_from_value, browser_permission_value_for_tool_call,
};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use url::{Host, Url};

/// Groups Browser intents into stable permission-facing action sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum BrowserActionCategory {
    Inspect,
    Navigate,
    Interact,
    Evaluate,
}

/// Classifies the Browser target into coarse permission-relevant buckets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum BrowserTargetClass {
    LocalDev,
    WorkspaceFile,
    NonWorkspaceFile,
    DataUrl,
    OpenWeb,
}

impl BrowserTargetClass {
    /// Returns the stable snake_case slug used by Browser policy files.
    pub(crate) fn as_slug(self) -> &'static str {
        match self {
            Self::LocalDev => "local_dev",
            Self::WorkspaceFile => "workspace_file",
            Self::NonWorkspaceFile => "non_workspace_file",
            Self::DataUrl => "data_url",
            Self::OpenWeb => "open_web",
        }
    }
}

/// Carries the structured Browser target metadata derived from one tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BrowserTarget {
    pub(crate) raw_url: String,
    pub(crate) scheme: String,
    pub(crate) origin: Option<String>,
    pub(crate) host: Option<String>,
    pub(crate) registrable_domain: Option<String>,
    pub(crate) port: Option<u16>,
    pub(crate) target_class: BrowserTargetClass,
}

/// Carries the normalized Browser permission context for one tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BrowserPermissionContext {
    pub(crate) action: Option<BrowserActionCategory>,
    pub(crate) target: Option<BrowserTarget>,
    pub(crate) tab_id: Option<String>,
    pub(crate) root_session_id: String,
}

/// Builds the structured Browser permission context for one tool call.
pub(crate) fn browser_permission_context(
    input: &Value,
    current_session_id: &str,
    workspace_roots: &[PathBuf],
) -> BrowserPermissionContext {
    browser_permission_context_for_tool("Browser", input, current_session_id, workspace_roots)
}

/// Builds the structured Browser permission context for one permission-bearing tool call.
pub(crate) fn browser_permission_context_for_tool(
    tool_id: &str,
    input: &Value,
    current_session_id: &str,
    workspace_roots: &[PathBuf],
) -> BrowserPermissionContext {
    let browser_input =
        browser_permission_value_for_tool_call(tool_id, input).unwrap_or(Value::Null);
    let target_input = if browser_input.is_null() {
        input
    } else {
        &browser_input
    };
    let root_session_id = browser_root_session_id(target_input, current_session_id);
    BrowserPermissionContext {
        action: browser_action_set_from_value(target_input).map(action_set_category),
        target: browser_target(target_input, workspace_roots),
        tab_id: string_field(target_input, "tabId")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string),
        root_session_id,
    }
}

fn string_field<'a>(input: &'a Value, key: &str) -> Option<&'a str> {
    input.get(key).and_then(Value::as_str)
}

fn browser_target(input: &Value, workspace_roots: &[PathBuf]) -> Option<BrowserTarget> {
    let raw_url = string_field(input, "url")
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)?;
    let normalized_url = normalize_browser_url(&raw_url);
    let Ok(parsed) = Url::parse(&normalized_url) else {
        return Some(BrowserTarget {
            raw_url,
            scheme: normalized_url
                .split(':')
                .next()
                .unwrap_or_default()
                .to_ascii_lowercase(),
            origin: None,
            host: None,
            registrable_domain: None,
            port: None,
            target_class: BrowserTargetClass::OpenWeb,
        });
    };
    let host = parsed.host_str().map(|value| value.to_ascii_lowercase());
    let scheme = parsed.scheme().to_ascii_lowercase();
    let target_class = classify_target(&parsed, workspace_roots);
    let origin =
        matches!(scheme.as_str(), "http" | "https").then(|| parsed.origin().ascii_serialization());
    let registrable_domain = host.as_deref().and_then(registrable_domain);
    Some(BrowserTarget {
        raw_url,
        scheme,
        origin,
        host,
        registrable_domain,
        port: parsed.port_or_known_default(),
        target_class,
    })
}

fn normalize_browser_url(raw_url: &str) -> String {
    let trimmed = raw_url.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("about:blank") {
        return "about:blank".to_string();
    }
    if let Ok(parsed) = Url::parse(trimmed) {
        if matches!(
            parsed.scheme(),
            "http" | "https" | "file" | "data" | "about"
        ) {
            return trimmed.to_string();
        }
    }
    if trimmed.starts_with("localhost")
        || trimmed.starts_with("127.")
        || trimmed.starts_with("[::1]")
        || trimmed.starts_with("0.0.0.0")
        || trimmed.starts_with("[::]")
    {
        format!("http://{trimmed}")
    } else {
        format!("https://{trimmed}")
    }
}

fn classify_target(parsed: &Url, workspace_roots: &[PathBuf]) -> BrowserTargetClass {
    match parsed.scheme() {
        "data" => BrowserTargetClass::DataUrl,
        "file" => classify_file_target(parsed, workspace_roots),
        // Treat `about:blank` as open-web ambient browser state instead of
        // introducing a separate target class that would complicate future
        // origin-aware rules.
        "about" if parsed.path().eq_ignore_ascii_case("blank") => BrowserTargetClass::OpenWeb,
        "http" | "https" if is_local_dev_host(parsed.host()) => BrowserTargetClass::LocalDev,
        _ => BrowserTargetClass::OpenWeb,
    }
}

fn classify_file_target(parsed: &Url, workspace_roots: &[PathBuf]) -> BrowserTargetClass {
    let Ok(path) = parsed.to_file_path() else {
        return BrowserTargetClass::NonWorkspaceFile;
    };
    let Ok(canonical_path) = fs::canonicalize(&path) else {
        return BrowserTargetClass::NonWorkspaceFile;
    };
    if workspace_roots
        .iter()
        .any(|root| path_is_within(&canonical_path, root))
    {
        BrowserTargetClass::WorkspaceFile
    } else {
        BrowserTargetClass::NonWorkspaceFile
    }
}

fn path_is_within(path: &Path, root: &Path) -> bool {
    path == root || path.starts_with(root)
}

fn is_local_dev_host(host: Option<Host<&str>>) -> bool {
    match host {
        Some(Host::Domain(domain)) => {
            domain.eq_ignore_ascii_case("localhost") || domain.ends_with(".localhost")
        }
        Some(Host::Ipv4(address)) => address.is_loopback() || address.is_unspecified(),
        Some(Host::Ipv6(address)) => address.is_loopback() || address.is_unspecified(),
        None => false,
    }
}

fn registrable_domain(host: &str) -> Option<String> {
    if host.eq_ignore_ascii_case("localhost")
        || host.ends_with(".localhost")
        || host.parse::<std::net::IpAddr>().is_ok()
    {
        return None;
    }
    psl::domain_str(host).map(|domain| domain.trim_end_matches('.').to_ascii_lowercase())
}

fn browser_root_session_id(input: &Value, current_session_id: &str) -> String {
    string_field(input, "sessionId")
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .filter(|value| *value != "current")
        .unwrap_or(current_session_id)
        .to_string()
}

fn action_set_category(
    action_set: super::browser_action::BrowserActionSet,
) -> BrowserActionCategory {
    match action_set {
        super::browser_action::BrowserActionSet::Inspect => BrowserActionCategory::Inspect,
        super::browser_action::BrowserActionSet::Navigate => BrowserActionCategory::Navigate,
        super::browser_action::BrowserActionSet::Interact => BrowserActionCategory::Interact,
        super::browser_action::BrowserActionSet::Evaluate => BrowserActionCategory::Evaluate,
    }
}

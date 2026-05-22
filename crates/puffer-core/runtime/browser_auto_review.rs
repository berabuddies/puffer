use super::agent_support::{
    build_agent_system_prompt, combine_agent_prompt, filter_resources_for_agent,
};
use super::permission_prompt::{
    BrowserPermissionPromptActionSet, BrowserPermissionPromptPayload,
    BrowserPermissionPromptSource, BrowserPermissionPromptTargetClass,
};
use crate::permissions::browser_grants::BrowserGrantScopeKind;
use crate::tool_names::canonical_tool_name;
use crate::AppState;
use anyhow::{anyhow, Result};
use puffer_provider_registry::{AuthStore, ProviderRegistry};
use puffer_resources::{agent_by_id, LoadedResources};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Value};
#[cfg(test)]
use std::cell::RefCell;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use uuid::Uuid;

/// Captures one Browser auto-review request for a side agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserAutoReviewRequest {
    /// The tool identifier that triggered the review.
    pub tool_id: String,
    /// The human-readable summary shown to the reviewer.
    pub summary: String,
    /// An optional approval reason from the permission layer.
    pub reason: Option<String>,
    /// The Browser-bearing entrypoint that produced the request.
    pub source: BrowserAutoReviewSource,
    /// The normalized Browser action bucket.
    pub action_set: BrowserAutoReviewActionSet,
    /// The concrete Browser action when one can be identified.
    pub raw_action: BrowserAutoReviewRawAction,
    /// The raw target URL, if one is available.
    #[serde(default)]
    pub url: Option<String>,
    /// The resolved target origin, if one is available.
    #[serde(default)]
    pub origin: Option<String>,
    /// The resolved target host, if one is available.
    #[serde(default)]
    pub host: Option<String>,
    /// The normalized target URL scheme, if one is available.
    #[serde(default)]
    pub scheme: Option<String>,
    /// The registrable domain derived from the host, if one is available.
    #[serde(default)]
    pub registrable_domain: Option<String>,
    /// True when the resolved host is an IP literal.
    pub is_ip_host: bool,
    /// True when the resolved host is local development infrastructure.
    pub is_local_host: bool,
    /// True when the resolved URL carries a non-default port.
    pub has_non_default_port: bool,
    /// True when the host looks punycoded or heavily encoded.
    pub host_looks_encoded_or_punycode: bool,
    /// The normalized Browser target class.
    pub target_class: BrowserAutoReviewTargetClass,
    /// The Browser tab id, if one is available.
    #[serde(default)]
    pub tab_id: Option<String>,
    /// The resolved root session id used for Browser grant evaluation.
    pub resolved_root_session_id: String,
    /// The exact session-targeting fact carried into the reviewer.
    pub session_targeting: BrowserAutoReviewSessionTargeting,
    /// The backend-suggested grant scope for this Browser request.
    pub suggested_grant_scope: BrowserAutoReviewSuggestedGrantScope,
    /// Describes where the effective URL facts came from.
    pub url_source: BrowserAutoReviewUrlSource,
    /// Carries the explicit requested URL when the caller supplied one.
    #[serde(default)]
    pub requested_url: Option<String>,
    /// Carries the current-tab URL when it differs from the requested URL.
    #[serde(default)]
    pub current_tab_url: Option<String>,
    /// Marks low-risk Browser tab-management operations.
    pub tab_management: bool,
}

/// Identifies how the Browser request entered the reviewer path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserAutoReviewSource {
    /// The `Browser` tool triggered the request.
    BrowserTool,
    /// A first-party internal Browser tool triggered the request.
    BrowserInternalTool,
}

/// Groups Browser actions into stable reviewer-facing buckets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserAutoReviewActionSet {
    /// Read-only inspection.
    Inspect,
    /// Navigation and tab management.
    Navigate,
    /// DOM interaction and input entry.
    Interact,
    /// JavaScript evaluation.
    Evaluate,
}

/// Preserves the concrete Browser action for reviewer-side reasoning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserAutoReviewRawAction {
    /// List Browser tabs or tab-like state.
    List,
    /// Open one Browser target.
    Open,
    /// Create one new Browser tab.
    New,
    /// Focus an existing Browser tab.
    Focus,
    /// Close one Browser tab.
    Close,
    /// Navigate the active or targeted Browser tab.
    Navigate,
    /// Reload one Browser tab.
    Reload,
    /// Move backward in history.
    Back,
    /// Move forward in history.
    Forward,
    /// Capture a DOM snapshot.
    Snapshot,
    /// Capture a screenshot.
    Screenshot,
    /// Click one Browser target.
    Click,
    /// Double-click one Browser target.
    Dblclick,
    /// Hover one Browser target.
    Hover,
    /// Focus one DOM ref.
    FocusRef,
    /// Fill one form control.
    Fill,
    /// Select one option.
    Select,
    /// Upload one file set.
    Upload,
    /// Check one control.
    Check,
    /// Uncheck one control.
    Uncheck,
    /// Type text.
    Type,
    /// Insert text.
    InsertText,
    /// Press one key.
    Press,
    /// Hold one key down.
    Keydown,
    /// Release one key.
    Keyup,
    /// Scroll.
    Scroll,
    /// Scroll one ref into view.
    ScrollIntoView,
    /// Evaluate JavaScript.
    Evaluate,
    /// No stable concrete action could be derived.
    Unknown,
}

/// Classifies the Browser target for reviewer consumption.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserAutoReviewTargetClass {
    /// A local development URL or host.
    LocalDev,
    /// A workspace file URL.
    WorkspaceFile,
    /// A file outside the workspace roots.
    NonWorkspaceFile,
    /// A `data:` URL.
    DataUrl,
    /// General open-web state.
    OpenWeb,
    /// No concrete target class could be derived.
    Unknown,
}

/// Describes the precise Browser session-targeting fact carried to the reviewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserAutoReviewSessionTargeting {
    /// The Browser request targeted the current session.
    CurrentSession,
    /// The Browser request explicitly targeted a session id.
    ExplicitSession,
}

/// Mirrors the backend-suggested Browser grant scope sent to the reviewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserAutoReviewSuggestedGrantScope {
    /// Approve this exact call only.
    AllowOnce,
    /// Approve this origin for the session.
    AllowOriginSession,
    /// Approve this registrable domain for the session.
    AllowDomainSession,
    /// Approve this action/origin pair for the session.
    AllowActionOnOriginSession,
    /// Approve this tab for the session.
    AllowTabSession,
}

/// Describes where Browser URL facts originated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserAutoReviewUrlSource {
    /// The request carried an explicit target URL.
    Explicit,
    /// The request URL was hydrated from the daemon current-tab snapshot.
    CurrentTab,
    /// The request URL was recovered from remembered session/tab cache.
    Remembered,
    /// No effective URL facts were available.
    None,
}

/// Describes the normalized Browser auto-review result returned by the side agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserAutoReviewRuntimeResult {
    /// Allow this call once.
    AllowOnce,
    /// Allow this Browser context for the session.
    AllowSession,
    /// Deny this Browser call.
    Deny,
    /// Defer to the user.
    NeedsUser,
    /// The reviewer output was malformed or unavailable.
    Unavailable,
}

const DEFAULT_BROWSER_AUTO_REVIEW_TIMEOUT: Duration = Duration::from_secs(10);
const BROWSER_AUTO_REVIEW_OUTPUT_SCHEMA_NAME: &str = "browser_auto_review_decision";
const APPROVAL_REVIEWER_AGENT_ID: &str = "approval-reviewer";
const BROWSER_REVIEW_METADATA_KEY: &str = "__pufferBrowserReview";

#[cfg(test)]
thread_local! {
    static TEST_BROWSER_AUTO_REVIEW_HANDLER: RefCell<
        Option<Box<dyn FnMut(&BrowserAutoReviewRequest) -> BrowserAutoReviewRuntimeResult>>
    > = const { RefCell::new(None) };
}

/// Parses reviewer JSON into a stable Browser auto-review result.
///
/// Any malformed, missing, or unrecognized JSON is mapped to
/// [`BrowserAutoReviewRuntimeResult::Unavailable`].
pub fn browser_auto_review_runtime_result_from_json(json: &str) -> BrowserAutoReviewRuntimeResult {
    serde_json::from_str(json).unwrap_or(BrowserAutoReviewRuntimeResult::Unavailable)
}

/// Runs the Browser reviewer side agent and returns its normalized disposition.
///
/// The reviewer runs against a cloned session with only structured output
/// available. Provider errors, parse failures, and timeouts all map to
/// [`BrowserAutoReviewRuntimeResult::Unavailable`].
pub fn run_browser_auto_review(
    state: &AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &AuthStore,
    request: &BrowserAutoReviewRequest,
) -> BrowserAutoReviewRuntimeResult {
    #[cfg(test)]
    if let Some(result) = test_browser_auto_review(request) {
        return result;
    }
    run_browser_auto_review_with_timeout(
        state,
        resources,
        providers,
        auth_store,
        request,
        DEFAULT_BROWSER_AUTO_REVIEW_TIMEOUT,
    )
}

/// Runs a closure while Browser auto-review results are served by a test handler.
#[cfg(test)]
pub(crate) fn with_browser_auto_review_test_handler<R>(
    handler: impl FnMut(&BrowserAutoReviewRequest) -> BrowserAutoReviewRuntimeResult + 'static,
    run: impl FnOnce() -> R,
) -> R {
    TEST_BROWSER_AUTO_REVIEW_HANDLER.with(|slot| {
        let previous = slot.borrow_mut().take();
        *slot.borrow_mut() = Some(Box::new(handler));
        let result = run();
        let _ = slot.borrow_mut().take();
        *slot.borrow_mut() = previous;
        result
    })
}

/// Builds one reviewer request from the current Browser permission prompt facts.
pub fn build_browser_auto_review_request(
    tool_id: &str,
    input: &Value,
    summary: String,
    reason: Option<String>,
    browser: &BrowserPermissionPromptPayload,
    resolved_root_session_id: String,
    session_targeting: BrowserAutoReviewSessionTargeting,
    suggested_grant_scope: BrowserGrantScopeKind,
) -> BrowserAutoReviewRequest {
    let effective_url = browser.url.clone();
    let review_url_source = review_metadata_url_source(input);
    let explicit_requested_url = if review_url_source.is_some() {
        review_metadata_requested_url(input)
    } else {
        browser_requested_url(tool_id, input)
    };
    let url_source = review_url_source.unwrap_or_else(|| {
        browser_url_source(explicit_requested_url.as_deref(), effective_url.as_deref())
    });
    let current_tab_url = review_metadata_current_tab_url(input).or_else(|| match url_source {
        BrowserAutoReviewUrlSource::Explicit | BrowserAutoReviewUrlSource::None => None,
        BrowserAutoReviewUrlSource::CurrentTab | BrowserAutoReviewUrlSource::Remembered => {
            effective_url.clone()
        }
    });
    let raw_action = browser_raw_action(tool_id, input);
    let host = browser.host.clone();
    let (target_scheme, registrable_domain, has_non_default_port) =
        browser_url_facts(browser.url.as_deref());
    BrowserAutoReviewRequest {
        tool_id: tool_id.to_string(),
        summary,
        reason,
        source: match browser.source {
            BrowserPermissionPromptSource::BrowserTool => BrowserAutoReviewSource::BrowserTool,
            BrowserPermissionPromptSource::BrowserInternalTool => {
                BrowserAutoReviewSource::BrowserInternalTool
            }
        },
        action_set: match browser.action_set {
            BrowserPermissionPromptActionSet::Inspect => BrowserAutoReviewActionSet::Inspect,
            BrowserPermissionPromptActionSet::Navigate => BrowserAutoReviewActionSet::Navigate,
            BrowserPermissionPromptActionSet::Interact => BrowserAutoReviewActionSet::Interact,
            BrowserPermissionPromptActionSet::Evaluate => BrowserAutoReviewActionSet::Evaluate,
        },
        raw_action,
        url: browser.url.clone(),
        origin: browser.origin.clone(),
        host: host.clone(),
        scheme: target_scheme.clone(),
        registrable_domain: registrable_domain.clone(),
        is_ip_host: host
            .as_deref()
            .is_some_and(|candidate| candidate.parse::<std::net::IpAddr>().is_ok()),
        is_local_host: host
            .as_deref()
            .zip(target_scheme.as_deref())
            .map(|(candidate, _)| is_local_like_host(candidate))
            .unwrap_or(false),
        has_non_default_port,
        host_looks_encoded_or_punycode: host
            .as_deref()
            .map(host_looks_encoded_or_punycode)
            .unwrap_or(false),
        target_class: match browser.target_class {
            BrowserPermissionPromptTargetClass::LocalDev => BrowserAutoReviewTargetClass::LocalDev,
            BrowserPermissionPromptTargetClass::WorkspaceFile => {
                BrowserAutoReviewTargetClass::WorkspaceFile
            }
            BrowserPermissionPromptTargetClass::NonWorkspaceFile => {
                BrowserAutoReviewTargetClass::NonWorkspaceFile
            }
            BrowserPermissionPromptTargetClass::DataUrl => BrowserAutoReviewTargetClass::DataUrl,
            BrowserPermissionPromptTargetClass::OpenWeb => BrowserAutoReviewTargetClass::OpenWeb,
            BrowserPermissionPromptTargetClass::Unknown => BrowserAutoReviewTargetClass::Unknown,
        },
        tab_id: browser.tab_id.clone(),
        resolved_root_session_id,
        session_targeting,
        suggested_grant_scope: suggested_grant_scope.into(),
        url_source,
        requested_url: explicit_requested_url,
        current_tab_url,
        tab_management: matches!(
            raw_action,
            BrowserAutoReviewRawAction::List
                | BrowserAutoReviewRawAction::Open
                | BrowserAutoReviewRawAction::New
                | BrowserAutoReviewRawAction::Focus
                | BrowserAutoReviewRawAction::Close
        ),
    }
}

fn browser_url_facts(url: Option<&str>) -> (Option<String>, Option<String>, bool) {
    let Some(url) = url.map(str::trim).filter(|value| !value.is_empty()) else {
        return (None, None, false);
    };
    let Ok(parsed) = url::Url::parse(url) else {
        return (None, None, false);
    };
    let scheme = Some(parsed.scheme().to_ascii_lowercase());
    let host = parsed.host_str().map(|value| value.to_ascii_lowercase());
    let registrable_domain = host.as_deref().and_then(|candidate| {
        if candidate.eq_ignore_ascii_case("localhost")
            || candidate.ends_with(".localhost")
            || candidate.parse::<std::net::IpAddr>().is_ok()
        {
            None
        } else {
            psl::domain_str(candidate)
                .map(|domain| domain.trim_end_matches('.').to_ascii_lowercase())
        }
    });
    let has_non_default_port = match (parsed.port(), parsed.scheme()) {
        (Some(port), "http") => port != 80,
        (Some(port), "https") => port != 443,
        (Some(_), _) => true,
        (None, _) => false,
    };
    (scheme, registrable_domain, has_non_default_port)
}

fn is_local_like_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host.ends_with(".localhost")
        || host.parse::<std::net::IpAddr>().is_ok_and(|ip| match ip {
            std::net::IpAddr::V4(v4) => v4.is_loopback() || v4.is_unspecified(),
            std::net::IpAddr::V6(v6) => v6.is_loopback() || v6.is_unspecified(),
        })
}

fn host_looks_encoded_or_punycode(host: &str) -> bool {
    let lower = host.to_ascii_lowercase();
    lower.contains("xn--")
        || host.chars().filter(|ch| ch.is_ascii_digit()).count() >= 8
        || host.matches('-').count() >= 4
}

/// Converts one JSON value into a stable Browser auto-review result.
pub fn browser_auto_review_runtime_result_from_value(
    value: &Value,
) -> BrowserAutoReviewRuntimeResult {
    match value.as_str() {
        Some("allow_once") => BrowserAutoReviewRuntimeResult::AllowOnce,
        Some("allow_session") => BrowserAutoReviewRuntimeResult::AllowSession,
        Some("deny") => BrowserAutoReviewRuntimeResult::Deny,
        Some("needs_user") => BrowserAutoReviewRuntimeResult::NeedsUser,
        Some("unavailable") => BrowserAutoReviewRuntimeResult::Unavailable,
        _ => BrowserAutoReviewRuntimeResult::Unavailable,
    }
}

impl<'de> Deserialize<'de> for BrowserAutoReviewRuntimeResult {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        Ok(browser_auto_review_runtime_result_from_value(&value))
    }
}

impl From<BrowserGrantScopeKind> for BrowserAutoReviewSuggestedGrantScope {
    /// Converts the existing Browser grant scope into the reviewer contract.
    fn from(value: BrowserGrantScopeKind) -> Self {
        match value {
            BrowserGrantScopeKind::AllowOnce => Self::AllowOnce,
            BrowserGrantScopeKind::AllowOriginSession => Self::AllowOriginSession,
            BrowserGrantScopeKind::AllowDomainSession => Self::AllowDomainSession,
            BrowserGrantScopeKind::AllowActionOnOriginSession => Self::AllowActionOnOriginSession,
            BrowserGrantScopeKind::AllowTabSession => Self::AllowTabSession,
        }
    }
}

fn browser_requested_url(tool_id: &str, input: &Value) -> Option<String> {
    match canonical_tool_name(tool_id).as_str() {
        "browser" => input
            .get("url")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string),
        "bash" | "powershell" => {
            crate::permissions::browser_action::browser_permission_value_for_tool_call(
                tool_id, input,
            )
            .and_then(|value| {
                value
                    .get("url")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|candidate| !candidate.is_empty())
                    .map(ToString::to_string)
            })
        }
        _ => None,
    }
}

fn review_metadata_requested_url(input: &Value) -> Option<String> {
    input
        .get(BROWSER_REVIEW_METADATA_KEY)
        .and_then(|value| value.get("requestedUrl"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn review_metadata_url_source(input: &Value) -> Option<BrowserAutoReviewUrlSource> {
    match input
        .get(BROWSER_REVIEW_METADATA_KEY)
        .and_then(|value| value.get("urlSource"))
        .and_then(Value::as_str)
        .map(str::trim)
    {
        Some("explicit") => Some(BrowserAutoReviewUrlSource::Explicit),
        Some("current_tab") => Some(BrowserAutoReviewUrlSource::CurrentTab),
        Some("remembered") => Some(BrowserAutoReviewUrlSource::Remembered),
        Some("none") => Some(BrowserAutoReviewUrlSource::None),
        _ => None,
    }
}

fn review_metadata_current_tab_url(input: &Value) -> Option<String> {
    input
        .get(BROWSER_REVIEW_METADATA_KEY)
        .and_then(|value| value.get("currentTabUrl"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn browser_url_source(
    requested_url: Option<&str>,
    effective_url: Option<&str>,
) -> BrowserAutoReviewUrlSource {
    match (requested_url, effective_url) {
        (Some(requested), Some(effective)) if requested.eq_ignore_ascii_case(effective) => {
            BrowserAutoReviewUrlSource::Explicit
        }
        (Some(_), Some(_)) => BrowserAutoReviewUrlSource::CurrentTab,
        (None, Some(_)) => BrowserAutoReviewUrlSource::CurrentTab,
        (_, None) => BrowserAutoReviewUrlSource::None,
    }
}

fn browser_raw_action(tool_id: &str, input: &Value) -> BrowserAutoReviewRawAction {
    let browser_input =
        crate::permissions::browser_action::browser_permission_value_for_tool_call(tool_id, input)
            .unwrap_or_else(|| input.clone());
    let Some(action) = browser_input.get("action").and_then(Value::as_str) else {
        return BrowserAutoReviewRawAction::Unknown;
    };
    match action.trim().to_ascii_lowercase().as_str() {
        "list" => BrowserAutoReviewRawAction::List,
        "open" => BrowserAutoReviewRawAction::Open,
        "new" => BrowserAutoReviewRawAction::New,
        "focus" => BrowserAutoReviewRawAction::Focus,
        "close" => BrowserAutoReviewRawAction::Close,
        "navigate" => BrowserAutoReviewRawAction::Navigate,
        "reload" => BrowserAutoReviewRawAction::Reload,
        "back" => BrowserAutoReviewRawAction::Back,
        "forward" => BrowserAutoReviewRawAction::Forward,
        "snapshot" => BrowserAutoReviewRawAction::Snapshot,
        "screenshot" => BrowserAutoReviewRawAction::Screenshot,
        "click" => BrowserAutoReviewRawAction::Click,
        "dblclick" => BrowserAutoReviewRawAction::Dblclick,
        "hover" => BrowserAutoReviewRawAction::Hover,
        "focus_ref" => BrowserAutoReviewRawAction::FocusRef,
        "fill" => BrowserAutoReviewRawAction::Fill,
        "select" => BrowserAutoReviewRawAction::Select,
        "upload" => BrowserAutoReviewRawAction::Upload,
        "check" => BrowserAutoReviewRawAction::Check,
        "uncheck" => BrowserAutoReviewRawAction::Uncheck,
        "type" => BrowserAutoReviewRawAction::Type,
        "inserttext" | "insert_text" => BrowserAutoReviewRawAction::InsertText,
        "press" => BrowserAutoReviewRawAction::Press,
        "keydown" => BrowserAutoReviewRawAction::Keydown,
        "keyup" => BrowserAutoReviewRawAction::Keyup,
        "scroll" => BrowserAutoReviewRawAction::Scroll,
        "scrollintoview" | "scroll_into_view" => BrowserAutoReviewRawAction::ScrollIntoView,
        "evaluate" | "eval" => BrowserAutoReviewRawAction::Evaluate,
        _ => BrowserAutoReviewRawAction::Unknown,
    }
}

fn run_browser_auto_review_with_timeout(
    state: &AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &AuthStore,
    request: &BrowserAutoReviewRequest,
    timeout: Duration,
) -> BrowserAutoReviewRuntimeResult {
    let review_input = match build_browser_auto_review_input(request) {
        Ok(prompt) => prompt,
        Err(_) => return BrowserAutoReviewRuntimeResult::Unavailable,
    };
    let reviewer = match approval_reviewer_agent(resources) {
        Ok(reviewer) => reviewer,
        Err(_) => return BrowserAutoReviewRuntimeResult::Unavailable,
    };
    let side_resources = build_browser_auto_review_resources(resources, &reviewer.value);
    let structured_output = build_browser_auto_review_structured_output();
    let mut side_state = build_browser_auto_review_side_state(state);
    let prompt = match build_browser_auto_review_prompt(resources, &reviewer.value, &review_input) {
        Ok(prompt) => prompt,
        Err(_) => return BrowserAutoReviewRuntimeResult::Unavailable,
    };
    apply_browser_auto_review_agent_config(&mut side_state, providers, &reviewer.value);
    let max_turns = reviewer.value.max_turns.or(Some(1));
    let providers = providers.clone();
    let mut auth_store = auth_store.clone();
    run_browser_auto_review_executor_with_timeout(timeout, move |cancel| {
        super::execute_user_prompt_with_options(
            &mut side_state,
            &side_resources,
            &providers,
            &mut auth_store,
            &prompt,
            super::TurnRequestOptions {
                structured_output: Some(&structured_output),
                tool_filter: None,
                reflection: None,
                cancel: Some(cancel),
                max_turns,
                observability: None,
                lightweight_context: false,
            },
        )
        .map(|turn| browser_auto_review_runtime_result_from_json(turn.assistant_text.trim()))
        .unwrap_or(BrowserAutoReviewRuntimeResult::Unavailable)
    })
}

#[cfg(test)]
fn test_browser_auto_review(
    request: &BrowserAutoReviewRequest,
) -> Option<BrowserAutoReviewRuntimeResult> {
    TEST_BROWSER_AUTO_REVIEW_HANDLER.with(|slot| {
        let mut borrowed = slot.borrow_mut();
        borrowed.as_mut().map(|handler| handler(request))
    })
}

fn build_browser_auto_review_input(request: &BrowserAutoReviewRequest) -> Result<String> {
    Ok(serde_json::to_string_pretty(request)?)
}

fn approval_reviewer_agent(
    resources: &LoadedResources,
) -> Result<&puffer_resources::LoadedItem<puffer_resources::AgentSpec>> {
    agent_by_id(resources, APPROVAL_REVIEWER_AGENT_ID).ok_or_else(|| {
        anyhow!("required approval reviewer agent `{APPROVAL_REVIEWER_AGENT_ID}` is not available")
    })
}

fn build_browser_auto_review_prompt(
    resources: &LoadedResources,
    agent: &puffer_resources::AgentSpec,
    review_input: &str,
) -> Result<String> {
    let system_prompt = build_agent_system_prompt(resources, agent)?;
    Ok(combine_agent_prompt(Some(&system_prompt), review_input))
}

fn build_browser_auto_review_side_state(state: &AppState) -> AppState {
    let mut side_state = state.clone();
    side_state.session.parent_session_id = Some(state.session.id);
    side_state.session.id = Uuid::new_v4();
    side_state.transcript.clear();
    side_state.reflection_config = None;
    side_state.plan_mode = false;
    side_state.plan_mode_attachment_turns = 0;
    side_state.plan_mode_attachment_count = 0;
    side_state.plan_mode_has_exited = false;
    side_state.plan_mode_needs_reentry_attachment = false;
    side_state.plan_mode_needs_exit_attachment = false;
    side_state.active_team_name = None;
    side_state.last_input_tokens = None;
    side_state.last_cache_hit_ratio = None;
    side_state.session_cache_hit_ratio = None;
    side_state
}

fn apply_browser_auto_review_agent_config(
    side_state: &mut AppState,
    providers: &ProviderRegistry,
    agent: &puffer_resources::AgentSpec,
) {
    if let Some(effort) = agent
        .effort
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        side_state.effort_level = effort.to_string();
    }
    if let Some(model) = agent
        .model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("inherit"))
    {
        let resolved = providers
            .resolve_model(model)
            .or_else(|| resolve_model_case_insensitive(providers, model));
        let selector = resolved
            .as_ref()
            .map(|descriptor| format!("{}/{}", descriptor.provider, descriptor.id))
            .unwrap_or_else(|| model.to_string());
        side_state.current_model = Some(selector.clone());
        side_state.current_provider = resolved
            .map(|descriptor| descriptor.provider.clone())
            .or_else(|| {
                selector
                    .split_once('/')
                    .map(|(provider, _)| provider.to_string())
            })
            .or_else(|| side_state.current_provider.clone());
    }
}

fn build_browser_auto_review_resources(
    resources: &LoadedResources,
    agent: &puffer_resources::AgentSpec,
) -> LoadedResources {
    filter_resources_for_agent(resources, &agent.tools, &agent.disallowed_tools)
}

fn build_browser_auto_review_structured_output() -> super::StructuredOutputConfig {
    super::StructuredOutputConfig::new(
        BROWSER_AUTO_REVIEW_OUTPUT_SCHEMA_NAME,
        json!({
            "type": "string",
            "enum": [
                "allow_once",
                "allow_session",
                "deny",
                "needs_user"
            ]
        }),
    )
    .with_description("Return exactly one browser auto-review decision enum.")
}

fn run_browser_auto_review_executor_with_timeout<F>(
    timeout: Duration,
    execute: F,
) -> BrowserAutoReviewRuntimeResult
where
    F: FnOnce(&crate::runtime::CancelToken) -> BrowserAutoReviewRuntimeResult + Send + 'static,
{
    let cancel = crate::runtime::CancelToken::new();
    let worker_cancel = cancel.clone();
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        let result = execute(&worker_cancel);
        let _ = tx.send(result);
    });

    match rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            cancel.cancel();
            BrowserAutoReviewRuntimeResult::Unavailable
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => BrowserAutoReviewRuntimeResult::Unavailable,
    }
}

fn resolve_model_case_insensitive<'a>(
    providers: &'a ProviderRegistry,
    selector: &str,
) -> Option<&'a puffer_provider_registry::ModelDescriptor> {
    providers.providers().find_map(|provider| {
        provider.models.iter().find(|model| {
            format!("{}/{}", model.provider, model.id).eq_ignore_ascii_case(selector)
                || model.id.eq_ignore_ascii_case(selector)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::tests::state;
    use puffer_provider_registry::AuthStore;
    use puffer_resources::LoadedResources;
    use serde_json::json;
    use std::thread;

    fn sample_request() -> BrowserAutoReviewRequest {
        BrowserAutoReviewRequest {
            tool_id: "Browser".to_string(),
            summary: "Navigate to docs".to_string(),
            reason: Some("browser action requires explicit approval".to_string()),
            source: BrowserAutoReviewSource::BrowserTool,
            action_set: BrowserAutoReviewActionSet::Navigate,
            raw_action: BrowserAutoReviewRawAction::Navigate,
            url: Some("https://docs.example.com/page".to_string()),
            origin: Some("https://docs.example.com".to_string()),
            host: Some("docs.example.com".to_string()),
            scheme: Some("https".to_string()),
            registrable_domain: Some("example.com".to_string()),
            is_ip_host: false,
            is_local_host: false,
            has_non_default_port: false,
            host_looks_encoded_or_punycode: false,
            target_class: BrowserAutoReviewTargetClass::OpenWeb,
            tab_id: Some("t7".to_string()),
            resolved_root_session_id: "root-1".to_string(),
            session_targeting: BrowserAutoReviewSessionTargeting::ExplicitSession,
            suggested_grant_scope: BrowserAutoReviewSuggestedGrantScope::AllowTabSession,
            url_source: BrowserAutoReviewUrlSource::Explicit,
            requested_url: Some("https://docs.example.com/page".to_string()),
            current_tab_url: None,
            tab_management: false,
        }
    }

    #[test]
    fn browser_auto_review_request_serializes_all_contract_fields() {
        let request = sample_request();

        let value = serde_json::to_value(request).unwrap();
        assert_eq!(value["tool_id"], json!("Browser"));
        assert_eq!(value["summary"], json!("Navigate to docs"));
        assert_eq!(
            value["reason"],
            json!("browser action requires explicit approval")
        );
        assert_eq!(value["source"], json!("browser_tool"));
        assert_eq!(value["action_set"], json!("navigate"));
        assert_eq!(value["raw_action"], json!("navigate"));
        assert_eq!(value["url"], json!("https://docs.example.com/page"));
        assert_eq!(value["origin"], json!("https://docs.example.com"));
        assert_eq!(value["host"], json!("docs.example.com"));
        assert_eq!(value["scheme"], json!("https"));
        assert_eq!(value["registrable_domain"], json!("example.com"));
        assert_eq!(value["is_ip_host"], json!(false));
        assert_eq!(value["is_local_host"], json!(false));
        assert_eq!(value["has_non_default_port"], json!(false));
        assert_eq!(value["host_looks_encoded_or_punycode"], json!(false));
        assert_eq!(value["target_class"], json!("open_web"));
        assert_eq!(value["tab_id"], json!("t7"));
        assert_eq!(value["resolved_root_session_id"], json!("root-1"));
        assert_eq!(value["session_targeting"], json!("explicit_session"));
        assert_eq!(value["suggested_grant_scope"], json!("allow_tab_session"));
        assert_eq!(value["url_source"], json!("explicit"));
        assert_eq!(
            value["requested_url"],
            json!("https://docs.example.com/page")
        );
        assert_eq!(value["current_tab_url"], Value::Null);
        assert_eq!(value["tab_management"], json!(false));
    }

    #[test]
    fn browser_auto_review_request_uses_review_metadata_for_url_source() {
        let browser = BrowserPermissionPromptPayload {
            source: BrowserPermissionPromptSource::BrowserTool,
            action_set: BrowserPermissionPromptActionSet::Navigate,
            url: Some("https://docs.example.com/page".to_string()),
            origin: Some("https://docs.example.com".to_string()),
            host: Some("docs.example.com".to_string()),
            target_class: BrowserPermissionPromptTargetClass::OpenWeb,
            tab_id: Some("t1".to_string()),
            is_cross_session: false,
        };
        let request = build_browser_auto_review_request(
            "Browser",
            &json!({
                "action": "new",
                "url": "https://docs.example.com/page",
                "__pufferBrowserReview": {
                    "urlSource": "current_tab",
                    "requestedUrl": null
                }
            }),
            "Open current browser tab".to_string(),
            Some("browser navigation and interaction require approval".to_string()),
            &browser,
            "root-1".to_string(),
            BrowserAutoReviewSessionTargeting::CurrentSession,
            BrowserGrantScopeKind::AllowTabSession,
        );

        assert_eq!(request.raw_action, BrowserAutoReviewRawAction::New);
        assert_eq!(request.url_source, BrowserAutoReviewUrlSource::CurrentTab);
        assert_eq!(request.requested_url, None);
        assert_eq!(
            request.current_tab_url.as_deref(),
            Some("https://docs.example.com/page")
        );
        assert!(request.tab_management);
    }

    #[test]
    fn browser_auto_review_runtime_result_accepts_known_decisions() {
        assert_eq!(
            browser_auto_review_runtime_result_from_json("\"allow_once\""),
            BrowserAutoReviewRuntimeResult::AllowOnce
        );
        assert_eq!(
            browser_auto_review_runtime_result_from_json("\"allow_session\""),
            BrowserAutoReviewRuntimeResult::AllowSession
        );
        assert_eq!(
            browser_auto_review_runtime_result_from_json("\"deny\""),
            BrowserAutoReviewRuntimeResult::Deny
        );
        assert_eq!(
            browser_auto_review_runtime_result_from_json("\"needs_user\""),
            BrowserAutoReviewRuntimeResult::NeedsUser
        );
        assert_eq!(
            browser_auto_review_runtime_result_from_json("\"unavailable\""),
            BrowserAutoReviewRuntimeResult::Unavailable
        );
    }

    #[test]
    fn browser_auto_review_runtime_result_maps_invalid_json_to_unavailable() {
        assert_eq!(
            browser_auto_review_runtime_result_from_json("{}"),
            BrowserAutoReviewRuntimeResult::Unavailable
        );
        assert_eq!(
            browser_auto_review_runtime_result_from_json("{\"decision\":\"allow_once\"}"),
            BrowserAutoReviewRuntimeResult::Unavailable
        );
        assert_eq!(
            browser_auto_review_runtime_result_from_json("not-json"),
            BrowserAutoReviewRuntimeResult::Unavailable
        );
        assert_eq!(
            browser_auto_review_runtime_result_from_value(&json!({"decision": "deny"})),
            BrowserAutoReviewRuntimeResult::Unavailable
        );
    }

    #[test]
    fn run_browser_auto_review_returns_known_dispositions() {
        for (decision, expected) in [
            ("allow_once", BrowserAutoReviewRuntimeResult::AllowOnce),
            (
                "allow_session",
                BrowserAutoReviewRuntimeResult::AllowSession,
            ),
            ("deny", BrowserAutoReviewRuntimeResult::Deny),
            ("needs_user", BrowserAutoReviewRuntimeResult::NeedsUser),
        ] {
            let decision = decision.to_string();
            let result = run_browser_auto_review_executor_with_timeout(
                Duration::from_millis(100),
                move |_cancel| {
                    browser_auto_review_runtime_result_from_json(&format!("\"{decision}\""))
                },
            );
            assert_eq!(result, expected);
        }
    }

    #[test]
    fn run_browser_auto_review_maps_provider_errors_to_unavailable() {
        let result = run_browser_auto_review_executor_with_timeout(
            Duration::from_millis(100),
            move |_cancel| BrowserAutoReviewRuntimeResult::Unavailable,
        );
        assert_eq!(result, BrowserAutoReviewRuntimeResult::Unavailable);
    }

    #[test]
    fn run_browser_auto_review_maps_parse_errors_to_unavailable() {
        let result = run_browser_auto_review_executor_with_timeout(
            Duration::from_millis(100),
            move |_cancel| browser_auto_review_runtime_result_from_json("\"unavailable\""),
        );
        assert_eq!(result, BrowserAutoReviewRuntimeResult::Unavailable);
    }

    #[test]
    fn run_browser_auto_review_maps_timeouts_to_unavailable() {
        let result = run_browser_auto_review_executor_with_timeout(
            Duration::from_millis(20),
            move |_cancel| {
                thread::sleep(Duration::from_millis(200));
                BrowserAutoReviewRuntimeResult::AllowOnce
            },
        );
        assert_eq!(result, BrowserAutoReviewRuntimeResult::Unavailable);
    }

    #[test]
    fn run_browser_auto_review_public_entry_returns_unavailable_without_provider() {
        let app_state = state();
        let result = run_browser_auto_review(
            &app_state,
            &LoadedResources::default(),
            &ProviderRegistry::default(),
            &AuthStore::default(),
            &sample_request(),
        );
        assert_eq!(result, BrowserAutoReviewRuntimeResult::Unavailable);
    }
}

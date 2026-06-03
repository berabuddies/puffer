use super::browser_auto_review::{
    BrowserAutoReviewRuntimeResult, BrowserAutoReviewSessionTargeting,
};
use crate::permissions::browser_action::browser_permission_value_for_tool_call;
use crate::permissions::browser_target::{
    browser_permission_context_for_tool, BrowserActionCategory, BrowserTargetClass,
};
use crate::tool_names::canonical_tool_name;
use puffer_tools::ToolDefinition;
use serde_json::{Map, Value};
use std::cell::RefCell;
use std::path::PathBuf;

/// Describes one runtime permission request that may need user approval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionPromptRequest {
    pub tool_id: String,
    pub summary: String,
    pub reason: Option<String>,
    pub browser: Option<BrowserPermissionPromptPayload>,
    pub review: Option<PermissionPromptReviewPayload>,
}

/// Carries the structured Browser approval payload shared across all Browser prompts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserPermissionPromptPayload {
    pub source: BrowserPermissionPromptSource,
    pub action_set: BrowserPermissionPromptActionSet,
    pub url: Option<String>,
    pub origin: Option<String>,
    pub host: Option<String>,
    pub target_class: BrowserPermissionPromptTargetClass,
    pub tab_id: Option<String>,
    pub is_cross_session: bool,
}

/// Carries the reviewer conclusion shown when Browser approval falls back to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionPromptReviewPayload {
    pub decision: BrowserAutoReviewRuntimeResult,
    pub risk: String,
    pub rationale: String,
    pub resolved_root_session_id: String,
    pub session_targeting: BrowserAutoReviewSessionTargeting,
}

/// Identifies how one Browser approval request reached the prompt layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserPermissionPromptSource {
    BrowserTool,
    BrowserInternalTool,
}

/// Identifies one stable Browser action-set bucket shown in permission prompts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserPermissionPromptActionSet {
    Inspect,
    Navigate,
    Interact,
    Evaluate,
}

/// Identifies one stable Browser target class shown in permission prompts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserPermissionPromptTargetClass {
    LocalDev,
    WorkspaceFile,
    NonWorkspaceFile,
    DataUrl,
    OpenWeb,
    Unknown,
}

/// Describes how the user responded to a runtime permission prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionPromptAction {
    AllowOnce,
    AllowSession,
    AllowAllSession,
    Deny,
}

/// Describes one `AskUserQuestion` request that may need user answers.
#[derive(Debug, Clone, PartialEq)]
pub struct UserQuestionPromptRequest {
    pub questions: Value,
}

/// Describes the answers collected for one `AskUserQuestion` request.
#[derive(Debug, Clone, PartialEq)]
pub struct UserQuestionPromptResponse {
    pub answers: Map<String, Value>,
    pub annotations: Map<String, Value>,
}

thread_local! {
    static PERMISSION_PROMPT_HANDLER: RefCell<Option<Box<dyn FnMut(PermissionPromptRequest) -> PermissionPromptAction>>> =
        const { RefCell::new(None) };
    static USER_QUESTION_PROMPT_HANDLER: RefCell<Option<Box<dyn FnMut(UserQuestionPromptRequest) -> UserQuestionPromptResponse>>> =
        const { RefCell::new(None) };
}

/// Runs a closure while the current thread has an active permission prompt handler.
pub fn with_permission_prompt_handler<R>(
    handler: impl FnMut(PermissionPromptRequest) -> PermissionPromptAction + 'static,
    run: impl FnOnce() -> R,
) -> R {
    PERMISSION_PROMPT_HANDLER.with(|slot| {
        let previous = slot.borrow_mut().take();
        *slot.borrow_mut() = Some(Box::new(handler));
        let result = run();
        let _ = slot.borrow_mut().take();
        *slot.borrow_mut() = previous;
        result
    })
}

/// Runs a closure while the current thread can answer `AskUserQuestion` prompts.
pub fn with_user_question_prompt_handler<R>(
    handler: impl FnMut(UserQuestionPromptRequest) -> UserQuestionPromptResponse + 'static,
    run: impl FnOnce() -> R,
) -> R {
    USER_QUESTION_PROMPT_HANDLER.with(|slot| {
        let previous = slot.borrow_mut().take();
        *slot.borrow_mut() = Some(Box::new(handler));
        let result = run();
        let _ = slot.borrow_mut().take();
        *slot.borrow_mut() = previous;
        result
    })
}

pub(crate) fn prompt_for_permission(request: PermissionPromptRequest) -> PermissionPromptAction {
    PERMISSION_PROMPT_HANDLER.with(|slot| {
        let mut borrowed = slot.borrow_mut();
        let Some(handler) = borrowed.as_mut() else {
            return PermissionPromptAction::Deny;
        };
        handler(request)
    })
}

pub(crate) fn prompt_for_user_question(
    request: UserQuestionPromptRequest,
) -> Option<UserQuestionPromptResponse> {
    USER_QUESTION_PROMPT_HANDLER.with(|slot| {
        let mut borrowed = slot.borrow_mut();
        borrowed.as_mut().map(|handler| handler(request))
    })
}

pub(crate) fn build_permission_prompt_request(
    definition: &ToolDefinition,
    input: &Value,
    reason: Option<&str>,
    current_session_id: &str,
    workspace_roots: &[PathBuf],
) -> PermissionPromptRequest {
    PermissionPromptRequest {
        tool_id: definition.id.clone(),
        summary: permission_request_summary(definition, input),
        reason: reason.map(str::to_string),
        browser: browser_prompt_payload(definition, input, current_session_id, workspace_roots),
        review: None,
    }
}

fn browser_prompt_payload(
    definition: &ToolDefinition,
    input: &Value,
    current_session_id: &str,
    workspace_roots: &[PathBuf],
) -> Option<BrowserPermissionPromptPayload> {
    browser_permission_value_for_tool_call(&definition.id, input)?;
    let context = browser_permission_context_for_tool(
        &definition.id,
        input,
        current_session_id,
        workspace_roots,
    );
    let source = match canonical_tool_name(&definition.id).as_str() {
        "browser" => BrowserPermissionPromptSource::BrowserTool,
        _ => BrowserPermissionPromptSource::BrowserInternalTool,
    };
    let action_set = match context.action {
        Some(BrowserActionCategory::Inspect) => BrowserPermissionPromptActionSet::Inspect,
        Some(BrowserActionCategory::Navigate) => BrowserPermissionPromptActionSet::Navigate,
        Some(BrowserActionCategory::Interact) => BrowserPermissionPromptActionSet::Interact,
        Some(BrowserActionCategory::Evaluate) => BrowserPermissionPromptActionSet::Evaluate,
        None => BrowserPermissionPromptActionSet::Inspect,
    };
    let (url, origin, host, target_class) = context
        .target
        .as_ref()
        .map(|target| {
            (
                Some(target.raw_url.clone()),
                target.origin.clone(),
                target.host.clone(),
                match target.target_class {
                    BrowserTargetClass::LocalDev => BrowserPermissionPromptTargetClass::LocalDev,
                    BrowserTargetClass::WorkspaceFile => {
                        BrowserPermissionPromptTargetClass::WorkspaceFile
                    }
                    BrowserTargetClass::NonWorkspaceFile => {
                        BrowserPermissionPromptTargetClass::NonWorkspaceFile
                    }
                    BrowserTargetClass::DataUrl => BrowserPermissionPromptTargetClass::DataUrl,
                    BrowserTargetClass::OpenWeb => BrowserPermissionPromptTargetClass::OpenWeb,
                },
            )
        })
        .unwrap_or((
            None,
            None,
            None,
            BrowserPermissionPromptTargetClass::Unknown,
        ));
    Some(BrowserPermissionPromptPayload {
        source,
        action_set,
        url,
        origin,
        host,
        target_class,
        tab_id: context.tab_id.clone(),
        is_cross_session: context.root_session_id != current_session_id,
    })
}

fn permission_request_summary(definition: &ToolDefinition, input: &Value) -> String {
    match definition.id.as_str() {
        "Browser" => browser_permission_summary(input),
        "Bash" | "PowerShell" => input
            .get("command")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| definition.id.clone()),
        "Config" => {
            let setting = input
                .get("setting")
                .and_then(Value::as_str)
                .unwrap_or("setting");
            match input.get("value") {
                Some(value) => format!("Set {setting} to {}", value),
                None => format!("Read {setting}"),
            }
        }
        "WebSearch" => input
            .get("query")
            .and_then(Value::as_str)
            .map(|query| format!("Search the web for: {query}"))
            .unwrap_or_else(|| definition.id.clone()),
        "SendMessage" => input
            .get("to")
            .and_then(Value::as_str)
            .map(|to| format!("Send a message to {to}"))
            .unwrap_or_else(|| definition.id.clone()),
        "AskUserQuestion" => "Answer questions?".to_string(),
        "requestuserbrowseraction" | "RequestUserBrowserAction" => {
            "Complete browser action?".to_string()
        }
        "ExitPlanMode" => "Exit plan mode?".to_string(),
        "Email" => email_permission_summary(input),
        "Slack" => slack_permission_summary(input),
        "Telegram" => telegram_permission_summary(input),
        _ => definition.id.clone(),
    }
}

fn email_permission_summary(input: &Value) -> String {
    match input.get("action").and_then(Value::as_str) {
        Some("configure") => "Configure email subscriber".to_string(),
        _ => "Use email internal tool".to_string(),
    }
}

fn slack_permission_summary(input: &Value) -> String {
    match input.get("action").and_then(Value::as_str) {
        Some("configure_app") => "Configure Slack app credentials".to_string(),
        Some("import_local") => input
            .get("path")
            .and_then(Value::as_str)
            .map(|path| format!("Import Slack auth from {path}"))
            .unwrap_or_else(|| "Import Slack local app auth".to_string()),
        Some("list_conversations") => "List Slack conversations".to_string(),
        Some("login_browser") => "Configure Slack browser login".to_string(),
        Some("login_token") => "Configure Slack OAuth login".to_string(),
        Some("read_messages") => input
            .get("channel")
            .and_then(Value::as_str)
            .map(|channel| format!("Read Slack messages in {channel}"))
            .unwrap_or_else(|| "Read Slack messages".to_string()),
        Some("search_conversations") => input
            .get("query")
            .and_then(Value::as_str)
            .map(|query| format!("Search Slack conversations for {query}"))
            .unwrap_or_else(|| "Search Slack conversations".to_string()),
        Some("search_messages") => input
            .get("query")
            .and_then(Value::as_str)
            .map(|query| format!("Search Slack messages for {query}"))
            .unwrap_or_else(|| "Search Slack messages".to_string()),
        Some("search_users") => input
            .get("query")
            .and_then(Value::as_str)
            .map(|query| format!("Search Slack users for {query}"))
            .unwrap_or_else(|| "Search Slack users".to_string()),
        _ => "Use Slack internal tool".to_string(),
    }
}

fn telegram_permission_summary(input: &Value) -> String {
    match input.get("action").and_then(Value::as_str) {
        Some("import_desktop") => input
            .get("path")
            .and_then(Value::as_str)
            .map(|path| format!("Import Telegram Desktop auth from {path}"))
            .unwrap_or_else(|| "Import Telegram Desktop auth".to_string()),
        Some("list_peers") => "List Telegram peers".to_string(),
        Some("login_qr") => "Start Telegram QR login".to_string(),
        Some("login_qr_wait") => "Wait for Telegram QR login approval".to_string(),
        Some("login_start") => input
            .get("phone")
            .and_then(Value::as_str)
            .map(|phone| format!("Start Telegram login for {phone}"))
            .unwrap_or_else(|| "Start Telegram login".to_string()),
        Some("login_submit_code") => "Submit Telegram login code".to_string(),
        Some("login_submit_password") => "Submit Telegram 2FA password".to_string(),
        Some("search_messages") => input
            .get("query")
            .and_then(Value::as_str)
            .map(|query| format!("Search Telegram messages for {query}"))
            .unwrap_or_else(|| "Search Telegram messages".to_string()),
        Some("search_peers") => input
            .get("query")
            .and_then(Value::as_str)
            .map(|query| format!("Search Telegram peers for {query}"))
            .unwrap_or_else(|| "Search Telegram peers".to_string()),
        _ => "Use Telegram internal tool".to_string(),
    }
}

fn browser_permission_summary(input: &Value) -> String {
    let context = browser_permission_context_for_tool("Browser", input, "current", &[]);
    let target = browser_summary_target(&context);
    match context.action {
        Some(BrowserActionCategory::Inspect) => {
            if target == "browser tabs" {
                "Inspect browser tabs".to_string()
            } else {
                format!("Inspect {target}")
            }
        }
        Some(BrowserActionCategory::Navigate) => format!("Open {target}"),
        Some(BrowserActionCategory::Interact) => {
            if target == "current browser tab" {
                "Interact with current browser tab".to_string()
            } else {
                format!("Interact with {target}")
            }
        }
        Some(BrowserActionCategory::Evaluate) => {
            if target == "current browser tab" {
                "Run JavaScript in current browser tab".to_string()
            } else {
                format!("Run JavaScript on {target}")
            }
        }
        None => "Browser".to_string(),
    }
}

fn browser_summary_target(
    context: &crate::permissions::browser_target::BrowserPermissionContext,
) -> String {
    if let Some(target) = &context.target {
        return match target.target_class {
            BrowserTargetClass::LocalDev => target
                .raw_url
                .strip_prefix("http://")
                .map(str::to_string)
                .unwrap_or_else(|| target.raw_url.clone()),
            BrowserTargetClass::WorkspaceFile => "workspace file".to_string(),
            BrowserTargetClass::NonWorkspaceFile => "file outside workspace".to_string(),
            BrowserTargetClass::DataUrl => "data URL".to_string(),
            BrowserTargetClass::OpenWeb => target.raw_url.clone(),
        };
    }
    if context.tab_id.is_some() {
        "current browser tab".to_string()
    } else {
        "browser tabs".to_string()
    }
}

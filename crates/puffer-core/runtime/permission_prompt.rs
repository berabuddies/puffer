use puffer_tools::ToolDefinition;
use serde_json::Value;
use std::cell::RefCell;

/// Describes one runtime permission request that may need user approval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionPromptRequest {
    pub tool_id: String,
    pub summary: String,
    pub reason: Option<String>,
}

/// Describes how the user responded to a runtime permission prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionPromptAction {
    AllowOnce,
    AllowSession,
    Deny,
}

thread_local! {
    static PERMISSION_PROMPT_HANDLER: RefCell<Option<Box<dyn FnMut(PermissionPromptRequest) -> PermissionPromptAction>>> =
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

pub(crate) fn prompt_for_permission(request: PermissionPromptRequest) -> PermissionPromptAction {
    PERMISSION_PROMPT_HANDLER.with(|slot| {
        let mut borrowed = slot.borrow_mut();
        let Some(handler) = borrowed.as_mut() else {
            return PermissionPromptAction::Deny;
        };
        handler(request)
    })
}

pub(crate) fn build_permission_prompt_request(
    definition: &ToolDefinition,
    input: &Value,
    reason: Option<&str>,
) -> PermissionPromptRequest {
    PermissionPromptRequest {
        tool_id: definition.id.clone(),
        summary: permission_request_summary(definition, input),
        reason: reason.map(str::to_string),
    }
}

fn permission_request_summary(definition: &ToolDefinition, input: &Value) -> String {
    match definition.id.as_str() {
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
        "ExitPlanMode" => "Exit plan mode?".to_string(),
        _ => definition.id.clone(),
    }
}

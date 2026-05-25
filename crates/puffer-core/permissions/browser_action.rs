use crate::tool_names::canonical_tool_name;
use serde_json::Value;

/// Groups Browser intents into stable permission-facing action sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BrowserActionSet {
    Inspect,
    Navigate,
    Interact,
    Evaluate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum BrowserIntentAction {
    List,
    Snapshot,
    DomInspect,
    WaitNetworkIdle,
    ConsoleLogs,
    Screenshot,
    OpenConsoleLogs,
    OpenScreenshot,
    Open,
    New,
    Focus,
    Close,
    Quit,
    Navigate,
    Reload,
    Back,
    Forward,
    FocusRef,
    Click,
    Dblclick,
    Hover,
    Type,
    InsertText,
    Fill,
    Select,
    Upload,
    Check,
    Uncheck,
    Press,
    Keydown,
    Keyup,
    Scroll,
    ScrollIntoView,
    Evaluate,
}

impl BrowserIntentAction {
    fn action_set(self) -> BrowserActionSet {
        match self {
            Self::List
            | Self::Snapshot
            | Self::DomInspect
            | Self::WaitNetworkIdle
            | Self::ConsoleLogs
            | Self::Screenshot => BrowserActionSet::Inspect,
            Self::Open
            | Self::OpenConsoleLogs
            | Self::OpenScreenshot
            | Self::New
            | Self::Focus
            | Self::Close
            | Self::Quit
            | Self::Navigate
            | Self::Reload
            | Self::Back
            | Self::Forward => BrowserActionSet::Navigate,
            Self::FocusRef
            | Self::Click
            | Self::Dblclick
            | Self::Hover
            | Self::Type
            | Self::InsertText
            | Self::Fill
            | Self::Select
            | Self::Upload
            | Self::Check
            | Self::Uncheck
            | Self::Press
            | Self::Keydown
            | Self::Keyup
            | Self::Scroll
            | Self::ScrollIntoView => BrowserActionSet::Interact,
            Self::Evaluate => BrowserActionSet::Evaluate,
        }
    }
}

/// Returns the shared Browser action set for one Browser action or CLI alias.
pub fn browser_action_set_for_action(action: &str) -> Option<BrowserActionSet> {
    browser_intent_action(action).map(BrowserIntentAction::action_set)
}

pub(crate) fn browser_action_set_from_value(input: &Value) -> Option<BrowserActionSet> {
    input
        .get("action")
        .and_then(Value::as_str)
        .and_then(browser_action_set_for_action)
}

pub(crate) fn browser_permission_value_for_tool_call(
    tool_id: &str,
    input: &Value,
) -> Option<Value> {
    match canonical_tool_name(tool_id).as_str() {
        "browser" => browser_action_set_from_value(input).map(|_| input.clone()),
        _ => None,
    }
}

fn browser_intent_action(action: &str) -> Option<BrowserIntentAction> {
    match normalized_token(action).as_str() {
        "list" => Some(BrowserIntentAction::List),
        "snapshot" => Some(BrowserIntentAction::Snapshot),
        "dominspect" | "inspectdom" => Some(BrowserIntentAction::DomInspect),
        "waitnetworkidle" | "networkidle" => Some(BrowserIntentAction::WaitNetworkIdle),
        "consolelogs" | "console" => Some(BrowserIntentAction::ConsoleLogs),
        "screenshot" => Some(BrowserIntentAction::Screenshot),
        "openconsolelogs" | "openconsole" => Some(BrowserIntentAction::OpenConsoleLogs),
        "openscreenshot" => Some(BrowserIntentAction::OpenScreenshot),
        "open" => Some(BrowserIntentAction::Open),
        "new" => Some(BrowserIntentAction::New),
        "focus" => Some(BrowserIntentAction::Focus),
        "close" => Some(BrowserIntentAction::Close),
        "quit" | "exit" => Some(BrowserIntentAction::Quit),
        "navigate" | "goto" => Some(BrowserIntentAction::Navigate),
        "reload" => Some(BrowserIntentAction::Reload),
        "back" => Some(BrowserIntentAction::Back),
        "forward" => Some(BrowserIntentAction::Forward),
        "focusref" => Some(BrowserIntentAction::FocusRef),
        "click" => Some(BrowserIntentAction::Click),
        "dblclick" => Some(BrowserIntentAction::Dblclick),
        "hover" => Some(BrowserIntentAction::Hover),
        "type" => Some(BrowserIntentAction::Type),
        "inserttext" => Some(BrowserIntentAction::InsertText),
        "fill" => Some(BrowserIntentAction::Fill),
        "select" => Some(BrowserIntentAction::Select),
        "upload" => Some(BrowserIntentAction::Upload),
        "check" => Some(BrowserIntentAction::Check),
        "uncheck" => Some(BrowserIntentAction::Uncheck),
        "press" | "key" => Some(BrowserIntentAction::Press),
        "keydown" => Some(BrowserIntentAction::Keydown),
        "keyup" => Some(BrowserIntentAction::Keyup),
        "scroll" => Some(BrowserIntentAction::Scroll),
        "scrollintoview" | "scrollinto" => Some(BrowserIntentAction::ScrollIntoView),
        "evaluate" | "eval" => Some(BrowserIntentAction::Evaluate),
        _ => None,
    }
}

fn normalized_token(token: &str) -> String {
    token.trim().replace(['_', '-'], "").to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_set_mapping_covers_model_visible_and_shell_only_actions() {
        assert_eq!(
            browser_action_set_for_action("snapshot"),
            Some(BrowserActionSet::Inspect)
        );
        assert_eq!(
            browser_action_set_for_action("domInspect"),
            Some(BrowserActionSet::Inspect)
        );
        assert_eq!(
            browser_action_set_for_action("waitNetworkIdle"),
            Some(BrowserActionSet::Inspect)
        );
        assert_eq!(
            browser_action_set_for_action("screenshot"),
            Some(BrowserActionSet::Inspect)
        );
        assert_eq!(
            browser_action_set_for_action("consoleLogs"),
            Some(BrowserActionSet::Inspect)
        );
        assert_eq!(
            browser_action_set_for_action("openConsoleLogs"),
            Some(BrowserActionSet::Navigate)
        );
        assert_eq!(
            browser_action_set_for_action("openScreenshot"),
            Some(BrowserActionSet::Navigate)
        );
        assert_eq!(
            browser_action_set_for_action("focus_ref"),
            Some(BrowserActionSet::Interact)
        );
        assert_eq!(
            browser_action_set_for_action("insertText"),
            Some(BrowserActionSet::Interact)
        );
        assert_eq!(
            browser_action_set_for_action("scrollIntoView"),
            Some(BrowserActionSet::Interact)
        );
        assert_eq!(
            browser_action_set_for_action("eval"),
            Some(BrowserActionSet::Evaluate)
        );
        assert_eq!(
            browser_action_set_for_action("focus"),
            Some(BrowserActionSet::Navigate)
        );
    }
}

use crate::tool_names::canonical_tool_name;
use serde_json::{Map, Value};
use std::path::Path;

pub(crate) const EMBEDDED_BROWSER_PERMISSION_INPUT_KEY: &str = "__pufferBrowserPermission";
pub(crate) const AMBIGUOUS_BROWSER_SHELL_COMMAND_REASON: &str =
    "ambiguous browser shell command requires explicit approval";

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
    Screenshot,
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
            Self::List | Self::Snapshot | Self::Screenshot => BrowserActionSet::Inspect,
            Self::Open
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

    fn canonical_action(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Snapshot => "snapshot",
            Self::Screenshot => "screenshot",
            Self::Open => "open",
            Self::New => "new",
            Self::Focus => "focus",
            Self::Close => "close",
            Self::Quit => "quit",
            Self::Navigate => "navigate",
            Self::Reload => "reload",
            Self::Back => "back",
            Self::Forward => "forward",
            Self::FocusRef => "focus_ref",
            Self::Click => "click",
            Self::Dblclick => "dblclick",
            Self::Hover => "hover",
            Self::Type => "type",
            Self::InsertText => "insertText",
            Self::Fill => "fill",
            Self::Select => "select",
            Self::Upload => "upload",
            Self::Check => "check",
            Self::Uncheck => "uncheck",
            Self::Press => "press",
            Self::Keydown => "keydown",
            Self::Keyup => "keyup",
            Self::Scroll => "scroll",
            Self::ScrollIntoView => "scrollIntoView",
            Self::Evaluate => "evaluate",
        }
    }
}

/// Returns the shared Browser action set for one Browser action or CLI alias.
pub fn browser_action_set_for_action(action: &str) -> Option<BrowserActionSet> {
    browser_intent_action(action).map(BrowserIntentAction::action_set)
}

/// Returns the shared Browser action set for one simple `puffer browser ...` shell command.
pub fn browser_action_set_for_shell_command(command: &str) -> Option<BrowserActionSet> {
    browser_permission_value_from_shell_command(command)
        .as_ref()
        .and_then(browser_action_set_from_value)
}

pub(crate) fn attach_browser_permission_value(
    input: &mut Value,
    browser_input: Value,
) -> Option<()> {
    let payload = input.as_object_mut()?;
    payload.insert(
        EMBEDDED_BROWSER_PERMISSION_INPUT_KEY.to_string(),
        browser_input,
    );
    Some(())
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
    if let Some(embedded) = embedded_browser_permission_value(input) {
        return Some(embedded);
    }
    match canonical_tool_name(tool_id).as_str() {
        "browser" => browser_action_set_from_value(input).map(|_| input.clone()),
        "bash" | "powershell" => input
            .get("command")
            .and_then(Value::as_str)
            .and_then(browser_permission_value_from_shell_command),
        _ => None,
    }
}

/// Returns the Browser-shell ambiguity reason when a Bash-like command mentions
/// `puffer browser` but cannot be reduced to one stable Browser intent.
pub(crate) fn ambiguous_browser_shell_command_reason_for_tool_call(
    tool_id: &str,
    input: &Value,
) -> Option<&'static str> {
    match canonical_tool_name(tool_id).as_str() {
        "bash" | "powershell" => input
            .get("command")
            .and_then(Value::as_str)
            .filter(|command| is_ambiguous_browser_shell_command(command))
            .map(|_| AMBIGUOUS_BROWSER_SHELL_COMMAND_REASON),
        _ => None,
    }
}

pub(crate) fn browser_permission_value_from_shell_command(command: &str) -> Option<Value> {
    let tokens = shell_words::split(command).ok()?;
    if !is_simple_browser_shell_command(&tokens) {
        return None;
    }
    let args = browser_cli_args_from_tokens(&tokens)?;
    parse_browser_cli_tokens(args.to_vec())
}

fn embedded_browser_permission_value(input: &Value) -> Option<Value> {
    input.get(EMBEDDED_BROWSER_PERMISSION_INPUT_KEY).cloned()
}

fn is_simple_browser_shell_command(tokens: &[String]) -> bool {
    !tokens.is_empty()
        && !tokens.iter().any(|token| {
            matches!(
                token.as_str(),
                "|" | "||" | "&&" | ";" | "&" | ">" | ">>" | "<" | "<<" | "2>" | "2>>"
            )
        })
}

fn is_ambiguous_browser_shell_command(command: &str) -> bool {
    match shell_words::split(command) {
        Ok(tokens) => {
            contains_browser_shell_sequence(&tokens) && !is_simple_browser_shell_command(&tokens)
        }
        Err(_) => command.to_ascii_lowercase().contains("puffer browser"),
    }
}

fn contains_browser_shell_sequence(tokens: &[String]) -> bool {
    if tokens
        .iter()
        .any(|token| command_basename(token) == Some("browser"))
    {
        return true;
    }
    tokens.windows(2).any(|window| {
        command_basename(&window[0]) == Some("puffer") && normalized_token(&window[1]) == "browser"
    }) || tokens.windows(3).any(|window| {
        command_basename(&window[0]) == Some("puffer")
            && normalized_token(&window[1]) == "internaltool"
            && normalized_token(&window[2]) == "browser"
    })
}

fn command_basename(command: &str) -> Option<&str> {
    Path::new(command)
        .file_name()
        .and_then(|value| value.to_str())
}

fn browser_cli_args_from_tokens(tokens: &[String]) -> Option<&[String]> {
    let (root_command, args) = tokens.split_first()?;
    match command_basename(root_command)? {
        "browser" => Some(args),
        "puffer" if normalized_token(args.first()?) == "browser" => Some(&args[1..]),
        "puffer"
            if normalized_token(args.first()?) == "internaltool"
                && normalized_token(args.get(1)?) == "browser" =>
        {
            Some(&args[2..])
        }
        _ => None,
    }
}

fn parse_browser_cli_tokens(mut args: Vec<String>) -> Option<Value> {
    let session_id = take_option_value(&mut args, "--session-id");
    discard_flag(&mut args, "--json");
    let command = args.first()?.clone();
    let remainder = args[1..].to_vec();
    let normalized = normalized_token(&command);
    match normalized.as_str() {
        "list" => Some(browser_permission_value(
            BrowserIntentAction::List,
            session_id,
            None,
            None,
        )),
        "open" => parse_open_like(BrowserIntentAction::Open, session_id, remainder),
        "navigate" | "goto" => parse_navigate(session_id, remainder),
        "back" => parse_target_only(BrowserIntentAction::Back, session_id, remainder),
        "forward" => parse_target_only(BrowserIntentAction::Forward, session_id, remainder),
        "reload" => parse_target_only(BrowserIntentAction::Reload, session_id, remainder),
        "close" => parse_close(session_id, remainder),
        "quit" | "exit" => Some(browser_permission_value(
            BrowserIntentAction::Quit,
            session_id,
            None,
            None,
        )),
        "tab" => parse_tab_command(session_id, remainder),
        "snapshot" => parse_target_only(BrowserIntentAction::Snapshot, session_id, remainder),
        "screenshot" => parse_target_only(BrowserIntentAction::Screenshot, session_id, remainder),
        "click" => parse_target_only(BrowserIntentAction::Click, session_id, remainder),
        "dblclick" => parse_target_only(BrowserIntentAction::Dblclick, session_id, remainder),
        "hover" => parse_target_only(BrowserIntentAction::Hover, session_id, remainder),
        "focus" | "focusref" => {
            parse_target_only(BrowserIntentAction::FocusRef, session_id, remainder)
        }
        "fill" => parse_target_only(BrowserIntentAction::Fill, session_id, remainder),
        "select" => parse_target_only(BrowserIntentAction::Select, session_id, remainder),
        "upload" => parse_target_only(BrowserIntentAction::Upload, session_id, remainder),
        "check" => parse_target_only(BrowserIntentAction::Check, session_id, remainder),
        "uncheck" => parse_target_only(BrowserIntentAction::Uncheck, session_id, remainder),
        "type" => parse_target_only(BrowserIntentAction::Type, session_id, remainder),
        "press" | "key" => parse_target_only(BrowserIntentAction::Press, session_id, remainder),
        "keydown" => parse_target_only(BrowserIntentAction::Keydown, session_id, remainder),
        "keyup" => parse_target_only(BrowserIntentAction::Keyup, session_id, remainder),
        "keyboard" => parse_keyboard_command(session_id, remainder),
        "scroll" => parse_target_only(BrowserIntentAction::Scroll, session_id, remainder),
        "scrollintoview" | "scrollinto" => {
            parse_target_only(BrowserIntentAction::ScrollIntoView, session_id, remainder)
        }
        "eval" | "evaluate" => {
            parse_target_only(BrowserIntentAction::Evaluate, session_id, remainder)
        }
        _ => None,
    }
}

fn parse_tab_command(session_id: Option<String>, remainder: Vec<String>) -> Option<Value> {
    let (command, remainder) = remainder.split_first()?;
    match normalized_token(command).as_str() {
        "list" => Some(browser_permission_value(
            BrowserIntentAction::List,
            session_id,
            None,
            None,
        )),
        "new" => parse_open_like(BrowserIntentAction::New, session_id, remainder.to_vec()),
        "close" => {
            let mut args = remainder.to_vec();
            let tab_id =
                take_option_value(&mut args, "--tab-id").or_else(|| first_positional(&args));
            Some(browser_permission_value(
                BrowserIntentAction::Close,
                session_id,
                tab_id,
                None,
            ))
        }
        "focus" | "select" => {
            let mut args = remainder.to_vec();
            let tab_id =
                take_option_value(&mut args, "--tab-id").or_else(|| first_positional(&args));
            Some(browser_permission_value(
                BrowserIntentAction::Focus,
                session_id,
                tab_id,
                None,
            ))
        }
        _ => None,
    }
}

fn parse_keyboard_command(session_id: Option<String>, remainder: Vec<String>) -> Option<Value> {
    let (command, remainder) = remainder.split_first()?;
    match normalized_token(command).as_str() {
        "type" => parse_target_only(BrowserIntentAction::Type, session_id, remainder.to_vec()),
        "inserttext" => parse_target_only(
            BrowserIntentAction::InsertText,
            session_id,
            remainder.to_vec(),
        ),
        _ => None,
    }
}

fn parse_open_like(
    action: BrowserIntentAction,
    session_id: Option<String>,
    mut args: Vec<String>,
) -> Option<Value> {
    let tab_id = take_option_value(&mut args, "--tab-id");
    let url = first_positional(&args);
    Some(browser_permission_value(action, session_id, tab_id, url))
}

fn parse_navigate(session_id: Option<String>, mut args: Vec<String>) -> Option<Value> {
    let tab_id = take_option_value(&mut args, "--tab-id");
    let url = first_positional(&args)?;
    Some(browser_permission_value(
        BrowserIntentAction::Navigate,
        session_id,
        tab_id,
        Some(url),
    ))
}

fn parse_close(session_id: Option<String>, mut args: Vec<String>) -> Option<Value> {
    if discard_flag(&mut args, "--group") {
        return Some(browser_permission_value(
            BrowserIntentAction::Quit,
            session_id,
            None,
            None,
        ));
    }
    let tab_id = take_option_value(&mut args, "--tab-id");
    Some(browser_permission_value(
        BrowserIntentAction::Close,
        session_id,
        tab_id,
        None,
    ))
}

fn parse_target_only(
    action: BrowserIntentAction,
    session_id: Option<String>,
    mut args: Vec<String>,
) -> Option<Value> {
    let tab_id = take_option_value(&mut args, "--tab-id");
    Some(browser_permission_value(action, session_id, tab_id, None))
}

fn browser_permission_value(
    action: BrowserIntentAction,
    session_id: Option<String>,
    tab_id: Option<String>,
    url: Option<String>,
) -> Value {
    let mut payload = Map::new();
    payload.insert(
        "action".to_string(),
        Value::String(action.canonical_action().to_string()),
    );
    if let Some(session_id) = session_id.filter(|value| !value.trim().is_empty()) {
        payload.insert("sessionId".to_string(), Value::String(session_id));
    }
    if let Some(tab_id) = tab_id.filter(|value| !value.trim().is_empty()) {
        payload.insert("tabId".to_string(), Value::String(tab_id));
    }
    if let Some(url) = url.filter(|value| !value.trim().is_empty()) {
        payload.insert("url".to_string(), Value::String(url));
    }
    Value::Object(payload)
}

fn browser_intent_action(action: &str) -> Option<BrowserIntentAction> {
    match normalized_token(action).as_str() {
        "list" => Some(BrowserIntentAction::List),
        "snapshot" => Some(BrowserIntentAction::Snapshot),
        "screenshot" => Some(BrowserIntentAction::Screenshot),
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

fn take_option_value(args: &mut Vec<String>, option: &str) -> Option<String> {
    let index = args.iter().position(|token| token == option)?;
    if index + 1 >= args.len() {
        return None;
    }
    let value = args.remove(index + 1);
    args.remove(index);
    Some(value)
}

fn discard_flag(args: &mut Vec<String>, flag: &str) -> bool {
    let Some(index) = args.iter().position(|token| token == flag) else {
        return false;
    };
    args.remove(index);
    true
}

fn first_positional(args: &[String]) -> Option<String> {
    let mut index = 0usize;
    while index < args.len() {
        let token = &args[index];
        if token.starts_with("--") {
            index += if option_takes_value(token) { 2 } else { 1 };
            continue;
        }
        return Some(token.clone());
    }
    None
}

fn option_takes_value(option: &str) -> bool {
    matches!(
        option,
        "--session-id"
            | "--tab-id"
            | "--label"
            | "--width"
            | "--height"
            | "--ref"
            | "--screenshot-dir"
            | "--screenshot-format"
            | "--screenshot-quality"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn action_set_mapping_covers_model_visible_and_shell_only_actions() {
        assert_eq!(
            browser_action_set_for_action("snapshot"),
            Some(BrowserActionSet::Inspect)
        );
        assert_eq!(
            browser_action_set_for_action("screenshot"),
            Some(BrowserActionSet::Inspect)
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

    #[test]
    fn shell_browser_commands_map_to_same_action_sets() {
        assert_eq!(
            browser_action_set_for_shell_command("puffer browser screenshot --tab-id t1"),
            Some(BrowserActionSet::Inspect)
        );
        assert_eq!(
            browser_action_set_for_shell_command("browser screenshot --tab-id t1"),
            Some(BrowserActionSet::Inspect)
        );
        assert_eq!(
            browser_action_set_for_shell_command(
                "puffer internal-tool browser screenshot --tab-id t1"
            ),
            Some(BrowserActionSet::Inspect)
        );
        assert_eq!(
            browser_action_set_for_shell_command("puffer browser upload @e1 file.txt"),
            Some(BrowserActionSet::Interact)
        );
        assert_eq!(
            browser_action_set_for_shell_command("puffer browser tab focus t7"),
            Some(BrowserActionSet::Navigate)
        );
        assert_eq!(
            browser_action_set_for_shell_command("puffer browser evaluate document.title"),
            Some(BrowserActionSet::Evaluate)
        );
    }

    #[test]
    fn shell_browser_commands_normalize_into_browser_permission_payloads() {
        assert_eq!(
            browser_permission_value_from_shell_command(
                "puffer browser tab focus t7 --session-id root-2"
            ),
            Some(json!({
                "action": "focus",
                "sessionId": "root-2",
                "tabId": "t7"
            }))
        );
        assert_eq!(
            browser_permission_value_from_shell_command(
                "puffer internal-tool browser navigate https://example.com --tab-id t3"
            ),
            Some(json!({
                "action": "navigate",
                "tabId": "t3",
                "url": "https://example.com"
            }))
        );
        assert_eq!(
            browser_permission_value_from_shell_command(
                "puffer browser navigate https://example.com --tab-id t3"
            ),
            Some(json!({
                "action": "navigate",
                "tabId": "t3",
                "url": "https://example.com"
            }))
        );
        assert_eq!(
            browser_permission_value_from_shell_command("puffer browser close --group"),
            Some(json!({
                "action": "quit"
            }))
        );
    }

    #[test]
    fn compound_browser_shell_commands_are_not_reduced_to_single_intent() {
        assert_eq!(
            browser_permission_value_from_shell_command(
                "puffer browser navigate https://example.com && puffer browser snapshot"
            ),
            None
        );
        assert_eq!(
            ambiguous_browser_shell_command_reason_for_tool_call(
                "Bash",
                &json!({
                    "command":"puffer browser navigate https://example.com && puffer browser snapshot"
                })
            ),
            Some(AMBIGUOUS_BROWSER_SHELL_COMMAND_REASON)
        );
    }
}

//! CDP input serialization for the managed browser worker.

use anyhow::Result;
use serde_json::{json, Map, Value};
use std::net::TcpStream;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::WebSocket;

use super::{send_cdp, BrowserInputEvent};

/// Dispatches one UI or agent input event to Chrome through CDP.
pub(super) fn send_input(
    socket: &mut WebSocket<MaybeTlsStream<TcpStream>>,
    next_id: &mut u64,
    event: BrowserInputEvent,
) -> Result<u64> {
    let (method, params): (&str, Value) = match event {
        BrowserInputEvent::Mouse {
            event_type,
            x,
            y,
            button,
            buttons,
            click_count,
        } => {
            let mut params = json!({
                "type": event_type,
                "x": x,
                "y": y,
                "button": button,
                "clickCount": click_count
            });
            if let Some(buttons) = buttons {
                params["buttons"] = json!(buttons);
            }
            ("Input.dispatchMouseEvent", params)
        }
        BrowserInputEvent::Wheel {
            x,
            y,
            delta_x,
            delta_y,
        } => (
            "Input.dispatchMouseEvent",
            json!({
                "type": "mouseWheel",
                "x": x,
                "y": y,
                "deltaX": delta_x,
                "deltaY": delta_y
            }),
        ),
        BrowserInputEvent::Key {
            event_type,
            key,
            code,
            text,
            modifiers,
            commands,
        } => (
            "Input.dispatchKeyEvent",
            key_event_params(event_type, key, code, text, modifiers, commands),
        ),
        BrowserInputEvent::Text { text } => ("Input.insertText", json!({ "text": text })),
    };
    Ok(send_cdp(socket, next_id, method, params))
}

fn key_event_params(
    event_type: String,
    key: String,
    code: String,
    text: Option<String>,
    modifiers: u32,
    commands: Vec<String>,
) -> Value {
    let key = normalized_key_value(&key);
    let code = normalized_code_value(&key, &code);
    let event_text = key_event_text(&event_type, &key, text);
    let mut params = Map::new();
    params.insert("type".to_string(), json!(event_type));
    params.insert("key".to_string(), json!(key));
    params.insert("code".to_string(), json!(code));
    params.insert("modifiers".to_string(), json!(modifiers));
    if let Some(key_code) = virtual_key_code(
        params
            .get("key")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        params
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    ) {
        // Only emit windowsVirtualKeyCode (drives the web-visible DOM keyCode/which).
        // Deliberately omit nativeVirtualKeyCode: setting native_key_code makes Chromium
        // synthesize a real macOS NSEvent (NativeInputEventBuilder), which can wedge CDP via
        // an OS-level key-autorepeat storm (agentenv#636). Dropping it keeps dispatch on the
        // renderer-only path while leaving page-side key handling unaffected.
        params.insert("windowsVirtualKeyCode".to_string(), json!(key_code));
    }
    params.insert(
        "location".to_string(),
        json!(key_location(
            params
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or_default()
        )),
    );
    if let Some(text) = event_text.filter(|value| !value.is_empty()) {
        params.insert("text".to_string(), json!(text));
        params.insert("unmodifiedText".to_string(), json!(text));
    }
    if !commands.is_empty() {
        params.insert("commands".to_string(), json!(commands));
    }
    Value::Object(params)
}

fn key_event_text(event_type: &str, key: &str, text: Option<String>) -> Option<String> {
    if event_type == "keyUp" {
        return None;
    }
    if key == "Enter" {
        return Some("\r".to_string());
    }
    if key == " " {
        return Some(" ".to_string());
    }
    text
}

fn normalized_key_value(key: &str) -> String {
    match key {
        "Space" => " ".to_string(),
        _ => key.to_string(),
    }
}

fn normalized_code_value(key: &str, code: &str) -> String {
    if !code.is_empty() && code != key {
        return code.to_string();
    }
    code_from_printable_key(key).unwrap_or_else(|| code.to_string())
}

fn virtual_key_code(key: &str, code: &str) -> Option<u32> {
    match key {
        "Backspace" => Some(8),
        "Tab" => Some(9),
        "Enter" => Some(13),
        "Shift" => Some(16),
        "Control" => Some(17),
        "Alt" => Some(18),
        "CapsLock" => Some(20),
        "Escape" => Some(27),
        "Esc" => Some(27),
        " " => Some(32),
        "Space" => Some(32),
        "PageUp" => Some(33),
        "PageDown" => Some(34),
        "End" => Some(35),
        "Home" => Some(36),
        "ArrowLeft" => Some(37),
        "ArrowUp" => Some(38),
        "ArrowRight" => Some(39),
        "ArrowDown" => Some(40),
        "Insert" => Some(45),
        "Delete" => Some(46),
        "Meta" => virtual_key_code_from_code(code).or(Some(91)),
        "ContextMenu" => Some(93),
        _ => virtual_key_code_from_code(code).or_else(|| virtual_key_code_from_key(key)),
    }
}

fn virtual_key_code_from_code(code: &str) -> Option<u32> {
    let mapped = match code {
        "Backspace" => 8,
        "Tab" => 9,
        "Enter" | "NumpadEnter" => 13,
        "ShiftLeft" | "ShiftRight" => 16,
        "ControlLeft" | "ControlRight" => 17,
        "AltLeft" | "AltRight" => 18,
        "Pause" => 19,
        "CapsLock" => 20,
        "Escape" => 27,
        "Space" => 32,
        "PageUp" => 33,
        "PageDown" => 34,
        "End" => 35,
        "Home" => 36,
        "ArrowLeft" => 37,
        "ArrowUp" => 38,
        "ArrowRight" => 39,
        "ArrowDown" => 40,
        "PrintScreen" => 44,
        "Insert" => 45,
        "Delete" => 46,
        "MetaLeft" => 91,
        "MetaRight" => 92,
        "ContextMenu" => 93,
        "NumpadMultiply" => 106,
        "NumpadAdd" => 107,
        "NumpadSubtract" => 109,
        "NumpadDecimal" => 110,
        "NumpadDivide" => 111,
        "Semicolon" => 186,
        "Equal" => 187,
        "Comma" => 188,
        "Minus" => 189,
        "Period" => 190,
        "Slash" => 191,
        "Backquote" => 192,
        "BracketLeft" => 219,
        "Backslash" => 220,
        "BracketRight" => 221,
        "Quote" => 222,
        _ => 0,
    };
    if mapped != 0 {
        return Some(mapped);
    }
    if let Some(letter) = code.strip_prefix("Key") {
        return single_ascii(letter)
            .filter(u8::is_ascii_uppercase)
            .map(u32::from);
    }
    if let Some(digit) = code.strip_prefix("Digit") {
        return single_ascii(digit)
            .filter(u8::is_ascii_digit)
            .map(u32::from);
    }
    if let Some(digit) = code.strip_prefix("Numpad") {
        return single_ascii(digit)
            .filter(u8::is_ascii_digit)
            .map(|value| u32::from(value - b'0') + 96);
    }
    if let Some(number) = code.strip_prefix('F') {
        if let Ok(function_key) = number.parse::<u32>() {
            if (1..=24).contains(&function_key) {
                return Some(111 + function_key);
            }
        }
    }
    None
}

fn virtual_key_code_from_key(key: &str) -> Option<u32> {
    code_from_printable_key(key).and_then(|code| virtual_key_code_from_code(&code))
}

fn code_from_printable_key(key: &str) -> Option<String> {
    let mapped = match key {
        " " => "Space",
        "`" | "~" => "Backquote",
        "-" | "_" => "Minus",
        "=" | "+" => "Equal",
        "[" | "{" => "BracketLeft",
        "]" | "}" => "BracketRight",
        "\\" | "|" => "Backslash",
        ";" | ":" => "Semicolon",
        "'" | "\"" => "Quote",
        "," | "<" => "Comma",
        "." | ">" => "Period",
        "/" | "?" => "Slash",
        ")" => "Digit0",
        "!" => "Digit1",
        "@" => "Digit2",
        "#" => "Digit3",
        "$" => "Digit4",
        "%" => "Digit5",
        "^" => "Digit6",
        "&" => "Digit7",
        "*" => "Digit8",
        "(" => "Digit9",
        _ => "",
    };
    if !mapped.is_empty() {
        return Some(mapped.to_string());
    }
    single_ascii(key)
        .filter(u8::is_ascii_alphabetic)
        .map(|value| format!("Key{}", char::from(value).to_ascii_uppercase()))
        .or_else(|| {
            single_ascii(key)
                .filter(u8::is_ascii_digit)
                .map(|value| format!("Digit{}", char::from(value)))
        })
}

fn key_location(code: &str) -> u32 {
    match code {
        "ShiftLeft" | "ControlLeft" | "AltLeft" | "MetaLeft" => 1,
        "ShiftRight" | "ControlRight" | "AltRight" | "MetaRight" => 2,
        code if code.starts_with("Numpad") => 3,
        _ => 0,
    }
}

fn single_ascii(value: &str) -> Option<u8> {
    let bytes = value.as_bytes();
    (bytes.len() == 1).then_some(bytes[0])
}

#[cfg(test)]
mod tests {
    use super::key_event_params;

    #[test]
    fn editing_commands_are_forwarded_to_cdp() {
        let params = key_event_params(
            "rawKeyDown".to_string(),
            "a".to_string(),
            "KeyA".to_string(),
            None,
            4,
            vec!["selectAll".to_string()],
        );

        assert_eq!(params["commands"], serde_json::json!(["selectAll"]));
        assert_eq!(params["modifiers"], 4);
    }

    #[test]
    fn enter_key_event_includes_virtual_key_and_carriage_return() {
        let params = key_event_params(
            "rawKeyDown".to_string(),
            "Enter".to_string(),
            "Enter".to_string(),
            None,
            0,
            Vec::new(),
        );

        assert_eq!(params["windowsVirtualKeyCode"], 13);
        assert!(params.get("nativeVirtualKeyCode").is_none());
        assert_eq!(params["location"], 0);
        assert_eq!(params["text"], "\r");
        assert_eq!(params["unmodifiedText"], "\r");
    }

    #[test]
    fn backspace_key_event_includes_virtual_key_without_text() {
        let params = key_event_params(
            "rawKeyDown".to_string(),
            "Backspace".to_string(),
            "Backspace".to_string(),
            None,
            0,
            Vec::new(),
        );

        assert_eq!(params["windowsVirtualKeyCode"], 8);
        assert!(params.get("nativeVirtualKeyCode").is_none());
        assert_eq!(params["location"], 0);
        assert!(params.get("text").is_none());
        assert!(params.get("unmodifiedText").is_none());
    }

    #[test]
    fn printable_key_event_keeps_text_and_virtual_key() {
        let params = key_event_params(
            "keyDown".to_string(),
            "a".to_string(),
            "KeyA".to_string(),
            Some("a".to_string()),
            0,
            Vec::new(),
        );

        assert_eq!(params["windowsVirtualKeyCode"], 65);
        assert!(params.get("nativeVirtualKeyCode").is_none());
        assert_eq!(params["location"], 0);
        assert_eq!(params["text"], "a");
        assert_eq!(params["unmodifiedText"], "a");
    }

    #[test]
    fn key_up_event_omits_text() {
        let params = key_event_params(
            "keyUp".to_string(),
            "Enter".to_string(),
            "Enter".to_string(),
            None,
            0,
            Vec::new(),
        );

        assert_eq!(params["windowsVirtualKeyCode"], 13);
        assert!(params.get("nativeVirtualKeyCode").is_none());
        assert!(params.get("text").is_none());
        assert!(params.get("unmodifiedText").is_none());
    }

    #[test]
    fn named_space_key_is_normalized_with_text() {
        let params = key_event_params(
            "rawKeyDown".to_string(),
            "Space".to_string(),
            "Space".to_string(),
            None,
            0,
            Vec::new(),
        );

        assert_eq!(params["key"], " ");
        assert_eq!(params["code"], "Space");
        assert_eq!(params["windowsVirtualKeyCode"], 32);
        assert!(params.get("nativeVirtualKeyCode").is_none());
        assert_eq!(params["text"], " ");
        assert_eq!(params["unmodifiedText"], " ");
    }

    #[test]
    fn punctuation_code_maps_to_virtual_key() {
        let params = key_event_params(
            "rawKeyDown".to_string(),
            ":".to_string(),
            "Semicolon".to_string(),
            Some(":".to_string()),
            8,
            Vec::new(),
        );

        assert_eq!(params["code"], "Semicolon");
        assert_eq!(params["windowsVirtualKeyCode"], 186);
        assert!(params.get("nativeVirtualKeyCode").is_none());
        assert_eq!(params["location"], 0);
        assert_eq!(params["text"], ":");
    }

    #[test]
    fn punctuation_key_without_dom_code_gets_canonical_code() {
        let params = key_event_params(
            "rawKeyDown".to_string(),
            "?".to_string(),
            "?".to_string(),
            Some("?".to_string()),
            8,
            Vec::new(),
        );

        assert_eq!(params["code"], "Slash");
        assert_eq!(params["windowsVirtualKeyCode"], 191);
        assert!(params.get("nativeVirtualKeyCode").is_none());
        assert_eq!(params["text"], "?");
    }

    #[test]
    fn modifier_code_maps_virtual_key_and_location() {
        let params = key_event_params(
            "rawKeyDown".to_string(),
            "Shift".to_string(),
            "ShiftRight".to_string(),
            None,
            8,
            Vec::new(),
        );

        assert_eq!(params["windowsVirtualKeyCode"], 16);
        assert!(params.get("nativeVirtualKeyCode").is_none());
        assert_eq!(params["location"], 2);
        assert!(params.get("text").is_none());
    }

    #[test]
    fn native_virtual_key_code_is_omitted_to_avoid_macos_autorepeat_storm() {
        // Regression guard for agentenv#636: emitting nativeVirtualKeyCode sets
        // native_key_code, making Chromium build a real macOS NSEvent that can trigger
        // an OS-level key-autorepeat storm wedging CDP. We must keep windowsVirtualKeyCode
        // (drives the web-visible DOM keyCode) but never emit nativeVirtualKeyCode.
        for (key, code) in [("Enter", "Enter"), ("a", "KeyA"), ("Shift", "ShiftLeft")] {
            let params = key_event_params(
                "rawKeyDown".to_string(),
                key.to_string(),
                code.to_string(),
                None,
                0,
                Vec::new(),
            );

            assert!(
                params.get("windowsVirtualKeyCode").is_some(),
                "windowsVirtualKeyCode must be present for {key}"
            );
            assert!(
                params.get("nativeVirtualKeyCode").is_none(),
                "nativeVirtualKeyCode must be absent for {key} (agentenv#636)"
            );
        }
    }
}

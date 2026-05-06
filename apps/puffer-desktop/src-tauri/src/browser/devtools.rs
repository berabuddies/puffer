use serde_json::{json, Value};

use crate::events::EventEmitter;

/// Emits a renderer-facing DevTools event when `method` is one the UI consumes.
pub(super) fn emit_devtools_event(
    events: &EventEmitter,
    channel: &str,
    method: &str,
    value: &Value,
) -> bool {
    match method {
        "Runtime.consoleAPICalled" => emit(
            events,
            channel,
            json!({
                "kind": "console",
                "level": value.pointer("/params/type").and_then(Value::as_str).unwrap_or("log"),
                "text": console_args_text(value.pointer("/params/args").and_then(Value::as_array)),
                "timestamp": value.pointer("/params/timestamp").and_then(Value::as_f64)
            }),
        ),
        "Log.entryAdded" => {
            let entry = value
                .pointer("/params/entry")
                .cloned()
                .unwrap_or_else(|| json!({}));
            emit(
                events,
                channel,
                json!({
                    "kind": "console",
                    "level": entry.get("level").and_then(Value::as_str).unwrap_or("log"),
                    "text": entry.get("text").and_then(Value::as_str).unwrap_or(""),
                    "url": entry.get("url").and_then(Value::as_str).unwrap_or(""),
                    "timestamp": entry.get("timestamp").and_then(Value::as_f64)
                }),
            )
        }
        "Network.requestWillBeSent" => {
            let request = value
                .pointer("/params/request")
                .cloned()
                .unwrap_or_else(|| json!({}));
            emit(
                events,
                channel,
                json!({
                    "kind": "network",
                    "phase": "request",
                    "requestId": value.pointer("/params/requestId").and_then(Value::as_str).unwrap_or(""),
                    "method": request.get("method").and_then(Value::as_str).unwrap_or(""),
                    "url": request.get("url").and_then(Value::as_str).unwrap_or("")
                }),
            )
        }
        "Network.responseReceived" => {
            let response = value
                .pointer("/params/response")
                .cloned()
                .unwrap_or_else(|| json!({}));
            emit(
                events,
                channel,
                json!({
                    "kind": "network",
                    "phase": "response",
                    "requestId": value.pointer("/params/requestId").and_then(Value::as_str).unwrap_or(""),
                    "status": response.get("status").and_then(Value::as_u64).unwrap_or(0),
                    "url": response.get("url").and_then(Value::as_str).unwrap_or(""),
                    "mimeType": response.get("mimeType").and_then(Value::as_str).unwrap_or("")
                }),
            )
        }
        "Network.loadingFailed" => emit(
            events,
            channel,
            json!({
                "kind": "network",
                "phase": "failed",
                "requestId": value.pointer("/params/requestId").and_then(Value::as_str).unwrap_or(""),
                "errorText": value.pointer("/params/errorText").and_then(Value::as_str).unwrap_or("")
            }),
        ),
        _ => return false,
    }
    true
}

fn console_args_text(args: Option<&Vec<Value>>) -> String {
    args.map(|values| {
        values
            .iter()
            .map(|arg| {
                arg.get("value")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
                    .or_else(|| arg.get("value").map(Value::to_string))
                    .or_else(|| {
                        arg.get("description")
                            .and_then(Value::as_str)
                            .map(ToString::to_string)
                    })
                    .unwrap_or_else(|| {
                        arg.get("type")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string()
                    })
            })
            .collect::<Vec<_>>()
            .join(" ")
    })
    .unwrap_or_default()
}

fn emit(events: &EventEmitter, channel: &str, payload: Value) {
    events.emit(channel.to_string(), payload);
}

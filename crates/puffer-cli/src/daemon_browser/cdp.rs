use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::net::TcpStream;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use super::BrowserEvaluation;

/// Decodes one CDP evaluation response into the Browser API result shape.
pub(crate) fn parse_evaluation_response(value: &Value) -> Result<BrowserEvaluation> {
    if let Some(details) = value.pointer("/result/exceptionDetails") {
        let description = details
            .pointer("/exception/description")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::trim)
            .map(ToString::to_string);
        let text = details
            .get("text")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::trim)
            .map(ToString::to_string);
        let line_number = details.get("lineNumber").and_then(Value::as_u64);
        let column_number = details.get("columnNumber").and_then(Value::as_u64);
        let message = description
            .or(text)
            .unwrap_or_else(|| "unknown browser exception".to_string());
        if let (Some(line), Some(column)) = (line_number, column_number) {
            bail!(
                "browser evaluation failed at line {}, column {}: {}",
                line + 1,
                column + 1,
                message
            );
        }
        bail!("browser evaluation failed: {message}");
    }
    let Some(result) = value.pointer("/result/result") else {
        bail!("browser evaluation returned no result");
    };
    Ok(BrowserEvaluation {
        value: result.get("value").cloned().unwrap_or(Value::Null),
    })
}

/// Decodes one raw CDP method response (e.g. `DOM.getDocument`,
/// `DOM.getContentQuads`, `Page.getFrameTree`), returning the `result` object
/// directly. Unlike [`parse_evaluation_response`], these methods put their
/// payload under `/result`, not `/result/result/value`. A protocol-level
/// `error` is surfaced as an `Err`.
pub(crate) fn parse_cdp_call_response(value: &Value) -> Result<Value> {
    if let Some(error) = value.get("error") {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown CDP error");
        if let Some(code) = error.get("code").and_then(Value::as_i64) {
            bail!("CDP call failed ({code}): {message}");
        }
        bail!("CDP call failed: {message}");
    }
    // `Runtime.callFunctionOn` reports a thrown script error via
    // `result.exceptionDetails`, not a top-level `error`. Surface it rather than
    // returning the exception payload as if it were a successful result.
    if let Some(details) = value.pointer("/result/exceptionDetails") {
        let message = details
            .pointer("/exception/description")
            .and_then(Value::as_str)
            .or_else(|| details.get("text").and_then(Value::as_str))
            .map(str::trim)
            .filter(|message| !message.is_empty())
            .unwrap_or("unknown script exception");
        bail!("CDP call raised: {message}");
    }
    let Some(result) = value.get("result") else {
        bail!("CDP call returned no result");
    };
    Ok(result.clone())
}

/// Sends one raw CDP message and returns the assigned request id.
pub(crate) fn send_cdp(
    socket: &mut WebSocket<MaybeTlsStream<TcpStream>>,
    next_id: &mut u64,
    method: &str,
    params: Value,
) -> u64 {
    let id = *next_id;
    *next_id += 1;
    let _ = socket.send(Message::Text(
        json!({ "id": id, "method": method, "params": params })
            .to_string()
            .into(),
    ));
    id
}

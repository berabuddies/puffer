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

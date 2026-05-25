//! Shared Chrome DevTools Protocol helpers for browser sessions.

use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::net::TcpStream;
use std::time::Duration;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};
use url::Url;

use super::DEFAULT_URL;

/// Applies a viewport size to a Chrome DevTools Protocol page target.
pub(crate) fn apply_viewport(
    socket: &mut WebSocket<MaybeTlsStream<TcpStream>>,
    next_id: &mut u64,
    width: u32,
    height: u32,
) -> Result<u64> {
    Ok(send_cdp(
        socket,
        next_id,
        "Emulation.setDeviceMetricsOverride",
        json!({
            "width": width,
            "height": height,
            "deviceScaleFactor": 1,
            "mobile": false
        }),
    ))
}

/// Starts the Chrome screencast stream for the current page target.
pub(crate) fn start_screencast(
    socket: &mut WebSocket<MaybeTlsStream<TcpStream>>,
    next_id: &mut u64,
    width: u32,
    height: u32,
) -> Result<u64> {
    Ok(send_cdp(
        socket,
        next_id,
        "Page.startScreencast",
        json!({
            "format": "jpeg",
            "quality": 70,
            "maxWidth": width,
            "maxHeight": height,
            "everyNthFrame": 1
        }),
    ))
}

/// Sends one Chrome DevTools Protocol command and returns its request id.
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

/// Sets the read timeout on plain TCP Chrome DevTools sockets.
pub(crate) fn set_read_timeout(
    socket: &WebSocket<MaybeTlsStream<TcpStream>>,
    timeout: Option<Duration>,
) {
    let stream = socket.get_ref();
    let tcp: &TcpStream = match stream {
        MaybeTlsStream::Plain(s) => s,
        _ => return,
    };
    let _ = tcp.set_read_timeout(timeout);
}

/// Normalizes user-provided browser URLs into Chrome-loadable URLs.
pub(crate) fn normalize_url(raw: &str) -> Result<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(DEFAULT_URL.to_string());
    }
    if trimmed == DEFAULT_URL {
        return Ok(DEFAULT_URL.to_string());
    }
    if let Ok(parsed) = Url::parse(trimmed) {
        if matches!(parsed.scheme(), "http" | "https" | "file" | "data") {
            return Ok(trimmed.to_string());
        }
    }
    let with_scheme = if trimmed.starts_with("localhost")
        || trimmed.starts_with("127.")
        || trimmed.starts_with("[::1]")
    {
        format!("http://{trimmed}")
    } else {
        format!("https://{trimmed}")
    };
    Url::parse(&with_scheme).with_context(|| format!("invalid browser URL `{raw}`"))?;
    Ok(with_scheme)
}

/// Builds the renderer-facing frame id from a CDP screencast session id.
pub(crate) fn frame_session_id_string(session_id: &str, cdp_session_id: Option<&Value>) -> String {
    match cdp_session_id.and_then(Value::as_i64) {
        Some(value) => format!("{session_id}:{value}"),
        None => session_id.to_string(),
    }
}

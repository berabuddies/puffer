//! Shared CDP worker helpers for managed browser page sessions.

use anyhow::Result;
use serde_json::{json, Value};
use std::net::TcpStream;
use std::time::Duration;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::WebSocket;

use super::send_cdp;

/// Applies one viewport size override to the current page target.
pub(super) fn apply_viewport(
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

/// Starts the low-rate JPEG screencast used by the Browser panel.
pub(super) fn start_screencast(
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

/// Requests one fresh `{ url, title }` state sample from the page.
pub(super) fn send_state_eval(
    socket: &mut WebSocket<MaybeTlsStream<TcpStream>>,
    next_id: &mut u64,
) -> u64 {
    send_cdp(
        socket,
        next_id,
        "Runtime.evaluate",
        json!({
            "expression": "({ url: location.href, title: document.title })",
            "returnByValue": true
        }),
    )
}

/// Releases one remote runtime object after an upload helper flow completes.
pub(super) fn release_remote_object(
    socket: &mut WebSocket<MaybeTlsStream<TcpStream>>,
    next_id: &mut u64,
    object_id: &str,
) {
    let _ = send_cdp(
        socket,
        next_id,
        "Runtime.releaseObject",
        json!({ "objectId": object_id }),
    );
}

/// Sets the underlying WebSocket read timeout for the page worker loop.
pub(super) fn set_read_timeout(
    socket: &WebSocket<MaybeTlsStream<TcpStream>>,
    timeout: Option<Duration>,
) {
    let stream = socket.get_ref();
    let tcp: &TcpStream = match stream {
        MaybeTlsStream::Plain(stream) => stream,
        MaybeTlsStream::Rustls(stream) => stream.get_ref(),
        _ => return,
    };
    let _ = tcp.set_read_timeout(timeout);
}

/// Formats one screencast frame id from the backend session id and CDP frame id.
pub(super) fn frame_session_id_string(session_id: &str, cdp_session_id: Option<&Value>) -> String {
    match cdp_session_id.and_then(Value::as_i64) {
        Some(value) => format!("{session_id}:{value}"),
        None => session_id.to_string(),
    }
}

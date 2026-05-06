use crate::backend::BackendState;
use crate::events::{subscribe_ws_events, EventEmitter};
use serde_json::{json, Value};
use std::io::ErrorKind;
use std::net::TcpListener;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tungstenite::{accept, Error as WsError, Message, WebSocket};

const DEFAULT_BIND: &str = "127.0.0.1:1421";

pub(crate) fn start_backend_ws(state: Arc<BackendState>) {
    let bind =
        std::env::var("CORBINA_BACKEND_WS_BIND").unwrap_or_else(|_| DEFAULT_BIND.to_string());
    thread::spawn(move || {
        let listener = match TcpListener::bind(&bind) {
            Ok(listener) => listener,
            Err(error) => {
                eprintln!("failed to bind Corbina backend WebSocket on {bind}: {error}");
                return;
            }
        };
        eprintln!("Corbina backend WebSocket listening on ws://{bind}/ws");

        for stream in listener.incoming() {
            let Ok(stream) = stream else {
                continue;
            };
            let state = state.clone();
            thread::spawn(move || {
                if let Err(error) = handle_connection(state, stream) {
                    eprintln!("Corbina backend WebSocket connection ended: {error}");
                }
            });
        }
    });
}

fn handle_connection(state: Arc<BackendState>, stream: std::net::TcpStream) -> anyhow::Result<()> {
    let mut socket = accept(stream)?;
    socket.get_mut().set_nonblocking(true)?;
    let events = subscribe_ws_events();
    let emitter = EventEmitter::websocket_only();

    loop {
        while let Ok(message) = events.try_recv() {
            send_message(&mut socket, Message::Text(message))?;
        }

        match socket.read() {
            Ok(Message::Text(text)) => {
                let response = handle_request(&state, &emitter, &text);
                send_message(&mut socket, Message::Text(response))?;
            }
            Ok(Message::Binary(_)) => {}
            Ok(Message::Ping(payload)) => send_message(&mut socket, Message::Pong(payload))?,
            Ok(Message::Pong(_)) => {}
            Ok(Message::Close(frame)) => {
                socket.close(frame)?;
                return Ok(());
            }
            Ok(Message::Frame(_)) => {}
            Err(WsError::Io(error)) if error.kind() == ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(8));
            }
            Err(WsError::ConnectionClosed) | Err(WsError::AlreadyClosed) => return Ok(()),
            Err(error) => return Err(error.into()),
        }
    }
}

fn send_message(
    socket: &mut WebSocket<std::net::TcpStream>,
    message: Message,
) -> Result<(), WsError> {
    loop {
        match socket.send(message.clone()) {
            Ok(()) => return Ok(()),
            Err(WsError::Io(error)) if error.kind() == ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(8));
            }
            Err(error) => return Err(error),
        }
    }
}

fn handle_request(state: &BackendState, emitter: &EventEmitter, text: &str) -> String {
    let request: Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(error) => {
            return json!({
                "type": "response",
                "id": Value::Null,
                "ok": false,
                "error": format!("invalid JSON request: {error}"),
            })
            .to_string();
        }
    };

    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let method = request.get("method").and_then(Value::as_str).unwrap_or("");
    let params = request.get("params").cloned().unwrap_or_else(|| json!({}));

    match state.handle(emitter.clone(), method, params) {
        Ok(result) => json!({
            "type": "response",
            "id": id,
            "ok": true,
            "result": result,
        }),
        Err(error) => json!({
            "type": "response",
            "id": id,
            "ok": false,
            "error": error.to_string(),
        }),
    }
    .to_string()
}

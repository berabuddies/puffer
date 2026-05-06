use serde_json::{json, Value};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Mutex, OnceLock};
use tauri::{AppHandle, Emitter};

pub(crate) const EVENT_BRIDGE: &str = "corbina:event";

static WS_EVENT_SUBSCRIBERS: OnceLock<Mutex<Vec<Sender<String>>>> = OnceLock::new();

fn subscribers() -> &'static Mutex<Vec<Sender<String>>> {
    WS_EVENT_SUBSCRIBERS.get_or_init(|| Mutex::new(Vec::new()))
}

pub(crate) fn subscribe_ws_events() -> Receiver<String> {
    let (tx, rx) = mpsc::channel();
    subscribers().lock().unwrap().push(tx);
    rx
}

pub(crate) fn broadcast_event(event: impl Into<String>, payload: Value) {
    let message = json!({
        "type": "event",
        "event": event.into(),
        "payload": payload,
    })
    .to_string();

    let mut subscribers = subscribers().lock().unwrap();
    subscribers.retain(|tx| tx.send(message.clone()).is_ok());
}

#[derive(Clone)]
pub(crate) struct EventEmitter {
    app: Option<AppHandle>,
}

impl EventEmitter {
    pub(crate) fn new(app: AppHandle) -> Self {
        Self { app: Some(app) }
    }

    pub(crate) fn websocket_only() -> Self {
        Self { app: None }
    }

    pub(crate) fn emit(&self, event: impl Into<String>, payload: Value) {
        let event = event.into();
        if let Some(app) = &self.app {
            let _ = app.emit(
                EVENT_BRIDGE,
                json!({"event": event.clone(), "payload": payload.clone()}),
            );
        }
        broadcast_event(event, payload);
    }
}

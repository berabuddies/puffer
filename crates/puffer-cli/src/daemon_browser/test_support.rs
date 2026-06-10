//! Shared test harness for the native-CEF browser path.
//!
//! The fake DevTools endpoint advertises a fixed pool of reusable
//! `about:blank` page slots and answers CDP over a generic echo websocket, so
//! the prewarm-pool logic can be exercised across modules without launching a
//! real browser.

use super::command::BrowserCommand;
use super::network_idle::BrowserNetworkState;
use super::session::BrowserSession;
use super::BrowserState;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tungstenite::Message;

impl BrowserSession {
    /// Creates a synthetic page worker for unit tests without launching Chrome.
    pub(super) fn new_for_test(
        tx: Sender<BrowserCommand>,
        state: Arc<Mutex<BrowserState>>,
        last_active: Arc<Mutex<Instant>>,
    ) -> Self {
        Self {
            tx,
            state,
            network: Arc::new(Mutex::new(BrowserNetworkState::default())),
            last_active,
            alive: Arc::new(AtomicBool::new(true)),
            root: None,
            native_cef_session_id: None,
        }
    }
}

/// Serializes mutation of the process-global `PUFFER_CEF_*` env vars so the
/// CEF-backed browser tests can run in parallel without clobbering each
/// other's fake endpoint.
pub(crate) fn cef_env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// A fake CEF DevTools server with `slot_count` reusable prewarm page targets.
/// Stops its accept loop on drop.
pub(crate) struct FakeCefDevtools {
    pub(crate) port: u16,
    finished: Arc<AtomicBool>,
}

impl FakeCefDevtools {
    /// Spawns a fake DevTools endpoint with `slot_count` reusable targets.
    pub(crate) fn spawn(slot_count: usize) -> Self {
        Self::spawn_inner(slot_count, Vec::new())
    }

    /// Like [`FakeCefDevtools::spawn`], but the slots whose indices are in
    /// `hung` accept the CDP websocket yet never answer any request, simulating
    /// a wedged page that cannot be reset.
    pub(crate) fn spawn_with_hung_slots(slot_count: usize, hung: Vec<usize>) -> Self {
        Self::spawn_inner(slot_count, hung)
    }

    fn spawn_inner(slot_count: usize, hung: Vec<usize>) -> Self {
        let hung: HashSet<usize> = hung.into_iter().collect();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let finished = Arc::new(AtomicBool::new(false));
        let server_finished = Arc::clone(&finished);
        std::thread::spawn(move || {
            let targets = (0..slot_count)
                .map(|index| {
                    format!(
                        r#"{{"id":"target-{index}","type":"page","url":"about:blank#puffer-cef-slot=__cef_prewarm_{index}__","webSocketDebuggerUrl":"ws://127.0.0.1:{port}/devtools/page/target-{index}"}}"#
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            let list_body = format!("[{targets}]");
            while !server_finished.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        stream.set_nonblocking(false).unwrap();
                        let list_body = list_body.clone();
                        let hung = hung.clone();
                        std::thread::spawn(move || {
                            handle_connection(stream, port, list_body, hung)
                        });
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });
        Self { port, finished }
    }
}

impl Drop for FakeCefDevtools {
    fn drop(&mut self) {
        self.finished.store(true, Ordering::SeqCst);
    }
}

fn handle_connection(stream: TcpStream, port: u16, list_body: String, hung: HashSet<usize>) {
    let mut peeked = [0u8; 4096];
    let count = stream.peek(&mut peeked).unwrap_or(0);
    let head_raw = String::from_utf8_lossy(&peeked[..count]).to_string();
    let head = head_raw.to_ascii_lowercase();
    if head.contains("upgrade: websocket") {
        handle_websocket(stream, &head_raw, hung);
    } else {
        handle_http(stream, port, list_body);
    }
}

fn handle_websocket(stream: TcpStream, head_raw: &str, hung: HashSet<usize>) {
    let slot_is_hung = head_raw
        .split_whitespace()
        .find_map(|token| token.rsplit_once("/devtools/page/target-"))
        .and_then(|(_, rest)| rest.parse::<usize>().ok())
        .is_some_and(|index| hung.contains(&index));
    let Ok(mut socket) = tungstenite::accept(stream) else {
        return;
    };
    loop {
        match socket.read() {
            Ok(Message::Text(text)) => {
                if slot_is_hung {
                    continue;
                }
                if let Ok(value) = serde_json::from_str::<Value>(&text) {
                    if let Some(id) = value.get("id").and_then(Value::as_u64) {
                        let _ = socket.send(Message::Text(
                            json!({ "id": id, "result": {} }).to_string().into(),
                        ));
                    }
                }
            }
            Ok(Message::Close(_)) | Err(_) => break,
            Ok(_) => {}
        }
    }
}

fn handle_http(mut stream: TcpStream, port: u16, list_body: String) {
    let mut buffer = [0u8; 4096];
    let read = stream.read(&mut buffer).unwrap_or(0);
    let request = String::from_utf8_lossy(&buffer[..read]);
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/")
        .to_string();
    let body = if path.starts_with("/json/version") {
        format!(r#"{{"webSocketDebuggerUrl":"ws://127.0.0.1:{port}/devtools/browser/root"}}"#)
    } else if path.starts_with("/json/list") {
        list_body
    } else {
        "[]".to_string()
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.write_all(response.as_bytes());
}

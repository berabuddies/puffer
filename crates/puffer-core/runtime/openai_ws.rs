use super::openai_sse::{process_openai_event, OpenAISseResult, OpenAISseState};
use super::TurnStreamEvent;
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::fmt;
use std::net::TcpStream;
use std::time::{Duration, Instant};
use tungstenite::client::IntoClientRequest;
use tungstenite::http::header::{HeaderName, HeaderValue};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{connect, Message, WebSocket};

/// Conservative reconnect threshold — slightly under the server's 60-minute
/// hard limit so we never hit the server-side disconnect mid-request.
const WS_MAX_LIFETIME: Duration = Duration::from_secs(55 * 60);

/// Maximum time to wait for close-handshake drain frames.
const WS_CLOSE_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

/// Structured error returned by the OpenAI WebSocket API. Carries the
/// machine-readable `code` (e.g. `"previous_response_not_found"`,
/// `"invalid_api_key"`) so callers can match on it without string parsing.
#[derive(Debug, Clone)]
pub(super) struct WsApiError {
    /// Machine-readable error code from `error.code` in the JSON event.
    pub code: String,
    /// Human-readable message from `error.message`.
    pub message: String,
}

impl fmt::Display for WsApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "OpenAI WebSocket error: {}: {}", self.code, self.message)
    }
}

impl std::error::Error for WsApiError {}

/// Manages a persistent WebSocket connection to the OpenAI Responses API
/// (`wss://.../v1/responses`). The connection stays open across multiple
/// `response.create` round-trips, reducing per-turn connection overhead.
pub(super) struct OpenAIWebSocket {
    socket: Option<WebSocket<MaybeTlsStream<TcpStream>>>,
    created_at: Instant,
}

impl OpenAIWebSocket {
    /// Establishes a new WebSocket connection to the given URL with the
    /// provided HTTP headers (typically `Authorization` and other metadata).
    pub(super) fn connect(url: &str, headers: &[(String, String)]) -> Result<Self> {
        let mut request = url
            .into_client_request()
            .context("failed to build WebSocket request URL")?;
        for (key, value) in headers {
            let header_name = HeaderName::from_bytes(key.as_bytes())
                .with_context(|| format!("invalid header name: {key}"))?;
            let header_value = HeaderValue::from_str(value)
                .with_context(|| format!("invalid header value for {key}"))?;
            request.headers_mut().insert(header_name, header_value);
        }
        let (socket, _response) =
            connect(request).context("failed to establish WebSocket connection to OpenAI")?;
        Ok(Self {
            socket: Some(socket),
            created_at: Instant::now(),
        })
    }

    /// Returns a mutable reference to the inner socket, panicking if already
    /// closed. This is always safe in normal usage since `close()` is the
    /// last operation before drop.
    fn socket_mut(&mut self) -> &mut WebSocket<MaybeTlsStream<TcpStream>> {
        self.socket.as_mut().expect("WebSocket already closed")
    }

    /// Sends a `response.create` event over the WebSocket. The `body` should
    /// be the same JSON payload you would POST to `/v1/responses`, minus
    /// the `stream` and `background` fields (which are not used in WS mode).
    pub(super) fn send_response_create(&mut self, body: &Value) -> Result<()> {
        let text = serde_json::to_string(&build_ws_envelope(body))?;
        self.socket_mut()
            .send(Message::Text(text.into()))
            .context("failed to send response.create over WebSocket")?;
        Ok(())
    }

    /// Reads streaming events from the WebSocket connection until a successful
    /// terminal event is received. Returns the same typed result as the SSE
    /// parser.
    pub(super) fn read_events<F>(&mut self, on_event: &mut F) -> Result<OpenAISseResult>
    where
        F: FnMut(TurnStreamEvent),
    {
        let mut state = OpenAISseState::default();
        let socket = self.socket_mut();
        loop {
            let msg = socket.read().context("failed to read WebSocket message")?;
            match msg {
                Message::Text(text) => {
                    let event: Value =
                        serde_json::from_str(&text).context("invalid WebSocket JSON payload")?;
                    // Handle WS-level error events (e.g. previous_response_not_found,
                    // websocket_connection_limit_reached).
                    if event.get("type").and_then(Value::as_str) == Some("error") {
                        let code = event
                            .pointer("/error/code")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown")
                            .to_string();
                        let message = event
                            .pointer("/error/message")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown error")
                            .to_string();
                        return Err(WsApiError { code, message }.into());
                    }
                    if process_openai_event(&event, &mut state, on_event)? {
                        break;
                    }
                }
                Message::Close(_) => {
                    if state.terminal {
                        break;
                    }
                    bail!("WebSocket connection closed before response completed");
                }
                Message::Ping(data) => {
                    let _ = socket.send(Message::Pong(data));
                }
                _ => {} // Ignore binary frames and pong.
            }
        }
        Ok(state.into_typed_result())
    }

    /// Returns `true` if the connection has been alive long enough that we
    /// should proactively reconnect before the server's 60-minute limit.
    pub(super) fn is_expired(&self) -> bool {
        self.created_at.elapsed() >= WS_MAX_LIFETIME
    }

    /// Gracefully closes the WebSocket connection with a bounded drain.
    /// After sending a Close frame, we wait up to [`WS_CLOSE_DRAIN_TIMEOUT`]
    /// for the server's Close reply before giving up.
    pub(super) fn close(&mut self) {
        let Some(mut socket) = self.socket.take() else {
            return;
        };
        let _ = socket.close(None);
        // Set a read timeout so the drain loop cannot block indefinitely.
        set_tcp_read_timeout(&socket, Some(WS_CLOSE_DRAIN_TIMEOUT));
        loop {
            match socket.read() {
                Ok(Message::Close(_)) | Err(_) => break,
                _ => continue,
            }
        }
    }
}

impl Drop for OpenAIWebSocket {
    /// Best-effort close: sends a Close frame if the socket was not already
    /// closed via [`OpenAIWebSocket::close`]. Does not drain remaining
    /// frames — the TCP stream will be dropped immediately after.
    fn drop(&mut self) {
        if let Some(mut socket) = self.socket.take() {
            let _ = socket.close(None);
        }
    }
}

/// Sets the read timeout on the underlying TCP stream of a tungstenite
/// `WebSocket<MaybeTlsStream<TcpStream>>`.
fn set_tcp_read_timeout(socket: &WebSocket<MaybeTlsStream<TcpStream>>, timeout: Option<Duration>) {
    let stream = socket.get_ref();
    let tcp: &TcpStream = match stream {
        MaybeTlsStream::Plain(s) => s,
        MaybeTlsStream::Rustls(tls) => tls.get_ref(),
        _ => return,
    };
    let _ = tcp.set_read_timeout(timeout);
}

/// Builds the WebSocket envelope from an HTTP-style request body. Adds the
/// `"type": "response.create"` field and strips `stream` and `background`
/// which are not used in WS mode.
fn build_ws_envelope(body: &Value) -> Value {
    let mut msg = body.clone();
    msg["type"] = Value::String("response.create".to_string());
    if let Some(obj) = msg.as_object_mut() {
        obj.remove("stream");
        obj.remove("background");
    }
    msg
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn build_ws_envelope_sets_type_and_strips_transport_fields() {
        let body = json!({
            "model": "gpt-4.1",
            "input": [{"role": "user", "content": "hello"}],
            "stream": true,
            "background": false,
            "instructions": "be helpful"
        });
        let envelope = build_ws_envelope(&body);
        assert_eq!(envelope["type"], "response.create");
        assert_eq!(envelope["model"], "gpt-4.1");
        assert_eq!(envelope["instructions"], "be helpful");
        assert!(envelope.get("stream").is_none());
        assert!(envelope.get("background").is_none());
        // input should be preserved.
        assert!(envelope["input"].is_array());
    }

    #[test]
    fn build_ws_envelope_preserves_all_body_fields() {
        let body = json!({
            "model": "o3-mini",
            "input": [],
            "tools": [{"type": "function", "name": "bash"}],
            "text": {"format": {"type": "text"}},
            "previous_response_id": "resp_abc",
            "stream": true
        });
        let envelope = build_ws_envelope(&body);
        assert_eq!(envelope["model"], "o3-mini");
        assert_eq!(envelope["tools"][0]["name"], "bash");
        assert_eq!(envelope["previous_response_id"], "resp_abc");
        assert!(envelope.get("stream").is_none());
    }

    #[test]
    fn ws_api_error_display() {
        let err = WsApiError {
            code: "rate_limit_exceeded".to_string(),
            message: "too many requests".to_string(),
        };
        let msg = format!("{err}");
        assert!(msg.contains("rate_limit_exceeded"));
        assert!(msg.contains("too many requests"));
    }

    #[test]
    fn ws_api_error_downcast() {
        let err: anyhow::Error = WsApiError {
            code: "server_error".to_string(),
            message: "internal".to_string(),
        }
        .into();
        let ws_err = err.downcast_ref::<WsApiError>().unwrap();
        assert_eq!(ws_err.code, "server_error");
    }

    #[test]
    fn close_is_idempotent() {
        // Construct an OpenAIWebSocket with socket already taken (simulates
        // a previously-closed connection). Calling close() twice must not
        // panic — the second call is a no-op because `socket` is `None`.
        let mut ws = OpenAIWebSocket {
            socket: None,
            created_at: Instant::now(),
        };
        ws.close(); // first call — socket already None, should be no-op
        ws.close(); // second call — still no-op, must not panic
    }

    /// E2E smoke test: connects to the OpenAI-compatible API over WebSocket,
    /// sends a simple prompt, and reads events until `response.completed`.
    /// Requires `INFER_API_KEY` env var. Skips if the endpoint rejects WS
    /// upgrade (not all proxies support WebSocket mode). Run with:
    /// `INFER_API_KEY=... cargo test -p puffer-core -- e2e_ws_smoke --ignored`
    #[test]
    #[ignore]
    fn e2e_ws_smoke_test() {
        let api_key =
            std::env::var("INFER_API_KEY").expect("INFER_API_KEY env var required for E2E test");
        let base_url = std::env::var("INFER_BASE_URL")
            .unwrap_or_else(|_| "https://api-infer.agentsey.ai/v1".to_string());

        // Build WS URL from base.
        let ws_url = {
            let trimmed = base_url.trim_end_matches('/');
            let ws = trimmed
                .replacen("https://", "wss://", 1)
                .replacen("http://", "ws://", 1);
            format!("{ws}/responses")
        };

        let headers = vec![("Authorization".to_string(), format!("Bearer {api_key}"))];

        let ws = match OpenAIWebSocket::connect(&ws_url, &headers) {
            Ok(ws) => ws,
            Err(error) => {
                // Some proxies don't support WebSocket upgrade — skip gracefully.
                let msg = format!("{error:#}");
                if msg.contains("403")
                    || msg.contains("Forbidden")
                    || msg.contains("501")
                    || msg.contains("Not Implemented")
                    || msg.contains("Upgrade")
                {
                    eprintln!("[e2e] skipping: endpoint does not support WebSocket upgrade: {msg}");
                    return;
                }
                panic!("failed to connect to WS endpoint: {error:#}");
            }
        };
        let mut ws = ws;

        let body = json!({
            "model": "gpt-4.1-mini",
            "input": [
                {"role": "user", "content": "Say exactly: hello from ws test"}
            ],
            "stream": true
        });

        ws.send_response_create(&body)
            .expect("failed to send response.create");

        let mut events_received = 0;
        let result = ws.read_events(&mut |_evt| {
            events_received += 1;
        });

        let result = result.expect("failed to read events from WS");
        assert!(events_received > 0, "should have received streaming events");
        assert!(
            !result.assistant_text.is_empty(),
            "assistant text should not be empty"
        );
        eprintln!(
            "[e2e] received {} events, text: {:?}",
            events_received,
            &result.assistant_text[..result.assistant_text.len().min(100)]
        );

        ws.close();
    }
}

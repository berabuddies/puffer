//! Tiny stdio MCP server used by `runner_mcp_stdio.rs` and the cross-backend
//! test in `puffer-runner-grpc`.
//!
//! Keeps things deliberately minimal:
//!
//! * `tools/list` returns three tools: `echo`, `slow_echo`, `crash`.
//! * `tools/call` dispatches: `echo` round-trips its `text` arg, `slow_echo`
//!   sleeps `delay_ms` then returns the text, `crash` exits the process so
//!   the connection manager has something to recover from.
//! * Everything else falls through to the rmcp `ServerHandler` defaults.
//!
//! Progress notifications are intentionally NOT emitted — pass 1.5b will
//! wire that through `ChunkSink` once the client side surfaces it.

use std::sync::Arc;
use std::time::Duration;

use rmcp::{
    ErrorData,
    handler::server::ServerHandler,
    model::{
        CallToolRequestParams, CallToolResult, Content, Implementation, InitializeResult,
        ListToolsResult, PaginatedRequestParams, ProtocolVersion, ServerCapabilities, Tool,
    },
    serve_server,
    service::{NotificationContext, RequestContext, RoleServer},
    transport::io::stdio,
};
use serde_json::{json, Map, Value};

#[derive(Clone, Default)]
struct StubServer;

impl ServerHandler for StubServer {
    fn get_info(&self) -> InitializeResult {
        InitializeResult {
            protocol_version: ProtocolVersion::default(),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation {
                name: "puffer-mcp-stub-server".into(),
                title: None,
                version: env!("CARGO_PKG_VERSION").into(),
                description: None,
                icons: None,
                website_url: None,
            },
            instructions: Some("puffer integration-test stub".into()),
        }
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult {
            tools: vec![
                Tool::new("echo", "Echo back the supplied text", echo_schema()),
                Tool::new(
                    "slow_echo",
                    "Sleep `delay_ms` then echo back `text`",
                    slow_echo_schema(),
                ),
                Tool::new(
                    "crash",
                    "Exit the stub server process immediately",
                    empty_object_schema(),
                ),
            ],
            next_cursor: None,
            meta: None,
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let args = request.arguments.unwrap_or_default();
        match request.name.as_ref() {
            "echo" => {
                let text = args
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ErrorData::invalid_params("missing `text`", None))?;
                Ok(CallToolResult::success(vec![Content::text(text)]))
            }
            "slow_echo" => {
                let text = args
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let delay_ms = args.get("delay_ms").and_then(Value::as_u64).unwrap_or(0);
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                Ok(CallToolResult::success(vec![Content::text(text)]))
            }
            "crash" => {
                // Give the response a moment to flush, then exit hard so the
                // connection manager has to respawn on the next call.
                tokio::spawn(async {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    std::process::exit(0);
                });
                Ok(CallToolResult::success(vec![Content::text("crashing")]))
            }
            other => Err(ErrorData::invalid_params(
                format!("unknown tool `{other}`"),
                None,
            )),
        }
    }

    async fn on_initialized(&self, _context: NotificationContext<RoleServer>) {}
}

fn echo_schema() -> Arc<Map<String, Value>> {
    object_schema(json!({
        "type": "object",
        "properties": {
            "text": { "type": "string" }
        },
        "required": ["text"]
    }))
}

fn slow_echo_schema() -> Arc<Map<String, Value>> {
    object_schema(json!({
        "type": "object",
        "properties": {
            "text": { "type": "string" },
            "delay_ms": { "type": "integer", "minimum": 0 }
        },
        "required": ["text"]
    }))
}

fn empty_object_schema() -> Arc<Map<String, Value>> {
    object_schema(json!({ "type": "object" }))
}

fn object_schema(value: Value) -> Arc<Map<String, Value>> {
    match value {
        Value::Object(map) => Arc::new(map),
        _ => Arc::new(Map::new()),
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--exit-immediately") {
        std::process::exit(2);
    }
    // `--marker <id>` is honored only as a way for tests to find their own
    // child processes via `pgrep -f`. The value is intentionally unused.
    let transport = stdio();
    let service = serve_server(StubServer, transport).await?;
    service.waiting().await?;
    Ok(())
}

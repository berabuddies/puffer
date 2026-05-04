//! Shared rmcp `ServerHandler` used by both the stdio bin
//! (`puffer-mcp-stub-server`) and the HTTP integration tests.
//!
//! Keeping the handler in one place guarantees the stdio and HTTP transport
//! tests speak to *exactly* the same server surface — every divergence we
//! used to debug between transports came from "but my stub does X
//! differently". Now there's nowhere for that to hide.
//!
//! Surface (mirrors the stdio doc-comment):
//!
//! * `tools/list` returns `echo`, `slow_echo`, `crash`, `slow_with_progress`,
//!   `request_user_input`.
//! * `tools/call` dispatches to the matching handlers.
//! * `resources/list` returns `stub://hello.txt`, `stub://binary.bin`.
//! * `resources/read` returns text + blob respectively.
//! * `prompts/list` returns `greet`.
//! * `prompts/get greet` interpolates `name`.

use std::sync::Arc;
use std::time::Duration;

use rmcp::{
    handler::server::ServerHandler,
    model::{
        BooleanSchema, CallToolRequestParams, CallToolResult, Content,
        CreateElicitationRequestParams, ElicitationAction, ElicitationSchema,
        GetPromptRequestParams, GetPromptResult, Implementation, InitializeResult,
        ListPromptsResult, ListResourcesResult, ListToolsResult, PaginatedRequestParams,
        PrimitiveSchema, ProgressNotificationParam, Prompt, PromptArgument, PromptMessage,
        PromptMessageRole, ProtocolVersion, RawResource, ReadResourceRequestParams,
        ReadResourceResult, Resource, ResourceContents, ServerCapabilities, Tool,
    },
    service::{NotificationContext, RequestContext, RoleServer},
    ErrorData,
};
use serde_json::{json, Map, Value};

#[derive(Clone, Default)]
pub struct StubServer;

pub const HELLO_URI: &str = "stub://hello.txt";
pub const BINARY_URI: &str = "stub://binary.bin";

impl ServerHandler for StubServer {
    fn get_info(&self) -> InitializeResult {
        InitializeResult {
            protocol_version: ProtocolVersion::default(),
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .enable_prompts()
                .enable_resources()
                .build(),
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
                Tool::new(
                    "slow_with_progress",
                    "Emit two `notifications/progress` then echo back `text`",
                    slow_echo_schema(),
                ),
                Tool::new(
                    "request_user_input",
                    "Issue an `elicitation/create` mid-call and echo back the response",
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
        context: RequestContext<RoleServer>,
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
                tokio::spawn(async {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    std::process::exit(0);
                });
                Ok(CallToolResult::success(vec![Content::text("crashing")]))
            }
            "slow_with_progress" => {
                let text = args
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let delay_ms = args.get("delay_ms").and_then(Value::as_u64).unwrap_or(20);
                let progress_token = context.meta.get_progress_token();
                if let Some(token) = progress_token.clone() {
                    let _ = context
                        .peer
                        .notify_progress(ProgressNotificationParam {
                            progress_token: token,
                            progress: 1.0,
                            total: Some(2.0),
                            message: Some("halfway".into()),
                        })
                        .await;
                }
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                if let Some(token) = progress_token {
                    let _ = context
                        .peer
                        .notify_progress(ProgressNotificationParam {
                            progress_token: token,
                            progress: 2.0,
                            total: Some(2.0),
                            message: Some("done".into()),
                        })
                        .await;
                }
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                Ok(CallToolResult::success(vec![Content::text(text)]))
            }
            "request_user_input" => {
                let schema = ElicitationSchema::builder()
                    .required_property(
                        "confirmed",
                        PrimitiveSchema::Boolean(BooleanSchema::new()),
                    )
                    .build()
                    .map_err(|e| {
                        ErrorData::internal_error(format!("schema build: {e}"), None)
                    })?;
                let response = context
                    .peer
                    .create_elicitation(CreateElicitationRequestParams::FormElicitationParams {
                        meta: None,
                        message: "Confirm the destructive action?".to_string(),
                        requested_schema: schema,
                    })
                    .await
                    .map_err(|e| {
                        ErrorData::internal_error(format!("elicitation failed: {e}"), None)
                    })?;
                let body = match response.action {
                    ElicitationAction::Accept => json!({
                        "action": "accept",
                        "content": response.content.unwrap_or(Value::Null),
                    }),
                    ElicitationAction::Decline => json!({
                        "action": "decline",
                        "content": Value::Null,
                    }),
                    ElicitationAction::Cancel => json!({
                        "action": "cancel",
                        "content": Value::Null,
                    }),
                };
                Ok(CallToolResult::success(vec![Content::text(body.to_string())]))
            }
            other => Err(ErrorData::invalid_params(
                format!("unknown tool `{other}`"),
                None,
            )),
        }
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        Ok(ListResourcesResult {
            resources: vec![
                Resource::new(
                    RawResource {
                        uri: HELLO_URI.into(),
                        name: "hello".into(),
                        title: None,
                        description: Some("Plain-text stub resource".into()),
                        mime_type: Some("text/plain".into()),
                        size: None,
                        icons: None,
                        meta: None,
                    },
                    None,
                ),
                Resource::new(
                    RawResource {
                        uri: BINARY_URI.into(),
                        name: "binary".into(),
                        title: None,
                        description: Some("Three-byte binary stub".into()),
                        mime_type: Some("application/octet-stream".into()),
                        size: Some(3),
                        icons: None,
                        meta: None,
                    },
                    None,
                ),
            ],
            next_cursor: None,
            meta: None,
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        match request.uri.as_str() {
            HELLO_URI => Ok(ReadResourceResult {
                contents: vec![ResourceContents::TextResourceContents {
                    uri: HELLO_URI.into(),
                    mime_type: Some("text/plain".into()),
                    text: "hello from stub".into(),
                    meta: None,
                }],
            }),
            BINARY_URI => Ok(ReadResourceResult {
                contents: vec![ResourceContents::BlobResourceContents {
                    uri: BINARY_URI.into(),
                    mime_type: Some("application/octet-stream".into()),
                    // base64("\xde\xad\xbe") = "3q2+"
                    blob: encode_base64(&[0xde, 0xad, 0xbe]),
                    meta: None,
                }],
            }),
            other => Err(ErrorData::invalid_params(
                format!("unknown resource `{other}`"),
                None,
            )),
        }
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        Ok(ListPromptsResult {
            prompts: vec![Prompt::new(
                "greet",
                Some("Greet the named caller"),
                Some(vec![PromptArgument {
                    name: "name".into(),
                    title: None,
                    description: Some("Who to greet".into()),
                    required: Some(true),
                }]),
            )],
            next_cursor: None,
            meta: None,
        })
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResult, ErrorData> {
        if request.name != "greet" {
            return Err(ErrorData::invalid_params(
                format!("unknown prompt `{}`", request.name),
                None,
            ));
        }
        let name = request
            .arguments
            .as_ref()
            .and_then(|m| m.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("world");
        Ok(GetPromptResult {
            description: Some("Stub greeting prompt".into()),
            messages: vec![PromptMessage::new_text(
                PromptMessageRole::User,
                format!("Hello, {name}!"),
            )],
        })
    }

    async fn on_initialized(&self, _context: NotificationContext<RoleServer>) {}
}

pub fn echo_schema() -> Arc<Map<String, Value>> {
    object_schema(json!({
        "type": "object",
        "properties": {
            "text": { "type": "string" }
        },
        "required": ["text"]
    }))
}

pub fn slow_echo_schema() -> Arc<Map<String, Value>> {
    object_schema(json!({
        "type": "object",
        "properties": {
            "text": { "type": "string" },
            "delay_ms": { "type": "integer", "minimum": 0 }
        },
        "required": ["text"]
    }))
}

pub fn empty_object_schema() -> Arc<Map<String, Value>> {
    object_schema(json!({ "type": "object" }))
}

fn object_schema(value: Value) -> Arc<Map<String, Value>> {
    match value {
        Value::Object(map) => Arc::new(map),
        _ => Arc::new(Map::new()),
    }
}

pub fn encode_base64(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((input.len() + 2) / 3 * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        out.push(ALPHABET[(b0 >> 2) as usize] as char);
        out.push(ALPHABET[(((b0 & 0b11) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[(((b1 & 0b1111) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[(b2 & 0b111111) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

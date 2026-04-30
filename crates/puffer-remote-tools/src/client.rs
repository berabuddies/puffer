use crate::model::proto::tool_runner_client::ToolRunnerClient;
use crate::model::proto::{
    DescribeCapabilitiesRequest, ExecuteToolEvent, LoadProjectResourcesRequest,
};
use crate::{
    RemoteProjectResourceFile, RemoteToolCapabilities, RemoteToolChunk, RemoteToolChunkStream,
    RemoteToolRequest,
};
use anyhow::{anyhow, Context, Result};
use puffer_tools::ToolExecutionResult;
use std::sync::mpsc;
use tonic::metadata::MetadataValue;
use tonic::Request;

enum ClientMessage {
    Chunk(RemoteToolChunk),
    Finished(Result<ToolExecutionResult>),
}

/// Connects to a remote tool runner and returns its advertised capabilities.
pub fn describe_capabilities_blocking(
    endpoint: &str,
    auth_token: Option<&str>,
) -> Result<RemoteToolCapabilities> {
    let endpoint = endpoint.to_string();
    let auth_token = auth_token.map(ToOwned::to_owned);
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("build tokio runtime")
            .and_then(|runtime| {
                runtime.block_on(async move {
                    let mut client = connect_client(&endpoint).await?;
                    let request =
                        authenticated_request(DescribeCapabilitiesRequest {}, auth_token)?;
                    let response = client
                        .describe_capabilities(request)
                        .await
                        .context("describe remote tool capabilities")?;
                    Ok(response.into_inner().into())
                })
            });
        let _ = tx.send(result);
    });
    rx.recv()
        .map_err(|_| anyhow!("remote capability request disconnected"))?
}

/// Executes one remote tool call and relays streamed stdout/stderr chunks to the callback.
pub fn execute_tool_blocking<F>(
    endpoint: &str,
    auth_token: Option<&str>,
    request: RemoteToolRequest,
    mut on_chunk: F,
) -> Result<ToolExecutionResult>
where
    F: FnMut(RemoteToolChunk),
{
    let endpoint = endpoint.to_string();
    let auth_token = auth_token.map(ToOwned::to_owned);
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("build tokio runtime")
            .and_then(|runtime| {
                let event_tx = tx.clone();
                runtime.block_on(async move {
                    let mut client = connect_client(&endpoint).await?;
                    let request = authenticated_request(request.into(), auth_token)?;
                    let response = client
                        .execute_tool(request)
                        .await
                        .context("execute remote tool")?;
                    let mut stream = response.into_inner();
                    while let Some(event) =
                        stream.message().await.context("read remote tool event")?
                    {
                        match parse_execute_tool_event(event) {
                            ParsedToolEvent::Chunk(chunk) => {
                                if event_tx.send(ClientMessage::Chunk(chunk)).is_err() {
                                    return Ok(());
                                }
                            }
                            ParsedToolEvent::Finished(result) => {
                                let _ = event_tx.send(ClientMessage::Finished(Ok(result)));
                                return Ok(());
                            }
                        }
                    }
                    let _ = event_tx.send(ClientMessage::Finished(Err(anyhow!(
                        "remote tool stream ended before completion"
                    ))));
                    Ok(())
                })
            });
        if let Err(error) = result {
            let _ = tx.send(ClientMessage::Finished(Err(error)));
        }
    });

    loop {
        match rx
            .recv()
            .map_err(|_| anyhow!("remote tool execution disconnected"))?
        {
            ClientMessage::Chunk(chunk) => on_chunk(chunk),
            ClientMessage::Finished(result) => return result,
        }
    }
}

/// Loads project-local resources from a remote tool runner.
pub fn load_project_resources_blocking(
    endpoint: &str,
    auth_token: Option<&str>,
    project_root: &str,
) -> Result<Vec<RemoteProjectResourceFile>> {
    let endpoint = endpoint.to_string();
    let auth_token = auth_token.map(ToOwned::to_owned);
    let project_root = project_root.to_string();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("build tokio runtime")
            .and_then(|runtime| {
                runtime.block_on(async move {
                    let mut client = connect_client(&endpoint).await?;
                    let request = authenticated_request(
                        LoadProjectResourcesRequest { project_root },
                        auth_token,
                    )?;
                    let response = client
                        .load_project_resources(request)
                        .await
                        .context("load remote project resources")?;
                    Ok(response
                        .into_inner()
                        .files
                        .into_iter()
                        .map(RemoteProjectResourceFile::from)
                        .collect())
                })
            });
        let _ = tx.send(result);
    });
    rx.recv()
        .map_err(|_| anyhow!("remote project resource request disconnected"))?
}
async fn connect_client(endpoint: &str) -> Result<ToolRunnerClient<tonic::transport::Channel>> {
    let channel = tonic::transport::Channel::from_shared(endpoint.to_string())
        .with_context(|| format!("invalid tool runner endpoint `{endpoint}`"))?
        .connect()
        .await
        .with_context(|| format!("connect tool runner `{endpoint}`"))?;
    Ok(ToolRunnerClient::new(channel))
}

fn authenticated_request<T>(message: T, auth_token: Option<String>) -> Result<Request<T>> {
    let mut request = Request::new(message);
    if let Some(token) = auth_token.filter(|value| !value.trim().is_empty()) {
        let value = MetadataValue::try_from(format!("Bearer {token}"))
            .context("invalid tool runner authorization token")?;
        request.metadata_mut().insert("authorization", value);
    }
    Ok(request)
}

enum ParsedToolEvent {
    Chunk(RemoteToolChunk),
    Finished(ToolExecutionResult),
}

fn parse_execute_tool_event(event: ExecuteToolEvent) -> ParsedToolEvent {
    match event.payload {
        Some(crate::proto::execute_tool_event::Payload::Stdout(stdout)) => {
            ParsedToolEvent::Chunk(RemoteToolChunk {
                stream: RemoteToolChunkStream::Stdout,
                text: stdout.text,
            })
        }
        Some(crate::proto::execute_tool_event::Payload::Stderr(stderr)) => {
            ParsedToolEvent::Chunk(RemoteToolChunk {
                stream: RemoteToolChunkStream::Stderr,
                text: stderr.text,
            })
        }
        Some(crate::proto::execute_tool_event::Payload::Completed(completed)) => {
            let metadata = serde_json::from_str(&completed.metadata_json).unwrap_or_default();
            ParsedToolEvent::Finished(ToolExecutionResult {
                tool_id: completed.tool_id,
                success: completed.success,
                output: puffer_tools::ToolOutput {
                    stdout: completed.stdout,
                    stderr: completed.stderr,
                    metadata,
                },
            })
        }
        Some(crate::proto::execute_tool_event::Payload::Failed(failed)) => {
            ParsedToolEvent::Finished(ToolExecutionResult {
                tool_id: String::new(),
                success: false,
                output: puffer_tools::ToolOutput {
                    stdout: String::new(),
                    stderr: failed.message,
                    metadata: serde_json::Value::Null,
                },
            })
        }
        None => ParsedToolEvent::Finished(ToolExecutionResult {
            tool_id: String::new(),
            success: false,
            output: puffer_tools::ToolOutput {
                stdout: String::new(),
                stderr: "remote tool event missing payload".to_string(),
                metadata: serde_json::Value::Null,
            },
        }),
    }
}

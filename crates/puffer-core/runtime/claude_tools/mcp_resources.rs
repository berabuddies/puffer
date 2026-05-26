use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Describes the lifecycle state of one MCP client connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpClientState {
    Connected,
    Pending,
    Disconnected,
}

/// Captures one MCP resource record returned by `resources/list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpResourceRecord {
    pub uri: String,
    pub name: String,
    pub mime_type: Option<String>,
    pub description: Option<String>,
}

/// Captures one MCP content block returned by `resources/read`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpReadResourceContent {
    Text {
        uri: String,
        mime_type: Option<String>,
        text: String,
    },
    Blob {
        uri: String,
        mime_type: Option<String>,
        blob: Vec<u8>,
    },
}

/// Describes the model input for `ListMcpResourcesTool`.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct ListMcpResourcesToolInput {
    #[serde(default)]
    pub server: Option<String>,
}

/// Describes one serialized output record for `ListMcpResourcesTool`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ListMcpResourcesToolOutputItem {
    pub uri: String,
    pub name: String,
    #[serde(rename = "mimeType", skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub server: String,
}

/// Describes the model input for `ReadMcpResourceTool`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ReadMcpResourceToolInput {
    pub server: String,
    pub uri: String,
}

/// Describes one serialized content record for `ReadMcpResourceTool`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReadMcpResourceToolOutputContent {
    pub uri: String,
    #[serde(rename = "mimeType", skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(rename = "blobSavedTo", skip_serializing_if = "Option::is_none")]
    pub blob_saved_to: Option<String>,
}

/// Describes the serialized output payload for `ReadMcpResourceTool`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReadMcpResourceToolOutput {
    pub contents: Vec<ReadMcpResourceToolOutputContent>,
}

/// Defines the runtime interface needed for live MCP resource list/read operations.
pub trait McpResourceClient {
    /// Returns the stable MCP server name used by tool inputs.
    fn name(&self) -> &str;

    /// Returns the current connection state for this client.
    fn state(&self) -> McpClientState;

    /// Returns true when the server advertises MCP resources support.
    fn supports_resources(&self) -> bool;

    /// Ensures the client is connected and ready for requests.
    fn ensure_connected(&mut self) -> Result<()>;

    /// Calls MCP `resources/list` and returns raw resource records.
    fn list_resources(&mut self) -> Result<Vec<McpResourceRecord>>;

    /// Calls MCP `resources/read` and returns one or more content blocks.
    fn read_resource(&mut self, uri: &str) -> Result<Vec<McpReadResourceContent>>;
}

/// Defines persistence behavior for binary MCP `resources/read` blobs.
pub trait McpBlobStore {
    /// Persists one binary blob and returns the filesystem path where it was written.
    fn persist_blob(
        &self,
        server: &str,
        uri: &str,
        mime_type: Option<&str>,
        bytes: &[u8],
    ) -> Result<PathBuf>;
}

/// Persists MCP blobs into a filesystem directory.
#[derive(Debug, Clone)]
pub struct FilesystemMcpBlobStore {
    root: PathBuf,
}

impl FilesystemMcpBlobStore {
    /// Creates a filesystem-backed blob store rooted at the provided path.
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
}

impl McpBlobStore for FilesystemMcpBlobStore {
    fn persist_blob(
        &self,
        server: &str,
        _uri: &str,
        mime_type: Option<&str>,
        bytes: &[u8],
    ) -> Result<PathBuf> {
        fs::create_dir_all(&self.root).with_context(|| {
            format!(
                "failed to create MCP blob directory {}",
                self.root.display()
            )
        })?;
        let server_fragment = sanitize_fragment(server);
        let extension = extension_for_mime_type(mime_type);
        let file_name = format!(
            "mcp-resource-{server_fragment}-{}.{}",
            Uuid::new_v4(),
            extension
        );
        let path = self.root.join(file_name);
        fs::write(&path, bytes)
            .with_context(|| format!("failed to write MCP blob {}", path.display()))?;
        Ok(path)
    }
}

/// Executes Claude-compatible `ListMcpResourcesTool` semantics against live MCP clients.
pub fn list_mcp_resources(
    input: &ListMcpResourcesToolInput,
    clients: &mut [Box<dyn McpResourceClient>],
) -> Result<Vec<ListMcpResourcesToolOutputItem>> {
    let indices = select_client_indices(input.server.as_deref(), clients)?;
    let mut output = Vec::new();

    for index in indices {
        let client = &mut clients[index];
        if client.state() != McpClientState::Connected || !client.supports_resources() {
            continue;
        }
        if client.ensure_connected().is_err() {
            continue;
        }
        let Ok(resources) = client.list_resources() else {
            continue;
        };
        let server_name = client.name().to_string();
        for resource in resources {
            output.push(ListMcpResourcesToolOutputItem {
                uri: resource.uri,
                name: resource.name,
                mime_type: resource.mime_type,
                description: resource.description,
                server: server_name.clone(),
            });
        }
    }

    Ok(output)
}

/// Executes Claude-compatible `ReadMcpResourceTool` semantics against one live MCP client.
pub fn read_mcp_resource(
    input: &ReadMcpResourceToolInput,
    clients: &mut [Box<dyn McpResourceClient>],
    blob_store: &dyn McpBlobStore,
) -> Result<ReadMcpResourceToolOutput> {
    let available = client_names(clients);
    let index = clients
        .iter()
        .position(|client| client.name() == input.server)
        .ok_or_else(|| anyhow!(missing_server_error(&input.server, &available)))?;

    let client = &mut clients[index];
    if client.state() != McpClientState::Connected {
        bail!("Server \"{}\" is not connected", input.server);
    }
    if !client.supports_resources() {
        bail!("Server \"{}\" does not support resources", input.server);
    }
    client
        .ensure_connected()
        .with_context(|| format!("failed to connect MCP server `{}`", input.server))?;
    let contents = client.read_resource(&input.uri).with_context(|| {
        format!(
            "failed to read MCP resource `{}` from server `{}`",
            input.uri, input.server
        )
    })?;

    let mut output_contents = Vec::new();
    for content in contents {
        match content {
            McpReadResourceContent::Text {
                uri,
                mime_type,
                text,
            } => output_contents.push(ReadMcpResourceToolOutputContent {
                uri,
                mime_type,
                text: Some(text),
                blob_saved_to: None,
            }),
            McpReadResourceContent::Blob {
                uri,
                mime_type,
                blob,
            } => {
                let persisted = blob_store.persist_blob(
                    &input.server,
                    &uri,
                    mime_type.as_deref(),
                    blob.as_slice(),
                )?;
                let persisted_text = binary_blob_saved_message(
                    &input.server,
                    &uri,
                    &persisted,
                    mime_type.as_deref(),
                    blob.len(),
                );
                output_contents.push(ReadMcpResourceToolOutputContent {
                    uri,
                    mime_type,
                    text: Some(persisted_text),
                    blob_saved_to: Some(persisted.display().to_string()),
                });
            }
        }
    }

    Ok(ReadMcpResourceToolOutput {
        contents: output_contents,
    })
}

/// Executes `ListMcpResourcesTool` and serializes the result as pretty JSON.
pub fn execute_list_mcp_resources_tool(
    input: ListMcpResourcesToolInput,
    clients: &mut [Box<dyn McpResourceClient>],
) -> Result<String> {
    let output = list_mcp_resources(&input, clients)?;
    Ok(serde_json::to_string_pretty(&output)?)
}

/// Executes `ReadMcpResourceTool` and serializes the result as pretty JSON.
pub fn execute_read_mcp_resource_tool(
    input: ReadMcpResourceToolInput,
    clients: &mut [Box<dyn McpResourceClient>],
    blob_store: &dyn McpBlobStore,
) -> Result<String> {
    let output = read_mcp_resource(&input, clients, blob_store)?;
    Ok(serde_json::to_string_pretty(&output)?)
}

fn select_client_indices(
    target_server: Option<&str>,
    clients: &[Box<dyn McpResourceClient>],
) -> Result<Vec<usize>> {
    if let Some(server) = target_server {
        let selected = clients
            .iter()
            .enumerate()
            .filter_map(|(index, client)| (client.name() == server).then_some(index))
            .collect::<Vec<_>>();
        if selected.is_empty() {
            let available = client_names(clients);
            bail!("{}", missing_server_error(server, &available));
        }
        return Ok(selected);
    }
    Ok((0..clients.len()).collect())
}

fn client_names(clients: &[Box<dyn McpResourceClient>]) -> Vec<String> {
    clients
        .iter()
        .map(|client| client.name().to_string())
        .collect()
}

fn missing_server_error(server: &str, available: &[String]) -> String {
    format!(
        "Server \"{}\" not found. Available servers: {}",
        server,
        available.join(", ")
    )
}

fn sanitize_fragment(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    for character in input.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
            result.push(character);
        } else {
            result.push('-');
        }
    }
    let compact = result.trim_matches('-').to_string();
    if compact.is_empty() {
        "server".to_string()
    } else {
        compact
    }
}

fn extension_for_mime_type(mime_type: Option<&str>) -> &'static str {
    match mime_type.unwrap_or("").to_ascii_lowercase().as_str() {
        "application/json" => "json",
        "application/yaml" | "text/yaml" | "application/x-yaml" => "yaml",
        "application/pdf" => "pdf",
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "text/plain" => "txt",
        "text/markdown" => "md",
        _ => "bin",
    }
}

fn binary_blob_saved_message(
    server: &str,
    uri: &str,
    path: &Path,
    mime_type: Option<&str>,
    size: usize,
) -> String {
    if let Some(mime_type) = mime_type.filter(|value| !value.trim().is_empty()) {
        return format!(
            "[Resource from {server} at {uri}] Binary content saved to {} ({size} bytes, mime type {mime_type}).",
            path.display()
        );
    }
    format!(
        "[Resource from {server} at {uri}] Binary content saved to {} ({size} bytes).",
        path.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone)]
    struct MockClient {
        name: String,
        state: McpClientState,
        supports_resources: bool,
        connect_ok: bool,
        list_ok: bool,
        read_ok: bool,
        list_records: Vec<McpResourceRecord>,
        read_records: Vec<McpReadResourceContent>,
    }

    impl McpResourceClient for MockClient {
        fn name(&self) -> &str {
            &self.name
        }

        fn state(&self) -> McpClientState {
            self.state
        }

        fn supports_resources(&self) -> bool {
            self.supports_resources
        }

        fn ensure_connected(&mut self) -> Result<()> {
            if self.connect_ok {
                Ok(())
            } else {
                bail!("connect failed")
            }
        }

        fn list_resources(&mut self) -> Result<Vec<McpResourceRecord>> {
            if self.list_ok {
                Ok(self.list_records.clone())
            } else {
                bail!("list failed")
            }
        }

        fn read_resource(&mut self, _uri: &str) -> Result<Vec<McpReadResourceContent>> {
            if self.read_ok {
                Ok(self.read_records.clone())
            } else {
                bail!("read failed")
            }
        }
    }

    fn boxed(client: MockClient) -> Box<dyn McpResourceClient> {
        Box::new(client)
    }

    #[test]
    fn list_mcp_resources_skips_non_connected_and_failed_servers() {
        let mut clients: Vec<Box<dyn McpResourceClient>> = vec![
            boxed(MockClient {
                name: "alpha".to_string(),
                state: McpClientState::Connected,
                supports_resources: true,
                connect_ok: true,
                list_ok: true,
                read_ok: true,
                list_records: vec![McpResourceRecord {
                    uri: "mcp://alpha/spec".to_string(),
                    name: "Alpha Spec".to_string(),
                    mime_type: Some("application/json".to_string()),
                    description: Some("Spec".to_string()),
                }],
                read_records: Vec::new(),
            }),
            boxed(MockClient {
                name: "pending".to_string(),
                state: McpClientState::Pending,
                supports_resources: true,
                connect_ok: true,
                list_ok: true,
                read_ok: true,
                list_records: vec![McpResourceRecord {
                    uri: "mcp://pending/spec".to_string(),
                    name: "Pending Spec".to_string(),
                    mime_type: None,
                    description: None,
                }],
                read_records: Vec::new(),
            }),
            boxed(MockClient {
                name: "broken".to_string(),
                state: McpClientState::Connected,
                supports_resources: true,
                connect_ok: true,
                list_ok: false,
                read_ok: true,
                list_records: Vec::new(),
                read_records: Vec::new(),
            }),
        ];

        let listed =
            list_mcp_resources(&ListMcpResourcesToolInput::default(), &mut clients).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].server, "alpha");
        assert_eq!(listed[0].uri, "mcp://alpha/spec");
    }

    #[test]
    fn list_mcp_resources_reports_missing_server() {
        let mut clients: Vec<Box<dyn McpResourceClient>> = vec![boxed(MockClient {
            name: "alpha".to_string(),
            state: McpClientState::Connected,
            supports_resources: true,
            connect_ok: true,
            list_ok: true,
            read_ok: true,
            list_records: Vec::new(),
            read_records: Vec::new(),
        })];
        let error = list_mcp_resources(
            &ListMcpResourcesToolInput {
                server: Some("missing".to_string()),
            },
            &mut clients,
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("Server \"missing\" not found. Available servers: alpha"));
    }

    #[test]
    fn read_mcp_resource_persists_binary_content() {
        let temp_dir = tempfile::tempdir().unwrap();
        let blob_store = FilesystemMcpBlobStore::new(temp_dir.path().join("mcp-blobs"));
        let mut clients: Vec<Box<dyn McpResourceClient>> = vec![boxed(MockClient {
            name: "alpha".to_string(),
            state: McpClientState::Connected,
            supports_resources: true,
            connect_ok: true,
            list_ok: true,
            read_ok: true,
            list_records: Vec::new(),
            read_records: vec![
                McpReadResourceContent::Text {
                    uri: "mcp://alpha/readme".to_string(),
                    mime_type: Some("text/plain".to_string()),
                    text: "hello".to_string(),
                },
                McpReadResourceContent::Blob {
                    uri: "mcp://alpha/blob".to_string(),
                    mime_type: Some("application/pdf".to_string()),
                    blob: vec![1_u8, 2, 3, 4],
                },
            ],
        })];
        let output = read_mcp_resource(
            &ReadMcpResourceToolInput {
                server: "alpha".to_string(),
                uri: "mcp://alpha/blob".to_string(),
            },
            &mut clients,
            &blob_store,
        )
        .unwrap();

        assert_eq!(output.contents.len(), 2);
        assert_eq!(output.contents[0].text.as_deref(), Some("hello"));
        let blob_path = PathBuf::from(output.contents[1].blob_saved_to.clone().unwrap());
        assert!(blob_path.exists());
        let bytes = fs::read(blob_path).unwrap();
        assert_eq!(bytes, vec![1_u8, 2, 3, 4]);
    }
}

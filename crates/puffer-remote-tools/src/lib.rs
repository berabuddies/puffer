mod client;
mod model;

pub use client::{
    describe_capabilities_blocking, execute_tool_blocking, load_project_resources_blocking,
};
pub use model::{
    proto, RemoteProjectResourceFile, RemoteToolCapabilities, RemoteToolChunk,
    RemoteToolChunkStream, RemoteToolExecutionContext, RemoteToolRequest, RemoteWebSearchBackend,
    RemoteWebSearchRequest,
};

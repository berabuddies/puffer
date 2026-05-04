//! Build an rmcp streamable-HTTP client transport from an
//! [`HttpTransportSpec`].
//!
//! rmcp 0.15's HTTP transport is generic over an `HttpClient`
//! implementation; the workspace enables the `reqwest` flavour
//! (`transport-streamable-http-client-reqwest`), so we hand it a
//! `reqwest::Client` configured with the user-supplied default headers.
//!
//! The streamable-HTTP transport speaks JSON over `POST` and SSE-streams
//! server-initiated notifications back over `GET`, matching the codex /
//! Cursor / Claude-Desktop interpretation of the MCP transport spec.
//!
//! Static-bearer authentication is folded into the default-headers map;
//! the runner does not currently implement OAuth — that's pass 1.5e —
//! though `headers` is a perfectly serviceable smuggling channel for a
//! pre-acquired access token in the meantime.

use std::time::Duration;

use puffer_runner_api::RunnerError;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
};

use super::transport::HttpTransportSpec;

/// Reasonable connection / request timeout for an MCP HTTP backend. Longer
/// than typical REST APIs because some MCP servers stream tool output back
/// inline (especially before a `progressToken` was negotiated). Capped so
/// a wedged backend doesn't block puffer's tokio runtime indefinitely.
const HTTP_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// Build an `rmcp` streamable-HTTP transport for the given spec.
///
/// Returns the concrete `StreamableHttpClientTransport<reqwest::Client>`,
/// ready to hand to `rmcp::ServiceExt::serve` exactly the way the stdio
/// path hands a `TokioChildProcess`.
pub(crate) fn build_streamable_http_transport(
    server_id: &str,
    spec: &HttpTransportSpec,
) -> Result<StreamableHttpClientTransport<reqwest::Client>, RunnerError> {
    let mut header_map = HeaderMap::new();
    for (name, value) in &spec.headers {
        let header_name = HeaderName::try_from(name.as_str()).map_err(|e| {
            RunnerError::Mcp(format!(
                "MCP server `{server_id}`: invalid header name `{name}`: {e}"
            ))
        })?;
        let header_value = HeaderValue::from_str(value).map_err(|e| {
            RunnerError::Mcp(format!(
                "MCP server `{server_id}`: invalid header value for `{name}`: {e}"
            ))
        })?;
        header_map.append(header_name, header_value);
    }

    let client = reqwest::Client::builder()
        .default_headers(header_map)
        .timeout(HTTP_REQUEST_TIMEOUT)
        .build()
        .map_err(|e| {
            RunnerError::Mcp(format!(
                "MCP server `{server_id}`: build reqwest client: {e}"
            ))
        })?;

    let config = StreamableHttpClientTransportConfig {
        uri: spec.url.clone().into(),
        // We don't use rmcp's own bearer-only `auth_header` — the user's
        // headers map is the single source of truth. OAuth (pass 1.5e)
        // will plug in here without colliding.
        auth_header: None,
        ..Default::default()
    };

    Ok(StreamableHttpClientTransport::with_client(client, config))
}

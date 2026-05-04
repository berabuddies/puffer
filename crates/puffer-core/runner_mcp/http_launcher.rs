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
//! Two transport shapes:
//!
//! * **Static-bearer / no-auth** ([`build_streamable_http_transport`]): a
//!   bare `StreamableHttpClientTransport<reqwest::Client>`. The user's
//!   `headers` map is the only authentication channel.
//! * **OAuth-fronted** ([`build_oauth_streamable_http_transport`]): wraps
//!   the reqwest client in rmcp's `AuthClient<reqwest::Client>`, which
//!   injects the `Authorization: Bearer <token>` header on every
//!   underlying call and (via the rmcp `streamable_http_client` impl)
//!   triggers a refresh on missing-token responses.

use std::time::Duration;

use puffer_runner_api::RunnerError;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use rmcp::transport::auth::AuthClient;
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
    let client = build_reqwest_client(server_id, spec)?;

    let config = StreamableHttpClientTransportConfig {
        uri: spec.url.clone().into(),
        // We don't use rmcp's own bearer-only `auth_header` — the user's
        // headers map is the single source of truth. OAuth (pass 1.5e)
        // goes through `build_oauth_streamable_http_transport` instead,
        // which wraps the client in rmcp's `AuthClient`.
        auth_header: None,
        ..Default::default()
    };

    Ok(StreamableHttpClientTransport::with_client(client, config))
}

/// Build an OAuth-fronted streamable-HTTP transport.
///
/// `auth_client` is an `AuthClient<reqwest::Client>` returned by
/// [`puffer_mcp_oauth::OAuthService::resolve`]; the wrapping handles
/// token injection + refresh on every underlying call. The user's
/// static `headers` map is still folded into the inner reqwest client
/// (so things like `X-Tenant` keep working alongside OAuth).
pub(crate) fn build_oauth_streamable_http_transport(
    spec: &HttpTransportSpec,
    auth_client: AuthClient<reqwest::Client>,
) -> StreamableHttpClientTransport<AuthClient<reqwest::Client>> {
    let config = StreamableHttpClientTransportConfig {
        uri: spec.url.clone().into(),
        // rmcp's AuthClient prepends the Bearer header on every call;
        // suppress the bearer-only fast-path in `StreamableHttpClient` so
        // we don't double-set it.
        auth_header: None,
        ..Default::default()
    };
    StreamableHttpClientTransport::with_client(auth_client, config)
}

/// Build the bare reqwest client used by both transport flavours. Folds
/// the manifest's `headers` map into the default-header set and pins the
/// shared timeout.
pub(crate) fn build_reqwest_client(
    server_id: &str,
    spec: &HttpTransportSpec,
) -> Result<reqwest::Client, RunnerError> {
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

    reqwest::Client::builder()
        .default_headers(header_map)
        .timeout(HTTP_REQUEST_TIMEOUT)
        .build()
        .map_err(|e| {
            RunnerError::Mcp(format!(
                "MCP server `{server_id}`: build reqwest client: {e}"
            ))
        })
}

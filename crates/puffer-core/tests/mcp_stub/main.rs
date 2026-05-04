//! Tiny stdio MCP server used by `runner_mcp_stdio.rs` and the cross-backend
//! test in `puffer-runner-grpc`.
//!
//! The actual rmcp `ServerHandler` lives in `stub_server.rs` so the HTTP
//! integration test (`runner_mcp_http.rs`) and the cross-backend HTTP test
//! can hand the same handler to an axum-mounted
//! `StreamableHttpService`. This file is just a thin wrapper that wires
//! the handler to `rmcp::transport::io::stdio()` so the connection
//! manager can spawn it as a child process.

mod stub_server;

use rmcp::{serve_server, transport::io::stdio};
use stub_server::StubServer;

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

#[cfg(test)]
mod tests {
    use super::stub_server::encode_base64;

    #[test]
    fn base64_roundtrip_matches_known_value() {
        assert_eq!(encode_base64(&[0xde, 0xad, 0xbe]), "3q2+");
    }
}

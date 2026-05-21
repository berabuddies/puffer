//! Standalone gRPC server that exposes a `LocalToolRunner` over the puffer
//! tool-runner protocol. The puffer runtime can connect to this binary
//! instead of running tools in-process — useful for sandboxed workers,
//! remote desktop / cloud workspaces, and integration testing.

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use puffer_runner_grpc::server::ToolRunnerServer;
use puffer_tool_runner::{build_service_from_cwd, resolve_cwd};
use tonic::transport::Server;

#[derive(Debug, Parser)]
#[command(name = "puffer-tool-runner", version)]
struct Args {
    /// Address to bind the gRPC server to.
    #[arg(long, default_value = "127.0.0.1:50051")]
    bind: SocketAddr,

    /// Bearer token required on every RPC. Falls back to PUFFER_RUNNER_TOKEN.
    #[arg(long)]
    token: Option<String>,

    /// Allow unauthenticated RPCs. Intended only for local tests.
    #[arg(long)]
    allow_unauthenticated: bool,

    /// Working directory for tool execution. Also added to the sandbox roots.
    #[arg(long)]
    cwd: Option<PathBuf>,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<()> {
    let args = Args::parse();
    let token = resolve_auth_token(
        args.token.clone(),
        std::env::var("PUFFER_RUNNER_TOKEN").ok(),
        args.allow_unauthenticated,
    )?;

    let cwd = resolve_cwd(args.cwd)?;

    let (service, mcp_count) = build_service_from_cwd(&cwd, token.clone())
        .with_context(|| format!("build tool runner service for {}", cwd.display()))?;

    if mcp_count == 0 {
        eprintln!(
            "puffer-tool-runner: warning — discovered 0 MCP servers from {}; \
             MCP RPCs will return empty results until a server manifest is added \
             to .puffer/resources/mcp_servers/",
            cwd.display()
        );
    } else {
        eprintln!(
            "puffer-tool-runner: loaded {mcp_count} MCP server{plural} from {}",
            cwd.display(),
            plural = if mcp_count == 1 { "" } else { "s" },
        );
    }

    eprintln!(
        "puffer-tool-runner: listening on {} (cwd={}, auth={})",
        args.bind,
        cwd.display(),
        if token.is_some() { "yes" } else { "no" },
    );

    Server::builder()
        .add_service(ToolRunnerServer::new(service))
        .serve_with_shutdown(args.bind, async {
            let _ = tokio::signal::ctrl_c().await;
            eprintln!("puffer-tool-runner: shutdown signal received");
        })
        .await
        .context("server")?;

    Ok(())
}

fn resolve_auth_token(
    cli_token: Option<String>,
    env_token: Option<String>,
    allow_unauthenticated: bool,
) -> Result<Option<String>> {
    let token = cli_token
        .or(env_token)
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty());
    if token.is_none() && !allow_unauthenticated {
        anyhow::bail!(
            "puffer-tool-runner requires --token or PUFFER_RUNNER_TOKEN; pass --allow-unauthenticated only for local tests"
        );
    }
    Ok(token)
}

#[cfg(test)]
mod tests {
    use super::resolve_auth_token;

    #[test]
    fn auth_token_is_required_by_default() {
        let error = resolve_auth_token(None, None, false).unwrap_err();

        assert!(error.to_string().contains("requires --token"));
    }

    #[test]
    fn auth_token_can_come_from_env_or_explicit_test_override() {
        assert_eq!(
            resolve_auth_token(None, Some(" env-token ".to_string()), false).unwrap(),
            Some("env-token".to_string())
        );
        assert_eq!(resolve_auth_token(None, None, true).unwrap(), None);
    }
}

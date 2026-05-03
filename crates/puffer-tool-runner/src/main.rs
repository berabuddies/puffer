//! Standalone gRPC server that exposes a `LocalToolRunner` over the puffer
//! tool-runner protocol. The puffer runtime can connect to this binary
//! instead of running tools in-process — useful for sandboxed workers,
//! remote desktop / cloud workspaces, and integration testing.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;
use puffer_runner_api::ToolRunner;
use puffer_runner_grpc::server::{PermissionMode, ToolRunnerServer};
use puffer_runner_grpc::ToolRunnerService;
use puffer_runner_local::LocalToolRunner;
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

    /// Working directory for tool execution. Also added to the sandbox roots.
    #[arg(long)]
    cwd: Option<PathBuf>,

    /// Auto-approve every permission prompt. Intended for tests / dev use only.
    #[arg(long, default_value_t = false)]
    auto_approve: bool,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<()> {
    let args = Args::parse();
    let token = args
        .token
        .or_else(|| std::env::var("PUFFER_RUNNER_TOKEN").ok());

    let cwd = match args.cwd {
        Some(p) => p,
        None => std::env::current_dir().context("cwd")?,
    };
    if !cwd.is_dir() {
        anyhow::bail!("--cwd {:?} is not a directory", cwd);
    }

    let runner: Arc<dyn ToolRunner> =
        Arc::new(LocalToolRunner::with_sandbox_roots(vec![cwd.clone()]));

    let permission_mode = if args.auto_approve {
        PermissionMode::AutoApprove
    } else {
        PermissionMode::Unsupported
    };

    let service = ToolRunnerService::new(runner)
        .with_auth_token(token.clone())
        .with_permission_mode(permission_mode);

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

//! Picks a `ToolRunner` implementation for `AppState` based on the layered
//! `PufferConfig`.
//!
//! When `config.remote_runner` is `Some`, we attempt to connect a
//! [`puffer_runner_grpc::RemoteToolRunner`]. On failure we log the reason and
//! fall back to the in-process `LocalToolRunner` so the binary stays usable
//! even if the operator misconfigures the remote endpoint.

use std::sync::Arc;

use puffer_config::PufferConfig;
use puffer_runner_api::ToolRunner;
use puffer_runner_grpc::RemoteToolRunner;
use puffer_runner_local::LocalToolRunner;

pub fn select_tool_runner(config: &PufferConfig) -> Arc<dyn ToolRunner> {
    let Some(remote) = config.remote_runner.as_ref() else {
        return Arc::new(LocalToolRunner::new());
    };
    let token = remote.resolve_auth_token();
    match RemoteToolRunner::connect(&remote.endpoint, token.as_deref()) {
        Ok(runner) => Arc::new(runner),
        Err(err) => {
            eprintln!(
                "puffer: failed to connect to remote tool runner at {}: {err}; falling back to local",
                remote.endpoint
            );
            Arc::new(LocalToolRunner::new())
        }
    }
}

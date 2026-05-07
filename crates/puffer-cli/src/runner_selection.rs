//! Picks a `ToolRunner` implementation for `AppState` based on the layered
//! `PufferConfig`.
//!
//! ## Deployment model
//!
//! In the multi-tenant deployment, a Python orchestrator spawns one puffer
//! process per user pointing at that user's tool runner. The runner may
//! still be coming up when puffer launches — if puffer eagerly probed and
//! fell back to a local runner on failure, the central service would end
//! up executing commands on the central host's filesystem instead of the
//! user's, with the user none the wiser.
//!
//! So when `config.remote_runner` is set, this module:
//!   1. Constructs a `RemoteToolRunner` (lazy gRPC channel, no I/O).
//!   2. Loops calling `runner.ping()` with exponential backoff (capped),
//!      logging every attempt, until the runner answers.
//!   3. Returns the runner.
//!
//! There is **no fallback to `LocalToolRunner`** when a remote is
//! configured: the orchestrator is responsible for eventually starting the
//! runner, and puffer waits for it. The local-runner branch only fires
//! when no `remote_runner` is configured at all.

use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use puffer_config::{PufferConfig, RemoteRunnerConfig};
use puffer_resources::LoadedResources;
use puffer_runner_api::ToolRunner;
use puffer_runner_grpc::RemoteToolRunner;
use puffer_runner_local::local_runner_from_resources;

const DEFAULT_INITIAL_BACKOFF_MS: u64 = 1_000;
const DEFAULT_MAX_BACKOFF_MS: u64 = 10_000;

/// Returns the `ToolRunner` instance that `AppState` should use for this
/// process. With `config.remote_runner` set, this blocks until the remote
/// runner answers a `Ping` (no fallback). Without it, we hand back a
/// `LocalToolRunner` hydrated with the resolved MCP manifest.
pub fn select_tool_runner(
    config: &PufferConfig,
    resources: &LoadedResources,
    workspace_root: PathBuf,
) -> Arc<dyn ToolRunner> {
    if let Some(remote) = config.remote_runner.as_ref() {
        return wait_for_remote_runner(remote);
    }
    // The TUI / in-process flow leaves the sandbox wide open so resource
    // loaders can still reach `~/.config/puffer` and other non-workspace
    // paths. The standalone `puffer-tool-runner` binary, by contrast, opts
    // into a sandbox by passing `Some(vec![cwd])`.
    Arc::new(local_runner_from_resources(resources, workspace_root, None))
}

/// Builds a `RemoteToolRunner` against `config.endpoint` and pings it in a
/// loop until the remote reports back. Construction itself is infallible
/// because the channel is lazy; the only failure mode here is a malformed
/// endpoint, which we treat as fatal (panic) to surface a misconfiguration.
fn wait_for_remote_runner(config: &RemoteRunnerConfig) -> Arc<dyn ToolRunner> {
    let token = config.resolve_auth_token();
    let runner =
        RemoteToolRunner::connect(&config.endpoint, token.as_deref()).unwrap_or_else(|err| {
            panic!(
                "puffer: failed to construct remote runner for {}: {err}",
                config.endpoint
            )
        });

    let initial = Duration::from_millis(
        config
            .initial_backoff_ms
            .unwrap_or(DEFAULT_INITIAL_BACKOFF_MS),
    );
    let cap = Duration::from_millis(config.max_backoff_ms.unwrap_or(DEFAULT_MAX_BACKOFF_MS));
    let mut delay = initial;
    let mut attempt: u32 = 1;
    loop {
        match runner.ping() {
            Ok(ping) => {
                eprintln!(
                    "puffer: remote tool runner at {} is live (version={}, uptime={}s)",
                    config.endpoint,
                    ping.version,
                    ping.uptime.as_secs()
                );
                return Arc::new(runner);
            }
            Err(err) => {
                eprintln!(
                    "puffer: ping {} failed (attempt {attempt}): {err}; retrying in {:?}",
                    config.endpoint, delay
                );
                thread::sleep(delay);
                attempt = attempt.saturating_add(1);
                delay = next_backoff(delay, cap);
            }
        }
    }
}

/// Exponential backoff with a hard cap: 1s, 2s, 5s, 10s, 10s... by
/// default. We double then clamp; the 1→2→5→10 progression falls out of
/// the cap when the initial delay is 1s and the cap is 10s.
fn next_backoff(current: Duration, cap: Duration) -> Duration {
    let doubled = current.saturating_mul(2);
    if doubled > cap {
        cap
    } else if doubled < Duration::from_secs(5) && current < Duration::from_secs(2) {
        // 1s → 2s
        doubled
    } else if doubled < cap && current < Duration::from_secs(5) {
        // 2s → 5s (skip ahead so we don't sit at 4s briefly)
        Duration::from_secs(5)
    } else {
        cap
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_backoff_progresses_1_2_5_10() {
        let cap = Duration::from_secs(10);
        let mut d = Duration::from_secs(1);
        d = next_backoff(d, cap);
        assert_eq!(d, Duration::from_secs(2));
        d = next_backoff(d, cap);
        assert_eq!(d, Duration::from_secs(5));
        d = next_backoff(d, cap);
        assert_eq!(d, Duration::from_secs(10));
        d = next_backoff(d, cap);
        assert_eq!(d, Duration::from_secs(10));
    }
}

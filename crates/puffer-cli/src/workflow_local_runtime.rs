//! Local AgentEnv workflow runtime Docker Compose lifecycle management.

use anyhow::{Context, Result};
use puffer_config::{
    save_user_config, ConfigPaths, PufferConfig, WorkflowBackendConfig, WorkflowBackendMode,
};
use puffer_secrets::{SecretUpsert, SecretVault};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use url::Url;
use uuid::Uuid;

#[path = "workflow_local_runtime_bootstrap.rs"]
mod workflow_local_runtime_bootstrap;
use workflow_local_runtime_bootstrap::{
    non_empty_env, read_env_file, valid_uuid_env, write_runtime_files,
};

const AGENTENV_LOCAL_RUNTIME_IMAGE: &str = "agentenv/api-server:local";
const LOCAL_WORKFLOW_RUNTIME_PROJECT: &str = "puffer-workflow-runtime";
const LOCAL_WORKFLOW_RUNTIME_PROJECT_ENV: &str = "PUFFER_WORKFLOW_RUNTIME_PROJECT";
const LOCAL_WORKFLOW_RUNTIME_API_PORT: u16 = 3000;
const LOCAL_WORKFLOW_RUNTIME_READY_PATH: &str = "/v1/health/ready";
const LOCAL_WORKFLOW_RUNTIME_NODE_DEFINITIONS_PATH: &str = "/v1/workflows/node-definitions";
const LOCAL_WORKFLOW_RUNTIME_DIR: &str = "workflow-runtime";
const LOCAL_WORKFLOW_RUNTIME_DATA_DIR: &str = "data";
const LOCAL_WORKFLOW_RUNTIME_BOOTSTRAP_DIR: &str = "bootstrap";
const LOCAL_WORKFLOW_RUNTIME_COMPOSE_FILE: &str = "docker-compose.yml";
const LOCAL_WORKFLOW_RUNTIME_CONFIG_FILE: &str = "runtime.json";
const LOCAL_WORKFLOW_RUNTIME_ENV_FILE: &str = ".env";
const LOCAL_WORKFLOW_RUNTIME_SEED_FILE: &str = "seed.sql";
const POSTGRES_IMAGE: &str = "postgres:14-alpine";
const REDIS_IMAGE: &str = "redis:7-alpine";
const POSTGRES_URL: &str = "postgres://tintin:tintin@postgres:5432/tintin_cloud";
const REDIS_URL: &str = "redis://redis:6379";
const WORKFLOW_RUNTIME_TOKEN_LABEL: &str = "Workflow runtime API token";
const WORKFLOW_RUNTIME_TOKEN_DESCRIPTION: &str =
    "API token for the local AgentEnv workflow runtime.";
const READY_WAIT_ATTEMPTS: usize = 60;
const READY_WAIT_DELAY: Duration = Duration::from_millis(500);

/// Lifecycle state for the Puffer-managed local workflow runtime stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LocalWorkflowRuntimeState {
    /// Docker or Docker Compose could not be executed successfully.
    DockerMissing,
    /// The AgentEnv API image is not present locally.
    ImageMissing,
    /// Core services are being started.
    Starting,
    /// AgentEnv database migrations are being applied.
    Migrating,
    /// Puffer-owned local auth/workspace rows are being seeded.
    Seeding,
    /// The API is healthy and exposes AgentEnv workflow node definitions.
    Ready,
    /// The AgentEnv image or migrated schema does not match Puffer bootstrap.
    IncompatibleRuntime,
    /// The local runtime manager hit an unrecoverable local failure.
    Failed,
    /// The Compose project exists but the API is not running.
    Stopped,
}

impl LocalWorkflowRuntimeState {
    /// Returns the stable snake_case state label.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::DockerMissing => "docker_missing",
            Self::ImageMissing => "image_missing",
            Self::Starting => "starting",
            Self::Migrating => "migrating",
            Self::Seeding => "seeding",
            Self::Ready => "ready",
            Self::IncompatibleRuntime => "incompatible_runtime",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
        }
    }
}

/// Status snapshot for the Puffer-managed local workflow runtime stack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LocalWorkflowRuntimeStatus {
    /// Current lifecycle state.
    pub(crate) state: LocalWorkflowRuntimeState,
    /// AgentEnv API image used by `migrate` and `api`.
    pub(crate) image: String,
    /// Fixed Docker Compose project name.
    pub(crate) stack_name: String,
    /// Configured local API base URL when available.
    pub(crate) api_base_url: Option<String>,
    /// Generated Compose file path when available.
    pub(crate) compose_file: Option<PathBuf>,
    /// Local data directory mounted into Postgres and Redis.
    pub(crate) data_dir: Option<PathBuf>,
    /// Optional diagnostic detail.
    pub(crate) message: Option<String>,
}

/// Result of a user-confirmed local runtime data repair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LocalWorkflowRuntimeRepairResult {
    /// Runtime status after Puffer attempted to rebuild the local stack.
    pub(crate) status: LocalWorkflowRuntimeStatus,
    /// Previous local runtime data directories moved aside before rebuild.
    pub(crate) archived_data_dirs: Vec<PathBuf>,
}

/// Returns the current local workflow runtime status without changing config.
pub(crate) fn status(
    paths: &ConfigPaths,
    config: &PufferConfig,
) -> Result<LocalWorkflowRuntimeStatus> {
    let runner = SystemCommandRunner;
    let health = ReqwestHealthChecker::new()?;
    status_with(&runner, &health, paths, config)
}

/// Ensures the local workflow runtime is bootstrapped, started, and healthy.
pub(crate) fn ensure_ready(
    paths: &ConfigPaths,
    config: &mut PufferConfig,
) -> Result<LocalWorkflowRuntimeStatus> {
    start(paths, config)
}

/// Ensures the local runtime is ready without persisting local settings into
/// the user's selected workflow backend config.
pub(crate) fn ensure_ready_transient(
    paths: &ConfigPaths,
    config: &mut PufferConfig,
) -> Result<LocalWorkflowRuntimeStatus> {
    let runner = SystemCommandRunner;
    let health = ReqwestHealthChecker::new()?;
    start_transient_with(
        &runner,
        &health,
        paths,
        config,
        WaitPolicy::new(READY_WAIT_ATTEMPTS, READY_WAIT_DELAY),
    )
}

/// Starts the Puffer-managed AgentEnv Docker Compose workflow runtime stack.
pub(crate) fn start(
    paths: &ConfigPaths,
    config: &mut PufferConfig,
) -> Result<LocalWorkflowRuntimeStatus> {
    let runner = SystemCommandRunner;
    let health = ReqwestHealthChecker::new()?;
    start_with(
        &runner,
        &health,
        paths,
        config,
        true,
        WaitPolicy::new(READY_WAIT_ATTEMPTS, READY_WAIT_DELAY),
    )
}

/// Rebuilds the Puffer-managed local runtime data after explicit user confirmation.
pub(crate) fn repair(
    paths: &ConfigPaths,
    config: &mut PufferConfig,
) -> Result<LocalWorkflowRuntimeRepairResult> {
    let runner = SystemCommandRunner;
    let health = ReqwestHealthChecker::new()?;
    repair_with(
        &runner,
        &health,
        paths,
        config,
        WaitPolicy::new(READY_WAIT_ATTEMPTS, READY_WAIT_DELAY),
    )
}

fn start_transient_with<R, H>(
    runner: &R,
    health: &H,
    paths: &ConfigPaths,
    config: &mut PufferConfig,
    wait: WaitPolicy,
) -> Result<LocalWorkflowRuntimeStatus>
where
    R: CommandRunner,
    H: HealthChecker,
{
    start_with(runner, health, paths, config, false, wait)
}

fn repair_with<R, H>(
    runner: &R,
    health: &H,
    paths: &ConfigPaths,
    config: &mut PufferConfig,
    wait: WaitPolicy,
) -> Result<LocalWorkflowRuntimeRepairResult>
where
    R: CommandRunner,
    H: HealthChecker,
{
    if config.workflow_backend.mode != WorkflowBackendMode::Local {
        return Ok(LocalWorkflowRuntimeRepairResult {
            status: base_status(LocalWorkflowRuntimeState::Failed).with_message(
                "local workflow runtime repair requires workflow_backend.mode = local",
            ),
            archived_data_dirs: Vec::new(),
        });
    }

    let runtime = bootstrap_runtime_config(paths, config, true)?;
    if !docker_available(runner) {
        return Ok(LocalWorkflowRuntimeRepairResult {
            status: status_for_runtime(LocalWorkflowRuntimeState::DockerMissing, Some(&runtime)),
            archived_data_dirs: Vec::new(),
        });
    }
    if !image_exists(runner)? && !pull_image(runner)? {
        return Ok(LocalWorkflowRuntimeRepairResult {
            status: status_for_runtime(LocalWorkflowRuntimeState::ImageMissing, Some(&runtime))
                .with_message(
                    "local AgentEnv runtime image is not installed or could not be downloaded",
                ),
            archived_data_dirs: Vec::new(),
        });
    }

    if let Some(reason) = stale_api_container_reason(runner, &runtime)? {
        remove_stale_project_containers(runner, &runtime, &reason)?;
    }

    let down = compose(runner, &runtime, &["down", "--remove-orphans"])?;
    if !down.is_success() {
        return Ok(LocalWorkflowRuntimeRepairResult {
            status: status_for_runtime(LocalWorkflowRuntimeState::Failed, Some(&runtime))
                .with_message(format!(
                    "docker compose down --remove-orphans failed: {}",
                    trim_diagnostic(&down.stderr)
                )),
            archived_data_dirs: Vec::new(),
        });
    }

    let archived_data_dirs = archive_runtime_data_dirs(&runtime.data_dir)?;
    let mut status = start_with(runner, health, paths, config, true, wait)?;
    if status.state == LocalWorkflowRuntimeState::Ready && !archived_data_dirs.is_empty() {
        status = status.with_message(format!(
            "Archived previous local runtime data at {}.",
            archived_data_dirs
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    Ok(LocalWorkflowRuntimeRepairResult {
        status,
        archived_data_dirs,
    })
}

/// Stops the Puffer-managed local workflow runtime stack containers.
pub(crate) fn stop() -> Result<LocalWorkflowRuntimeStatus> {
    let runner = SystemCommandRunner;
    let paths =
        ConfigPaths::discover(std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    stop_with(&runner, &local_runtime_compose_file(&paths))
}

/// Returns Docker Compose logs for the Puffer-managed local workflow runtime stack.
pub(crate) fn logs() -> Result<String> {
    let runner = SystemCommandRunner;
    let paths =
        ConfigPaths::discover(std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    logs_with(&runner, &local_runtime_compose_file(&paths))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RuntimeContext {
    api_base_url: String,
    api_key: String,
    api_key_pepper: String,
    gateway_encryption_key: String,
    jwt_secret: String,
    jwt_refresh_secret: String,
    user_id: String,
    workspace_id: String,
    stack_name: String,
    stack_dir: PathBuf,
    compose_file: PathBuf,
    env_file: PathBuf,
    seed_file: PathBuf,
    data_dir: PathBuf,
    bootstrap_dir: PathBuf,
    host_port: u16,
}

#[derive(Debug, Clone, Copy)]
struct WaitPolicy {
    attempts: usize,
    delay: Duration,
}

impl WaitPolicy {
    fn new(attempts: usize, delay: Duration) -> Self {
        Self { attempts, delay }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StoredLocalRuntimeConfig {
    #[serde(default)]
    api_base_url: String,
    #[serde(default)]
    workspace_id: String,
    #[serde(default)]
    api_token_secret_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CommandOutput {
    status_code: i32,
    stdout: String,
    stderr: String,
}

impl CommandOutput {
    fn success(stdout: impl Into<String>) -> Self {
        Self {
            status_code: 0,
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }

    fn failure(stderr: impl Into<String>) -> Self {
        Self {
            status_code: 1,
            stdout: String::new(),
            stderr: stderr.into(),
        }
    }

    fn is_success(&self) -> bool {
        self.status_code == 0
    }
}

trait CommandRunner {
    fn run(
        &self,
        program: &str,
        args: &[String],
        env: &[(&str, String)],
    ) -> io::Result<CommandOutput>;
}

struct SystemCommandRunner;

impl CommandRunner for SystemCommandRunner {
    fn run(
        &self,
        program: &str,
        args: &[String],
        env: &[(&str, String)],
    ) -> io::Result<CommandOutput> {
        let mut command = Command::new(program);
        command.args(args);
        for (key, value) in env {
            command.env(key, value);
        }
        let output = command.output()?;
        Ok(CommandOutput {
            status_code: output.status.code().unwrap_or(1),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }
}

trait HealthChecker {
    fn ready(&self, api_base_url: &str) -> Result<bool>;

    fn node_definitions(
        &self,
        api_base_url: &str,
        workspace_id: &str,
        api_key: &str,
    ) -> Result<bool>;
}

struct ReqwestHealthChecker {
    client: Client,
}

impl ReqwestHealthChecker {
    fn new() -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .context("build local workflow runtime health client")?;
        Ok(Self { client })
    }
}

impl HealthChecker for ReqwestHealthChecker {
    fn ready(&self, api_base_url: &str) -> Result<bool> {
        let response = self
            .client
            .get(runtime_url(
                api_base_url,
                LOCAL_WORKFLOW_RUNTIME_READY_PATH,
            )?)
            .send()
            .context("check local workflow runtime readiness")?;
        Ok(response.status().is_success())
    }

    fn node_definitions(
        &self,
        api_base_url: &str,
        workspace_id: &str,
        api_key: &str,
    ) -> Result<bool> {
        let response = self
            .client
            .get(runtime_url(
                api_base_url,
                LOCAL_WORKFLOW_RUNTIME_NODE_DEFINITIONS_PATH,
            )?)
            .header("X-API-Key", api_key)
            .header("X-Workspace-ID", workspace_id)
            .send()
            .context("check local workflow runtime node definitions")?;
        Ok(response.status().is_success())
    }
}

fn status_with<R, H>(
    runner: &R,
    health: &H,
    paths: &ConfigPaths,
    config: &PufferConfig,
) -> Result<LocalWorkflowRuntimeStatus>
where
    R: CommandRunner,
    H: HealthChecker,
{
    if config.workflow_backend.mode != WorkflowBackendMode::Local {
        return Ok(base_status(LocalWorkflowRuntimeState::Failed)
            .with_message("local workflow runtime requires workflow_backend.mode = local"));
    }
    let runtime = runtime_context_from_config(paths, config).ok();
    inspect_status(runner, health, runtime.as_ref())
}

fn start_with<R, H>(
    runner: &R,
    health: &H,
    paths: &ConfigPaths,
    config: &mut PufferConfig,
    persist_user_config: bool,
    wait: WaitPolicy,
) -> Result<LocalWorkflowRuntimeStatus>
where
    R: CommandRunner,
    H: HealthChecker,
{
    if persist_user_config && config.workflow_backend.mode != WorkflowBackendMode::Local {
        return Ok(base_status(LocalWorkflowRuntimeState::Failed)
            .with_message("local workflow runtime requires workflow_backend.mode = local"));
    }

    let runtime = bootstrap_runtime_config(paths, config, persist_user_config)?;
    if !docker_available(runner) {
        return Ok(status_for_runtime(
            LocalWorkflowRuntimeState::DockerMissing,
            Some(&runtime),
        ));
    }
    if !image_exists(runner)? && !pull_image(runner)? {
        return Ok(
            status_for_runtime(LocalWorkflowRuntimeState::ImageMissing, Some(&runtime))
                .with_message(
                    "local AgentEnv runtime image is not installed or could not be downloaded",
                ),
        );
    }

    let stale_api_container = stale_api_container_reason(runner, &runtime)?;
    if let Some(reason) = stale_api_container.as_deref() {
        remove_stale_project_containers(runner, &runtime, reason)?;
    }
    let status = start_runtime_sequence(
        runner,
        health,
        &runtime,
        wait,
        stale_api_container.is_some(),
    )?;
    if status.state == LocalWorkflowRuntimeState::IncompatibleRuntime {
        match pull_image(runner) {
            Ok(true) => {
                let retry = start_runtime_sequence(runner, health, &runtime, wait, true)?;
                return Ok(
                    if retry.state == LocalWorkflowRuntimeState::IncompatibleRuntime {
                        let retry_message = retry
                            .message
                            .as_deref()
                            .unwrap_or("local runtime is still incompatible")
                            .to_string();
                        retry.with_message(format!(
                            "{retry_message} after refreshing {AGENTENV_LOCAL_RUNTIME_IMAGE}"
                        ))
                    } else {
                        retry
                    },
                );
            }
            Ok(false) => {
                let status_message = status
                    .message
                    .as_deref()
                    .unwrap_or("local runtime is incompatible")
                    .to_string();
                return Ok(status.with_message(format!(
                    "{status_message}; could not refresh {AGENTENV_LOCAL_RUNTIME_IMAGE}"
                )));
            }
            Err(error) => {
                let status_message = status
                    .message
                    .as_deref()
                    .unwrap_or("local runtime is incompatible")
                    .to_string();
                return Ok(status.with_message(format!(
                    "{status_message}; failed to refresh {AGENTENV_LOCAL_RUNTIME_IMAGE}: {error}"
                )));
            }
        }
    }
    Ok(status)
}

fn start_runtime_sequence<R, H>(
    runner: &R,
    health: &H,
    runtime: &RuntimeContext,
    wait: WaitPolicy,
    force_recreate_api: bool,
) -> Result<LocalWorkflowRuntimeStatus>
where
    R: CommandRunner,
    H: HealthChecker,
{
    let status = run_compose_step(
        runner,
        runtime,
        &["up", "-d", "postgres", "redis"],
        LocalWorkflowRuntimeState::Starting,
        LocalWorkflowRuntimeState::Failed,
        "docker compose up -d postgres redis failed",
    )?;
    if status.state != LocalWorkflowRuntimeState::Starting {
        return Ok(status);
    }
    let status = run_compose_step(
        runner,
        runtime,
        &["run", "--rm", "migrate"],
        LocalWorkflowRuntimeState::Migrating,
        LocalWorkflowRuntimeState::IncompatibleRuntime,
        "docker compose run --rm migrate failed",
    )?;
    if status.state != LocalWorkflowRuntimeState::Migrating {
        return Ok(status);
    }
    let status = run_compose_step(
        runner,
        runtime,
        &["run", "--rm", "seed"],
        LocalWorkflowRuntimeState::Seeding,
        LocalWorkflowRuntimeState::IncompatibleRuntime,
        "docker compose run --rm seed failed",
    )?;
    if status.state != LocalWorkflowRuntimeState::Seeding {
        return Ok(status);
    }
    let api_args = if force_recreate_api {
        vec!["up", "-d", "--force-recreate", "api"]
    } else {
        vec!["up", "-d", "api"]
    };
    let status = run_compose_step(
        runner,
        runtime,
        &api_args,
        LocalWorkflowRuntimeState::Starting,
        LocalWorkflowRuntimeState::Failed,
        "docker compose up -d api failed",
    )?;
    if status.state != LocalWorkflowRuntimeState::Starting {
        return Ok(status);
    }

    wait_until_ready(health, runtime, wait)
}

fn stop_with<R>(runner: &R, compose_file: &Path) -> Result<LocalWorkflowRuntimeStatus>
where
    R: CommandRunner,
{
    if !docker_available(runner) {
        return Ok(base_status(LocalWorkflowRuntimeState::DockerMissing));
    }
    let output = compose_at(
        runner,
        compose_file,
        &local_runtime_project_name(),
        &["stop"],
    )?;
    if output.is_success() {
        Ok(base_status(LocalWorkflowRuntimeState::Stopped))
    } else {
        Ok(base_status(LocalWorkflowRuntimeState::Failed).with_message(output.stderr))
    }
}

fn logs_with<R>(runner: &R, compose_file: &Path) -> Result<String>
where
    R: CommandRunner,
{
    let output = compose_at(
        runner,
        compose_file,
        &local_runtime_project_name(),
        &["logs", "--no-color"],
    )?;
    if output.is_success() {
        Ok(output.stdout)
    } else {
        anyhow::bail!(
            "docker compose logs failed: {}",
            trim_diagnostic(&output.stderr)
        );
    }
}

fn inspect_status<R, H>(
    runner: &R,
    health: &H,
    runtime: Option<&RuntimeContext>,
) -> Result<LocalWorkflowRuntimeStatus>
where
    R: CommandRunner,
    H: HealthChecker,
{
    if !docker_available(runner) {
        return Ok(status_for_runtime(
            LocalWorkflowRuntimeState::DockerMissing,
            runtime,
        ));
    }
    if !image_exists(runner)? {
        return Ok(status_for_runtime(
            LocalWorkflowRuntimeState::ImageMissing,
            runtime,
        ));
    }
    let Some(runtime) = runtime else {
        return Ok(base_status(LocalWorkflowRuntimeState::Failed)
            .with_message("local workflow runtime config is incomplete"));
    };
    let output = compose(
        runner,
        runtime,
        &["ps", "--services", "--status", "running"],
    )?;
    if !output.is_success() {
        return Ok(
            status_for_runtime(LocalWorkflowRuntimeState::Stopped, Some(runtime))
                .with_message(output.stderr),
        );
    }
    let running = output
        .stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<BTreeSet<_>>();
    if !running.contains("api") {
        return Ok(status_for_runtime(
            LocalWorkflowRuntimeState::Stopped,
            Some(runtime),
        ));
    }
    if !["postgres", "redis", "api"]
        .into_iter()
        .all(|service| running.contains(service))
    {
        return Ok(status_for_runtime(
            LocalWorkflowRuntimeState::Starting,
            Some(runtime),
        ));
    }
    runtime_ready_status(health, runtime)
}

fn runtime_ready_status<H>(
    health: &H,
    runtime: &RuntimeContext,
) -> Result<LocalWorkflowRuntimeStatus>
where
    H: HealthChecker,
{
    match health.ready(&runtime.api_base_url) {
        Ok(true) => {}
        Ok(false) => {
            return Ok(status_for_runtime(
                LocalWorkflowRuntimeState::Starting,
                Some(runtime),
            ));
        }
        Err(error) => {
            return Ok(
                status_for_runtime(LocalWorkflowRuntimeState::Starting, Some(runtime))
                    .with_message(error.to_string()),
            );
        }
    }
    match health.node_definitions(
        &runtime.api_base_url,
        &runtime.workspace_id,
        &runtime.api_key,
    ) {
        Ok(true) => Ok(status_for_runtime(
            LocalWorkflowRuntimeState::Ready,
            Some(runtime),
        )),
        Ok(false) => Ok(status_for_runtime(
            LocalWorkflowRuntimeState::IncompatibleRuntime,
            Some(runtime),
        )
        .with_message("AgentEnv node definitions endpoint rejected local credentials")),
        Err(error) => Ok(status_for_runtime(
            LocalWorkflowRuntimeState::IncompatibleRuntime,
            Some(runtime),
        )
        .with_message(error.to_string())),
    }
}

fn wait_until_ready<H>(
    health: &H,
    runtime: &RuntimeContext,
    wait: WaitPolicy,
) -> Result<LocalWorkflowRuntimeStatus>
where
    H: HealthChecker,
{
    let attempts = wait.attempts.max(1);
    let mut last_message = None;
    for index in 0..attempts {
        match health.ready(&runtime.api_base_url) {
            Ok(true) => return runtime_ready_status(health, runtime),
            Ok(false) => last_message = Some("local workflow runtime is not ready yet".to_string()),
            Err(error) => last_message = Some(error.to_string()),
        }
        if index + 1 < attempts && !wait.delay.is_zero() {
            thread::sleep(wait.delay);
        }
    }
    let mut status = status_for_runtime(LocalWorkflowRuntimeState::Starting, Some(runtime));
    if let Some(message) = last_message {
        status = status.with_message(message);
    }
    Ok(status)
}

fn run_compose_step<R>(
    runner: &R,
    runtime: &RuntimeContext,
    args: &[&str],
    running_state: LocalWorkflowRuntimeState,
    failed_state: LocalWorkflowRuntimeState,
    failed_label: &str,
) -> Result<LocalWorkflowRuntimeStatus>
where
    R: CommandRunner,
{
    let output = compose(runner, runtime, args)?;
    Ok(if output.is_success() {
        status_for_runtime(running_state, Some(runtime))
    } else {
        status_for_runtime(failed_state, Some(runtime)).with_message(format!(
            "{failed_label}: {}",
            trim_diagnostic(&output.stderr)
        ))
    })
}

fn docker_available<R>(runner: &R) -> bool
where
    R: CommandRunner,
{
    docker(runner, &["info"]).is_ok_and(|output| output.is_success())
        && docker(runner, &["compose", "version"]).is_ok_and(|output| output.is_success())
}

fn image_exists<R>(runner: &R) -> Result<bool>
where
    R: CommandRunner,
{
    Ok(docker(runner, &["image", "inspect", AGENTENV_LOCAL_RUNTIME_IMAGE])?.is_success())
}

fn pull_image<R>(runner: &R) -> Result<bool>
where
    R: CommandRunner,
{
    Ok(docker(runner, &["pull", AGENTENV_LOCAL_RUNTIME_IMAGE])?.is_success())
}

fn archive_runtime_data_dirs(data_dir: &Path) -> Result<Vec<PathBuf>> {
    fs::create_dir_all(data_dir).with_context(|| {
        format!(
            "create local workflow runtime data dir {}",
            data_dir.display()
        )
    })?;

    let stamp = repair_archive_stamp();
    let mut archived = Vec::new();
    for name in ["postgres", "redis"] {
        let path = data_dir.join(name);
        if !runtime_data_dir_has_entries(&path)? {
            continue;
        }
        let archive_path = unique_runtime_data_archive_path(data_dir, name, &stamp);
        fs::rename(&path, &archive_path).with_context(|| {
            format!(
                "archive local workflow runtime data dir {} to {}",
                path.display(),
                archive_path.display()
            )
        })?;
        archived.push(archive_path);
    }

    for name in ["postgres", "redis"] {
        let path = data_dir.join(name);
        fs::create_dir_all(&path).with_context(|| {
            format!(
                "create fresh local workflow runtime data dir {}",
                path.display()
            )
        })?;
    }
    Ok(archived)
}

fn runtime_data_dir_has_entries(path: &Path) -> Result<bool> {
    match fs::read_dir(path) {
        Ok(mut entries) => Ok(entries.next().transpose()?.is_some()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error)
            .with_context(|| format!("inspect local workflow runtime data dir {}", path.display())),
    }
}

fn unique_runtime_data_archive_path(data_dir: &Path, name: &str, stamp: &str) -> PathBuf {
    let base = data_dir.join(format!("{name}.repair-{stamp}"));
    if !base.exists() {
        return base;
    }
    for index in 1.. {
        let candidate = data_dir.join(format!("{name}.repair-{stamp}.{index}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!("unbounded archive path suffix search")
}

fn repair_archive_stamp() -> String {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}-{:09}", duration.as_secs(), duration.subsec_nanos())
}

fn stale_api_container_reason<R>(runner: &R, runtime: &RuntimeContext) -> Result<Option<String>>
where
    R: CommandRunner,
{
    let ids_output = docker_owned_api_containers(runner, runtime)?;
    if !ids_output.is_success() {
        return Ok(None);
    }
    let ids = ids_output
        .stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if ids.is_empty() {
        return Ok(None);
    }

    let mut inspect_args = vec!["inspect".to_string()];
    inspect_args.extend(ids.iter().map(|id| (*id).to_string()));
    let output = docker_strings(runner, &inspect_args)?;
    if !output.is_success() {
        return Ok(Some(format!(
            "stale local runtime container could not be inspected: {}",
            trim_diagnostic(&output.stderr)
        )));
    }
    let inspected: Value = match serde_json::from_str(&output.stdout) {
        Ok(value) => value,
        Err(error) => {
            return Ok(Some(format!(
                "stale local runtime container inspect output was invalid: {error}"
            )));
        }
    };
    let Some(containers) = inspected.as_array() else {
        return Ok(Some(
            "stale local runtime container inspect output was not a list".to_string(),
        ));
    };

    for container in containers {
        if let Some(reason) = stale_api_container_mismatch(container, runtime) {
            return Ok(Some(reason));
        }
    }
    Ok(None)
}

fn remove_stale_project_containers<R>(
    runner: &R,
    runtime: &RuntimeContext,
    reason: &str,
) -> Result<bool>
where
    R: CommandRunner,
{
    let ids_output = docker_project_containers(runner, runtime)?;
    if !ids_output.is_success() {
        anyhow::bail!(
            "stale local runtime container detected ({reason}), but docker ps failed: {}",
            trim_diagnostic(&ids_output.stderr)
        );
    }
    let ids = ids_output
        .stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if ids.is_empty() {
        return Ok(false);
    }

    let mut args = vec!["rm".to_string(), "-f".to_string()];
    args.extend(ids);
    let output = docker_strings(runner, &args)?;
    if !output.is_success() {
        anyhow::bail!(
            "stale local runtime container detected ({reason}), but docker rm -f failed: {}",
            trim_diagnostic(&output.stderr)
        );
    }
    Ok(true)
}

fn stale_api_container_mismatch(container: &Value, runtime: &RuntimeContext) -> Option<String> {
    let labels = container
        .get("Config")
        .and_then(|value| value.get("Labels"))
        .and_then(Value::as_object);
    let compose_files = labels
        .and_then(|labels| labels.get("com.docker.compose.project.config_files"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !compose_files
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .any(|value| same_path_label(value, &runtime.compose_file))
    {
        return Some(format!(
            "stale local runtime container was created from `{compose_files}`, expected `{}`",
            runtime.compose_file.display()
        ));
    }

    let api_port_key = format!("{LOCAL_WORKFLOW_RUNTIME_API_PORT}/tcp");
    let actual_port = container
        .get("NetworkSettings")
        .and_then(|value| value.get("Ports"))
        .and_then(|value| value.get(api_port_key.as_str()))
        .and_then(Value::as_array)
        .and_then(|bindings| bindings.iter().find_map(container_host_port));
    if actual_port != Some(runtime.host_port) {
        return Some(format!(
            "stale local runtime container exposes port {:?}, expected {}",
            actual_port, runtime.host_port
        ));
    }
    None
}

fn container_host_port(binding: &Value) -> Option<u16> {
    binding
        .get("HostPort")
        .and_then(Value::as_str)
        .and_then(|value| value.parse::<u16>().ok())
}

fn same_path_label(label: &str, expected: &Path) -> bool {
    let label_path = Path::new(label);
    if label_path == expected {
        return true;
    }
    let expected_canonical = fs::canonicalize(expected).unwrap_or_else(|_| expected.to_path_buf());
    let label_canonical = fs::canonicalize(label_path).unwrap_or_else(|_| label_path.to_path_buf());
    label_canonical == expected_canonical
}

fn docker_project_containers<R>(runner: &R, runtime: &RuntimeContext) -> io::Result<CommandOutput>
where
    R: CommandRunner,
{
    docker_strings(
        runner,
        &[
            "ps".to_string(),
            "-aq".to_string(),
            "--filter".to_string(),
            format!("label=com.docker.compose.project={}", runtime.stack_name),
        ],
    )
}

fn docker_owned_api_containers<R>(runner: &R, runtime: &RuntimeContext) -> io::Result<CommandOutput>
where
    R: CommandRunner,
{
    docker_strings(
        runner,
        &[
            "ps".to_string(),
            "-aq".to_string(),
            "--filter".to_string(),
            format!("label=com.docker.compose.project={}", runtime.stack_name),
            "--filter".to_string(),
            "label=com.docker.compose.service=api".to_string(),
        ],
    )
}

fn compose<R>(
    runner: &R,
    runtime: &RuntimeContext,
    command_args: &[&str],
) -> io::Result<CommandOutput>
where
    R: CommandRunner,
{
    compose_at(
        runner,
        &runtime.compose_file,
        &runtime.stack_name,
        command_args,
    )
}

fn compose_at<R>(
    runner: &R,
    compose_file: &Path,
    project_name: &str,
    command_args: &[&str],
) -> io::Result<CommandOutput>
where
    R: CommandRunner,
{
    let mut args = vec![
        "compose".to_string(),
        "-f".to_string(),
        compose_file.display().to_string(),
        "-p".to_string(),
        project_name.to_string(),
    ];
    args.extend(command_args.iter().map(|arg| (*arg).to_string()));
    runner.run("docker", &args, &[])
}

fn docker<R>(runner: &R, args: &[&str]) -> io::Result<CommandOutput>
where
    R: CommandRunner,
{
    let args = args
        .iter()
        .map(|arg| (*arg).to_string())
        .collect::<Vec<_>>();
    docker_strings(runner, &args)
}

fn docker_strings<R>(runner: &R, args: &[String]) -> io::Result<CommandOutput>
where
    R: CommandRunner,
{
    runner.run("docker", &args, &[])
}

fn bootstrap_runtime_config(
    paths: &ConfigPaths,
    config: &mut PufferConfig,
    persist_user_config: bool,
) -> Result<RuntimeContext> {
    let mut changed = false;
    let stack_dir = local_runtime_stack_dir(paths);
    let mut backend = if persist_user_config {
        config.workflow_backend.clone()
    } else {
        let stored =
            read_stored_local_runtime_config(&stack_dir.join(LOCAL_WORKFLOW_RUNTIME_CONFIG_FILE))?;
        WorkflowBackendConfig {
            mode: WorkflowBackendMode::Local,
            api_base_url: stored.api_base_url,
            frontend_url: WorkflowBackendConfig::default_frontend_url(WorkflowBackendMode::Local)
                .to_string(),
            workspace_id: stored.workspace_id,
            api_token_secret_id: stored.api_token_secret_id,
        }
    };
    if backend.mode != WorkflowBackendMode::Local {
        backend.mode = WorkflowBackendMode::Local;
        changed = true;
    }

    let bootstrap_dir = stack_dir.join(LOCAL_WORKFLOW_RUNTIME_BOOTSTRAP_DIR);
    let data_dir = stack_dir.join(LOCAL_WORKFLOW_RUNTIME_DATA_DIR);
    fs::create_dir_all(data_dir.join("postgres")).with_context(|| {
        format!(
            "create local workflow runtime postgres data dir {}",
            data_dir.join("postgres").display()
        )
    })?;
    fs::create_dir_all(data_dir.join("redis")).with_context(|| {
        format!(
            "create local workflow runtime redis data dir {}",
            data_dir.join("redis").display()
        )
    })?;
    fs::create_dir_all(&bootstrap_dir).with_context(|| {
        format!(
            "create local workflow runtime bootstrap dir {}",
            bootstrap_dir.display()
        )
    })?;

    let existing_env = read_env_file(&stack_dir.join(LOCAL_WORKFLOW_RUNTIME_ENV_FILE))?;
    let had_local_runtime_identity = local_workspace_id(&backend.workspace_id).is_some()
        && !backend.api_token_secret_id.trim().is_empty();
    let workspace_id = local_workspace_id(&backend.workspace_id).unwrap_or_else(|| {
        changed = true;
        Uuid::new_v4().to_string()
    });
    if backend.workspace_id != workspace_id {
        backend.workspace_id = workspace_id.clone();
        changed = true;
    }

    let (api_base_url, host_port) = match local_api_base_url(&backend.api_base_url) {
        Some(value)
            if had_local_runtime_identity
                || value.0
                    != WorkflowBackendConfig::default_api_base_url(WorkflowBackendMode::Local) =>
        {
            value
        }
        _ => {
            changed = true;
            allocated_local_api_base_url()?
        }
    };
    if backend.api_base_url != api_base_url {
        backend.api_base_url = api_base_url.clone();
        changed = true;
    }

    let (api_key, secret_id, secret_changed) =
        resolve_or_create_api_token(paths, &backend.api_token_secret_id, &api_base_url)?;
    if secret_changed || backend.api_token_secret_id != secret_id {
        backend.api_token_secret_id = secret_id;
        changed = true;
    }

    config.workflow_backend = backend.clone();
    if changed {
        if persist_user_config {
            save_user_config(paths, config).context("save local workflow runtime config")?;
        } else {
            write_stored_local_runtime_config(
                &stack_dir.join(LOCAL_WORKFLOW_RUNTIME_CONFIG_FILE),
                &backend,
            )?;
        }
    } else if !persist_user_config && !stack_dir.join(LOCAL_WORKFLOW_RUNTIME_CONFIG_FILE).exists() {
        write_stored_local_runtime_config(
            &stack_dir.join(LOCAL_WORKFLOW_RUNTIME_CONFIG_FILE),
            &backend,
        )?;
    }

    let user_id = valid_uuid_env(&existing_env, "LOCAL_USER_ID")
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let api_key_pepper =
        non_empty_env(&existing_env, "API_KEY_PEPPER").unwrap_or_else(generated_api_key_pepper);
    let gateway_encryption_key = non_empty_env(&existing_env, "GATEWAY_ENCRYPTION_KEY")
        .unwrap_or_else(generated_runtime_secret);
    let jwt_secret =
        non_empty_env(&existing_env, "JWT_SECRET").unwrap_or_else(generated_runtime_secret);
    let jwt_refresh_secret =
        non_empty_env(&existing_env, "JWT_REFRESH_SECRET").unwrap_or_else(generated_runtime_secret);
    let runtime = RuntimeContext {
        api_base_url,
        api_key,
        api_key_pepper,
        gateway_encryption_key,
        jwt_secret,
        jwt_refresh_secret,
        user_id,
        workspace_id,
        stack_name: local_runtime_project_name(),
        compose_file: stack_dir.join(LOCAL_WORKFLOW_RUNTIME_COMPOSE_FILE),
        env_file: stack_dir.join(LOCAL_WORKFLOW_RUNTIME_ENV_FILE),
        seed_file: bootstrap_dir.join(LOCAL_WORKFLOW_RUNTIME_SEED_FILE),
        stack_dir,
        data_dir,
        bootstrap_dir,
        host_port,
    };
    write_runtime_files(&runtime)?;
    Ok(runtime)
}

fn runtime_context_from_config(
    paths: &ConfigPaths,
    config: &PufferConfig,
) -> Result<RuntimeContext> {
    let stack_dir = local_runtime_stack_dir(paths);
    let bootstrap_dir = stack_dir.join(LOCAL_WORKFLOW_RUNTIME_BOOTSTRAP_DIR);
    let data_dir = stack_dir.join(LOCAL_WORKFLOW_RUNTIME_DATA_DIR);
    let env_file = stack_dir.join(LOCAL_WORKFLOW_RUNTIME_ENV_FILE);
    let env = read_env_file(&env_file)?;
    let (api_base_url, host_port) = local_api_base_url(&config.workflow_backend.api_base_url)
        .context("local workflow runtime API URL is not configured for localhost")?;
    let workspace_id = local_workspace_id(&config.workflow_backend.workspace_id)
        .context("local workflow runtime workspace id is not a UUID")?;
    let user_id = valid_uuid_env(&env, "LOCAL_USER_ID")
        .context("local workflow runtime user id is not configured")?;
    let api_key_pepper = non_empty_env(&env, "API_KEY_PEPPER")
        .context("local workflow runtime API key pepper is not configured")?;
    let gateway_encryption_key = non_empty_env(&env, "GATEWAY_ENCRYPTION_KEY")
        .context("local workflow runtime gateway encryption key is not configured")?;
    let jwt_secret = non_empty_env(&env, "JWT_SECRET").unwrap_or_default();
    let jwt_refresh_secret = non_empty_env(&env, "JWT_REFRESH_SECRET").unwrap_or_default();
    let secret_id = trimmed(&config.workflow_backend.api_token_secret_id)
        .context("local workflow runtime token is not configured")?;
    let vault = SecretVault::open(SecretVault::default_path(&paths.user_config_dir))
        .context("open encrypted secret store")?;
    let api_key = vault
        .reveal(secret_id)
        .context("load local workflow runtime token from secret store")?
        .value;
    Ok(RuntimeContext {
        api_base_url,
        api_key,
        api_key_pepper,
        gateway_encryption_key,
        jwt_secret,
        jwt_refresh_secret,
        user_id,
        workspace_id,
        stack_name: local_runtime_project_name(),
        compose_file: stack_dir.join(LOCAL_WORKFLOW_RUNTIME_COMPOSE_FILE),
        env_file,
        seed_file: bootstrap_dir.join(LOCAL_WORKFLOW_RUNTIME_SEED_FILE),
        stack_dir,
        data_dir,
        bootstrap_dir,
        host_port,
    })
}

fn resolve_or_create_api_token(
    paths: &ConfigPaths,
    api_token_secret_id: &str,
    api_base_url: &str,
) -> Result<(String, String, bool)> {
    let vault = SecretVault::open(SecretVault::default_path(&paths.user_config_dir))
        .context("open encrypted secret store")?;
    if let Some(secret_id) = trimmed(api_token_secret_id) {
        match vault.reveal(secret_id) {
            Ok(resolved) => return Ok((resolved.value, secret_id.to_string(), false)),
            Err(error) => {
                tracing::warn!(
                    secret_id,
                    error = %error,
                    "regenerating unreadable local workflow runtime token"
                );
            }
        }
    }
    store_new_api_token(&vault, api_base_url)
}

fn store_new_api_token(vault: &SecretVault, api_base_url: &str) -> Result<(String, String, bool)> {
    let token = generated_api_token();
    let summary = vault.put(SecretUpsert {
        id: None,
        label: WORKFLOW_RUNTIME_TOKEN_LABEL.to_string(),
        description: Some(WORKFLOW_RUNTIME_TOKEN_DESCRIPTION.to_string()),
        value: token.clone(),
        username: None,
        origin: Some(api_base_url.to_string()),
        source: "local_workflow_runtime".to_string(),
    })?;
    Ok((token, summary.id, true))
}

fn read_stored_local_runtime_config(path: &Path) -> Result<StoredLocalRuntimeConfig> {
    if !path.exists() {
        return Ok(StoredLocalRuntimeConfig::default());
    }
    let raw = fs::read_to_string(path)
        .with_context(|| format!("read local workflow runtime config {}", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(StoredLocalRuntimeConfig::default());
    }
    serde_json::from_str(&raw)
        .with_context(|| format!("parse local workflow runtime config {}", path.display()))
}

fn write_stored_local_runtime_config(path: &Path, backend: &WorkflowBackendConfig) -> Result<()> {
    let stored = StoredLocalRuntimeConfig {
        api_base_url: backend.api_base_url.clone(),
        workspace_id: backend.workspace_id.clone(),
        api_token_secret_id: backend.api_token_secret_id.clone(),
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "create local workflow runtime config dir {}",
                parent.display()
            )
        })?;
    }
    let body =
        serde_json::to_vec_pretty(&stored).context("serialize local workflow runtime config")?;
    fs::write(path, body)
        .with_context(|| format!("write local workflow runtime config {}", path.display()))
}

fn local_runtime_stack_dir(paths: &ConfigPaths) -> PathBuf {
    paths.user_config_dir.join(LOCAL_WORKFLOW_RUNTIME_DIR)
}

fn local_runtime_compose_file(paths: &ConfigPaths) -> PathBuf {
    local_runtime_stack_dir(paths).join(LOCAL_WORKFLOW_RUNTIME_COMPOSE_FILE)
}

fn local_runtime_project_name() -> String {
    std::env::var(LOCAL_WORKFLOW_RUNTIME_PROJECT_ENV)
        .ok()
        .and_then(|value| valid_compose_project_name(&value))
        .unwrap_or_else(|| LOCAL_WORKFLOW_RUNTIME_PROJECT.to_string())
}

fn valid_compose_project_name(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut chars = trimmed.chars();
    let first = chars.next()?;
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return None;
    }
    if !chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_') {
        return None;
    }
    Some(trimmed.to_string())
}

fn local_api_base_url(value: &str) -> Option<(String, u16)> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let parsed = Url::parse(trimmed).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") || !is_loopback_host(parsed.host_str()?) {
        return None;
    }
    let port = parsed.port()?;
    Some((parsed.origin().ascii_serialization(), port))
}

fn local_workspace_id(value: &str) -> Option<String> {
    let trimmed = trimmed(value)?;
    Uuid::parse_str(trimmed).ok()?;
    Some(trimmed.to_string())
}

fn allocated_local_api_base_url() -> Result<(String, u16)> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).context("allocate local workflow port")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok((format!("http://127.0.0.1:{port}"), port))
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1"
}

fn generated_api_token() -> String {
    format!(
        "pufw_{}{}",
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple()
    )
}

fn generated_api_key_pepper() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

fn generated_runtime_secret() -> String {
    format!(
        "{}{}{}{}",
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple()
    )
}

fn runtime_url(api_base_url: &str, path: &str) -> Result<Url> {
    let mut url = Url::parse(api_base_url).context("parse local workflow runtime API URL")?;
    url.set_path(path);
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

fn base_status(state: LocalWorkflowRuntimeState) -> LocalWorkflowRuntimeStatus {
    LocalWorkflowRuntimeStatus {
        state,
        image: AGENTENV_LOCAL_RUNTIME_IMAGE.to_string(),
        stack_name: local_runtime_project_name(),
        api_base_url: None,
        compose_file: None,
        data_dir: None,
        message: None,
    }
}

fn status_for_runtime(
    state: LocalWorkflowRuntimeState,
    runtime: Option<&RuntimeContext>,
) -> LocalWorkflowRuntimeStatus {
    let mut status = base_status(state);
    if let Some(runtime) = runtime {
        status.stack_name = runtime.stack_name.clone();
        status.api_base_url = Some(runtime.api_base_url.clone());
        status.compose_file = Some(runtime.compose_file.clone());
        status.data_dir = Some(runtime.data_dir.clone());
    }
    status
}

impl LocalWorkflowRuntimeStatus {
    fn with_message(mut self, message: impl Into<String>) -> Self {
        let message = message.into();
        if !message.trim().is_empty() {
            self.message = Some(message);
        }
        self
    }
}

fn trimmed(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn trim_diagnostic(value: &str) -> String {
    value.trim().to_string()
}

#[cfg(test)]
#[path = "workflow_local_runtime_tests.rs"]
mod tests;

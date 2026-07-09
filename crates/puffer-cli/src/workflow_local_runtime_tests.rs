use super::*;
use crate::daemon_workflow_backend_settings::test_support::{
    lock_secret_store, temp_paths, ScopedSecretStoreKey,
};
use puffer_config::{ensure_workspace_dirs, load_config, save_user_config, WorkflowBackendMode};
use puffer_secrets::{SecretUpsert, SecretVault};
use std::collections::VecDeque;
use std::fs;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;

#[derive(Clone, Debug, PartialEq, Eq)]
struct FakeCommandCall {
    program: String,
    args: Vec<String>,
    env: Vec<(String, String)>,
}

#[derive(Clone)]
struct FakeCommandRunner {
    calls: Arc<Mutex<Vec<FakeCommandCall>>>,
    responses: Arc<Mutex<VecDeque<io::Result<CommandOutput>>>>,
}

impl FakeCommandRunner {
    fn new(responses: Vec<io::Result<CommandOutput>>) -> Self {
        Self {
            calls: Arc::new(Mutex::new(Vec::new())),
            responses: Arc::new(Mutex::new(VecDeque::from(responses))),
        }
    }

    fn calls(&self) -> Vec<FakeCommandCall> {
        self.calls.lock().expect("calls").clone()
    }
}

impl CommandRunner for FakeCommandRunner {
    fn run(
        &self,
        program: &str,
        args: &[String],
        env: &[(&str, String)],
    ) -> io::Result<CommandOutput> {
        self.calls.lock().expect("calls").push(FakeCommandCall {
            program: program.to_string(),
            args: args.to_vec(),
            env: env
                .iter()
                .map(|(key, value)| ((*key).to_string(), value.clone()))
                .collect(),
        });
        self.responses
            .lock()
            .expect("responses")
            .pop_front()
            .expect("fake response")
    }
}

#[derive(Clone)]
struct FakeHealthChecker {
    ready: bool,
    node_definitions: bool,
    node_definition_results: Arc<Mutex<VecDeque<bool>>>,
    ready_calls: Arc<Mutex<Vec<String>>>,
    node_definition_calls: Arc<Mutex<Vec<(String, String, String)>>>,
}

impl FakeHealthChecker {
    fn ready() -> Self {
        Self {
            ready: true,
            node_definitions: true,
            node_definition_results: Arc::new(Mutex::new(VecDeque::new())),
            ready_calls: Arc::new(Mutex::new(Vec::new())),
            node_definition_calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn not_ready() -> Self {
        Self {
            ready: false,
            node_definitions: false,
            node_definition_results: Arc::new(Mutex::new(VecDeque::new())),
            ready_calls: Arc::new(Mutex::new(Vec::new())),
            node_definition_calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn without_node_definitions() -> Self {
        Self {
            ready: true,
            node_definitions: false,
            node_definition_results: Arc::new(Mutex::new(VecDeque::new())),
            ready_calls: Arc::new(Mutex::new(Vec::new())),
            node_definition_calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn node_definitions_ready_after_retry() -> Self {
        Self {
            ready: true,
            node_definitions: true,
            node_definition_results: Arc::new(Mutex::new(VecDeque::from(vec![false, true]))),
            ready_calls: Arc::new(Mutex::new(Vec::new())),
            node_definition_calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn node_definition_calls(&self) -> Vec<(String, String, String)> {
        self.node_definition_calls
            .lock()
            .expect("node definition calls")
            .clone()
    }
}

impl HealthChecker for FakeHealthChecker {
    fn ready(&self, api_base_url: &str) -> Result<bool> {
        self.ready_calls
            .lock()
            .expect("ready calls")
            .push(api_base_url.to_string());
        Ok(self.ready)
    }

    fn node_definitions(
        &self,
        api_base_url: &str,
        workspace_id: &str,
        api_key: &str,
    ) -> Result<bool> {
        self.node_definition_calls
            .lock()
            .expect("node definition calls")
            .push((
                api_base_url.to_string(),
                workspace_id.to_string(),
                api_key.to_string(),
            ));
        Ok(self
            .node_definition_results
            .lock()
            .expect("node definition results")
            .pop_front()
            .unwrap_or(self.node_definitions))
    }
}

fn missing_docker_error() -> io::Result<CommandOutput> {
    Err(io::Error::new(io::ErrorKind::NotFound, "docker"))
}

fn ok(stdout: &str) -> io::Result<CommandOutput> {
    Ok(CommandOutput::success(stdout))
}

fn fail(stderr: &str) -> io::Result<CommandOutput> {
    Ok(CommandOutput::failure(stderr))
}

fn no_api_container() -> io::Result<CommandOutput> {
    ok("")
}

fn available() -> Vec<io::Result<CommandOutput>> {
    vec![ok("Docker version 1\n"), ok("Docker Compose version 2\n")]
}

fn test_paths(temp: &TempDir) -> ConfigPaths {
    let paths = temp_paths(temp);
    ensure_workspace_dirs(&paths).expect("workspace dirs");
    paths
}

fn configured_local_runtime(paths: &ConfigPaths) -> PufferConfig {
    let mut config = PufferConfig::default();
    config.workflow_backend.workspace_id = "11111111-1111-4111-8111-111111111111".to_string();
    let vault =
        SecretVault::open(SecretVault::default_path(&paths.user_config_dir)).expect("open vault");
    let summary = vault
        .put(SecretUpsert {
            id: None,
            label: "test".to_string(),
            description: None,
            value: "token-test".to_string(),
            username: None,
            origin: Some(config.workflow_backend.api_base_url.clone()),
            source: "test".to_string(),
        })
        .expect("store token");
    config.workflow_backend.api_token_secret_id = summary.id;
    write_test_env(paths, "22222222-2222-4222-8222-222222222222", "pepper-test");
    config
}

fn write_test_env(paths: &ConfigPaths, user_id: &str, pepper: &str) {
    let stack_dir = local_runtime_stack_dir(paths);
    fs::create_dir_all(&stack_dir).expect("stack dir");
    fs::write(
        stack_dir.join(LOCAL_WORKFLOW_RUNTIME_ENV_FILE),
        format!(
            "NODE_ENV=development\nGRPC_USE_TLS=false\nSCHEDULER_PROTO_PATH=/app/protos/scheduler/scheduler.proto\nHYPERVISOR_PROTO_PATH=/app/protos/hypervisor/hypervisor.proto\nDATABASE_URL={POSTGRES_URL}\nREDIS_URL={REDIS_URL}\nAPI_KEY_PEPPER={pepper}\nGATEWAY_ENCRYPTION_KEY=gateway-test\nLOCAL_USER_ID={user_id}\nLOCAL_WORKSPACE_ID=11111111-1111-4111-8111-111111111111\n"
        ),
    )
    .expect("write env");
}

fn reveal_runtime_token(paths: &ConfigPaths, config: &PufferConfig) -> String {
    let vault =
        SecretVault::open(SecretVault::default_path(&paths.user_config_dir)).expect("open vault");
    vault
        .reveal(&config.workflow_backend.api_token_secret_id)
        .expect("reveal token")
        .value
}

fn compose_service_headings(compose: &str) -> Vec<String> {
    compose
        .lines()
        .filter(|line| line.starts_with("  ") && !line.starts_with("    "))
        .filter(|line| line.trim_end().ends_with(':'))
        .map(|line| line.trim().to_string())
        .collect()
}

fn is_compose_command(call: &FakeCommandCall, tail: &[&str]) -> bool {
    call.program == "docker"
        && call.args.first().is_some_and(|arg| arg == "compose")
        && call.args.ends_with(
            &tail
                .iter()
                .map(|arg| (*arg).to_string())
                .collect::<Vec<_>>(),
        )
}

#[test]
fn docker_unavailable_returns_docker_missing() {
    let _guard = lock_secret_store();
    let _secret_store_key = ScopedSecretStoreKey::set();
    let temp = tempfile::tempdir().expect("tempdir");
    let paths = test_paths(&temp);
    let config = configured_local_runtime(&paths);
    let runner = FakeCommandRunner::new(vec![missing_docker_error()]);

    let status =
        status_with(&runner, &FakeHealthChecker::not_ready(), &paths, &config).expect("status");

    assert_eq!(status.state, LocalWorkflowRuntimeState::DockerMissing);
}

#[test]
fn image_missing_pulls_before_reporting_missing() {
    let _guard = lock_secret_store();
    let _secret_store_key = ScopedSecretStoreKey::set();
    let temp = tempfile::tempdir().expect("tempdir");
    let paths = test_paths(&temp);
    let mut config = PufferConfig::default();
    let mut responses = Vec::new();
    responses.extend(available());
    responses.push(fail("no image"));
    responses.push(fail("pull failed"));
    let runner = FakeCommandRunner::new(responses);

    let status = start_with(
        &runner,
        &FakeHealthChecker::not_ready(),
        &paths,
        &mut config,
        true,
        WaitPolicy::new(1, Duration::ZERO),
    )
    .expect("start");

    assert_eq!(status.state, LocalWorkflowRuntimeState::ImageMissing);
    assert!(runner.calls().iter().any(|call| call.program == "docker"
        && call.args.as_slice() == ["pull", "agentenv/api-server:local"]));
    assert!(!runner
        .calls()
        .iter()
        .any(|call| is_compose_command(call, &["up", "-d", "postgres", "redis"])));
}

#[test]
fn start_generates_compose_env_seed_and_runs_fixed_sequence() {
    let _guard = lock_secret_store();
    let _secret_store_key = ScopedSecretStoreKey::set();
    let temp = tempfile::tempdir().expect("tempdir");
    let paths = test_paths(&temp);
    let mut config = PufferConfig::default();
    let mut responses = Vec::new();
    responses.extend(available());
    responses.push(ok("image"));
    responses.push(no_api_container());
    responses.push(ok("postgres redis"));
    responses.push(ok("migrated"));
    responses.push(ok("seeded"));
    responses.push(ok("api"));
    let runner = FakeCommandRunner::new(responses);
    let health = FakeHealthChecker::ready();

    let status = start_with(
        &runner,
        &health,
        &paths,
        &mut config,
        true,
        WaitPolicy::new(1, Duration::ZERO),
    )
    .expect("start");

    assert_eq!(status.state, LocalWorkflowRuntimeState::Ready);
    assert!(Uuid::parse_str(&config.workflow_backend.workspace_id).is_ok());
    assert!(config
        .workflow_backend
        .api_base_url
        .starts_with("http://127.0.0.1:"));
    assert_ne!(
        config.workflow_backend.api_base_url,
        "http://127.0.0.1:3000"
    );
    assert!(!config.workflow_backend.api_token_secret_id.is_empty());

    let token = reveal_runtime_token(&paths, &config);
    let stack_dir = paths.user_config_dir.join(LOCAL_WORKFLOW_RUNTIME_DIR);
    let compose = fs::read_to_string(stack_dir.join(LOCAL_WORKFLOW_RUNTIME_COMPOSE_FILE))
        .expect("read compose");
    assert_eq!(
        compose_service_headings(&compose),
        vec!["postgres:", "redis:", "migrate:", "seed:", "api:"]
    );
    assert!(compose.contains("name: puffer-workflow-runtime"));
    assert!(compose.contains("image: postgres:14-alpine"));
    assert!(compose.contains("image: redis:7-alpine"));
    assert!(compose.contains("image: agentenv/api-server:local"));
    assert!(compose.contains("command: [\"node\", \"dist/database/migrate.js\"]"));
    assert!(compose.contains("psql \\\"$$DATABASE_URL\\\" -f /bootstrap/seed.sql"));
    assert!(compose.contains("127.0.0.1:"));
    assert!(!compose.contains("127.0.0.1:3000:3000"));
    assert!(!compose.contains("\n  api-server:"));
    assert!(!compose.contains("workflow-worker"));
    assert!(!compose.contains(&token));

    let env = fs::read_to_string(stack_dir.join(LOCAL_WORKFLOW_RUNTIME_ENV_FILE)).expect("env");
    assert!(env.contains("NODE_ENV=development"));
    assert!(env.contains("GRPC_USE_TLS=false"));
    assert!(env.contains("SCHEDULER_PROTO_PATH=/app/protos/scheduler/scheduler.proto"));
    assert!(env.contains("HYPERVISOR_PROTO_PATH=/app/protos/hypervisor/hypervisor.proto"));
    assert!(env.contains(&format!("DATABASE_URL={POSTGRES_URL}")));
    assert!(env.contains(&format!("REDIS_URL={REDIS_URL}")));
    assert!(env.contains("API_KEY_PEPPER="));
    assert!(env.contains("GATEWAY_ENCRYPTION_KEY="));
    assert!(env.contains("JWT_SECRET="));
    assert!(env.contains("JWT_REFRESH_SECRET="));
    assert!(env.contains("LOCAL_USER_ID="));
    assert!(env.contains("LOCAL_WORKSPACE_ID="));
    assert!(!env.contains(&token));

    let seed = fs::read_to_string(
        stack_dir
            .join(LOCAL_WORKFLOW_RUNTIME_BOOTSTRAP_DIR)
            .join(LOCAL_WORKFLOW_RUNTIME_SEED_FILE),
    )
    .expect("seed");
    assert!(seed.contains("INSERT INTO users"));
    assert!(seed.contains("INSERT INTO workspaces"));
    assert!(seed.contains("INSERT INTO user_workspaces"));
    assert!(seed.contains("INSERT INTO api_keys"));
    assert!(seed.contains("'user', NULL, 'Puffer Local'"));
    assert!(!seed.contains("'workspace',"));
    assert_eq!(seed.matches("ON CONFLICT").count(), 4);
    assert!(seed.contains("\"keyHash\""));
    assert!(!seed.contains(&token));

    let raw_config = fs::read_to_string(paths.user_config_file()).expect("read config");
    assert!(!raw_config.contains(&token));
    assert_eq!(
        health.node_definition_calls(),
        vec![(
            config.workflow_backend.api_base_url.clone(),
            config.workflow_backend.workspace_id.clone(),
            token
        )]
    );

    let calls = runner.calls();
    assert!(calls
        .iter()
        .any(|call| is_compose_command(call, &["up", "-d", "postgres", "redis"])));
    assert!(calls
        .iter()
        .any(|call| is_compose_command(call, &["run", "--rm", "migrate"])));
    assert!(calls
        .iter()
        .any(|call| is_compose_command(call, &["run", "--rm", "seed"])));
    assert!(calls
        .iter()
        .any(|call| is_compose_command(call, &["up", "-d", "api"])));
    assert!(calls.iter().all(|call| {
        !(call.program == "docker" && call.args.first().is_some_and(|arg| arg == "run"))
    }));
    let first_compose = calls
        .iter()
        .find(|call| {
            call.args.first().is_some_and(|arg| arg == "compose")
                && call.args.iter().any(|arg| arg == "-f")
        })
        .expect("compose call");
    let file_index = first_compose
        .args
        .iter()
        .position(|arg| arg == "-f")
        .expect("-f");
    let project_index = first_compose
        .args
        .iter()
        .position(|arg| arg == "-p")
        .expect("-p");
    assert!(file_index < project_index);
}

#[test]
fn stale_api_container_forces_api_recreate() {
    let _guard = lock_secret_store();
    let _secret_store_key = ScopedSecretStoreKey::set();
    let temp = tempfile::tempdir().expect("tempdir");
    let paths = test_paths(&temp);
    let mut config = PufferConfig::default();
    let stale_inspect = r#"[
      {
        "Config": {
          "Labels": {
            "com.docker.compose.project.config_files": "/tmp/old/docker-compose.yml"
          }
        },
        "NetworkSettings": {
          "Ports": {
            "3000/tcp": [
              { "HostIp": "127.0.0.1", "HostPort": "3000" }
            ]
          }
        }
      }
    ]"#;
    let mut responses = Vec::new();
    responses.extend(available());
    responses.push(ok("image"));
    responses.push(ok("api-container-id\n"));
    responses.push(ok(stale_inspect));
    responses.push(ok(
        "api-container-id\npostgres-container-id\nredis-container-id\n",
    ));
    responses.push(ok("removed\n"));
    responses.push(ok("postgres redis"));
    responses.push(ok("migrated"));
    responses.push(ok("seeded"));
    responses.push(ok("api recreated"));
    let runner = FakeCommandRunner::new(responses);
    let health = FakeHealthChecker::ready();

    let status = start_with(
        &runner,
        &health,
        &paths,
        &mut config,
        true,
        WaitPolicy::new(1, Duration::ZERO),
    )
    .expect("start");

    assert_eq!(status.state, LocalWorkflowRuntimeState::Ready);
    assert!(runner.calls().iter().any(|call| call.program == "docker"
        && call.args.as_slice()
            == [
                "rm",
                "-f",
                "api-container-id",
                "postgres-container-id",
                "redis-container-id"
            ]));
    assert!(runner
        .calls()
        .iter()
        .any(|call| is_compose_command(call, &["up", "-d", "--force-recreate", "api"])));
}

#[test]
fn start_regenerates_unreadable_local_runtime_token() {
    let _guard = lock_secret_store();
    let _secret_store_key = ScopedSecretStoreKey::set();
    let temp = tempfile::tempdir().expect("tempdir");
    let paths = test_paths(&temp);
    let mut config = PufferConfig::default();
    config.workflow_backend.workspace_id = "11111111-1111-4111-8111-111111111111".to_string();

    let wrong_key_vault = SecretVault::open_with_key(
        SecretVault::default_path(&paths.user_config_dir),
        [8_u8; 32],
    );
    let stale = wrong_key_vault
        .put(SecretUpsert {
            id: None,
            label: WORKFLOW_RUNTIME_TOKEN_LABEL.to_string(),
            description: Some(WORKFLOW_RUNTIME_TOKEN_DESCRIPTION.to_string()),
            value: "stale-token".to_string(),
            username: None,
            origin: Some(config.workflow_backend.api_base_url.clone()),
            source: "local_workflow_runtime".to_string(),
        })
        .expect("store stale token");
    config.workflow_backend.api_token_secret_id = stale.id.clone();

    let mut responses = Vec::new();
    responses.extend(available());
    responses.push(ok("image"));
    responses.push(no_api_container());
    responses.push(ok("postgres redis"));
    responses.push(ok("migrated"));
    responses.push(ok("seeded"));
    responses.push(ok("api"));
    let runner = FakeCommandRunner::new(responses);
    let health = FakeHealthChecker::ready();

    let status = start_with(
        &runner,
        &health,
        &paths,
        &mut config,
        true,
        WaitPolicy::new(1, Duration::ZERO),
    )
    .expect("start");

    assert_eq!(status.state, LocalWorkflowRuntimeState::Ready);
    assert!(!config.workflow_backend.api_token_secret_id.is_empty());
    let regenerated = reveal_runtime_token(&paths, &config);
    assert_ne!(regenerated, "stale-token");
    assert_eq!(
        health.node_definition_calls(),
        vec![(
            config.workflow_backend.api_base_url.clone(),
            config.workflow_backend.workspace_id.clone(),
            regenerated
        )]
    );
}

#[test]
fn repair_archives_runtime_data_and_rebuilds_stack() {
    let _guard = lock_secret_store();
    let temp = tempfile::tempdir().unwrap();
    let paths = test_paths(&temp);
    let mut config = configured_local_runtime(&paths);
    let data_dir = local_runtime_stack_dir(&paths).join(LOCAL_WORKFLOW_RUNTIME_DATA_DIR);
    fs::create_dir_all(data_dir.join("postgres")).unwrap();
    fs::create_dir_all(data_dir.join("redis")).unwrap();
    fs::write(data_dir.join("postgres").join("PG_VERSION"), "14").unwrap();
    fs::write(data_dir.join("redis").join("appendonly.aof"), "redis").unwrap();

    let mut responses = available();
    responses.push(ok("image\n"));
    responses.push(no_api_container());
    responses.push(ok("down\n"));
    responses.extend(available());
    responses.push(ok("image\n"));
    responses.push(no_api_container());
    responses.push(ok("postgres\nredis\n"));
    responses.push(ok("migrated\n"));
    responses.push(ok("seeded\n"));
    responses.push(ok("api\n"));
    let runner = FakeCommandRunner::new(responses);
    let health = FakeHealthChecker::ready();

    let result = repair_with(
        &runner,
        &health,
        &paths,
        &mut config,
        WaitPolicy::new(1, Duration::ZERO),
    )
    .unwrap();

    assert_eq!(result.status.state, LocalWorkflowRuntimeState::Ready);
    assert_eq!(result.archived_data_dirs.len(), 2);
    assert!(data_dir.join("postgres").is_dir());
    assert!(data_dir.join("redis").is_dir());
    assert!(result.archived_data_dirs.iter().any(|path| {
        path.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("postgres.repair-")
    }));
    assert!(result.archived_data_dirs.iter().any(|path| {
        path.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("redis.repair-")
    }));
    assert!(result
        .archived_data_dirs
        .iter()
        .any(|path| path.join("PG_VERSION").exists()));

    let calls = runner.calls();
    assert!(calls.iter().any(|call| {
        call.program == "docker"
            && call
                .args
                .ends_with(&["down".to_string(), "--remove-orphans".to_string()])
    }));
    assert!(calls.iter().any(|call| {
        call.program == "docker"
            && call
                .args
                .ends_with(&["run".to_string(), "--rm".to_string(), "migrate".to_string()])
    }));
}

#[test]
fn transient_start_from_cloud_config_preserves_user_workflow_backend() {
    let _guard = lock_secret_store();
    let _secret_store_key = ScopedSecretStoreKey::set();
    let temp = tempfile::tempdir().expect("tempdir");
    let paths = test_paths(&temp);
    let mut config = PufferConfig::default();
    config.workflow_backend.mode = WorkflowBackendMode::AgentEnvCloud;
    config.workflow_backend.api_base_url = "https://api.agentenv.io".to_string();
    config.workflow_backend.frontend_url = "https://agentenv.io".to_string();
    config.workflow_backend.workspace_id = "workspace-cloud".to_string();
    let vault =
        SecretVault::open(SecretVault::default_path(&paths.user_config_dir)).expect("open vault");
    let cloud_secret = vault
        .put(SecretUpsert {
            id: None,
            label: "cloud".to_string(),
            description: None,
            value: "cloud-token".to_string(),
            username: None,
            origin: Some("https://api.agentenv.io".to_string()),
            source: "test".to_string(),
        })
        .expect("cloud token");
    config.workflow_backend.api_token_secret_id = cloud_secret.id.clone();
    save_user_config(&paths, &config).expect("save cloud config");

    let mut responses = Vec::new();
    responses.extend(available());
    responses.push(ok("image"));
    responses.push(no_api_container());
    responses.push(ok("postgres redis"));
    responses.push(ok("migrated"));
    responses.push(ok("seeded"));
    responses.push(ok("api"));
    let runner = FakeCommandRunner::new(responses);
    let health = FakeHealthChecker::ready();

    let status = start_transient_with(
        &runner,
        &health,
        &paths,
        &mut config,
        WaitPolicy::new(1, Duration::ZERO),
    )
    .expect("transient start");

    assert_eq!(status.state, LocalWorkflowRuntimeState::Ready);
    assert_eq!(config.workflow_backend.mode, WorkflowBackendMode::Local);
    assert!(config
        .workflow_backend
        .api_base_url
        .starts_with("http://127.0.0.1:"));
    assert_ne!(config.workflow_backend.api_token_secret_id, cloud_secret.id);
    assert!(paths
        .user_config_dir
        .join(LOCAL_WORKFLOW_RUNTIME_DIR)
        .join(LOCAL_WORKFLOW_RUNTIME_CONFIG_FILE)
        .exists());

    let saved = load_config(&paths).expect("reload saved config");
    assert_eq!(
        saved.workflow_backend.mode,
        WorkflowBackendMode::AgentEnvCloud
    );
    assert_eq!(
        saved.workflow_backend.api_base_url,
        "https://api.agentenv.io"
    );
    assert_eq!(saved.workflow_backend.workspace_id, "workspace-cloud");
    assert_eq!(saved.workflow_backend.api_token_secret_id, cloud_secret.id);
}

#[test]
fn migrate_failure_reports_incompatible_runtime() {
    let _guard = lock_secret_store();
    let _secret_store_key = ScopedSecretStoreKey::set();
    let temp = tempfile::tempdir().expect("tempdir");
    let paths = test_paths(&temp);
    let mut config = PufferConfig::default();
    let mut responses = Vec::new();
    responses.extend(available());
    responses.push(ok("image"));
    responses.push(no_api_container());
    responses.push(ok("postgres redis"));
    responses.push(fail("missing migration"));
    responses.push(fail("pull failed"));
    let runner = FakeCommandRunner::new(responses);

    let status = start_with(
        &runner,
        &FakeHealthChecker::not_ready(),
        &paths,
        &mut config,
        true,
        WaitPolicy::new(1, Duration::ZERO),
    )
    .expect("start");

    assert_eq!(status.state, LocalWorkflowRuntimeState::IncompatibleRuntime);
    assert!(status
        .message
        .as_deref()
        .unwrap_or_default()
        .contains("migrate failed"));
}

#[test]
fn seed_failure_reports_incompatible_runtime() {
    let _guard = lock_secret_store();
    let _secret_store_key = ScopedSecretStoreKey::set();
    let temp = tempfile::tempdir().expect("tempdir");
    let paths = test_paths(&temp);
    let mut config = PufferConfig::default();
    let mut responses = Vec::new();
    responses.extend(available());
    responses.push(ok("image"));
    responses.push(no_api_container());
    responses.push(ok("postgres redis"));
    responses.push(ok("migrated"));
    responses.push(fail("bad sql"));
    responses.push(fail("pull failed"));
    let runner = FakeCommandRunner::new(responses);

    let status = start_with(
        &runner,
        &FakeHealthChecker::not_ready(),
        &paths,
        &mut config,
        true,
        WaitPolicy::new(1, Duration::ZERO),
    )
    .expect("start");

    assert_eq!(status.state, LocalWorkflowRuntimeState::IncompatibleRuntime);
    assert!(status
        .message
        .as_deref()
        .unwrap_or_default()
        .contains("seed failed"));
}

#[test]
fn ready_without_node_definitions_reports_incompatible_runtime() {
    let _guard = lock_secret_store();
    let _secret_store_key = ScopedSecretStoreKey::set();
    let temp = tempfile::tempdir().expect("tempdir");
    let paths = test_paths(&temp);
    let mut config = PufferConfig::default();
    let mut responses = Vec::new();
    responses.extend(available());
    responses.push(ok("image"));
    responses.push(no_api_container());
    responses.push(ok("postgres redis"));
    responses.push(ok("migrated"));
    responses.push(ok("seeded"));
    responses.push(ok("api"));
    responses.push(fail("pull failed"));
    let runner = FakeCommandRunner::new(responses);

    let status = start_with(
        &runner,
        &FakeHealthChecker::without_node_definitions(),
        &paths,
        &mut config,
        true,
        WaitPolicy::new(1, Duration::ZERO),
    )
    .expect("start");

    assert_eq!(status.state, LocalWorkflowRuntimeState::IncompatibleRuntime);
    assert!(runner.calls().iter().any(|call| call.program == "docker"
        && call.args.as_slice() == ["pull", "agentenv/api-server:local"]));
}

#[test]
fn incompatible_runtime_refreshes_image_and_retries_with_recreated_api() {
    let _guard = lock_secret_store();
    let _secret_store_key = ScopedSecretStoreKey::set();
    let temp = tempfile::tempdir().expect("tempdir");
    let paths = test_paths(&temp);
    let mut config = PufferConfig::default();
    let mut responses = Vec::new();
    responses.extend(available());
    responses.push(ok("image"));
    responses.push(no_api_container());
    responses.push(ok("postgres redis"));
    responses.push(ok("migrated"));
    responses.push(ok("seeded"));
    responses.push(ok("api"));
    responses.push(ok("pulled newer image"));
    responses.push(ok("postgres redis"));
    responses.push(ok("migrated"));
    responses.push(ok("seeded"));
    responses.push(ok("api recreated"));
    let runner = FakeCommandRunner::new(responses);
    let health = FakeHealthChecker::node_definitions_ready_after_retry();

    let status = start_with(
        &runner,
        &health,
        &paths,
        &mut config,
        true,
        WaitPolicy::new(1, Duration::ZERO),
    )
    .expect("start");

    assert_eq!(status.state, LocalWorkflowRuntimeState::Ready);
    let calls = runner.calls();
    assert!(calls.iter().any(|call| call.program == "docker"
        && call.args.as_slice() == ["pull", "agentenv/api-server:local"]));
    assert!(calls
        .iter()
        .any(|call| is_compose_command(call, &["up", "-d", "--force-recreate", "api"])));
    assert_eq!(health.node_definition_calls().len(), 2);
}

#[test]
fn status_ready_checks_api_and_node_definitions() {
    let _guard = lock_secret_store();
    let _secret_store_key = ScopedSecretStoreKey::set();
    let temp = tempfile::tempdir().expect("tempdir");
    let paths = test_paths(&temp);
    let config = configured_local_runtime(&paths);
    let mut responses = Vec::new();
    responses.extend(available());
    responses.push(ok("image"));
    responses.push(ok("api\npostgres\nredis\n"));
    let runner = FakeCommandRunner::new(responses);
    let health = FakeHealthChecker::ready();

    let status = status_with(&runner, &health, &paths, &config).expect("status");

    assert_eq!(status.state, LocalWorkflowRuntimeState::Ready);
    assert_eq!(status.state.as_str(), "ready");
    assert_eq!(
        health.node_definition_calls(),
        vec![(
            config.workflow_backend.api_base_url.clone(),
            "11111111-1111-4111-8111-111111111111".to_string(),
            "token-test".to_string()
        )]
    );
}

#[test]
fn cloud_mode_does_not_call_docker() {
    let _guard = lock_secret_store();
    let _secret_store_key = ScopedSecretStoreKey::set();
    let temp = tempfile::tempdir().expect("tempdir");
    let paths = test_paths(&temp);
    let mut config = PufferConfig::default();
    config.workflow_backend.mode = WorkflowBackendMode::AgentEnvCloud;
    let runner = FakeCommandRunner::new(vec![]);

    let status =
        status_with(&runner, &FakeHealthChecker::not_ready(), &paths, &config).expect("status");

    assert_eq!(status.state, LocalWorkflowRuntimeState::Failed);
    assert!(runner.calls().is_empty());
}

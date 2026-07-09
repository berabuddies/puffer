use serde_json::{json, Value};
use std::io::{ErrorKind, Read};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tungstenite::{connect, stream::MaybeTlsStream, Message, WebSocket};
use url::Url;

const API_URL_ENV: &str = "PUFFER_WORKFLOW_API_URL";
const WORKSPACE_ID_ENV: &str = "PUFFER_WORKFLOW_WORKSPACE_ID";
const API_TOKEN_ENV: &str = "PUFFER_WORKFLOW_API_TOKEN";
const MODE_ENV: &str = "PUFFER_AUTOMATION_E2E_MODE";
const RUNTIME_PROJECT_ENV: &str = "PUFFER_WORKFLOW_RUNTIME_PROJECT";
const TEST_SECRET_STORE_KEY: &str = "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=";

// Connector-trigger + agent-loop + connector-action e2e knobs. The loop body
// runs a real daemon-owned `puffer_agent`, so this scenario is gated on an
// agent credential in addition to Docker.
const AGENT_API_KEY_ENV: &str = "PUFFER_AUTOMATION_E2E_AGENT_API_KEY";
const AGENT_PROVIDER_ENV: &str = "PUFFER_AUTOMATION_E2E_AGENT_PROVIDER";
const AGENT_MODEL_ENV: &str = "PUFFER_AUTOMATION_E2E_AGENT_MODEL";
const AGENT_BASE_URL_ENV: &str = "PUFFER_AUTOMATION_E2E_AGENT_BASE_URL";
const READONLY_CONNECTOR_SLUG: &str = "e2e-readonly";
const READONLY_CONNECTION_SLUG: &str = "e2e-readonly-account";
const READONLY_ACTION: &str = "read_status";

#[test]
#[ignore = "requires local Docker runtime or real AgentEnv Cloud credentials"]
fn automation_real_e2e_compile_deploy_execute_preview() {
    let mode = AutomationE2eMode::from_env();

    let tempdir = tempfile::tempdir().expect("tempdir");
    let workspace = tempdir.path().join("workspace");
    let puffer_home = tempdir.path().join("home");
    let puffer_config = puffer_home.join(".puffer");
    let runtime_project = format!(
        "puffer-workflow-runtime-e2e-{}-{}",
        std::process::id(),
        unix_timestamp_ms()
    );
    let _local_runtime_cleanup = LocalRuntimeCleanup::new(
        matches!(mode, AutomationE2eMode::Local),
        &puffer_config,
        &runtime_project,
    );
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::create_dir_all(&puffer_config).expect("puffer config");
    let discovery_cache = tempdir.path().join("discovery.json");
    std::fs::write(&discovery_cache, discovery_cache_json()).expect("discovery cache");

    let mut extra_env = vec![("PUFFER_SECRET_STORE_KEY", TEST_SECRET_STORE_KEY)];
    if matches!(mode, AutomationE2eMode::Local) {
        extra_env.push((RUNTIME_PROJECT_ENV, runtime_project.as_str()));
    }
    let mut daemon =
        DaemonProcess::start_with_env(&workspace, &puffer_home, &discovery_cache, &extra_env);
    let mut client = DaemonClient::connect(&daemon.handshake);

    configure_workflow_backend_for_mode(&mut client, &mode);

    let automation_id = format!("automation-real-e2e-{}", unix_timestamp_ms());
    let spec = automation_spec(&automation_id);
    let saved = client.rpc(
        "automation_save",
        json!({
            "id": automation_id,
            "status": "paused",
            "spec": spec,
        }),
    );
    let revision = saved["revision"].as_u64().expect("saved revision");

    let deployed = client.rpc_with_mode(
        &mode,
        "automation_compile_deploy",
        json!({
            "id": saved["id"],
            "expectedRevision": revision,
        }),
    );
    assert_eq!(deployed["runtime"]["status"], "deployed");
    assert_eq!(deployed["runtime"]["compiled_revision"], revision);

    let preview = client.rpc_with_mode(
        &mode,
        "automation_run_preview",
        json!({
            "id": saved["id"],
            "input": {
                "source": "automation-real-e2e",
                "timestamp_ms": unix_timestamp_ms()
            }
        }),
    );
    assert_eq!(preview["status"], "completed");
    assert_eq!(preview["runtime"]["status"], "deployed");
    assert_eq!(preview["runtime"]["compiled_revision"], revision);
    assert!(
        preview["runtime"]["agentenv_workflow_count"]
            .as_u64()
            .unwrap_or_default()
            >= 1
    );
    assert_public_preview_response(&preview);

    let fetched = client.rpc("automation_get", json!({ "id": saved["id"] }));
    assert_eq!(fetched["runtime"]["status"], "deployed");
    assert_eq!(fetched["runtime"]["compiled_revision"], revision);
    assert!(
        fetched["runtime"]["agentenv_workflow_count"]
            .as_u64()
            .unwrap_or_default()
            >= 1
    );
    assert_public_preview_response(&fetched);

    daemon.stop();
}

/// Full-chain e2e for a realistic user Automation: a connector *event* trigger
/// (not a webhook) drives a loop whose body runs a daemon-owned `puffer_agent`
/// (agent-in-the-loop) and then executes a read-only connector *action*. It runs
/// against the selected real runtime and a real provider credential, exercising
/// trigger compilation, the Puffer-side loop runner, the Puffer agent boundary,
/// and the daemon connector-action executor together.
///
/// Loop Automations only support `puffer_connection` triggers (Puffer owns the
/// loop; AgentEnv ingress does not bridge back into the runner), so the trigger
/// here is a connector connection rather than a webhook. It is driven through
/// `automation_compile_deploy` + `automation_run_preview`: deploy compiles and
/// deploys the workflow artifacts (warming the runtime sandbox), then the preview
/// executes the whole group in-memory. No authorized connection is required
/// because the read-only connector action runs through the daemon executor.
///
/// Prerequisites to pass:
/// - `PUFFER_AUTOMATION_E2E_AGENT_API_KEY` (provider key for the agent).
/// - `PUFFER_AUTOMATION_E2E_MODE=cloud` plus AgentEnv Cloud credentials, or a
///   local runtime whose node-execution sandbox is reachable. The default
///   `agentenv/api-server:local` compose (api + postgres + redis) does NOT run
///   the code-execution sandbox on `127.0.0.1:50052`, so executor nodes such as
///   `transform_js` fail with `ECONNREFUSED 50052`.
///   Until that service is provisioned, this test surfaces that gap rather than
///   passing.
#[test]
#[ignore = "requires local Docker runtime or real AgentEnv Cloud credentials plus an agent provider credential"]
fn automation_real_e2e_connection_trigger_agent_loop_connector_action() {
    let mode = AutomationE2eMode::from_env();
    let agent = AutomationAgentEnv::from_env();

    let tempdir = tempfile::tempdir().expect("tempdir");
    let workspace = tempdir.path().join("workspace");
    let puffer_home = tempdir.path().join("home");
    let puffer_config = puffer_home.join(".puffer");
    let runtime_project = format!(
        "puffer-workflow-runtime-e2e-{}-{}",
        std::process::id(),
        unix_timestamp_ms()
    );
    let _local_runtime_cleanup = LocalRuntimeCleanup::new(
        matches!(mode, AutomationE2eMode::Local),
        &puffer_config,
        &runtime_project,
    );
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::create_dir_all(&puffer_config).expect("puffer config");
    let discovery_cache = tempdir.path().join("discovery.json");
    std::fs::write(&discovery_cache, discovery_cache_json()).expect("discovery cache");

    // A read-only connector whose `act` command shells out to a trivial script
    // that always returns a side-effect-free success. This lets the connector
    // action step run through the real daemon executor without any external
    // credentials or outward effects.
    write_readonly_connector_catalog(&puffer_config);
    // A provider API key so the loop's puffer_agent can run through the daemon
    // provider loop.
    write_agent_api_key(&puffer_config, &agent);

    let mut extra_env = vec![("PUFFER_SECRET_STORE_KEY", TEST_SECRET_STORE_KEY)];
    if matches!(mode, AutomationE2eMode::Local) {
        extra_env.push((RUNTIME_PROJECT_ENV, runtime_project.as_str()));
    }
    let mut daemon =
        DaemonProcess::start_with_env(&workspace, &puffer_home, &discovery_cache, &extra_env);
    let mut client = DaemonClient::connect(&daemon.handshake);

    configure_workflow_backend_for_mode(&mut client, &mode);

    // Select the provider/model used by the daemon-owned puffer_agent. This
    // persists to the user config that preview execution reads.
    let mut config_patch = json!({
        "defaultProvider": agent.provider,
        "defaultModel": format!("{}/{}", agent.provider, agent.model),
    });
    if let Some(base_url) = &agent.base_url {
        config_patch["openaiBaseUrl"] = json!(base_url);
    }
    client.rpc("update_config", config_patch);

    let automation_id = format!("automation-agent-loop-e2e-{}", unix_timestamp_ms());
    let spec = connection_trigger_agent_loop_spec(&automation_id, mode.run_location());
    let saved = client.rpc(
        "automation_save",
        json!({
            "id": automation_id,
            "status": "paused",
            "spec": spec,
        }),
    );
    let revision = saved["revision"].as_u64().expect("saved revision");

    // Compile + deploy compiles the AgentEnv-owned automation segments and
    // deploys the workflow artifacts to the runtime. Deploying warms the
    // runtime execution sandbox so the subsequent in-memory preview can run its
    // transform nodes; puffer_agent runs later inside the daemon.
    let deployed = client.rpc_with_mode(
        &mode,
        "automation_compile_deploy",
        json!({
            "id": saved["id"],
            "expectedRevision": revision,
        }),
    );
    assert_eq!(deployed["runtime"]["status"], "deployed");
    assert_eq!(deployed["runtime"]["compiled_revision"], revision);
    assert!(
        deployed["runtime"]["agentenv_workflow_count"]
            .as_u64()
            .unwrap_or_default()
            >= 2,
        "expected root plus loop-body workflows: {deployed:#}"
    );

    let preview = client.rpc_slow_with_mode(
        &mode,
        "automation_run_preview",
        json!({
            "id": saved["id"],
            "input": {
                "source": "automation-agent-loop-e2e",
                "text": "hello from a connector event",
                "timestamp_ms": unix_timestamp_ms()
            }
        }),
    );
    assert_eq!(preview["status"], "completed");
    let preview_text = preview.to_string();
    assert!(
        preview_text.contains("connector_action_result"),
        "preview result should include the connector action output: {preview:#}"
    );
    assert!(
        preview_text.contains("read-only status ok"),
        "preview result should include the read-only connector summary: {preview:#}"
    );
    assert_public_preview_response(&preview);

    let history = client.rpc("automation_run_history", json!({ "id": saved["id"] }));
    let runs = history["runs"].as_array().expect("run history runs array");
    assert!(
        runs.iter().any(|run| run["status"] == "completed"),
        "expected a completed preview run in history: {history:#}"
    );

    daemon.stop();
}

enum AutomationE2eMode {
    Local,
    Cloud(AutomationE2eCloudEnv),
}

impl AutomationE2eMode {
    fn from_env() -> Self {
        match env_trimmed(MODE_ENV).as_deref() {
            Some("cloud") => Self::Cloud(AutomationE2eCloudEnv::from_env()),
            Some("local") | None => Self::Local,
            Some(other) => panic!("{MODE_ENV} must be `local` or `cloud`, got `{other}`"),
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Cloud(_) => "cloud",
        }
    }

    fn run_location(&self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Cloud(_) => "agent_env_cloud",
        }
    }
}

fn configure_workflow_backend_for_mode(client: &mut DaemonClient, mode: &AutomationE2eMode) {
    match mode {
        AutomationE2eMode::Local => {
            let config = client.rpc("workflow_backend_get_config", json!({}));
            assert_eq!(config["mode"], "local");
        }
        AutomationE2eMode::Cloud(env) => {
            let saved_backend = client.rpc(
                "workflow_backend_save_config",
                json!({
                    "mode": "agent_env_cloud",
                    "apiUrl": env.api_url,
                    "uiUrl": "https://agentenv.io",
                    "workspaceId": env.workspace_id,
                    "apiToken": env.api_token,
                    "keepToken": false,
                }),
            );
            assert_eq!(saved_backend["hasToken"], true);
            let saved_backend_text = saved_backend.to_string();
            assert!(!saved_backend_text.contains("apiToken"));
            assert!(!saved_backend_text.contains("api_token"));
        }
    }
}

struct AutomationE2eCloudEnv {
    api_url: String,
    workspace_id: String,
    api_token: String,
}

impl AutomationE2eCloudEnv {
    fn from_env() -> Self {
        let missing = [API_URL_ENV, WORKSPACE_ID_ENV, API_TOKEN_ENV]
            .into_iter()
            .filter(|name| env_trimmed(name).is_none())
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            panic!(
                "cloud Automation e2e requires missing environment variable(s): {}",
                missing.join(", ")
            );
        }
        Self {
            api_url: env_trimmed(API_URL_ENV).expect("api url"),
            workspace_id: env_trimmed(WORKSPACE_ID_ENV).expect("workspace id"),
            api_token: env_trimmed(API_TOKEN_ENV).expect("api token"),
        }
    }
}

fn env_trimmed(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn automation_spec(automation_id: &str) -> Value {
    json!({
        "spec_version": 1,
        "name": format!("Automation real e2e {automation_id}"),
        "source": { "type": "blank" },
        "instructions": "Run a minimal real AgentEnv workflow from Puffer Automation preview.",
        "triggers": [
            {
                "type": "agent_env_node",
                "id": "smoke-webhook",
                "node": {
                    "node_type": "webhook",
                    "name": "Smoke webhook",
                    "trusted": false,
                    "config": {
                        "path": automation_id,
                        "methods": ["POST"],
                        "authentication": "none"
                    }
                }
            }
        ],
        "flow": {
            "steps": [
                {
                    "type": "agent_env_node",
                    "id": "transform",
                    "node": {
                        "node_type": "transform_js",
                        "name": "Transform",
                        "trusted": true,
                        "config": transform_config()
                    }
                }
            ]
        },
        "review": {
            "human_approval_required": true
        }
    })
}

fn transform_config() -> Value {
    json!({ "code": "return { ok: true, input };" })
}

/// Provider credential used by the loop's `puffer_agent`.
struct AutomationAgentEnv {
    provider: String,
    model: String,
    api_key: String,
    base_url: Option<String>,
}

impl AutomationAgentEnv {
    fn from_env() -> Self {
        let api_key = env_trimmed(AGENT_API_KEY_ENV).unwrap_or_else(|| {
            panic!(
                "connector-trigger agent-loop e2e requires {AGENT_API_KEY_ENV} (a provider API key for the loop agent)"
            )
        });
        Self {
            provider: env_trimmed(AGENT_PROVIDER_ENV).unwrap_or_else(|| "openai".to_string()),
            model: env_trimmed(AGENT_MODEL_ENV).unwrap_or_else(|| "gpt-4o-mini".to_string()),
            api_key,
            base_url: env_trimmed(AGENT_BASE_URL_ENV),
        }
    }
}

/// Writes a user connector catalog with a single read-only connector whose
/// `act` command is a trivial shell script. The daemon's connector store reads
/// this file on startup and resolves the connector for the automation action.
fn write_readonly_connector_catalog(puffer_config: &Path) {
    // Extra argv (`act <connection> <action>`) is appended by the runtime and
    // ignored here; the script drains stdin so the writer never sees a broken
    // pipe, then emits a fixed read-only action response on stdout.
    let script =
        "cat >/dev/null 2>&1; printf '%s' '{\"success\":true,\"summary\":\"read-only status ok\",\"output\":{\"status\":\"green\"}}'";

    let mut permission = serde_json::Map::new();
    permission.insert("category".into(), json!("read"));
    permission.insert("summary".into(), json!("Read status"));
    permission.insert("external_side_effect".into(), json!(false));

    let mut action = serde_json::Map::new();
    action.insert("slug".into(), json!(READONLY_ACTION));
    action.insert(
        "description".into(),
        json!("Return a read-only status snapshot"),
    );
    action.insert("permission".into(), Value::Object(permission));

    let mut actions = serde_json::Map::new();
    actions.insert(READONLY_ACTION.to_string(), Value::Object(action));

    let catalog = json!({
        "version": 1,
        "connectors": [
            {
                "slug": READONLY_CONNECTOR_SLUG,
                "description": "E2E read-only connector used to verify Automation connector actions",
                "skill": "none",
                "binary": "/bin/sh",
                "command": ["/bin/sh", "-c", script],
                "requires_auth": false,
                "can_subscribe": false,
                "can_proxy_agent": false,
                "actions": Value::Object(actions),
            }
        ]
    });
    std::fs::write(
        puffer_config.join("connectors.json"),
        serde_json::to_vec_pretty(&catalog).expect("serialize connector catalog"),
    )
    .expect("write connectors.json");
}

/// Writes an auth store containing an API key for the agent provider. API keys
/// are persisted in plaintext (only OAuth secrets are encrypted), so the file
/// can be written directly without the secret store key.
fn write_agent_api_key(puffer_config: &Path, agent: &AutomationAgentEnv) {
    let mut providers = serde_json::Map::new();
    providers.insert(
        agent.provider.clone(),
        json!({ "kind": "api_key", "key": agent.api_key }),
    );
    let auth = json!({
        "format_version": 1,
        "providers": Value::Object(providers),
    });
    std::fs::write(
        puffer_config.join("auth.json"),
        serde_json::to_vec_pretty(&auth).expect("serialize auth store"),
    )
    .expect("write auth.json");
}

/// A connector-event-triggered Automation whose loop body runs a Puffer agent
/// per item and then executes a read-only connector action as the terminal
/// loop-body suffix.
fn connection_trigger_agent_loop_spec(automation_id: &str, run_location: &str) -> Value {
    let mut connector_config = serde_json::Map::new();
    connector_config.insert("connector_slug".into(), json!(READONLY_CONNECTOR_SLUG));
    connector_config.insert("connection_slug".into(), json!(READONLY_CONNECTION_SLUG));
    connector_config.insert("action".into(), json!(READONLY_ACTION));
    connector_config.insert("input".into(), json!({ "query": "status" }));
    // Read-only: none of the approval-gating flags are set, so the preview is
    // allowed to actually execute the connector action.
    connector_config.insert("external_side_effect".into(), json!(false));
    connector_config.insert("draft_only".into(), json!(false));
    connector_config.insert("human_approval_required".into(), json!(false));

    json!({
        "spec_version": 1,
        "name": format!("Automation agent-loop e2e {automation_id}"),
        "source": { "type": "blank" },
        "instructions": "When a connector event arrives, review each item and record a read-only status check.",
        "run_location": run_location,
        "triggers": [
            {
                "type": "puffer_connection",
                "id": "incoming",
                "connection_slug": READONLY_CONNECTION_SLUG,
                "connector_slug": READONLY_CONNECTOR_SLUG
            }
        ],
        "flow": {
            "steps": [
                {
                    "type": "agent_env_node",
                    "id": "seed",
                    "node": {
                        "node_type": "transform_js",
                        "name": "Seed",
                        "trusted": true,
                        "config": transform_config()
                    }
                },
                {
                    "type": "loop",
                    "id": "per-item",
                    "loop": {
                        "mode": "for_each",
                        "input": { "type": "static", "value": ["only"] },
                        "item_alias": "item"
                    },
                    "body": {
                        "steps": [
                            {
                                "type": "agent_env_node",
                                "id": "agent",
                                "node": {
                                    "node_type": "puffer_agent",
                                    "name": "Loop agent",
                                    "config": {
                                        "instructions": "Summarize the current item in one short sentence."
                                    }
                                }
                            },
                            {
                                "type": "agent_env_node",
                                "id": "record-status",
                                "node": {
                                    "node_type": "puffer_connector_action",
                                    "name": "Record status",
                                    "trusted": true,
                                    "config": Value::Object(connector_config)
                                }
                            }
                        ]
                    }
                }
            ]
        },
        "review": { "human_approval_required": true }
    })
}

fn assert_public_preview_response(value: &Value) {
    let text = value.to_string();
    assert!(!text.contains("workflowId"));
    assert!(!text.contains("workflow_id"));
    assert!(!text.contains("workflowSlug"));
    assert!(!text.contains("workflow_slug"));
    assert!(!text.contains("bindingSlug"));
    assert!(!text.contains("binding_slug"));
}

fn friendly_rpc_error(mode: &AutomationE2eMode, error: &Value) -> String {
    let text = error.to_string();
    if matches!(mode, AutomationE2eMode::Local) {
        if text.contains("docker_missing") {
            return format!("Docker is not running. Raw daemon error: {text}");
        }
        if text.contains("image_missing") {
            return format!(
                "Local AgentEnv runtime image agentenv/api-server:local is missing. Raw daemon error: {text}"
            );
        }
    }
    text
}

struct DaemonProcess {
    child: Child,
    handshake: Value,
    stderr: Arc<Mutex<String>>,
}

struct LocalRuntimeCleanup {
    enabled: bool,
    compose_file: std::path::PathBuf,
    project_name: String,
}

impl LocalRuntimeCleanup {
    fn new(enabled: bool, puffer_config: &Path, project_name: &str) -> Self {
        Self {
            enabled,
            compose_file: puffer_config
                .join("workflow-runtime")
                .join("docker-compose.yml"),
            project_name: project_name.to_string(),
        }
    }
}

impl Drop for LocalRuntimeCleanup {
    fn drop(&mut self) {
        if !self.enabled || !self.compose_file.exists() {
            return;
        }
        let _ = Command::new("docker")
            .args([
                "compose",
                "-f",
                self.compose_file.to_string_lossy().as_ref(),
                "-p",
                &self.project_name,
                "down",
                "-v",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

impl DaemonProcess {
    fn start_with_env(
        workspace: &Path,
        puffer_home: &Path,
        discovery_cache: &Path,
        extra_env: &[(&str, &str)],
    ) -> Self {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("cli crate parent")
            .parent()
            .expect("repo root");
        let mut command = Command::new(env!("CARGO_BIN_EXE_puffer"));
        command
            .args([
                "daemon",
                "--bind",
                "127.0.0.1:0",
                "--token",
                "automation-smoke-token",
                "--print-handshake",
                "--no-browser",
                "--disable-auto-title",
            ])
            .current_dir(workspace)
            .env("PUFFER_HOME", puffer_home)
            .env("PUFFER_BUILTIN_RESOURCES_DIR", repo_root.join("resources"))
            .env("PUFFER_DISCOVERY_CACHE_PATH", discovery_cache)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in extra_env {
            command.env(key, value);
        }
        let mut child = command.spawn().expect("spawn daemon");

        let stderr = Arc::new(Mutex::new(String::new()));
        let stderr_thread = Arc::clone(&stderr);
        let mut err = child.stderr.take().expect("daemon stderr");
        thread::spawn(move || {
            let mut buf = String::new();
            let _ = err.read_to_string(&mut buf);
            *stderr_thread.lock().unwrap() = buf;
        });

        let mut stdout = child.stdout.take().expect("daemon stdout");
        let handshake = read_handshake_line(&mut stdout, &mut child, &stderr);
        Self {
            child,
            handshake,
            stderr,
        }
    }

    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for DaemonProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let stderr = self.stderr.lock().unwrap();
        if !stderr.is_empty() {
            eprintln!("daemon stderr:\n{stderr}");
        }
    }
}

struct DaemonClient {
    socket: WebSocket<MaybeTlsStream<TcpStream>>,
    next_id: u64,
    backlog: Vec<Value>,
}

impl DaemonClient {
    fn connect(handshake: &Value) -> Self {
        let mut url = Url::parse(handshake["url"].as_str().expect("daemon url")).expect("url");
        url.query_pairs_mut()
            .append_pair("token", handshake["token"].as_str().expect("token"));
        let (socket, _) = connect(url.as_str()).expect("connect daemon websocket");
        set_daemon_socket_read_timeout(&socket, Some(Duration::from_millis(100)));
        Self {
            socket,
            next_id: 1,
            backlog: Vec::new(),
        }
    }

    fn rpc(&mut self, method: &str, params: Value) -> Value {
        self.rpc_with_context(method, params, None, Duration::from_secs(90))
    }

    fn rpc_with_mode(&mut self, mode: &AutomationE2eMode, method: &str, params: Value) -> Value {
        self.rpc_with_context(method, params, Some(mode), Duration::from_secs(90))
    }

    /// RPC variant with a longer deadline for calls that run real Puffer agents
    /// through the provider loop on a cold runtime.
    fn rpc_slow_with_mode(
        &mut self,
        mode: &AutomationE2eMode,
        method: &str,
        params: Value,
    ) -> Value {
        self.rpc_with_context(method, params, Some(mode), Duration::from_secs(300))
    }

    fn rpc_with_context(
        &mut self,
        method: &str,
        params: Value,
        mode: Option<&AutomationE2eMode>,
        timeout: Duration,
    ) -> Value {
        let message = self.rpc_response(method, params, timeout);
        if message["error"].is_null() {
            message["result"].clone()
        } else {
            if let Some(mode) = mode {
                panic!(
                    "{method} failed in {} mode: {}",
                    mode.name(),
                    friendly_rpc_error(mode, &message["error"])
                );
            }
            panic!("{method} failed: {}", message["error"]);
        }
    }

    fn rpc_response(&mut self, method: &str, params: Value, timeout: Duration) -> Value {
        let id = self.next_id.to_string();
        self.next_id += 1;
        self.socket
            .send(Message::Text(
                json!({ "id": id, "method": method, "params": params })
                    .to_string()
                    .into(),
            ))
            .expect("send daemon request");
        let deadline = Instant::now() + timeout;
        loop {
            assert!(Instant::now() < deadline, "{method} timed out");
            let message = self.read_message_until(deadline);
            if message["id"].as_str() == Some(id.as_str()) {
                return message;
            }
            self.backlog.push(message);
        }
    }

    fn read_message_until(&mut self, deadline: Instant) -> Value {
        loop {
            assert!(Instant::now() < deadline, "daemon message timed out");
            match self.socket.read() {
                Ok(Message::Text(text)) => {
                    return serde_json::from_str(&text).expect("daemon message json");
                }
                Ok(_) => {}
                Err(tungstenite::Error::Io(error))
                    if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
                Err(error) => panic!("read daemon message: {error}"),
            }
        }
    }
}

fn read_handshake_line(
    stdout: &mut impl Read,
    child: &mut Child,
    stderr: &Arc<Mutex<String>>,
) -> Value {
    let mut line = String::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut buf = [0_u8; 1];
    while Instant::now() < deadline {
        match stdout.read(&mut buf) {
            Ok(0) => {
                if let Some(status) = child.try_wait().expect("daemon status") {
                    panic!(
                        "daemon exited before handshake: {status}\n{}",
                        stderr.lock().unwrap()
                    );
                }
                thread::sleep(Duration::from_millis(10));
            }
            Ok(_) if buf[0] == b'\n' => break,
            Ok(_) => line.push(buf[0] as char),
            Err(error) => panic!("read daemon handshake: {error}"),
        }
    }
    assert!(!line.is_empty(), "daemon handshake timed out");
    serde_json::from_str(&line).expect("handshake json")
}

fn set_daemon_socket_read_timeout(
    socket: &WebSocket<MaybeTlsStream<TcpStream>>,
    timeout: Option<Duration>,
) {
    let tcp = match socket.get_ref() {
        MaybeTlsStream::Plain(stream) => stream,
        MaybeTlsStream::Rustls(stream) => stream.get_ref(),
        _ => return,
    };
    let _ = tcp.set_read_timeout(timeout);
}

fn discovery_cache_json() -> String {
    let now = 1_700_000_000_000_u64;
    json!({
        "entries": {
            "llama-cpp": { "models": [], "cached_at_ms": now },
            "lmstudio": { "models": [], "cached_at_ms": now },
            "ollama": { "models": [], "cached_at_ms": now },
            "vllm": { "models": [], "cached_at_ms": now }
        }
    })
    .to_string()
}

fn unix_timestamp_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time after epoch")
        .as_millis()
}

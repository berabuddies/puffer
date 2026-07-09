use anyhow::{Context, Result};
use puffer_config::{ConfigPaths, PufferConfig, WorkflowBackendConfig, WorkflowBackendMode};
use puffer_core::{blocking_client_for_url, HttpPurpose};
use puffer_secrets::SecretVault;
use puffer_workflow::{
    WorkflowRuntimeClient, WorkflowRuntimeClientConfig, WorkflowRuntimeConnectionStep,
    WorkflowRuntimeConnectionStepState, WorkflowRuntimeConnectionTest, WorkflowRuntimeError,
    WorkflowRuntimeErrorKind,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;
use url::Url;

use crate::automation_runtime_errors::{
    public_automation_runtime_detail_message, public_automation_runtime_error,
    public_automation_runtime_error_message, AutomationRuntimeErrorContext,
};
use crate::daemon::DaemonState;
use crate::daemon_workflow_backend_settings::save_workflow_backend_settings;
use crate::desktop_api::workflow_backend_settings_dto;
use crate::desktop_api_types::SaveWorkflowBackendSettingsParams;

#[allow(dead_code)]
#[path = "workflow_local_runtime.rs"]
mod workflow_local_runtime;

const WORKFLOW_RUNTIME_TIMEOUT: Duration = Duration::from_secs(120);

/// Returns the redacted workflow backend config for desktop callers.
pub(crate) fn handle_workflow_backend_get_config(state: &DaemonState) -> Result<Value> {
    let config = state.config_snapshot();
    workflow_backend_config_value(state.config_paths(), &config)
}

/// Saves workflow backend config and returns the redacted post-save snapshot.
pub(crate) fn handle_workflow_backend_save_config(
    state: &DaemonState,
    params: &Value,
) -> Result<Value> {
    let input: SaveWorkflowBackendSettingsParams =
        serde_json::from_value(params.clone()).context("invalid workflow backend config")?;
    let mut config = state.config_snapshot();
    let response = save_workflow_backend_config_value(state.config_paths(), &mut config, input)?;
    state.replace_config(config);
    Ok(response)
}

/// Runs the workflow backend connection test using saved config.
pub(crate) fn handle_workflow_backend_test_connection(state: &DaemonState) -> Result<Value> {
    let config = state.config_snapshot();
    workflow_backend_test_connection_value(state.config_paths(), &config)
}

/// Rebuilds the Puffer-managed local runtime data after explicit confirmation.
pub(crate) fn handle_workflow_backend_repair_local_runtime(
    state: &DaemonState,
    params: &Value,
) -> Result<Value> {
    let input: WorkflowBackendRepairLocalRuntimeParams =
        serde_json::from_value(params.clone()).context("parse workflow backend repair params")?;
    if !input.confirm {
        anyhow::bail!("Local automation runtime repair requires confirmation.");
    }
    let mut config = state.config_snapshot();
    workflow_backend_repair_local_runtime_value(state.config_paths(), &mut config)
}

/// Opens the configured workflow runtime console in the system browser.
pub(crate) fn handle_workflow_open_ui(state: &DaemonState) -> Result<Value> {
    let config = state.config_snapshot();
    workflow_open_ui_value(&config, true)
}

#[derive(Debug, Clone)]
struct ResolvedWorkflowRuntimeConfig {
    api_base_url: String,
    api_token: String,
    workspace_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkflowBackendConnectionTestDto {
    success: bool,
    ready: WorkflowBackendConnectionCheckDto,
    runtime: WorkflowBackendConnectionCheckDto,
    auth: WorkflowBackendConnectionCheckDto,
    workspace: WorkflowBackendConnectionCheckDto,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkflowBackendConnectionCheckDto {
    state: WorkflowRuntimeConnectionStepState,
    message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<WorkflowRuntimeErrorDto>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkflowRuntimeErrorDto {
    kind: WorkflowRuntimeErrorKind,
    message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    status_code: Option<u16>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkflowBackendRepairLocalRuntimeParams {
    #[serde(default)]
    confirm: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkflowBackendRepairLocalRuntimeDto {
    success: bool,
    state: workflow_local_runtime::LocalWorkflowRuntimeState,
    message: Option<String>,
    archived_data_dirs: Vec<String>,
    data_dir: Option<String>,
}

impl From<WorkflowRuntimeConnectionTest> for WorkflowBackendConnectionTestDto {
    fn from(value: WorkflowRuntimeConnectionTest) -> Self {
        let ready = ready_check(&value.ready);
        let runtime = runtime_check(&value.api_surface);
        let auth = auth_check(&value.api_surface);
        let workspace = workspace_check(&value.workspace_access);
        Self {
            success: ready.state == WorkflowRuntimeConnectionStepState::Passed
                && runtime.state == WorkflowRuntimeConnectionStepState::Passed
                && auth.state == WorkflowRuntimeConnectionStepState::Passed
                && workspace.state == WorkflowRuntimeConnectionStepState::Passed,
            ready,
            runtime,
            auth,
            workspace,
        }
    }
}

impl From<WorkflowRuntimeError> for WorkflowRuntimeErrorDto {
    fn from(value: WorkflowRuntimeError) -> Self {
        Self {
            kind: value.kind,
            message: public_workflow_runtime_error(&value),
            status_code: value.status_code,
        }
    }
}

fn ready_check(step: &WorkflowRuntimeConnectionStep) -> WorkflowBackendConnectionCheckDto {
    match step.state {
        WorkflowRuntimeConnectionStepState::Passed => WorkflowBackendConnectionCheckDto {
            state: WorkflowRuntimeConnectionStepState::Passed,
            message: "Automation runtime readiness endpoint is healthy.".to_string(),
            error: None,
        },
        WorkflowRuntimeConnectionStepState::Failed => {
            failed_check("Automation runtime readiness check failed.", step)
        }
        WorkflowRuntimeConnectionStepState::Skipped => skipped_check(
            step.message
                .as_deref()
                .unwrap_or("Skipped because automation runtime was not checked."),
        ),
    }
}

fn runtime_check(step: &WorkflowRuntimeConnectionStep) -> WorkflowBackendConnectionCheckDto {
    match step.state {
        WorkflowRuntimeConnectionStepState::Passed => WorkflowBackendConnectionCheckDto {
            state: WorkflowRuntimeConnectionStepState::Passed,
            message: "Automation runtime schema and token are accepted.".to_string(),
            error: None,
        },
        WorkflowRuntimeConnectionStepState::Failed => failed_check(
            "Automation runtime schema or token validation failed.",
            step,
        ),
        WorkflowRuntimeConnectionStepState::Skipped => skipped_check(
            step.message
                .as_deref()
                .unwrap_or("Skipped because automation runtime was not checked."),
        ),
    }
}

fn auth_check(step: &WorkflowRuntimeConnectionStep) -> WorkflowBackendConnectionCheckDto {
    match step.state {
        WorkflowRuntimeConnectionStepState::Passed => WorkflowBackendConnectionCheckDto {
            state: WorkflowRuntimeConnectionStepState::Passed,
            message: "Automation runtime token is accepted.".to_string(),
            error: None,
        },
        WorkflowRuntimeConnectionStepState::Failed => {
            let auth_failure = step
                .error
                .as_ref()
                .is_some_and(|error| auth_error_kind(error.kind));
            if auth_failure {
                failed_check("Automation runtime authentication failed.", step)
            } else {
                skipped_check("Skipped because automation runtime was not reachable.")
            }
        }
        WorkflowRuntimeConnectionStepState::Skipped => skipped_check(
            step.message
                .as_deref()
                .unwrap_or("Skipped because automation runtime was not checked."),
        ),
    }
}

fn workspace_check(step: &WorkflowRuntimeConnectionStep) -> WorkflowBackendConnectionCheckDto {
    match step.state {
        WorkflowRuntimeConnectionStepState::Passed => WorkflowBackendConnectionCheckDto {
            state: WorkflowRuntimeConnectionStepState::Passed,
            message: "Automation workspace is accessible.".to_string(),
            error: None,
        },
        WorkflowRuntimeConnectionStepState::Failed => {
            failed_check("Automation workspace access failed.", step)
        }
        WorkflowRuntimeConnectionStepState::Skipped => skipped_check(
            step.message
                .as_deref()
                .unwrap_or("Skipped because authentication did not pass."),
        ),
    }
}

fn failed_check(
    fallback: &str,
    step: &WorkflowRuntimeConnectionStep,
) -> WorkflowBackendConnectionCheckDto {
    WorkflowBackendConnectionCheckDto {
        state: WorkflowRuntimeConnectionStepState::Failed,
        message: step
            .error
            .as_ref()
            .map(public_workflow_runtime_error)
            .unwrap_or_else(|| fallback.to_string()),
        error: step.error.clone().map(Into::into),
    }
}

fn skipped_check(message: &str) -> WorkflowBackendConnectionCheckDto {
    WorkflowBackendConnectionCheckDto {
        state: WorkflowRuntimeConnectionStepState::Skipped,
        message: message.to_string(),
        error: None,
    }
}

fn auth_error_kind(kind: WorkflowRuntimeErrorKind) -> bool {
    matches!(
        kind,
        WorkflowRuntimeErrorKind::InvalidToken | WorkflowRuntimeErrorKind::PermissionDenied
    )
}

pub(crate) fn public_workflow_runtime_error_message(error: &anyhow::Error) -> String {
    public_automation_runtime_error_message(error, AutomationRuntimeErrorContext::Request)
}

pub(crate) fn public_workflow_runtime_error(error: &WorkflowRuntimeError) -> String {
    public_automation_runtime_error(error, AutomationRuntimeErrorContext::Request)
}

pub(crate) fn public_workflow_runtime_detail_message(detail: &str) -> String {
    public_automation_runtime_detail_message(detail, AutomationRuntimeErrorContext::Request)
}

fn workflow_backend_config_value(paths: &ConfigPaths, config: &PufferConfig) -> Result<Value> {
    Ok(serde_json::to_value(workflow_backend_settings_dto(
        paths, config,
    )?)?)
}

fn save_workflow_backend_config_value(
    paths: &ConfigPaths,
    config: &mut PufferConfig,
    input: SaveWorkflowBackendSettingsParams,
) -> Result<Value> {
    save_workflow_backend_settings(paths, config, input)?;
    workflow_backend_config_value(paths, config)
}

fn workflow_backend_test_connection_value(
    paths: &ConfigPaths,
    config: &PufferConfig,
) -> Result<Value> {
    let client = workflow_runtime_client(paths, config).map_err(|error| {
        let detail = format!("{error:#}");
        tracing::warn!(error = %detail, "automation runtime connection test setup failed");
        anyhow::anyhow!(public_workflow_runtime_error_message(&error))
    })?;
    let report = client.test_connection();
    Ok(serde_json::to_value(
        WorkflowBackendConnectionTestDto::from(report),
    )?)
}

fn workflow_backend_repair_local_runtime_value(
    paths: &ConfigPaths,
    config: &mut PufferConfig,
) -> Result<Value> {
    if config.workflow_backend.mode != WorkflowBackendMode::Local {
        anyhow::bail!(
            "Local automation runtime repair is only available when Automation Runtime is set to Run locally."
        );
    }
    let result = workflow_local_runtime::repair(paths, config).map_err(|error| {
        let detail = format!("{error:#}");
        tracing::warn!(error = %detail, "local automation runtime repair failed");
        anyhow::anyhow!(public_workflow_runtime_error_message(&error))
    })?;
    Ok(serde_json::to_value(local_runtime_repair_dto(result))?)
}

fn local_runtime_repair_dto(
    result: workflow_local_runtime::LocalWorkflowRuntimeRepairResult,
) -> WorkflowBackendRepairLocalRuntimeDto {
    let status = result.status;
    let success = status.state == workflow_local_runtime::LocalWorkflowRuntimeState::Ready;
    WorkflowBackendRepairLocalRuntimeDto {
        success,
        state: status.state,
        message: status
            .message
            .as_deref()
            .map(|message| public_local_runtime_status_message(status.state, message)),
        archived_data_dirs: result
            .archived_data_dirs
            .iter()
            .map(|path| path.display().to_string())
            .collect(),
        data_dir: status
            .data_dir
            .as_ref()
            .map(|path| path.display().to_string()),
    }
}

fn public_local_runtime_status_message(
    state: workflow_local_runtime::LocalWorkflowRuntimeState,
    message: &str,
) -> String {
    if state == workflow_local_runtime::LocalWorkflowRuntimeState::Ready {
        return message.to_string();
    }
    public_workflow_runtime_detail_message(message)
}

fn workflow_open_ui_value(config: &PufferConfig, open_in_browser: bool) -> Result<Value> {
    let frontend_url = config.workflow_backend.frontend_url.trim();
    if frontend_url.is_empty() {
        anyhow::bail!("workflow backend UI URL is not configured");
    }
    let url = workflow_ui_url(frontend_url)?;
    let opened = open_in_browser && crate::authflow::open_browser(&url);
    Ok(json!({
        "url": url,
        "opened": opened,
    }))
}

pub(crate) fn workflow_runtime_client(
    paths: &ConfigPaths,
    config: &PufferConfig,
) -> Result<WorkflowRuntimeClient> {
    let resolved = resolve_workflow_runtime_config(paths, config)?;
    workflow_runtime_client_from_resolved(config, resolved)
}

pub(crate) fn workflow_runtime_client_for_mode(
    paths: &ConfigPaths,
    config: &PufferConfig,
    mode: WorkflowBackendMode,
) -> Result<WorkflowRuntimeClient> {
    let mut selected = config.clone();
    let transient_local =
        selected.workflow_backend.mode != mode && mode == WorkflowBackendMode::Local;
    if selected.workflow_backend.mode != mode {
        selected.workflow_backend = WorkflowBackendConfig {
            mode,
            api_base_url: WorkflowBackendConfig::default_api_base_url(mode).to_string(),
            frontend_url: WorkflowBackendConfig::default_frontend_url(mode).to_string(),
            workspace_id: String::new(),
            api_token_secret_id: String::new(),
        };
    }
    let resolved = if transient_local {
        resolve_workflow_runtime_config_with(
            paths,
            &selected,
            workflow_local_runtime::ensure_ready_transient,
        )?
    } else {
        resolve_workflow_runtime_config(paths, &selected)?
    };
    workflow_runtime_client_from_resolved(&selected, resolved)
}

fn workflow_runtime_client_from_resolved(
    config: &PufferConfig,
    resolved: ResolvedWorkflowRuntimeConfig,
) -> Result<WorkflowRuntimeClient> {
    let http_client = blocking_client_for_url(
        &config.network.proxy,
        HttpPurpose::Discovery,
        &resolved.api_base_url,
        WORKFLOW_RUNTIME_TIMEOUT,
    )
    .context("build workflow runtime HTTP client")?;
    WorkflowRuntimeClient::with_client(
        WorkflowRuntimeClientConfig::new(
            resolved.api_base_url,
            resolved.api_token,
            resolved.workspace_id,
        )
        .with_timeout(WORKFLOW_RUNTIME_TIMEOUT),
        http_client,
    )
    .context("create workflow runtime client")
}

fn resolve_workflow_runtime_config(
    paths: &ConfigPaths,
    config: &PufferConfig,
) -> Result<ResolvedWorkflowRuntimeConfig> {
    resolve_workflow_runtime_config_with(paths, config, workflow_local_runtime::ensure_ready)
}

fn resolve_workflow_runtime_config_with<F>(
    paths: &ConfigPaths,
    config: &PufferConfig,
    ensure_local_ready: F,
) -> Result<ResolvedWorkflowRuntimeConfig>
where
    F: FnOnce(
        &ConfigPaths,
        &mut PufferConfig,
    ) -> Result<workflow_local_runtime::LocalWorkflowRuntimeStatus>,
{
    let mut backend = config.workflow_backend.clone();
    backend.normalize();
    if backend.mode == WorkflowBackendMode::Local {
        let mut local_config = config.clone();
        let status = ensure_local_ready(paths, &mut local_config)
            .context("ensure local workflow runtime is ready")?;
        if status.state != workflow_local_runtime::LocalWorkflowRuntimeState::Ready {
            let detail = status
                .message
                .as_deref()
                .unwrap_or("local workflow runtime is not ready");
            anyhow::bail!(
                "local workflow runtime is {}: {detail}",
                status.state.as_str()
            );
        }
        backend = local_config.workflow_backend.clone();
        backend.normalize();
    }
    Ok(ResolvedWorkflowRuntimeConfig {
        api_token: resolve_workflow_runtime_token(
            paths,
            backend.mode,
            &backend.api_token_secret_id,
        )?,
        api_base_url: backend.api_base_url,
        workspace_id: backend.workspace_id,
    })
}

fn resolve_workflow_runtime_token(
    paths: &ConfigPaths,
    mode: WorkflowBackendMode,
    secret_id: &str,
) -> Result<String> {
    let secret_id = required_trimmed("workflow runtime token secret id", secret_id)
        .map_err(|_| anyhow::anyhow!("{} token is not configured", workflow_runtime_name(mode)))?;
    let vault = SecretVault::open(SecretVault::default_path(&paths.user_config_dir))
        .context("open encrypted secret store")?;
    let resolved = vault.reveal(&secret_id).with_context(|| {
        format!(
            "load {} token from secret store",
            workflow_runtime_name(mode)
        )
    })?;
    Ok(resolved.value)
}

fn workflow_ui_url(frontend_url: &str) -> Result<String> {
    let mut url = normalized_frontend_url(frontend_url)?;
    let already_workflows = url
        .path_segments()
        .and_then(|segments| segments.filter(|segment| !segment.is_empty()).last())
        == Some("workflows");
    let mut segments = url
        .path_segments_mut()
        .map_err(|_| anyhow::anyhow!("workflow backend UI URL must be hierarchical"))?;
    segments.pop_if_empty();
    if !already_workflows {
        segments.push("workflows");
    }
    drop(segments);
    Ok(url.to_string())
}

fn normalized_frontend_url(value: &str) -> Result<Url> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        anyhow::bail!("workflow backend UI URL is not configured");
    }
    let mut parsed = Url::parse(trimmed).context("workflow backend UI URL must be a valid URL")?;
    match parsed.scheme() {
        "http" | "https" => {}
        other => {
            anyhow::bail!("workflow backend UI URL must use http or https, got `{other}`")
        }
    }
    parsed.set_query(None);
    parsed.set_fragment(None);
    Ok(parsed)
}

fn workflow_runtime_name(mode: WorkflowBackendMode) -> &'static str {
    match mode {
        WorkflowBackendMode::Local => "automation runtime",
        WorkflowBackendMode::AgentEnvCloud => "AgentEnv Cloud automation runtime",
    }
}

fn required_trimmed(label: &str, value: &str) -> Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        anyhow::bail!("missing {label}");
    }
    Ok(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon_workflow_backend_settings::test_support::{
        lock_secret_store, temp_paths, ScopedSecretStoreKey,
    };
    use puffer_config::{ensure_workspace_dirs, load_config, WorkflowBackendConfig};
    use std::fs;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};
    use std::thread;

    // Config persists across a save/reload and the token never lands in config,
    // the snapshot, or the RPC response (only `hasToken` is reported).
    #[test]
    fn workflow_backend_config_save_and_get_mask_token() {
        let _guard = lock_secret_store();
        let _secret_store_key = ScopedSecretStoreKey::set();
        let temp = tempfile::tempdir().expect("tempdir");
        let paths = temp_paths(&temp);
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        let mut config = PufferConfig::default();

        let saved = save_workflow_backend_config_value(
            &paths,
            &mut config,
            SaveWorkflowBackendSettingsParams {
                mode: WorkflowBackendMode::AgentEnvCloud,
                api_url: "https://api.agentenv.io/v1/workflows".to_string(),
                ui_url: "https://agentenv.io/console/".to_string(),
                workspace_id: " workspace-123 ".to_string(),
                api_token: Some("super-secret-token".to_string()),
                keep_token: false,
            },
        )
        .expect("save config");

        assert_eq!(saved["mode"], "agent_env_cloud");
        assert_eq!(saved["apiUrl"], "https://api.agentenv.io");
        assert_eq!(saved["uiUrl"], "https://agentenv.io/console");
        assert_eq!(saved["workspaceId"], "workspace-123");
        assert_eq!(saved["hasToken"], true);
        assert!(saved.get("apiToken").is_none());

        let raw_config = fs::read_to_string(paths.user_config_file()).expect("read user config");
        assert!(!raw_config.contains("super-secret-token"));
        assert!(!raw_config.contains("\"apiToken\""));
        assert!(raw_config.contains("api_token_secret_id"));

        let loaded = load_config(&paths).expect("load config");
        let fetched = workflow_backend_config_value(&paths, &loaded).expect("get config");
        let serialized = serde_json::to_string(&fetched).expect("serialize config");
        assert_eq!(fetched["hasToken"], true);
        assert!(!serialized.contains("super-secret-token"));
    }

    fn read_request(stream: &mut TcpStream) -> String {
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 1024];
        loop {
            let read = stream.read(&mut buffer).expect("read request");
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..read]);
            if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        String::from_utf8(bytes).expect("request utf8")
    }

    fn write_json_response(stream: &mut TcpStream, body: &str) {
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream
            .write_all(response.as_bytes())
            .expect("write response");
    }

    fn spawn_runtime_server() -> (String, Arc<Mutex<Vec<String>>>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test runtime");
        let url = format!("http://{}", listener.local_addr().expect("local addr"));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&requests);
        let handle = thread::spawn(move || {
            for (index, stream) in listener.incoming().take(3).enumerate() {
                let mut stream = stream.expect("accept connection");
                let request = read_request(&mut stream);
                captured.lock().expect("requests lock").push(request);
                let body = if index == 0 {
                    r#"{"status":"ready"}"#
                } else if index == 1 {
                    r#"{"data":[{"id":"node-a"},{"id":"node-b"}]}"#
                } else {
                    r#"{"data":[{"id":"workflow-a"}]}"#
                };
                write_json_response(&mut stream, body);
            }
        });
        (url, requests, handle)
    }

    // The daemon test uses a real local HTTP listener so it validates the
    // saved secret, reqwest client wiring, runtime paths, and headers together.
    #[test]
    fn workflow_backend_test_connection_uses_runtime_client_wiring() {
        let _guard = lock_secret_store();
        let _secret_store_key = ScopedSecretStoreKey::set();
        let temp = tempfile::tempdir().expect("tempdir");
        let paths = temp_paths(&temp);
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        let (api_url, requests, handle) = spawn_runtime_server();
        let mut config = PufferConfig::default();

        save_workflow_backend_settings(
            &paths,
            &mut config,
            SaveWorkflowBackendSettingsParams {
                mode: WorkflowBackendMode::AgentEnvCloud,
                api_url,
                ui_url: "http://localhost:5173".to_string(),
                workspace_id: "workspace-local".to_string(),
                api_token: Some("runtime-token".to_string()),
                keep_token: false,
            },
        )
        .expect("save backend settings");

        let response =
            workflow_backend_test_connection_value(&paths, &config).expect("test connection");
        handle.join().expect("runtime server joined");

        assert_eq!(response["success"], true);
        assert_eq!(response["ready"]["state"], "passed");
        assert_eq!(response["runtime"]["state"], "passed");
        assert_eq!(response["auth"]["state"], "passed");
        assert_eq!(response["workspace"]["state"], "passed");

        let captured = requests.lock().expect("requests lock");
        assert_eq!(captured.len(), 3);
        assert!(captured[0].starts_with("GET /v1/health/ready "));
        assert!(!captured[0].to_ascii_lowercase().contains("x-api-key"));
        assert!(captured[1].starts_with("GET /v1/workflows/node-definitions "));
        assert!(captured[1]
            .to_ascii_lowercase()
            .contains("x-api-key: runtime-token"));
        assert!(!captured[1].to_ascii_lowercase().contains("x-workspace-id"));
        assert!(captured[2].starts_with("GET /v1/workflows "));
        assert!(captured[2]
            .to_ascii_lowercase()
            .contains("x-workspace-id: workspace-local"));
    }

    // A missing token surfaces a clear product error instead of falling back
    // to any default or environment value.
    #[test]
    fn missing_token_reports_clear_error_without_fallback() {
        let _guard = lock_secret_store();
        let _secret_store_key = ScopedSecretStoreKey::set();
        let temp = tempfile::tempdir().expect("tempdir");
        let paths = temp_paths(&temp);
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        let mut config = PufferConfig::default();
        config.workflow_backend = WorkflowBackendConfig {
            mode: WorkflowBackendMode::AgentEnvCloud,
            api_base_url: "https://api.agentenv.io".to_string(),
            frontend_url: "https://agentenv.io".to_string(),
            workspace_id: "workspace-123".to_string(),
            api_token_secret_id: String::new(),
        };

        let error = workflow_backend_test_connection_value(&paths, &config)
            .expect_err("missing token should error");
        assert!(error
            .to_string()
            .contains("Automation runtime token is not configured"));
    }

    #[test]
    fn public_runtime_error_hides_local_migration_diagnostics() {
        let raw = r#"local workflow runtime is incompatible_runtime: docker compose run --rm migrate failed: Container puffer-workflow-runtime-postgres-1 Healthy Database migration failed: error: could not open file "global/pg_filenode.map": No such file or directory at Parser.parseErrorMessage (/app/node_modules/pg-protocol/dist/parser.js:285:98) at TCP.onStreamRead (node:internal/stream_base_commons:191:23); could not refresh agentenv/api-server:local"#;

        let message = public_workflow_runtime_detail_message(raw);

        assert_eq!(
            message,
            "The Puffer-managed local automation runtime database could not be prepared. Puffer needs to rebuild the local runtime data before automations can run."
        );
        assert!(!message.contains("global/pg_filenode.map"));
        assert!(!message.contains("Parser.parseErrorMessage"));
        assert!(!message.contains("node_modules"));
        assert!(!message.contains("agentenv/api-server"));
    }

    #[test]
    fn connection_error_dto_uses_public_runtime_message() {
        let error = WorkflowRuntimeError {
            kind: WorkflowRuntimeErrorKind::IncompatibleRuntime,
            message:
                "incompatible workflow runtime: invalid JSON from http://127.0.0.1:3000/v1/workflows"
                    .to_string(),
            status_code: None,
        };

        let dto = WorkflowRuntimeErrorDto::from(error);

        assert_eq!(
            dto.message,
            "The Puffer-managed local automation runtime is not compatible with this Puffer build. Puffer could not update it automatically; try again after Docker is ready."
        );
        assert!(!dto.message.contains("127.0.0.1"));
        assert!(!dto.message.contains("/v1/"));
    }

    #[test]
    fn local_runtime_config_resolution_ensures_ready() {
        let _guard = lock_secret_store();
        let _secret_store_key = ScopedSecretStoreKey::set();
        let temp = tempfile::tempdir().expect("tempdir");
        let paths = temp_paths(&temp);
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        let mut config = PufferConfig::default();
        config.workflow_backend.workspace_id = "workspace-local".to_string();
        save_workflow_backend_settings(
            &paths,
            &mut config,
            SaveWorkflowBackendSettingsParams {
                mode: WorkflowBackendMode::Local,
                api_url: "http://127.0.0.1:3456".to_string(),
                ui_url: "http://localhost:5173".to_string(),
                workspace_id: "workspace-local".to_string(),
                api_token: Some("runtime-token".to_string()),
                keep_token: false,
            },
        )
        .expect("save backend settings");
        let called = Arc::new(Mutex::new(false));
        let observed = Arc::clone(&called);

        let resolved = resolve_workflow_runtime_config_with(&paths, &config, move |_, _| {
            *observed.lock().expect("called") = true;
            Ok(workflow_local_runtime::LocalWorkflowRuntimeStatus {
                state: workflow_local_runtime::LocalWorkflowRuntimeState::Ready,
                image: "agentenv/api-server:local".to_string(),
                stack_name: "puffer-workflow-runtime".to_string(),
                api_base_url: None,
                compose_file: None,
                data_dir: None,
                message: None,
            })
        })
        .expect("resolve runtime config");

        assert_eq!(resolved.api_base_url, "http://127.0.0.1:3456");
        assert_eq!(resolved.api_token, "runtime-token");
        assert_eq!(*called.lock().expect("called"), true);
    }

    #[test]
    fn cloud_runtime_config_resolution_does_not_ensure_local_runtime() {
        let _guard = lock_secret_store();
        let _secret_store_key = ScopedSecretStoreKey::set();
        let temp = tempfile::tempdir().expect("tempdir");
        let paths = temp_paths(&temp);
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        let mut config = PufferConfig::default();
        save_workflow_backend_settings(
            &paths,
            &mut config,
            SaveWorkflowBackendSettingsParams {
                mode: WorkflowBackendMode::AgentEnvCloud,
                api_url: "https://api.agentenv.io".to_string(),
                ui_url: "https://agentenv.io".to_string(),
                workspace_id: "workspace-cloud".to_string(),
                api_token: Some("cloud-token".to_string()),
                keep_token: false,
            },
        )
        .expect("save backend settings");
        let called = Arc::new(Mutex::new(false));
        let observed = Arc::clone(&called);

        let resolved = resolve_workflow_runtime_config_with(&paths, &config, move |_, _| {
            *observed.lock().expect("called") = true;
            unreachable!("cloud mode must not ensure the local Docker runtime")
        })
        .expect("resolve runtime config");

        assert_eq!(resolved.api_base_url, "https://api.agentenv.io");
        assert_eq!(resolved.api_token, "cloud-token");
        assert_eq!(*called.lock().expect("called"), false);
    }

    // `open_ui` normalizes the configured UI URL before appending the stable
    // first-phase Workflow Console path.
    #[test]
    fn workflow_open_ui_normalizes_url_paths() {
        let mut config = PufferConfig::default();
        config.workflow_backend = WorkflowBackendConfig {
            mode: WorkflowBackendMode::AgentEnvCloud,
            api_base_url: "https://api.agentenv.io".to_string(),
            frontend_url: "https://agentenv.io/runtime/".to_string(),
            workspace_id: "workspace-123".to_string(),
            api_token_secret_id: "sec_runtime".to_string(),
        };

        let overview = workflow_open_ui_value(&config, false).expect("overview URL");

        assert_eq!(overview["url"], "https://agentenv.io/runtime/workflows");
        assert_eq!(overview["opened"], false);
    }

    #[test]
    fn workflow_open_ui_does_not_duplicate_workflows_path() {
        let mut config = PufferConfig::default();
        config.workflow_backend = WorkflowBackendConfig {
            mode: WorkflowBackendMode::Local,
            api_base_url: "http://127.0.0.1:3000".to_string(),
            frontend_url: "http://localhost:5173/workflows/".to_string(),
            workspace_id: String::new(),
            api_token_secret_id: "sec_runtime".to_string(),
        };

        let overview = workflow_open_ui_value(&config, false).expect("overview URL");

        assert_eq!(overview["url"], "http://localhost:5173/workflows");
    }

    #[test]
    fn workflow_open_ui_requires_configured_ui_url() {
        let mut config = PufferConfig::default();
        config.workflow_backend.frontend_url = String::new();

        let error = workflow_open_ui_value(&config, false).expect_err("missing UI URL");

        assert!(error
            .to_string()
            .contains("workflow backend UI URL is not configured"));
    }
}

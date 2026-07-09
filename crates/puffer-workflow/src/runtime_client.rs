use crate::agentenv_definition::{
    AgentEnvCreateWorkflowRequest, AgentEnvInMemoryExecuteRequest, AgentEnvUpdateWorkflowRequest,
};
use anyhow::{Context, Result};
use reqwest::blocking::Client;
use reqwest::{Method, StatusCode, Url};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);

/// Result alias returned by workflow runtime HTTP operations.
pub type WorkflowRuntimeResult<T> = std::result::Result<T, WorkflowRuntimeError>;

/// Configuration for the shared Workflow Runtime HTTP client.
#[derive(Debug, Clone)]
pub struct WorkflowRuntimeClientConfig {
    /// Base API origin for the runtime, such as `https://api.agentenv.io`.
    pub api_base_url: String,
    /// Runtime API token sent as `X-API-Key`.
    pub api_key: String,
    /// Workspace identifier sent as `X-Workspace-ID` on workspace-scoped calls.
    pub workspace_id: String,
    /// Per-request timeout for the blocking HTTP client.
    pub timeout: Duration,
}

impl WorkflowRuntimeClientConfig {
    /// Creates a client config using the default workflow runtime timeout.
    pub fn new(
        api_base_url: impl Into<String>,
        api_key: impl Into<String>,
        workspace_id: impl Into<String>,
    ) -> Self {
        Self {
            api_base_url: api_base_url.into(),
            api_key: api_key.into(),
            workspace_id: workspace_id.into(),
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Overrides the per-request timeout used by the runtime client.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

/// Shared blocking client for the remote Workflow Runtime HTTP API.
#[derive(Debug, Clone)]
pub struct WorkflowRuntimeClient {
    transport: WorkflowRuntimeTransport,
    api_base_url: Url,
    api_key: String,
    workspace_id: String,
}

#[derive(Debug, Clone)]
enum WorkflowRuntimeTransport {
    Http(Client),
    #[cfg(test)]
    Mock(MockTransport),
}

#[derive(Debug, Clone)]
struct WorkflowRuntimePreparedRequest {
    method: Method,
    url: Url,
    headers: Vec<(String, String)>,
    body: Option<String>,
}

#[derive(Debug, Clone)]
struct WorkflowRuntimeRawResponse {
    status: StatusCode,
    body_text: String,
}

/// Stable workflow runtime error classification surfaced to callers and UI.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowRuntimeErrorKind {
    /// The supplied API token is invalid.
    InvalidToken,
    /// The token is valid but does not have access to the requested operation.
    PermissionDenied,
    /// The workspace cannot be reached, does not exist, or is mismatched.
    WorkspaceInaccessible,
    /// The runtime endpoint could not be reached over the network.
    RuntimeUnreachable,
    /// The runtime responded with an unexpected JSON payload or schema.
    IncompatibleRuntime,
    /// The runtime returned a non-mapped HTTP failure response.
    ServiceError,
}

/// Error payload returned by workflow runtime HTTP operations.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowRuntimeError {
    /// Machine-readable error category for UI and control flow.
    pub kind: WorkflowRuntimeErrorKind,
    /// Human-readable summary suitable for direct display.
    pub message: String,
    /// Optional HTTP status code when the error came from a response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_code: Option<u16>,
}

impl WorkflowRuntimeError {
    fn invalid_token() -> Self {
        Self {
            kind: WorkflowRuntimeErrorKind::InvalidToken,
            message: "invalid token".to_string(),
            status_code: Some(StatusCode::UNAUTHORIZED.as_u16()),
        }
    }

    fn permission_denied() -> Self {
        Self {
            kind: WorkflowRuntimeErrorKind::PermissionDenied,
            message: "permission denied".to_string(),
            status_code: Some(StatusCode::FORBIDDEN.as_u16()),
        }
    }

    fn workspace_inaccessible() -> Self {
        Self {
            kind: WorkflowRuntimeErrorKind::WorkspaceInaccessible,
            message: "workspace inaccessible or not found".to_string(),
            status_code: Some(StatusCode::NOT_FOUND.as_u16()),
        }
    }

    fn runtime_unreachable(detail: impl fmt::Display) -> Self {
        Self {
            kind: WorkflowRuntimeErrorKind::RuntimeUnreachable,
            message: format!("runtime unreachable: {detail}"),
            status_code: None,
        }
    }

    fn incompatible_runtime(detail: impl fmt::Display) -> Self {
        Self {
            kind: WorkflowRuntimeErrorKind::IncompatibleRuntime,
            message: format!("incompatible workflow runtime: {detail}"),
            status_code: None,
        }
    }

    fn service_error(status_code: u16, detail: impl fmt::Display) -> Self {
        Self {
            kind: WorkflowRuntimeErrorKind::ServiceError,
            message: format!("workflow runtime error ({status_code}): {detail}"),
            status_code: Some(status_code),
        }
    }
}

impl fmt::Display for WorkflowRuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for WorkflowRuntimeError {}

/// Generic JSON object exchanged with the workflow runtime.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(transparent)]
pub struct WorkflowRuntimeRecord {
    fields: BTreeMap<String, Value>,
}

impl WorkflowRuntimeRecord {
    /// Creates a record wrapper around a JSON object field map.
    pub fn new(fields: BTreeMap<String, Value>) -> Self {
        Self { fields }
    }

    /// Returns one field from the runtime object.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.fields.get(key)
    }

    /// Returns all JSON object fields by reference.
    pub fn fields(&self) -> &BTreeMap<String, Value> {
        &self.fields
    }

    /// Consumes the record and returns its JSON object fields.
    pub fn into_fields(self) -> BTreeMap<String, Value> {
        self.fields
    }
}

impl From<BTreeMap<String, Value>> for WorkflowRuntimeRecord {
    fn from(fields: BTreeMap<String, Value>) -> Self {
        Self::new(fields)
    }
}

impl TryFrom<Value> for WorkflowRuntimeRecord {
    type Error = WorkflowRuntimeError;

    fn try_from(value: Value) -> WorkflowRuntimeResult<Self> {
        serde_json::from_value(value).map_err(|error| {
            WorkflowRuntimeError::incompatible_runtime(format!("expected JSON object: {error}"))
        })
    }
}

/// Node definition object returned by `GET /v1/workflows/node-definitions`.
pub type WorkflowRuntimeNodeDefinition = WorkflowRuntimeRecord;

/// API key context returned by `GET /v1/auth/api-key-context`.
pub type WorkflowRuntimeApiKeyContext = WorkflowRuntimeRecord;

/// AI Gateway upstream returned by `GET /v1/ai-gateway/upstreams`.
pub type WorkflowRuntimeGatewayUpstream = WorkflowRuntimeRecord;

/// Request payload sent to `POST /v1/ai-gateway/upstreams`.
pub type WorkflowRuntimeCreateGatewayUpstreamRequest = Value;

/// Request payload sent to `PUT /v1/ai-gateway/upstreams/:id`.
pub type WorkflowRuntimeUpdateGatewayUpstreamRequest = Value;

/// Request payload sent to `POST /v1/workflows`.
pub type WorkflowRuntimeCreateWorkflowRequest = AgentEnvCreateWorkflowRequest;

/// Request payload sent to `PUT /v1/workflows/:id`.
pub type WorkflowRuntimeUpdateWorkflowRequest = AgentEnvUpdateWorkflowRequest;

/// Workflow object returned by workflow list and get calls.
pub type WorkflowRuntimeWorkflow = WorkflowRuntimeRecord;

/// Response payload returned by `POST /v1/workflows/:id/deploy`.
pub type WorkflowRuntimeDeployResponse = WorkflowRuntimeRecord;

/// Response payload returned by `POST /v1/workflows/:id/undeploy`.
pub type WorkflowRuntimeUndeployResponse = WorkflowRuntimeRecord;

/// Execution object returned by workflow execution APIs.
pub type WorkflowRuntimeExecution = WorkflowRuntimeRecord;

/// Request payload sent to `POST /v1/workflows/:id/execute`.
pub type WorkflowRuntimeExecuteRequest = WorkflowRuntimeRecord;

/// Response payload returned by `POST /v1/workflows/:id/execute`.
pub type WorkflowRuntimeExecuteResponse = WorkflowRuntimeRecord;

/// Request payload sent to `POST /v1/workflows/execute-in-memory`.
pub type WorkflowRuntimeInMemoryExecuteRequest = AgentEnvInMemoryExecuteRequest;

/// Response payload returned by `POST /v1/workflows/execute-in-memory`.
pub type WorkflowRuntimeInMemoryExecuteResponse = WorkflowRuntimeRecord;

/// High-level state for one `test_connection` phase.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowRuntimeConnectionStepState {
    /// The phase succeeded.
    Passed,
    /// The phase failed with a classified runtime error.
    Failed,
    /// The phase did not run because an earlier phase failed.
    Skipped,
}

/// UI-friendly result for one phase of workflow runtime connection testing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowRuntimeConnectionStep {
    /// HTTP method executed for this phase.
    pub method: String,
    /// API path executed for this phase.
    pub path: String,
    /// Whether the phase passed, failed, or was skipped.
    pub state: WorkflowRuntimeConnectionStepState,
    /// Number of objects returned when the phase succeeded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_count: Option<usize>,
    /// Optional display message for skipped steps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Failure details when the phase failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<WorkflowRuntimeError>,
}

impl WorkflowRuntimeConnectionStep {
    fn passed(method: &str, path: &str, item_count: usize) -> Self {
        Self {
            method: method.to_string(),
            path: path.to_string(),
            state: WorkflowRuntimeConnectionStepState::Passed,
            item_count: Some(item_count),
            message: None,
            error: None,
        }
    }

    fn failed(method: &str, path: &str, error: WorkflowRuntimeError) -> Self {
        Self {
            method: method.to_string(),
            path: path.to_string(),
            state: WorkflowRuntimeConnectionStepState::Failed,
            item_count: None,
            message: None,
            error: Some(error),
        }
    }

    fn skipped(method: &str, path: &str, message: impl Into<String>) -> Self {
        Self {
            method: method.to_string(),
            path: path.to_string(),
            state: WorkflowRuntimeConnectionStepState::Skipped,
            item_count: None,
            message: Some(message.into()),
            error: None,
        }
    }
}

/// Two-phase connectivity report for the workflow runtime UI.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowRuntimeConnectionTest {
    /// Phase 1: validates the runtime readiness endpoint.
    pub ready: WorkflowRuntimeConnectionStep,
    /// Phase 2: validates the token and workflow API surface.
    pub api_surface: WorkflowRuntimeConnectionStep,
    /// Phase 3: validates workspace access with `X-Workspace-ID`.
    pub workspace_access: WorkflowRuntimeConnectionStep,
}

impl WorkflowRuntimeConnectionTest {
    /// Returns true when both phases passed.
    pub fn is_success(&self) -> bool {
        self.ready.state == WorkflowRuntimeConnectionStepState::Passed
            && self.api_surface.state == WorkflowRuntimeConnectionStepState::Passed
            && self.workspace_access.state == WorkflowRuntimeConnectionStepState::Passed
    }
}

impl WorkflowRuntimeTransport {
    fn send(
        &self,
        request: WorkflowRuntimePreparedRequest,
    ) -> WorkflowRuntimeResult<WorkflowRuntimeRawResponse> {
        match self {
            Self::Http(client) => {
                let mut builder = client.request(request.method, request.url);
                for (name, value) in request.headers {
                    builder = builder.header(name, value);
                }
                if let Some(body) = request.body {
                    builder = builder
                        .header("content-type", "application/json")
                        .body(body);
                }
                let response = builder
                    .send()
                    .map_err(|error| WorkflowRuntimeError::runtime_unreachable(error))?;
                let status = response.status();
                let body_text = response
                    .text()
                    .map_err(|error| WorkflowRuntimeError::runtime_unreachable(error))?;
                Ok(WorkflowRuntimeRawResponse { status, body_text })
            }
            #[cfg(test)]
            Self::Mock(transport) => transport.send(request),
        }
    }
}

impl WorkflowRuntimeClient {
    /// Builds a runtime client with its own blocking reqwest client.
    pub fn new(config: WorkflowRuntimeClientConfig) -> Result<Self> {
        let client = Client::builder()
            .timeout(config.timeout)
            .build()
            .context("failed to build workflow runtime HTTP client")?;
        Self::with_client(config, client)
    }

    /// Builds a runtime client from an externally configured blocking client.
    pub fn with_client(config: WorkflowRuntimeClientConfig, client: Client) -> Result<Self> {
        Ok(Self {
            transport: WorkflowRuntimeTransport::Http(client),
            api_base_url: normalized_api_base_url(&config.api_base_url)?,
            api_key: config.api_key,
            workspace_id: config.workspace_id,
        })
    }

    /// Returns the workspace id resolved for workspace-scoped runtime calls.
    pub fn workspace_id(&self) -> &str {
        &self.workspace_id
    }

    /// Returns API key context from `GET /v1/auth/api-key-context`.
    pub fn api_key_context(&self) -> WorkflowRuntimeResult<WorkflowRuntimeApiKeyContext> {
        let payload = self.send_request(Method::GET, &["auth", "api-key-context"], false, None)?;
        parse_bare_record(payload, "/v1/auth/api-key-context")
    }

    /// Lists runtime node definitions from `GET /v1/workflows/node-definitions`.
    pub fn list_node_definitions(
        &self,
    ) -> WorkflowRuntimeResult<Vec<WorkflowRuntimeNodeDefinition>> {
        let payload =
            self.send_request(Method::GET, &["workflows", "node-definitions"], false, None)?;
        parse_record_list(payload, "/v1/workflows/node-definitions")
    }

    /// Fetches one runtime node definition by AgentEnv node type.
    pub fn get_node_definition(
        &self,
        node_type: &str,
    ) -> WorkflowRuntimeResult<WorkflowRuntimeNodeDefinition> {
        let payload = self.send_request(
            Method::GET,
            &["workflows", "node-definitions", node_type],
            false,
            None,
        )?;
        parse_record(payload, "/v1/workflows/node-definitions/:type")
    }

    /// Lists workflows visible to the configured workspace.
    pub fn list_workflows(&self) -> WorkflowRuntimeResult<Vec<WorkflowRuntimeWorkflow>> {
        let payload = self.send_request(Method::GET, &["workflows"], true, None)?;
        parse_record_list(payload, "/v1/workflows")
    }

    /// Lists AI Gateway upstreams visible to the authenticated runtime user.
    ///
    /// This endpoint is user-scoped in AgentEnv and intentionally does not
    /// receive `X-Workspace-ID`; workspace API keys are rejected by AgentEnv.
    pub fn list_gateway_upstreams(
        &self,
    ) -> WorkflowRuntimeResult<Vec<WorkflowRuntimeGatewayUpstream>> {
        let payload = self.send_request(Method::GET, &["ai-gateway", "upstreams"], false, None)?;
        parse_bare_record_list(payload, "/v1/ai-gateway/upstreams")
    }

    /// Creates an AI Gateway upstream for the authenticated runtime user.
    pub fn create_gateway_upstream(
        &self,
        request: &WorkflowRuntimeCreateGatewayUpstreamRequest,
    ) -> WorkflowRuntimeResult<WorkflowRuntimeGatewayUpstream> {
        let payload =
            self.send_json_request(Method::POST, &["ai-gateway", "upstreams"], false, request)?;
        parse_bare_record(payload, "/v1/ai-gateway/upstreams")
    }

    /// Updates an AI Gateway upstream for the authenticated runtime user.
    pub fn update_gateway_upstream(
        &self,
        upstream_id: &str,
        request: &WorkflowRuntimeUpdateGatewayUpstreamRequest,
    ) -> WorkflowRuntimeResult<WorkflowRuntimeRecord> {
        let payload = self.send_json_request(
            Method::PUT,
            &["ai-gateway", "upstreams", upstream_id],
            false,
            request,
        )?;
        parse_bare_record(payload, "/v1/ai-gateway/upstreams/:id")
    }

    /// Creates a workflow with `POST /v1/workflows`.
    pub fn create_workflow(
        &self,
        request: &WorkflowRuntimeCreateWorkflowRequest,
    ) -> WorkflowRuntimeResult<WorkflowRuntimeWorkflow> {
        let payload = self.send_json_request(Method::POST, &["workflows"], true, request)?;
        parse_record(payload, "/v1/workflows")
    }

    /// Fetches one workflow by id.
    pub fn get_workflow(
        &self,
        workflow_id: &str,
    ) -> WorkflowRuntimeResult<WorkflowRuntimeWorkflow> {
        let payload = self.send_request(Method::GET, &["workflows", workflow_id], true, None)?;
        parse_record(payload, "/v1/workflows/:id")
    }

    /// Updates one workflow draft by id.
    pub fn update_workflow(
        &self,
        workflow_id: &str,
        request: &WorkflowRuntimeUpdateWorkflowRequest,
    ) -> WorkflowRuntimeResult<WorkflowRuntimeWorkflow> {
        let payload =
            self.send_json_request(Method::PUT, &["workflows", workflow_id], true, request)?;
        parse_record(payload, "/v1/workflows/:id")
    }

    /// Deploys one workflow by id.
    pub fn deploy_workflow(
        &self,
        workflow_id: &str,
    ) -> WorkflowRuntimeResult<WorkflowRuntimeDeployResponse> {
        let payload = self.send_request(
            Method::POST,
            &["workflows", workflow_id, "deploy"],
            true,
            None,
        )?;
        parse_record(payload, "/v1/workflows/:id/deploy")
    }

    /// Undeploys one workflow by id.
    pub fn undeploy_workflow(
        &self,
        workflow_id: &str,
    ) -> WorkflowRuntimeResult<WorkflowRuntimeUndeployResponse> {
        let payload = self.send_request(
            Method::POST,
            &["workflows", workflow_id, "undeploy"],
            true,
            None,
        )?;
        parse_record(payload, "/v1/workflows/:id/undeploy")
    }

    /// Executes one deployed workflow by id.
    pub fn execute_workflow(
        &self,
        workflow_id: &str,
        request: &WorkflowRuntimeExecuteRequest,
    ) -> WorkflowRuntimeResult<WorkflowRuntimeExecuteResponse> {
        let payload = self.send_json_request(
            Method::POST,
            &["workflows", workflow_id, "execute"],
            true,
            request,
        )?;
        parse_record(payload, "/v1/workflows/:id/execute")
    }

    /// Lists executions for one workflow id.
    pub fn list_executions(
        &self,
        workflow_id: &str,
    ) -> WorkflowRuntimeResult<Vec<WorkflowRuntimeExecution>> {
        let payload = self.send_request(
            Method::GET,
            &["workflows", workflow_id, "executions"],
            true,
            None,
        )?;
        parse_record_list(payload, "/v1/workflows/:id/executions")
    }

    /// Fetches one execution for one workflow id.
    pub fn get_execution(
        &self,
        workflow_id: &str,
        execution_id: &str,
    ) -> WorkflowRuntimeResult<WorkflowRuntimeExecution> {
        let payload = self.send_request(
            Method::GET,
            &["workflows", workflow_id, "executions", execution_id],
            true,
            None,
        )?;
        parse_record(payload, "/v1/workflows/:id/executions/:executionId")
    }

    /// Executes an in-memory workflow definition when the runtime exposes the optional endpoint.
    pub fn execute_in_memory(
        &self,
        request: &WorkflowRuntimeInMemoryExecuteRequest,
    ) -> WorkflowRuntimeResult<WorkflowRuntimeInMemoryExecuteResponse> {
        let payload = self.send_json_request(
            Method::POST,
            &["workflows", "execute-in-memory"],
            true,
            request,
        )?;
        parse_record(payload, "/v1/workflows/execute-in-memory")
    }

    /// Checks `GET /v1/health/ready` without workspace scope.
    pub fn health_ready(&self) -> WorkflowRuntimeResult<()> {
        self.send_request_for_status(Method::GET, &["health", "ready"], false, false)
    }

    /// Runs the two-step runtime connectivity check used by the UI.
    pub fn test_connection(&self) -> WorkflowRuntimeConnectionTest {
        let ready_method = "GET";
        let ready_path = "/v1/health/ready";
        let api_method = "GET";
        let api_path = "/v1/workflows/node-definitions";
        let workspace_method = "GET";
        let workspace_path = "/v1/workflows";

        let ready = match self.health_ready() {
            Ok(()) => WorkflowRuntimeConnectionStep::passed(ready_method, ready_path, 0),
            Err(error) => {
                return WorkflowRuntimeConnectionTest {
                    ready: WorkflowRuntimeConnectionStep::failed(ready_method, ready_path, error),
                    api_surface: WorkflowRuntimeConnectionStep::skipped(
                        api_method,
                        api_path,
                        "skipped because runtime readiness validation failed",
                    ),
                    workspace_access: WorkflowRuntimeConnectionStep::skipped(
                        workspace_method,
                        workspace_path,
                        "skipped because runtime readiness validation failed",
                    ),
                };
            }
        };

        match self.list_node_definitions() {
            Ok(records) => {
                let api_surface =
                    WorkflowRuntimeConnectionStep::passed(api_method, api_path, records.len());
                match self.list_workflows() {
                    Ok(records) => WorkflowRuntimeConnectionTest {
                        ready,
                        api_surface,
                        workspace_access: WorkflowRuntimeConnectionStep::passed(
                            workspace_method,
                            workspace_path,
                            records.len(),
                        ),
                    },
                    Err(error) => WorkflowRuntimeConnectionTest {
                        ready,
                        api_surface,
                        workspace_access: WorkflowRuntimeConnectionStep::failed(
                            workspace_method,
                            workspace_path,
                            error,
                        ),
                    },
                }
            }
            Err(error) => WorkflowRuntimeConnectionTest {
                ready,
                api_surface: WorkflowRuntimeConnectionStep::failed(api_method, api_path, error),
                workspace_access: WorkflowRuntimeConnectionStep::skipped(
                    workspace_method,
                    workspace_path,
                    "skipped because workflow API surface validation failed",
                ),
            },
        }
    }

    fn send_request(
        &self,
        method: Method,
        path_segments: &[&str],
        include_workspace_id: bool,
        body: Option<String>,
    ) -> WorkflowRuntimeResult<Value> {
        self.send_request_with_headers(method, path_segments, true, include_workspace_id, body)
    }

    fn send_request_for_status(
        &self,
        method: Method,
        path_segments: &[&str],
        include_api_key: bool,
        include_workspace_id: bool,
    ) -> WorkflowRuntimeResult<()> {
        let url = self.endpoint_url(path_segments);
        let response = self.transport.send(WorkflowRuntimePreparedRequest {
            method,
            url: url.clone(),
            headers: self.request_headers(include_api_key, include_workspace_id),
            body: None,
        })?;
        if response.status.is_success() {
            return Ok(());
        }
        match map_http_response(url, response.status, &response.body_text) {
            Ok(_) => Ok(()),
            Err(error) => Err(error),
        }
    }

    fn send_request_with_headers(
        &self,
        method: Method,
        path_segments: &[&str],
        include_api_key: bool,
        include_workspace_id: bool,
        body: Option<String>,
    ) -> WorkflowRuntimeResult<Value> {
        let url = self.endpoint_url(path_segments);
        let request = WorkflowRuntimePreparedRequest {
            method,
            url: url.clone(),
            headers: self.request_headers(include_api_key, include_workspace_id),
            body,
        };
        let response = self.transport.send(request)?;
        map_http_response(url, response.status, &response.body_text)
    }

    fn send_json_request<T: Serialize + ?Sized>(
        &self,
        method: Method,
        path_segments: &[&str],
        include_workspace_id: bool,
        body: &T,
    ) -> WorkflowRuntimeResult<Value> {
        let body_text =
            serde_json::to_string(body).map_err(WorkflowRuntimeError::incompatible_runtime)?;
        self.send_request(method, path_segments, include_workspace_id, Some(body_text))
    }

    fn request_headers(
        &self,
        include_api_key: bool,
        include_workspace_id: bool,
    ) -> Vec<(String, String)> {
        let mut headers = Vec::new();
        if include_api_key {
            headers.push(("X-API-Key".to_string(), self.api_key.clone()));
        }
        if include_workspace_id {
            headers.push(("X-Workspace-ID".to_string(), self.workspace_id.clone()));
        }
        headers
    }

    fn endpoint_url(&self, path_segments: &[&str]) -> Url {
        let mut url = self.api_base_url.clone();
        {
            let mut segments = url
                .path_segments_mut()
                .expect("workflow runtime API base URL should be hierarchical");
            segments.pop_if_empty();
            for segment in path_segments {
                segments.push(segment);
            }
        }
        url
    }
}

#[cfg(test)]
#[derive(Clone)]
struct MockTransport {
    exchanges: std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<MockExchange>>>,
}

#[cfg(test)]
impl fmt::Debug for MockTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MockTransport").finish_non_exhaustive()
    }
}

#[cfg(test)]
#[derive(Debug, Clone)]
struct MockExchange {
    method: &'static str,
    path: String,
    required_headers: Vec<(String, String)>,
    forbidden_headers: Vec<String>,
    expected_body: Option<String>,
    response: MockResponse,
}

#[cfg(test)]
#[derive(Debug, Clone)]
enum MockResponse {
    Http { status: u16, body: String },
    RuntimeUnreachable(String),
}

#[cfg(test)]
impl MockTransport {
    fn new(exchanges: Vec<MockExchange>) -> Self {
        Self {
            exchanges: std::sync::Arc::new(std::sync::Mutex::new(
                std::collections::VecDeque::from(exchanges),
            )),
        }
    }

    fn send(
        &self,
        request: WorkflowRuntimePreparedRequest,
    ) -> WorkflowRuntimeResult<WorkflowRuntimeRawResponse> {
        let exchange = self
            .exchanges
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected extra workflow runtime request");
        assert_eq!(
            request.method.as_str(),
            exchange.method,
            "unexpected HTTP method"
        );
        assert_eq!(request.url.path(), exchange.path, "unexpected request path");
        let headers: BTreeMap<String, String> = request
            .headers
            .into_iter()
            .map(|(name, value)| (name.to_ascii_lowercase(), value))
            .collect();
        for (name, value) in exchange.required_headers {
            let actual = headers
                .get(&name.to_ascii_lowercase())
                .unwrap_or_else(|| panic!("missing header `{name}`"));
            assert_eq!(actual, &value, "unexpected header `{name}`");
        }
        for name in exchange.forbidden_headers {
            assert!(
                !headers.contains_key(&name.to_ascii_lowercase()),
                "header `{name}` should be absent"
            );
        }
        match exchange.expected_body {
            Some(expected_body) => assert_eq!(
                request.body.as_deref().unwrap_or(""),
                expected_body,
                "unexpected request body"
            ),
            None => assert!(
                request.body.is_none(),
                "request body should be absent for {} {}",
                exchange.method,
                exchange.path
            ),
        }
        match exchange.response {
            MockResponse::Http { status, body } => Ok(WorkflowRuntimeRawResponse {
                status: StatusCode::from_u16(status).expect("valid status"),
                body_text: body,
            }),
            MockResponse::RuntimeUnreachable(detail) => {
                Err(WorkflowRuntimeError::runtime_unreachable(detail))
            }
        }
    }

    fn assert_drained(&self) {
        assert!(
            self.exchanges.lock().unwrap().is_empty(),
            "not all mock workflow runtime requests were consumed"
        );
    }
}

fn normalized_api_base_url(base_url: &str) -> Result<Url> {
    let trimmed = base_url.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        anyhow::bail!("workflow runtime api_base_url must not be empty");
    }
    let normalized = if trimmed.ends_with("/v1") {
        format!("{trimmed}/")
    } else {
        format!("{trimmed}/v1/")
    };
    Url::parse(&normalized).with_context(|| {
        format!(
            "invalid workflow runtime api_base_url `{}`",
            base_url.trim()
        )
    })
}

fn map_http_response(
    url: Url,
    status: StatusCode,
    body_text: &str,
) -> WorkflowRuntimeResult<Value> {
    match status {
        StatusCode::UNAUTHORIZED => Err(WorkflowRuntimeError::invalid_token()),
        StatusCode::FORBIDDEN => Err(WorkflowRuntimeError::permission_denied()),
        StatusCode::NOT_FOUND => Err(WorkflowRuntimeError::workspace_inaccessible()),
        _ if !status.is_success() => Err(WorkflowRuntimeError::service_error(
            status.as_u16(),
            response_error_message(status, body_text),
        )),
        _ => serde_json::from_str(body_text).map_err(|error| {
            WorkflowRuntimeError::incompatible_runtime(format!(
                "invalid JSON from {}: {error}",
                url
            ))
        }),
    }
}

fn parse_record_list(
    payload: Value,
    endpoint: &str,
) -> WorkflowRuntimeResult<Vec<WorkflowRuntimeRecord>> {
    serde_json::from_value(enveloped_data(payload, endpoint)?).map_err(|error| {
        WorkflowRuntimeError::incompatible_runtime(format!(
            "{endpoint} returned an unexpected schema: {error}"
        ))
    })
}

fn parse_bare_record_list(
    payload: Value,
    endpoint: &str,
) -> WorkflowRuntimeResult<Vec<WorkflowRuntimeRecord>> {
    serde_json::from_value(payload).map_err(|error| {
        WorkflowRuntimeError::incompatible_runtime(format!(
            "{endpoint} returned an unexpected schema: {error}"
        ))
    })
}

fn parse_record(payload: Value, endpoint: &str) -> WorkflowRuntimeResult<WorkflowRuntimeRecord> {
    serde_json::from_value(enveloped_data(payload, endpoint)?).map_err(|error| {
        WorkflowRuntimeError::incompatible_runtime(format!(
            "{endpoint} returned an unexpected schema: {error}"
        ))
    })
}

fn parse_bare_record(
    payload: Value,
    endpoint: &str,
) -> WorkflowRuntimeResult<WorkflowRuntimeRecord> {
    let object = payload.as_object().cloned().ok_or_else(|| {
        WorkflowRuntimeError::incompatible_runtime(format!(
            "{endpoint} must return a bare JSON object"
        ))
    })?;
    if object.contains_key("data") {
        return Err(WorkflowRuntimeError::incompatible_runtime(format!(
            "{endpoint} must return a bare JSON object, not a workflow data envelope"
        )));
    }
    serde_json::from_value(Value::Object(object)).map_err(|error| {
        WorkflowRuntimeError::incompatible_runtime(format!(
            "{endpoint} returned an unexpected schema: {error}"
        ))
    })
}

fn enveloped_data(payload: Value, endpoint: &str) -> WorkflowRuntimeResult<Value> {
    let mut object = payload.as_object().cloned().ok_or_else(|| {
        WorkflowRuntimeError::incompatible_runtime(format!(
            "{endpoint} must return a JSON object envelope"
        ))
    })?;
    object.remove("data").ok_or_else(|| {
        WorkflowRuntimeError::incompatible_runtime(format!(
            "{endpoint} response envelope is missing `data`"
        ))
    })
}

fn response_error_message(status: StatusCode, body_text: &str) -> String {
    if let Some(message) = extract_error_message(body_text) {
        return message;
    }
    format!("HTTP {}", status.as_u16())
}

fn extract_error_message(body_text: &str) -> Option<String> {
    let trimmed = body_text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let parsed = serde_json::from_str::<Value>(trimmed).ok();
    if let Some(message) = parsed.as_ref().and_then(json_error_message) {
        return Some(message);
    }
    Some(trimmed.to_string())
}

fn json_error_message(value: &Value) -> Option<String> {
    let object = value.as_object()?;
    for key in ["message", "detail"] {
        if let Some(message) = object.get(key).and_then(Value::as_str) {
            return Some(message.to_string());
        }
    }
    let error = object.get("error")?;
    if let Some(message) = error.as_str() {
        return Some(message.to_string());
    }
    error
        .as_object()
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

#[cfg(test)]
mod tests;

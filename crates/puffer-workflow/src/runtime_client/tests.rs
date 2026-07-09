use super::*;
use serde_json::json;

#[test]
fn runtime_calls_share_one_client_and_normalize_v1_paths() {
    let workflow_id = "wf-123";
    let execution_id = "exec-1";
    let create_request: WorkflowRuntimeCreateWorkflowRequest = serde_json::from_value(json!({
        "name": "Puffer smoke",
        "description": "Puffer runtime smoke test.",
        "definition": {
            "nodes": [
                {
                    "id": "smoke-noop",
                    "type": "noop",
                    "name": "Smoke noop",
                    "config": {},
                    "trusted": false,
                    "position": {"x": 0, "y": 0}
                }
            ],
            "edges": []
        }
    }))
    .expect("create request");
    let create_body = serde_json::to_string(&create_request).expect("create body");
    let update_request: WorkflowRuntimeUpdateWorkflowRequest = serde_json::from_value(json!({
        "name": "Puffer smoke edited",
        "definition": {
            "nodes": [
                {
                    "id": "smoke-noop",
                    "type": "noop",
                    "name": "Smoke noop",
                    "config": {},
                    "position": {"x": 12, "y": 34}
                }
            ],
            "edges": []
        }
    }))
    .expect("update request");
    let update_body = serde_json::to_string(&update_request).expect("update body");
    let execute_request: WorkflowRuntimeExecuteRequest = serde_json::from_value(json!({
        "input": {"manual": true}
    }))
    .expect("execute request");
    let execute_body = serde_json::to_string(&execute_request).expect("execute body");
    let in_memory_request: WorkflowRuntimeInMemoryExecuteRequest = serde_json::from_value(json!({
        "definition": {
            "nodes": [
                {
                    "id": "smoke-noop",
                    "type": "noop",
                    "name": "Smoke noop",
                    "config": {},
                    "position": {"x": 0, "y": 0}
                }
            ],
            "edges": []
        },
        "input": {"manual": true},
        "triggerNodeId": "smoke-noop"
    }))
    .expect("in-memory request");
    let in_memory_body = serde_json::to_string(&in_memory_request).expect("in-memory body");
    let transport = MockTransport::new(vec![
        MockExchange {
            method: "GET",
            path: "/v1/auth/api-key-context".to_string(),
            required_headers: vec![("x-api-key".to_string(), "token-123".to_string())],
            forbidden_headers: vec!["x-workspace-id".to_string()],
            expected_body: None,
            response: MockResponse::Http {
                status: 200,
                body: r#"{"id":"key-123","type":"workspace","workspaceId":"ws-123"}"#.to_string(),
            },
        },
        MockExchange {
            method: "GET",
            path: "/v1/workflows/node-definitions".to_string(),
            required_headers: vec![("x-api-key".to_string(), "token-123".to_string())],
            forbidden_headers: vec!["x-workspace-id".to_string()],
            expected_body: None,
            response: MockResponse::Http {
                status: 200,
                body: r#"{"data":[{"id":"node-a"}]}"#.to_string(),
            },
        },
        MockExchange {
            method: "GET",
            path: "/v1/workflows/node-definitions/noop".to_string(),
            required_headers: vec![("x-api-key".to_string(), "token-123".to_string())],
            forbidden_headers: vec!["x-workspace-id".to_string()],
            expected_body: None,
            response: MockResponse::Http {
                status: 200,
                body: r#"{"data":{"type":"noop","name":"Noop","schemas":{"config":{}}}}"#.to_string(),
            },
        },
        MockExchange {
            method: "GET",
            path: "/v1/workflows".to_string(),
            required_headers: workspace_headers(),
            forbidden_headers: Vec::new(),
            expected_body: None,
            response: MockResponse::Http {
                status: 200,
                body: r#"{"data":[{"id":"wf-123"}]}"#.to_string(),
            },
        },
        MockExchange {
            method: "POST",
            path: "/v1/workflows".to_string(),
            required_headers: workspace_headers(),
            forbidden_headers: Vec::new(),
            expected_body: Some(create_body),
            response: MockResponse::Http {
                status: 200,
                body: format!(
                    r#"{{"data":{{"id":"{workflow_id}","workspaceId":"ws-123","name":"Puffer smoke","description":null,"version":1,"status":"draft","definition":{{}},"publishedDefinition":null,"publishedAt":null,"createdAt":"2026-06-18T00:00:00Z","updatedAt":"2026-06-18T00:00:00Z","webhookEndpoints":[]}}}}"#
                ),
            },
        },
        MockExchange {
            method: "GET",
            path: format!("/v1/workflows/{workflow_id}"),
            required_headers: workspace_headers(),
            forbidden_headers: Vec::new(),
            expected_body: None,
            response: MockResponse::Http {
                status: 200,
                body: format!(r#"{{"data":{{"id":"{workflow_id}","name":"Demo"}}}}"#),
            },
        },
        MockExchange {
            method: "PUT",
            path: format!("/v1/workflows/{workflow_id}"),
            required_headers: workspace_headers(),
            forbidden_headers: Vec::new(),
            expected_body: Some(update_body),
            response: MockResponse::Http {
                status: 200,
                body: format!(r#"{{"data":{{"id":"{workflow_id}","name":"Puffer smoke edited","definition":{{"nodes":[],"edges":[]}}}}}}"#),
            },
        },
        MockExchange {
            method: "POST",
            path: format!("/v1/workflows/{workflow_id}/deploy"),
            required_headers: workspace_headers(),
            forbidden_headers: Vec::new(),
            expected_body: None,
            response: MockResponse::Http {
                status: 200,
                body: format!(
                    r#"{{"data":{{"id":"{workflow_id}","workspaceId":"ws-123","name":"Puffer smoke","description":null,"version":2,"status":"active","definition":{{}},"publishedDefinition":{{}},"publishedAt":"2026-06-18T00:00:00Z","createdAt":"2026-06-18T00:00:00Z","updatedAt":"2026-06-18T00:00:00Z","webhookEndpoints":[]}}}}"#
                ),
            },
        },
        MockExchange {
            method: "POST",
            path: format!("/v1/workflows/{workflow_id}/undeploy"),
            required_headers: workspace_headers(),
            forbidden_headers: Vec::new(),
            expected_body: None,
            response: MockResponse::Http {
                status: 200,
                body: format!(
                    r#"{{"data":{{"id":"{workflow_id}","workspaceId":"ws-123","name":"Puffer smoke","description":null,"version":2,"status":"draft","definition":{{}},"publishedDefinition":null,"publishedAt":null,"createdAt":"2026-06-18T00:00:00Z","updatedAt":"2026-06-18T00:00:00Z","webhookEndpoints":[]}}}}"#
                ),
            },
        },
        MockExchange {
            method: "POST",
            path: format!("/v1/workflows/{workflow_id}/execute"),
            required_headers: workspace_headers(),
            forbidden_headers: Vec::new(),
            expected_body: Some(execute_body),
            response: MockResponse::Http {
                status: 200,
                body: r#"{"data":{"executionId":"exec-1"}}"#.to_string(),
            },
        },
        MockExchange {
            method: "GET",
            path: format!("/v1/workflows/{workflow_id}/executions"),
            required_headers: workspace_headers(),
            forbidden_headers: Vec::new(),
            expected_body: None,
            response: MockResponse::Http {
                status: 200,
                body: r#"{"data":[{"id":"exec-1","status":"pending"}]}"#.to_string(),
            },
        },
        MockExchange {
            method: "GET",
            path: format!("/v1/workflows/{workflow_id}/executions/{execution_id}"),
            required_headers: workspace_headers(),
            forbidden_headers: Vec::new(),
            expected_body: None,
            response: MockResponse::Http {
                status: 200,
                body: r#"{"data":{"id":"exec-1","workflowId":"wf-123","workflowVersion":2,"status":"completed","input":{},"output":null,"nodeOutputs":{},"nodeErrors":{},"startedAt":null,"completedAt":null,"createdAt":"2026-06-18T00:00:00Z","error":null}}"#.to_string(),
            },
        },
        MockExchange {
            method: "POST",
            path: "/v1/workflows/execute-in-memory".to_string(),
            required_headers: workspace_headers(),
            forbidden_headers: Vec::new(),
            expected_body: Some(in_memory_body),
            response: MockResponse::Http {
                status: 200,
                body: r#"{"data":{"status":"completed","output":{"ok":true},"nodeOutputs":{},"nodeLogs":{},"duration":5,"startedAt":"2026-06-18T00:00:00Z","completedAt":"2026-06-18T00:00:00Z"}}"#.to_string(),
            },
        },
    ]);
    let client = mock_client("http://runtime.test/v1", transport.clone());

    let context = client.api_key_context().expect("api key context");
    assert_eq!(
        context.get("workspaceId").and_then(Value::as_str),
        Some("ws-123")
    );

    let node_definitions = client.list_node_definitions().expect("node definitions");
    assert_eq!(
        node_definitions[0].get("id").and_then(Value::as_str),
        Some("node-a")
    );

    let node_definition = client.get_node_definition("noop").expect("node detail");
    assert_eq!(
        node_definition.get("type").and_then(Value::as_str),
        Some("noop")
    );

    let workflows = client.list_workflows().expect("workflows");
    assert_eq!(
        workflows[0].get("id").and_then(Value::as_str),
        Some("wf-123")
    );

    let created = client
        .create_workflow(&create_request)
        .expect("create workflow");
    assert_eq!(created.get("id").and_then(Value::as_str), Some(workflow_id));

    let workflow = client.get_workflow(workflow_id).expect("workflow");
    assert_eq!(workflow.get("name").and_then(Value::as_str), Some("Demo"));

    let updated = client
        .update_workflow(workflow_id, &update_request)
        .expect("update workflow");
    assert_eq!(
        updated.get("name").and_then(Value::as_str),
        Some("Puffer smoke edited")
    );

    let deployed = client
        .deploy_workflow(workflow_id)
        .expect("deploy workflow");
    assert_eq!(
        deployed.get("status").and_then(Value::as_str),
        Some("active")
    );

    let undeployed = client
        .undeploy_workflow(workflow_id)
        .expect("undeploy workflow");
    assert_eq!(
        undeployed.get("status").and_then(Value::as_str),
        Some("draft")
    );

    let execute_response = client
        .execute_workflow(workflow_id, &execute_request)
        .expect("execute");
    assert_eq!(
        execute_response.get("executionId").and_then(Value::as_str),
        Some("exec-1")
    );

    let executions = client.list_executions(workflow_id).expect("executions");
    assert_eq!(
        executions[0].get("status").and_then(Value::as_str),
        Some("pending")
    );

    let execution = client
        .get_execution(workflow_id, execution_id)
        .expect("execution");
    assert_eq!(
        execution.get("status").and_then(Value::as_str),
        Some("completed")
    );

    let in_memory = client
        .execute_in_memory(&in_memory_request)
        .expect("execute in memory");
    assert_eq!(
        in_memory.get("status").and_then(Value::as_str),
        Some("completed")
    );
    transport.assert_drained();
}

#[test]
fn api_key_context_accepts_bare_object_response() {
    let transport = MockTransport::new(vec![MockExchange {
        method: "GET",
        path: "/v1/auth/api-key-context".to_string(),
        required_headers: vec![("x-api-key".to_string(), "token-123".to_string())],
        forbidden_headers: vec!["x-workspace-id".to_string()],
        expected_body: None,
        response: MockResponse::Http {
            status: 200,
            body: r#"{"workspaceId":"ws-123","scope":"workspace"}"#.to_string(),
        },
    }]);
    let client = mock_client("http://runtime.test", transport.clone());

    let context = client.api_key_context().expect("api key context");

    assert_eq!(
        context.get("workspaceId").and_then(Value::as_str),
        Some("ws-123")
    );
    assert_eq!(
        context.get("scope").and_then(Value::as_str),
        Some("workspace")
    );
    transport.assert_drained();
}

#[test]
fn api_key_context_rejects_workflow_envelope_response() {
    let transport = MockTransport::new(vec![MockExchange {
        method: "GET",
        path: "/v1/auth/api-key-context".to_string(),
        required_headers: vec![("x-api-key".to_string(), "token-123".to_string())],
        forbidden_headers: vec!["x-workspace-id".to_string()],
        expected_body: None,
        response: MockResponse::Http {
            status: 200,
            body: r#"{"data":{"workspaceId":"ws-123","scope":"workspace"}}"#.to_string(),
        },
    }]);
    let client = mock_client("http://runtime.test", transport.clone());

    let error = client
        .api_key_context()
        .expect_err("auth context must not accept workflow envelopes");

    assert_eq!(error.kind, WorkflowRuntimeErrorKind::IncompatibleRuntime);
    assert!(error.message.contains("bare JSON object"));
    transport.assert_drained();
}

#[test]
fn gateway_upstream_calls_use_user_scoped_api_without_workspace_header() {
    let create_request = json!({
        "name": "Puffer OpenAI",
        "providerType": "openai-api",
        "credentials": {
            "apiKey": "sk-test",
            "baseUrl": "https://api.openai.com",
            "defaultModel": "gpt-5.4",
            "preferredOpenAIEndpoint": "responses",
            "supportedModels": ["gpt-5.4"]
        }
    });
    let create_body = serde_json::to_string(&create_request).expect("create upstream body");
    let update_request = json!({
        "name": "Puffer OpenAI",
        "providerType": "openai-api",
        "credentials": {
            "defaultModel": "gpt-5.4",
            "supportedModels": ["gpt-5.4"]
        }
    });
    let update_body = serde_json::to_string(&update_request).expect("update upstream body");
    let transport = MockTransport::new(vec![
        MockExchange {
            method: "GET",
            path: "/v1/ai-gateway/upstreams".to_string(),
            required_headers: vec![("x-api-key".to_string(), "token-123".to_string())],
            forbidden_headers: vec!["x-workspace-id".to_string()],
            expected_body: None,
            response: MockResponse::Http {
                status: 200,
                body: r#"[{"id":"upstream-1","name":"Puffer OpenAI"}]"#.to_string(),
            },
        },
        MockExchange {
            method: "POST",
            path: "/v1/ai-gateway/upstreams".to_string(),
            required_headers: vec![("x-api-key".to_string(), "token-123".to_string())],
            forbidden_headers: vec!["x-workspace-id".to_string()],
            expected_body: Some(create_body),
            response: MockResponse::Http {
                status: 200,
                body: r#"{"id":"upstream-1","name":"Puffer OpenAI"}"#.to_string(),
            },
        },
        MockExchange {
            method: "PUT",
            path: "/v1/ai-gateway/upstreams/upstream-1".to_string(),
            required_headers: vec![("x-api-key".to_string(), "token-123".to_string())],
            forbidden_headers: vec!["x-workspace-id".to_string()],
            expected_body: Some(update_body),
            response: MockResponse::Http {
                status: 200,
                body: r#"{"success":true}"#.to_string(),
            },
        },
    ]);
    let client = mock_client("http://runtime.test", transport.clone());

    let upstreams = client
        .list_gateway_upstreams()
        .expect("list gateway upstreams");
    assert_eq!(
        upstreams[0].get("id").and_then(Value::as_str),
        Some("upstream-1")
    );

    let created = client
        .create_gateway_upstream(&create_request)
        .expect("create gateway upstream");
    assert_eq!(
        created.get("id").and_then(Value::as_str),
        Some("upstream-1")
    );

    let updated = client
        .update_gateway_upstream("upstream-1", &update_request)
        .expect("update gateway upstream");
    assert_eq!(updated.get("success").and_then(Value::as_bool), Some(true));
    transport.assert_drained();
}

#[test]
fn test_connection_reports_two_success_steps() {
    let transport = MockTransport::new(vec![
        MockExchange {
            method: "GET",
            path: "/v1/health/ready".to_string(),
            required_headers: Vec::new(),
            forbidden_headers: vec!["x-api-key".to_string(), "x-workspace-id".to_string()],
            expected_body: None,
            response: MockResponse::Http {
                status: 200,
                body: r#"{"status":"ready"}"#.to_string(),
            },
        },
        MockExchange {
            method: "GET",
            path: "/v1/workflows/node-definitions".to_string(),
            required_headers: vec![("x-api-key".to_string(), "token-123".to_string())],
            forbidden_headers: vec!["x-workspace-id".to_string()],
            expected_body: None,
            response: MockResponse::Http {
                status: 200,
                body: r#"{"data":[{"id":"node-a"},{"id":"node-b"}]}"#.to_string(),
            },
        },
        MockExchange {
            method: "GET",
            path: "/v1/workflows".to_string(),
            required_headers: workspace_headers(),
            forbidden_headers: Vec::new(),
            expected_body: None,
            response: MockResponse::Http {
                status: 200,
                body: r#"{"data":[{"id":"wf-123"}]}"#.to_string(),
            },
        },
    ]);
    let client = mock_client("http://runtime.test", transport.clone());

    let result = client.test_connection();

    assert!(result.is_success());
    assert_eq!(
        result.ready.state,
        WorkflowRuntimeConnectionStepState::Passed
    );
    assert_eq!(
        result.api_surface.state,
        WorkflowRuntimeConnectionStepState::Passed
    );
    assert_eq!(result.api_surface.item_count, Some(2));
    assert_eq!(
        result.workspace_access.state,
        WorkflowRuntimeConnectionStepState::Passed
    );
    assert_eq!(result.workspace_access.item_count, Some(1));
    transport.assert_drained();
}

#[test]
fn test_connection_maps_401_to_invalid_token_and_skips_workspace_phase() {
    let transport = MockTransport::new(vec![
        MockExchange {
            method: "GET",
            path: "/v1/health/ready".to_string(),
            required_headers: Vec::new(),
            forbidden_headers: vec!["x-api-key".to_string(), "x-workspace-id".to_string()],
            expected_body: None,
            response: MockResponse::Http {
                status: 200,
                body: r#"{"status":"ready"}"#.to_string(),
            },
        },
        MockExchange {
            method: "GET",
            path: "/v1/workflows/node-definitions".to_string(),
            required_headers: vec![("x-api-key".to_string(), "token-123".to_string())],
            forbidden_headers: vec!["x-workspace-id".to_string()],
            expected_body: None,
            response: MockResponse::Http {
                status: 401,
                body: "nope".to_string(),
            },
        },
    ]);
    let client = mock_client("http://runtime.test", transport.clone());

    let result = client.test_connection();

    assert_eq!(
        result.api_surface.state,
        WorkflowRuntimeConnectionStepState::Failed
    );
    assert_eq!(
        result.api_surface.error.as_ref().map(|error| error.kind),
        Some(WorkflowRuntimeErrorKind::InvalidToken)
    );
    assert_eq!(
        result.workspace_access.state,
        WorkflowRuntimeConnectionStepState::Skipped
    );
    transport.assert_drained();
}

#[test]
fn list_workflows_maps_403_to_permission_denied() {
    let transport = MockTransport::new(vec![MockExchange {
        method: "GET",
        path: "/v1/workflows".to_string(),
        required_headers: workspace_headers(),
        forbidden_headers: Vec::new(),
        expected_body: None,
        response: MockResponse::Http {
            status: 403,
            body: "forbidden".to_string(),
        },
    }]);
    let client = mock_client("http://runtime.test", transport.clone());

    let error = client.list_workflows().expect_err("403 should fail");

    assert_eq!(error.kind, WorkflowRuntimeErrorKind::PermissionDenied);
    transport.assert_drained();
}

#[test]
fn test_connection_maps_404_to_workspace_inaccessible() {
    let transport = MockTransport::new(vec![
        MockExchange {
            method: "GET",
            path: "/v1/health/ready".to_string(),
            required_headers: Vec::new(),
            forbidden_headers: vec!["x-api-key".to_string(), "x-workspace-id".to_string()],
            expected_body: None,
            response: MockResponse::Http {
                status: 200,
                body: r#"{"status":"ready"}"#.to_string(),
            },
        },
        MockExchange {
            method: "GET",
            path: "/v1/workflows/node-definitions".to_string(),
            required_headers: vec![("x-api-key".to_string(), "token-123".to_string())],
            forbidden_headers: vec!["x-workspace-id".to_string()],
            expected_body: None,
            response: MockResponse::Http {
                status: 200,
                body: r#"{"data":[{"id":"node-a"}]}"#.to_string(),
            },
        },
        MockExchange {
            method: "GET",
            path: "/v1/workflows".to_string(),
            required_headers: workspace_headers(),
            forbidden_headers: Vec::new(),
            expected_body: None,
            response: MockResponse::Http {
                status: 404,
                body: "workspace missing".to_string(),
            },
        },
    ]);
    let client = mock_client("http://runtime.test", transport.clone());

    let result = client.test_connection();

    assert_eq!(
        result.api_surface.state,
        WorkflowRuntimeConnectionStepState::Passed
    );
    assert_eq!(
        result
            .workspace_access
            .error
            .as_ref()
            .map(|error| error.kind),
        Some(WorkflowRuntimeErrorKind::WorkspaceInaccessible)
    );
    transport.assert_drained();
}

#[test]
fn execute_workflow_surfaces_server_errors() {
    let execute_request: WorkflowRuntimeExecuteRequest =
        serde_json::from_value(json!({"input": {}})).expect("execute request");
    let execute_body = serde_json::to_string(&execute_request).expect("execute body");
    let transport = MockTransport::new(vec![MockExchange {
        method: "POST",
        path: "/v1/workflows/wf-123/execute".to_string(),
        required_headers: workspace_headers(),
        forbidden_headers: Vec::new(),
        expected_body: Some(execute_body),
        response: MockResponse::Http {
            status: 500,
            body: r#"{"message":"workflow must be deployed"}"#.to_string(),
        },
    }]);
    let client = mock_client("http://runtime.test", transport.clone());

    let error = client
        .execute_workflow("wf-123", &execute_request)
        .expect_err("500 should fail");

    assert_eq!(error.kind, WorkflowRuntimeErrorKind::ServiceError);
    assert_eq!(error.status_code, Some(500));
    assert!(error.message.contains("workflow must be deployed"));
    transport.assert_drained();
}

#[test]
fn list_node_definitions_rejects_invalid_json() {
    let transport = MockTransport::new(vec![MockExchange {
        method: "GET",
        path: "/v1/workflows/node-definitions".to_string(),
        required_headers: vec![("x-api-key".to_string(), "token-123".to_string())],
        forbidden_headers: vec!["x-workspace-id".to_string()],
        expected_body: None,
        response: MockResponse::Http {
            status: 200,
            body: "not-json".to_string(),
        },
    }]);
    let client = mock_client("http://runtime.test", transport.clone());

    let error = client
        .list_node_definitions()
        .expect_err("invalid JSON should fail");

    assert_eq!(error.kind, WorkflowRuntimeErrorKind::IncompatibleRuntime);
    transport.assert_drained();
}

#[test]
fn list_workflows_rejects_invalid_schema() {
    let transport = MockTransport::new(vec![MockExchange {
        method: "GET",
        path: "/v1/workflows".to_string(),
        required_headers: workspace_headers(),
        forbidden_headers: Vec::new(),
        expected_body: None,
        response: MockResponse::Http {
            status: 200,
            body: r#"{"workflows":[]}"#.to_string(),
        },
    }]);
    let client = mock_client("http://runtime.test", transport.clone());

    let error = client
        .list_workflows()
        .expect_err("invalid schema should fail");

    assert_eq!(error.kind, WorkflowRuntimeErrorKind::IncompatibleRuntime);
    transport.assert_drained();
}

#[test]
fn list_executions_maps_timeouts_to_runtime_unreachable() {
    let transport = MockTransport::new(vec![MockExchange {
        method: "GET",
        path: "/v1/workflows/wf-123/executions".to_string(),
        required_headers: workspace_headers(),
        forbidden_headers: Vec::new(),
        expected_body: None,
        response: MockResponse::RuntimeUnreachable("operation timed out".to_string()),
    }]);
    let client = mock_client("http://runtime.test", transport.clone());

    let error = client
        .list_executions("wf-123")
        .expect_err("timeout should fail");

    assert_eq!(error.kind, WorkflowRuntimeErrorKind::RuntimeUnreachable);
    transport.assert_drained();
}

fn mock_client(base_url: &str, transport: MockTransport) -> WorkflowRuntimeClient {
    WorkflowRuntimeClient {
        transport: WorkflowRuntimeTransport::Mock(transport),
        api_base_url: normalized_api_base_url(base_url).expect("base URL"),
        api_key: "token-123".to_string(),
        workspace_id: "ws-123".to_string(),
    }
}

fn workspace_headers() -> Vec<(String, String)> {
    vec![
        ("x-api-key".to_string(), "token-123".to_string()),
        ("x-workspace-id".to_string(), "ws-123".to_string()),
    ]
}

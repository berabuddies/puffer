import { beforeEach, expect, test, vi } from "vitest";

const tauriHandshake = {
  url: "tauri://puffer",
  token: "",
  protocolVersion: "1",
  workspaceRoot: "/tmp/puffer"
};

beforeEach(() => {
  vi.resetModules();
  vi.doUnmock("@tauri-apps/api/core");
  vi.doUnmock("@tauri-apps/api/event");
  vi.doUnmock("./daemonClient");
});

function mockTauriModules() {
  const invoke = vi.fn();
  const listen = vi.fn();
  vi.doMock("@tauri-apps/api/core", () => ({ invoke }));
  vi.doMock("@tauri-apps/api/event", () => ({ listen }));
  return { invoke, listen };
}

function mockDesktopDaemonClient() {
  const invoke = vi.fn();
  const request = vi.fn().mockResolvedValue({});
  vi.doMock("@tauri-apps/api/core", () => ({ invoke }));
  vi.doMock("./daemonClient", () => ({
    canInvokeTauri: () => true,
    canReachDaemon: () => true,
    configuredBrowserRemoteDaemonHandshake: () => null,
    ensureLocalDaemonClient: async () => ({
      request
    }),
    switchDaemonClient: vi.fn()
  }));
  return { invoke, request };
}

test("keeps non-WebSocket requests on the Tauri backend fallback by default", async () => {
  const { invoke } = mockTauriModules();
  invoke.mockResolvedValueOnce({ ok: true });
  const { DaemonClient } = await import("./daemonClient");
  const client = new DaemonClient(tauriHandshake);

  await expect(client.request("load_settings_snapshot")).resolves.toEqual({ ok: true });

  expect(invoke).toHaveBeenCalledWith("backend_request", {
    method: "load_settings_snapshot",
    params: {}
  });
});

test("rejects daemon-only requests before Tauri backend fallback", async () => {
  const { invoke } = mockTauriModules();
  const { DaemonClient } = await import("./daemonClient");
  const client = new DaemonClient(tauriHandshake);

  await expect(
    client.request("workflow_list", {}, { requireWebSocket: true })
  ).rejects.toThrow("requires a WebSocket daemon connection");

  expect(invoke).not.toHaveBeenCalled();
});

test("sends duplicate risk acknowledgement only for explicit outbound retries", async () => {
  const { request } = mockDesktopDaemonClient();
  request.mockResolvedValueOnce({ status: "sent", actionId: "action-1", receipt: { ok: true } });
  const api = await import("./desktop");

  await api.executeOutboundAction({
    actionId: "action-1",
    version: 5,
    approvedMessage: "Approved text",
    clientRequestId: "client-ack",
    duplicateRiskAck: true
  });

  expect(request).toHaveBeenCalledWith(
    "outbound_action_execute",
    {
      action_id: "action-1",
      version: 5,
      approved_message: "Approved text",
      client_request_id: "client-ack",
      duplicate_risk_ack: true
    }
  );
});

test("marks automation and workflow runtime API calls as daemon-only", async () => {
  const { invoke, request } = mockDesktopDaemonClient();
  const api = await import("./desktop");
  const automationSpec = {
    spec_version: 1,
    name: "Automation 1",
    source: { type: "blank" as const },
    instructions: "Run the automation.",
    triggers: [
      {
        type: "agent_env_node" as const,
        id: "trigger-1",
        node: {
          node_type: "webhook",
          name: "Webhook",
          trusted: false,
          config: { path: "automation-1", methods: ["POST"], authentication: "none" }
        }
      }
    ],
    flow: {
      steps: [
        {
          type: "agent_env_node" as const,
          id: "agent",
          node: { node_type: "transform_js", config: { code: "return input;" } }
        }
      ]
    },
    review: { human_approval_required: true }
  };

  await api.listAutomations();
  await api.getAutomation("automation-1");
  await api.saveAutomationRecord({
    id: "automation-1",
    expectedRevision: 1,
    status: "paused",
    spec: automationSpec
  });
  await api.activateAutomationRecord("automation-1", 1);
  await api.deleteAutomationRecord("automation-1");
  await api.listAutomationPendingActions();
  await api.getAutomationPendingAction("draft-1", 1);
  await api.rejectAutomationPendingAction({ draftId: "draft-1", version: 1, reason: "No longer needed" });
  await api.loadWorkflowBackendConfig();
  await api.saveWorkflowBackendConfig({
    mode: "agent_env_cloud",
    apiUrl: "https://api.agentenv.test",
    uiUrl: "https://agentenv.test",
    workspaceId: "workspace-1",
    keepToken: true
  });
  await api.testWorkflowBackendConnection();
  await api.repairWorkflowBackendLocalRuntime();
  await api.loadWorkflowSnapshot();
  await api.openWorkflowConsole();
  await api.listWorkflowNodeDefinitions();
  await api.getWorkflowNodeDefinition("webhook");
  await api.createRuntimeWorkflow({
    name: "Workflow 1",
    definition: { nodes: [], edges: [] }
  });
  await api.updateRuntimeWorkflow("workflow-1", {
    definition: { nodes: [], edges: [] }
  });
  await api.deployRuntimeWorkflow("workflow-1");
  await api.undeployRuntimeWorkflow("workflow-1");
  await api.executeRuntimeWorkflow("workflow-1", { input: { ok: true } });
  await api.executeWorkflowInMemory({
    definition: { nodes: [], edges: [] },
    input: { ok: true }
  });
  await api.listWorkflowExecutions("workflow-1");
  await api.getWorkflowExecution("workflow-1", "execution-1");
  await api.createWorkflowBinding({
    connection_slug: "telegram-user",
    file_append_path: "/tmp/workflow.md"
  });
  await api.createMonitor("telegram-user", "gpt-5", ["contact-1"]);
  await api.ignoreMonitorTask("task-1", "done");
  await api.saveMonitorMemory("telegram-user", "remember this");
  await api.addMonitorRule({
    connection_slug: "telegram-user",
    mode: "include",
    kind: "keyword",
    keywords: ["urgent"]
  });
  await api.deleteMonitorRule("telegram-user", "include", { keywords: ["urgent"] });
  await api.loadMonitorHistory(5);
  await api.deleteWorkflowBinding("monitor-telegram-user");
  await api.deleteWorkflowConnection("telegram-user");
  await api.toggleWorkflow("monitor-telegram-user", false);

  const workflowOptions = { requireWebSocket: true, timeoutMs: 120000 };
  expect(request.mock.calls.map(([method, _params, options]) => [method, options])).toEqual([
    ["automation_list", workflowOptions],
    ["automation_get", workflowOptions],
    ["automation_save", workflowOptions],
    ["automation_compile_deploy", workflowOptions],
    ["automation_delete", workflowOptions],
    ["automation_pending_action_list", workflowOptions],
    ["automation_pending_action_get", workflowOptions],
    ["automation_pending_action_reject", workflowOptions],
    ["workflow_backend_get_config", workflowOptions],
    ["workflow_backend_save_config", workflowOptions],
    ["workflow_backend_test_connection", workflowOptions],
    ["workflow_backend_repair_local_runtime", workflowOptions],
    ["workflow_list", workflowOptions],
    ["workflow_open_ui", workflowOptions],
    ["workflow_node_definitions", workflowOptions],
    ["workflow_node_definition", workflowOptions],
    ["workflow_create", workflowOptions],
    ["workflow_update", workflowOptions],
    ["workflow_deploy", workflowOptions],
    ["workflow_undeploy", workflowOptions],
    ["workflow_execute", workflowOptions],
    ["workflow_execute_in_memory", workflowOptions],
    ["workflow_list_executions", workflowOptions],
    ["workflow_get_execution", workflowOptions],
    ["workflow_binding_create", workflowOptions],
    ["task_monitor_create", workflowOptions],
    ["task_monitor_ignore", workflowOptions],
    ["task_monitor_memory_save", workflowOptions],
    ["task_monitor_rule_add", workflowOptions],
    ["task_monitor_rule_delete", workflowOptions],
    ["task_monitor_history_list", workflowOptions],
    ["workflow_binding_delete", workflowOptions],
    ["workflow_connection_delete", workflowOptions],
    ["workflow_toggle", workflowOptions]
  ]);
  expect(invoke).not.toHaveBeenCalled();
});

test("routes outbound action approval APIs to unified daemon RPCs", async () => {
  const { invoke, request } = mockDesktopDaemonClient();
  request
    .mockResolvedValueOnce({ status: "sent", actionId: "action-1", receipt: { ok: true } })
    .mockResolvedValueOnce({ status: "draft_ready", actionId: "action-1", version: 2 })
    .mockResolvedValueOnce({ status: "cancelled", actionId: "action-1" });
  const api = await import("./desktop");

  await api.executeOutboundAction({
    actionId: "action-1",
    version: 2,
    approvedMessage: "Approved text",
    clientRequestId: "client-1"
  });
  await api.outboundActionStatus({ actionId: "action-1", version: 2 });
  await api.cancelOutboundAction({ actionId: "action-1", version: 2, reason: "user" });

  expect(request.mock.calls.map(([method, params]) => [method, params])).toEqual([
    [
      "outbound_action_execute",
      {
        action_id: "action-1",
        version: 2,
        approved_message: "Approved text",
        client_request_id: "client-1"
      }
    ],
    [
      "outbound_action_status",
      {
        action_id: "action-1",
        version: 2
      }
    ],
    [
      "outbound_action_cancel",
      {
        action_id: "action-1",
        version: 2,
        reason: "user"
      }
    ]
  ]);
  expect(invoke).not.toHaveBeenCalled();
});

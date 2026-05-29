<script lang="ts">
  import "../design/workflow.css";

  import { onMount } from "svelte";
  import {
    createWorkflowBinding,
    deleteWorkflowBinding,
    loadWorkflowSnapshot,
    saveWorkflow,
    toggleWorkflow
  } from "../api/desktop";
  import Icon, { type IconName } from "../design/Icon.svelte";
  import Puffer from "../design/Puffer.svelte";
  import type {
    WorkflowConnection,
    WorkflowBinding,
    WorkflowConnector,
    WorkflowDefinition,
    WorkflowMonitorTask,
    WorkflowMonitorTaskAction,
    WorkflowPipelineNode,
    WorkflowRun,
    WorkflowRunNode,
    WorkflowRunStatus,
    WorkflowSnapshot
  } from "../types";

  type AgentProvider = "codex" | "claude" | "puffer";
  type NodeKind = AgentProvider | "tool" | "merge" | "fanout";
  type TriggerMode = "subscription" | "connection" | "cron";
  type EditablePipelineNode = WorkflowPipelineNode & { type: NodeKind };
  type EditableWorkflow = Omit<WorkflowDefinition, "pipeline"> & {
    pipeline: Omit<WorkflowDefinition["pipeline"], "nodes"> & { nodes: EditablePipelineNode[] };
  };

  const TOOL_NAMES = [
    "Bash",
    "Read",
    "Write",
    "Edit",
    "Grep",
    "Glob",
    "WebFetch",
    "WebSearch",
    "NotebookEdit",
    "TodoWrite",
    "ExitPlanMode"
  ] as const;
  type ToolName = (typeof TOOL_NAMES)[number];

  type ProviderMeta = {
    id: AgentProvider;
    label: string;
    short: string;
    description: string;
    defaultAgent: string;
    defaultModel: string;
    accent: string;
  };

  type GraphNode = {
    id: string;
    type: "trigger" | "agent" | "tool" | "merge" | "fanout";
    title: string;
    subtitle: string;
    provider?: AgentProvider;
    node?: EditablePipelineNode;
  };

  type GraphEdge = {
    from: string;
    to: string;
    label?: string;
  };

  type NodePosition = {
    x: number;
    y: number;
  };

  type NodeDragState = {
    nodeId: string;
    origin: NodePosition;
    pointer: NodePosition;
  };

  type EdgeDraftState = {
    fromId: string;
    x: number;
    y: number;
  };

  type ConnectorSearchRow = {
    connector: WorkflowConnector;
    searchText: string;
  };

  type ConnectionSearchRow = {
    connection: WorkflowConnection;
    searchText: string;
  };

  type MonitorTaskSearchRow = {
    task: WorkflowMonitorTask;
    searchText: string;
  };

  type WorkflowBindingSearchRow = {
    binding: WorkflowBinding;
    searchText: string;
  };

  type WorkflowSearchRow = {
    workflow: EditableWorkflow;
    searchText: string;
  };

  type WorkflowRunSearchRow = {
    run: WorkflowRun;
    searchText: string;
  };

  type WorkflowRunStats = {
    runs: number;
    success: number;
    failure: number;
  };

  type WorkflowDraftSource = {
    slugBase?: string;
    name?: string;
    connectionSlug?: string;
    connectorSlug?: string;
    connectionName?: string;
    pattern?: string;
    saveMessage?: string;
  };

  type AppendQueryIntent = {
    path: string;
    pattern: string | null;
  };

  type ConnectorFilterPreset = {
    label: string;
    query: string;
  };

  type Props = {
    onRunWorkflowCommand?: (command: string) => boolean | Promise<boolean>;
  };

  type WorkflowPage = "overview" | "detail" | "create";

  let { onRunWorkflowCommand }: Props = $props();

  const providerOptions: ProviderMeta[] = [
    {
      id: "codex",
      label: "Codex",
      short: "Codex",
      description: "OpenAI Codex CLI lane for repo edits, tools, and MCP-aware automation.",
      defaultAgent: "Codex implementer",
      defaultModel: "gpt-5.4-codex",
      accent: "oklch(0.58 0.18 245)"
    },
    {
      id: "claude",
      label: "Claude Code",
      short: "Claude",
      description: "Anthropic Claude Code lane for codebase reasoning and patch work.",
      defaultAgent: "Claude reviewer",
      defaultModel: "claude-sonnet-4-5",
      accent: "oklch(0.64 0.17 55)"
    },
    {
      id: "puffer",
      label: "Puffer",
      short: "Puffer",
      description: "Native Puffer lane for local workflows, memory, and desktop runtime tasks.",
      defaultAgent: "Puffer orchestrator",
      defaultModel: "puffer-default",
      accent: "var(--puffer-accent)"
    }
  ];

  const COL_W = 176;
  const NODE_W = 142;
  const NODE_H = 76;
  const PAD_L = 18;
  const PAD_T = 22;
  const connectorFilterPresets: ConnectorFilterPreset[] = [
    { label: "All", query: "" },
    { label: "Trigger", query: "trigger-ready" },
    { label: "Draft", query: "draft" },
    { label: "Append", query: "append" },
    { label: "Monitor", query: "monitor" },
    { label: "Tasks", query: "monitor task" },
    { label: "Repair", query: "repair" },
    { label: "Active", query: "active" },
    { label: "Idle", query: "idle" },
    { label: "Actions", query: "has-actions" },
    { label: "Serve", query: "serve" },
    { label: "Subscriber", query: "subscriber" },
    { label: "Internal", query: "internal-tool" },
    { label: "No trigger", query: "no-trigger" }
  ];
  const appendQueryStopWords = new Set([
    "append",
    "any",
    "containing",
    "contains",
    "event",
    "events",
    "file",
    "into",
    "match",
    "matching",
    "message",
    "messages",
    "on",
    "save",
    "that",
    "to",
    "where",
    "with",
    "workflow",
    "workflows"
  ]);
  const connectorQueryFallbackStopWords = new Set([
    "action",
    "actions",
    "active",
    "append",
    "auth",
    "config",
    "configure",
    "connect",
    "connector",
    "connectors",
    "draft",
    "events",
    "file",
    "has-actions",
    "idle",
    "internal",
    "monitor",
    "no-trigger",
    "proxy",
    "repair",
    "save",
    "serve",
    "setup",
    "subscriber",
    "task",
    "tasks",
    "trigger",
    "trigger-ready",
    "workflow",
    "workflows"
  ]);

  let snapshot = $state<WorkflowSnapshot>({
    workflows: [],
    runs: [],
    connectors: [],
    connections: [],
    connector_error: null,
    workflow_bindings: [],
    workflow_binding_error: null,
    monitor_tasks: [],
    monitor_task_error: null
  });
  let editorWorkflows = $state<EditableWorkflow[]>([starterWorkflow()]);
  let workflowSlug = $state("agent-review-workflow");
  let selectedNodeId = $state<string | null>(null);
  let workflowQuery = $state("");
  let connectorQuery = $state("");
  let selectedConnectorSlug = $state<string | null>(null);
  let selectedConnectorConnectionName = $state("");
  let selectedConnectorDraftPattern = $state("");
  let selectedConnectorAppendPath = $state("/tmp/hi");
  let runIdx = $state<number | null>(null);
  let runQuery = $state("");
  let stepIdx = $state<number | null>(null);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let connectorCommandRunning = $state(false);
  let connectorCommandRunningFor = $state<string | null>(null);
  let selectedWorkflowCommandRunningFor = $state<"draft" | "append" | null>(null);
  let connectionCommandRunningFor = $state<string | null>(null);
  let monitorCommandRunningFor = $state<string | null>(null);
  let monitorTaskCommandRunningFor = $state<string | null>(null);
  let creatingWorkflowBinding = $state(false);
  let creatingConnectionAppendFor = $state<string | null>(null);
  let creatingConnectorAppendFor = $state<string | null>(null);
  let togglingWorkflowSlug = $state<string | null>(null);
  let deletingWorkflowBindingSlug = $state<string | null>(null);
  let savingWorkflowSlug = $state<string | null>(null);
  let refreshGeneration = 0;
  let dirtyWorkflowSlugs = $state<string[]>([]);
  let saveNotice = $state("Workflow changes save to the daemon.");
  let workflowPage = $state<WorkflowPage>("overview");
  let inspectorOpen = $state(true);
  let runsSheetOpen = $state(false);
  let nodePositions = $state<Record<string, Record<string, NodePosition>>>({});
  let nodeDrag = $state<NodeDragState | null>(null);
  let edgeDraft = $state<EdgeDraftState | null>(null);

  let workflows = $derived(editorWorkflows);
  let workflowQueryTerms = $derived(searchTerms(workflowQuery));
  let workflowSearchRows = $derived(indexWorkflows(workflows));
  let filteredWorkflows = $derived(filterWorkflows(workflowSearchRows, workflowQueryTerms));
  let connectors = $derived(snapshot.connectors ?? []);
  let connections = $derived(snapshot.connections ?? []);
  let workflowBindings = $derived(snapshot.workflow_bindings ?? []);
  let monitorBindings = $derived(workflowBindings.filter((binding) => binding.monitor === true));
  let actionBindings = $derived(workflowBindings.filter((binding) => binding.monitor !== true));
  let monitorTasks = $derived(snapshot.monitor_tasks ?? []);
  let activeMonitorTasks = $derived(monitorTasks.filter((task) => !monitorTaskIgnored(task)));
  let triggerReadyConnections = $derived(connections.filter((connection) => connectionTriggerSupported(connection)));
  let connectorQueryTerms = $derived(searchTerms(connectorQuery));
  let connectorQueryRawTokens = $derived(queryTokens(connectorQuery));
  let connectorQueryPlannedConnectionName = $derived(connectorQueryConnectionName(connectorQueryRawTokens));
  let connectorQueryFallbackTerms = $derived(
    connectorQueryConnectorFallbackTerms(connectorQueryRawTokens, connectorQueryPlannedConnectionName)
  );
  let connectionsByConnector = $derived(groupConnectionsByConnector(connections));
  let connectorSearchRows = $derived(indexConnectors(connectors, connectionsByConnector));
  let connectionSearchRows = $derived(indexConnections(connections, connectorSearchRows));
  let monitorBindingSearchRows = $derived(indexWorkflowBindings(monitorBindings, "monitor workflow connection monitor"));
  let actionBindingSearchRows = $derived(indexWorkflowBindings(actionBindings, "workflow action file append trigger save message events"));
  let monitorTaskSearchRows = $derived(indexMonitorTasks(activeMonitorTasks));
  let filteredConnections = $derived(filterConnections(connectionSearchRows, connectorQueryTerms));
  let filteredMonitorBindings = $derived(filterWorkflowBindings(monitorBindingSearchRows, connectorQueryTerms));
  let filteredActionBindings = $derived(filterWorkflowBindings(actionBindingSearchRows, connectorQueryTerms));
  let filteredMonitorTasks = $derived(filterMonitorTasks(monitorTaskSearchRows, connectorQueryTerms));
  let filteredConnectors = $derived(
    filterConnectors(
      connectorSearchRows,
      connectorQueryTerms,
      connectorQueryFallbackTerms,
      connectorQueryPlannedConnectionName
    )
  );
  let workflow = $derived(
    workflows.find((item) => item.slug === workflowSlug) ?? workflows[0] ?? null
  );
  let runs = $derived(
    workflow ? snapshot.runs.filter((run) => run.workflow_slug === workflow.slug) : []
  );
  let runQueryTerms = $derived(searchTerms(runQuery));
  let runSearchRows = $derived(indexRuns(runs));
  let filteredRuns = $derived(filterRuns(runSearchRows, runQueryTerms));
  let run = $derived(runs.find((item) => item.idx === runIdx) ?? runs[0] ?? null);
  let graphNodes = $derived(workflow ? nodesFor(workflow) : []);
  let graphEdges = $derived(workflow ? edgesFor(workflow) : []);
  let graphBaseWidth = $derived(PAD_L * 2 + Math.max(0, graphNodes.length - 1) * COL_W + NODE_W);
  let graphBounds = $derived(graphBoundsFor());
  let graphWidth = $derived(graphBounds.width);
  let graphHeight = $derived(graphBounds.height);
  let currentStepIndex = $derived(
    run ? (stepIdx === null ? Math.max(0, run.nodes.length - 1) : Math.min(stepIdx, run.nodes.length - 1)) : 0
  );
  let currentNode = $derived(run?.nodes[currentStepIndex] ?? null);
  let visited = $derived(new Set((run?.nodes ?? []).slice(0, currentStepIndex + 1).map((node) => node.id)));
  let activeNode = $derived(currentNode?.id ?? "");
  let isLive = $derived(run?.status === "running" && stepIdx === null);
  let selectedNode = $derived(
    workflow?.pipeline.nodes.find((node) => node.id === selectedNodeId) ?? null
  );
  let triggerSelected = $derived(selectedNodeId === "trigger" && workflow != null);
  let selectedConnector = $derived(connectors.find((connector) => connector.connector_slug === selectedConnectorSlug) ?? null);
  let selectedConnectorConnectionInvalid = $derived(
    selectedConnector !== null && !connectionSlugValid(selectedConnectorConnectionName)
  );
  let selectedConnectorAppendPathInvalid = $derived(
    selectedConnector !== null && !appendPathValid(selectedConnectorAppendPath)
  );
  let selectedConnectorCommand = $derived(
    selectedConnector && !selectedConnectorConnectionInvalid
      ? connectorConnectCommand(selectedConnector, selectedConnectorConnectionName.trim())
      : ""
  );
  let selectedConnectorDraftCommand = $derived(
    selectedConnector && connectorTriggerSupported(selectedConnector) && !selectedConnectorConnectionInvalid
      ? workflowDraftCommand(selectedConnectorConnectionName.trim(), selectedConnectorDraftPattern)
      : ""
  );
  let selectedConnectorAppendCommand = $derived(
    selectedConnector
      && connectorTriggerSupported(selectedConnector)
      && !selectedConnectorConnectionInvalid
      && !selectedConnectorAppendPathInvalid
      ? connectorAppendCommand(
          selectedConnector,
          selectedConnectorConnectionName.trim(),
          selectedConnectorAppendPath,
          selectedConnectorDraftPattern
        )
      : ""
  );
  let selectedAppendWorkflowPreview = $derived(selectedConnector ? appendWorkflowPreview() : "");
  let workflowDirty = $derived(workflow ? dirtyWorkflowSlugs.includes(workflow.slug) : false);
  let wrapEl = $state<HTMLDivElement | undefined>();
  let scale = $state(0.8);

  function starterWorkflow(
    slug = "agent-review-workflow",
    name = "Agent review workflow",
    enabled = true
  ): EditableWorkflow {
    return {
      schema: "puffer.workflow.v1",
      slug,
      enabled,
      trigger: { type: "subscription", source_topic: "workspace.task.created", pattern: "review|implement|ship" },
      pipeline: {
        name,
        working_dir: "/Users/shou/corbina",
        concurrency: 1,
        nodes: [
          {
            id: "codex-implement",
            type: "codex",
            agent: "Codex implementer",
            model: "gpt-5.4-codex",
            tools: ["read", "edit", "bash", "mcp"],
            prompt: "Implement the requested change, keep the diff focused, and surface any follow-up risks."
          },
          {
            id: "claude-review",
            type: "claude",
            agent: "Claude reviewer",
            model: "claude-sonnet-4-5",
            tools: ["read", "diff", "bash"],
            depends_on: ["codex-implement"],
            prompt: "Review the implementation for correctness, hidden regressions, and missing tests."
          },
          {
            id: "puffer-ship",
            type: "puffer",
            agent: "Puffer shipper",
            model: "puffer-default",
            tools: ["git", "test", "memory"],
            depends_on: ["claude-review"],
            prompt: "Run final verification, summarize the result, and prepare the branch for handoff."
          }
        ]
      }
    };
  }

  function editableFromWorkflow(item: WorkflowDefinition): EditableWorkflow {
    return {
      ...item,
      pipeline: {
        ...item.pipeline,
        nodes: item.pipeline.nodes.map((node) => normalizeNode(node))
      }
    };
  }

  function normalizeNode(node: WorkflowPipelineNode): EditablePipelineNode {
    if (node.type === "tool") {
      const tool = nodeToolName(node as EditablePipelineNode) || "Bash";
      return {
        ...node,
        id: node.id || uniqueNodeId(tool.toLowerCase()),
        type: "tool",
        agent: node.agent ?? `${tool} call`,
        tools: node.tools && node.tools.length > 0 ? node.tools : [tool],
        prompt: node.prompt ?? defaultToolPrompt(tool),
        depends_on: [...(node.depends_on ?? [])]
      };
    }
    if (node.type === "merge" || node.type === "fanout") {
      return {
        ...node,
        id: node.id || uniqueNodeId(node.type),
        type: node.type,
        agent: node.agent ?? (node.type === "merge" ? "Merge" : "Fanout"),
        tools: [],
        prompt: node.prompt ?? "",
        depends_on: [...(node.depends_on ?? [])]
      };
    }
    const type = providerFromNode(node);
    const meta = providerMeta(type);
    return {
      ...node,
      id: node.id || uniqueNodeId(type),
      type,
      agent: node.agent ?? meta.defaultAgent,
      model: node.model ?? meta.defaultModel,
      tools: node.tools ?? defaultTools(type),
      prompt: node.prompt || defaultPrompt(type),
      depends_on: [...(node.depends_on ?? [])]
    };
  }

  function providerFromNode(node: WorkflowPipelineNode): AgentProvider {
    const raw = `${node.type ?? ""} ${node.agent ?? ""} ${node.model ?? ""}`.toLowerCase();
    if (raw.includes("claude")) return "claude";
    if (raw.includes("puffer")) return "puffer";
    return "codex";
  }

  function providerMeta(provider: AgentProvider): ProviderMeta {
    return providerOptions.find((item) => item.id === provider) ?? providerOptions[0];
  }

  function nodeKindShort(kind: NodeKind): string {
    if (kind === "tool") return "Tool";
    if (kind === "merge") return "Merge";
    if (kind === "fanout") return "Fanout";
    return providerMeta(kind).short;
  }

  function defaultTools(provider: AgentProvider): string[] {
    if (provider === "claude") return ["read", "diff", "bash"];
    if (provider === "puffer") return ["workflow", "memory", "git"];
    return ["read", "edit", "bash", "mcp"];
  }

  function defaultPrompt(provider: AgentProvider): string {
    if (provider === "claude") return "Review the upstream result and call out risks before the workflow proceeds.";
    if (provider === "puffer") return "Coordinate local runtime state and produce a clean handoff summary.";
    return "Implement the assigned workflow step with a focused patch and verification notes.";
  }

  function measure() {
    if (!wrapEl || graphBaseWidth <= 0) return;
    const cw = wrapEl.clientWidth;
    if (!cw) return;
    scale = Math.min(1, cw / graphBaseWidth);
  }

  onMount(() => {
    void refresh();
    measure();
    const ro = new ResizeObserver(measure);
    if (wrapEl) ro.observe(wrapEl);
    window.addEventListener("resize", measure);
    return () => {
      ro.disconnect();
      window.removeEventListener("resize", measure);
    };
  });

  $effect(() => {
    workflowSlug;
    runIdx = null;
    runQuery = "";
    stepIdx = null;
  });

  $effect(() => {
    workflow;
    graphBaseWidth;
    setTimeout(measure, 0);
  });

  async function refresh() {
    if (loading) return;
    const generation = ++refreshGeneration;
    loading = true;
    error = null;
    try {
      const next = await loadWorkflowSnapshot();
      if (generation !== refreshGeneration) return;
      applyWorkflowSnapshot(next);
    } catch (err) {
      if (generation !== refreshGeneration) return;
      error = err instanceof Error ? err.message : String(err);
      if (editorWorkflows.length === 0) editorWorkflows = [starterWorkflow()];
      workflowSlug = editorWorkflows[0].slug;
      selectedNodeId = null;
    } finally {
      if (generation === refreshGeneration) {
        loading = false;
        setTimeout(measure, 0);
      }
    }
  }

  function applyWorkflowSnapshot(next: WorkflowSnapshot) {
    const incoming = next.workflows.length > 0 ? next.workflows.map(editableFromWorkflow) : [starterWorkflow()];
    const dirtyBySlug = new Map(
      editorWorkflows
        .filter((item) => dirtyWorkflowSlugs.includes(item.slug))
        .map((item) => [item.slug, item])
    );
    const merged = incoming.map((item) => dirtyBySlug.get(item.slug) ?? item);
    for (const dirty of dirtyBySlug.values()) {
      if (!merged.some((item) => item.slug === dirty.slug)) merged.push(dirty);
    }
    snapshot = {
      workflows: next.workflows,
      runs: [...next.runs].sort((a, b) => b.idx - a.idx),
      connectors: next.connectors ?? [],
      connections: next.connections ?? [],
      connector_error: next.connector_error ?? null,
      workflow_bindings: next.workflow_bindings ?? [],
      workflow_binding_error: next.workflow_binding_error ?? null,
      monitor_tasks: next.monitor_tasks ?? [],
      monitor_task_error: next.monitor_task_error ?? null
    };
    editorWorkflows = merged;
    if (!workflowSlug || !editorWorkflows.some((item) => item.slug === workflowSlug)) {
      workflowSlug = editorWorkflows[0]?.slug ?? "agent-review-workflow";
    }
    const activeWorkflow = editorWorkflows.find((item) => item.slug === workflowSlug) ?? editorWorkflows[0];
    if (!activeWorkflow) {
      selectedNodeId = null;
    } else if (
      selectedNodeId !== null &&
      selectedNodeId !== "trigger" &&
      !activeWorkflow.pipeline.nodes.some((node) => node.id === selectedNodeId)
    ) {
      selectedNodeId = null;
    }
  }

  function selectWorkflow(slug: string) {
    workflowSlug = slug;
    // For existing workflows, jump to the first agent node so the user lands
    // on a useful form. Blank drafts override this back to "trigger" in
    // createWorkflowDraft.
    const nextWorkflow = editorWorkflows.find((item) => item.slug === slug);
    selectedNodeId = nextWorkflow?.pipeline.nodes[0]?.id ?? "trigger";
  }

  function openWorkflowDetail(slug: string) {
    selectWorkflow(slug);
    runsSheetOpen = true;
    workflowPage = "detail";
  }

  function openWorkflowRunDetail(item: WorkflowRun) {
    selectWorkflow(item.workflow_slug);
    selectRun(item.idx);
    runsSheetOpen = true;
    workflowPage = "detail";
  }

  function backToWorkflowOverview() {
    workflowPage = "overview";
  }

  function createWorkflowDraft(source: WorkflowDraftSource = {}) {
    const slug = uniqueWorkflowSlug(source.slugBase ?? "workflow-draft");
    const draft = starterWorkflow(slug, source.name ?? "Workflow draft", false);
    draft.pipeline.nodes = [];
    const pattern = normalizedPattern(source.pattern) ?? ".*";
    const connection = source.connectionSlug
      ? connections.find((item) => item.slug === source.connectionSlug)
      : triggerReadyConnections[0];
    const connectionSlug = source.connectionSlug ?? connection?.slug;
    draft.trigger = connection
      ? { type: "connection", connection_slug: connection.slug, pattern }
      : connectionSlug
        ? { type: "connection", connection_slug: connectionSlug, pattern }
      : { type: "subscription", source_topic: "workspace.task.created", pattern };
    editorWorkflows = [...editorWorkflows, draft];
    workflowSlug = slug;
    selectedNodeId = "trigger";
    if (source.connectorSlug) selectedConnectorSlug = source.connectorSlug;
    if (source.connectionName ?? connectionSlug) selectedConnectorConnectionName = source.connectionName ?? connectionSlug ?? "";
    dirtyWorkflowSlugs = Array.from(new Set([...dirtyWorkflowSlugs, slug]));
    workflowQuery = "";
    workflowPage = "create";
    saveNotice = source.saveMessage ?? `Created ${slug} locally. Add nodes, then save to persist.`;
  }

  function uniqueWorkflowSlug(base: string): string {
    const existing = new Set(editorWorkflows.map((item) => item.slug));
    if (!existing.has(base)) return base;
    let index = 2;
    while (existing.has(`${base}-${index}`)) index += 1;
    return `${base}-${index}`;
  }

  function titleFromSlug(slug: string): string {
    const title = slug
      .split("-")
      .filter(Boolean)
      .map((part) => `${part.slice(0, 1).toUpperCase()}${part.slice(1)}`)
      .join(" ");
    return title || "Workflow";
  }

  function selectRun(idx: number) {
    runIdx = idx;
    stepIdx = null;
    runsSheetOpen = true;
  }

  function stepToIdx(i: number) {
    stepIdx = i;
  }

  function updateCurrentWorkflow(mutator: (item: EditableWorkflow) => EditableWorkflow) {
    if (!workflow) return;
    const dirtySlug = workflow.slug;
    let updatedSlug = dirtySlug;
    editorWorkflows = editorWorkflows.map((item) => {
      if (item.slug !== dirtySlug) return item;
      const updated = mutator(item);
      updatedSlug = updated.slug;
      return updated;
    });
    dirtyWorkflowSlugs = Array.from(
      new Set([...dirtyWorkflowSlugs.filter((slug) => slug !== dirtySlug), updatedSlug])
    );
    saveNotice = "Edited locally. Save to persist this workflow.";
  }

  function updateWorkflowField(field: "slug" | "enabled" | "name" | "working_dir" | "concurrency", value: string | boolean | number | null) {
    if (!workflow) return;
    const oldSlug = workflow.slug;
    updateCurrentWorkflow((item) => {
      if (field === "slug") return { ...item, slug: String(value || "workflow") };
      if (field === "enabled") return { ...item, enabled: Boolean(value) };
      if (field === "name") return { ...item, pipeline: { ...item.pipeline, name: String(value) } };
      if (field === "working_dir") return { ...item, pipeline: { ...item.pipeline, working_dir: String(value) } };
      return { ...item, pipeline: { ...item.pipeline, concurrency: Number(value) || 1 } };
    });
    if (field === "slug") workflowSlug = String(value || oldSlug);
  }

  function workflowForSave(item: EditableWorkflow): WorkflowDefinition {
    return {
      schema: item.schema || "puffer.workflow.v1",
      slug: item.slug,
      enabled: item.enabled,
      trigger: triggerForSave(item.trigger),
      pipeline: {
        name: item.pipeline.name,
        working_dir: item.pipeline.working_dir,
        concurrency: item.pipeline.concurrency,
        nodes: item.pipeline.nodes.map((node) => ({
          id: node.id,
          type: node.type,
          agent: node.agent,
          prompt: node.prompt,
          model: node.model,
          tools: [...(node.tools ?? [])],
          env: node.env,
          depends_on: [...(node.depends_on ?? [])]
        }))
      }
    };
  }

  async function saveCurrentWorkflow() {
    if (!workflow || !workflowDirty || savingWorkflowSlug) return;
    const savedSlug = workflow.slug;
    savingWorkflowSlug = savedSlug;
    error = null;
    saveNotice = `Saving ${savedSlug}...`;
    try {
      const next = await saveWorkflow(workflowForSave(workflow));
      dirtyWorkflowSlugs = dirtyWorkflowSlugs.filter((slug) => slug !== savedSlug);
      applyWorkflowSnapshot(next);
      workflowSlug = savedSlug;
      saveNotice = `Saved ${savedSlug}.`;
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      error = message;
      saveNotice = `Could not save ${savedSlug}: ${message}`;
    } finally {
      savingWorkflowSlug = null;
    }
  }

  async function toggleCurrentWorkflowEnabled() {
    if (!workflow || workflowDirty || savingWorkflowSlug || togglingWorkflowSlug) return;
    const slug = workflow.slug;
    const enabled = !workflow.enabled;
    togglingWorkflowSlug = slug;
    error = null;
    saveNotice = `${enabled ? "Resuming" : "Pausing"} ${slug}...`;
    try {
      const next = await toggleWorkflow(slug, enabled);
      applyWorkflowSnapshot(next);
      workflowSlug = slug;
      saveNotice = `${enabled ? "Resumed" : "Paused"} ${slug}.`;
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      error = message;
      saveNotice = `Could not toggle ${slug}: ${message}`;
    } finally {
      togglingWorkflowSlug = null;
    }
  }

  function updateTriggerField(field: "source_topic" | "connection_slug" | "pattern" | "cron", value: string) {
    updateCurrentWorkflow((item) => {
      if (item.trigger.type === "cron") return { ...item, trigger: { type: "cron", cron: value || "0 * * * *" } };
      if (item.trigger.type === "connection") {
        const key = field === "connection_slug" ? "connection_slug" : field === "pattern" ? "pattern" : "connection_slug";
        return {
          ...item,
          trigger: {
            ...item.trigger,
            [key]: value
          }
        };
      }
      return {
        ...item,
        trigger: {
          ...item.trigger,
          [field === "cron" ? "source_topic" : field]: value
        }
      };
    });
  }

  function setTriggerType(type: TriggerMode) {
    updateCurrentWorkflow((item) => ({
      ...item,
      trigger:
        type === "cron"
          ? { type: "cron", cron: "0 9 * * 1-5" }
          : type === "connection"
            ? { type: "connection", connection_slug: defaultConnectionSlug(item), pattern: defaultPattern(item) }
            : { type: "subscription", source_topic: sourceTopicFor(item), pattern: defaultPattern(item) }
    }));
  }

  function defaultPattern(item: EditableWorkflow): string {
    return item.trigger.type === "connection" || item.trigger.type === "subscription"
      ? normalizedPattern(item.trigger.pattern) ?? ".*"
      : ".*";
  }

  function normalizedPattern(value: string | null | undefined): string | null {
    const trimmed = (value ?? "").trim();
    if (!trimmed) return null;
    return trimmed === "*" ? ".*" : trimmed;
  }

  function triggerForSave(trigger: WorkflowDefinition["trigger"]): WorkflowDefinition["trigger"] {
    if (trigger.type === "cron") return trigger;
    return { ...trigger, pattern: normalizedPattern(trigger.pattern) };
  }

  function defaultConnectionSlug(item: EditableWorkflow): string {
    const trigger = item.trigger;
    if (trigger.type === "connection") {
      const current = connections.find((connection) => connection.slug === trigger.connection_slug);
      if (!current || connectionTriggerSupported(current)) return trigger.connection_slug;
    }
    if (triggerReadyConnections.length > 0) return triggerReadyConnections[0].slug;
    if (trigger.type === "subscription") return trigger.source_topic;
    return "telegram-user";
  }

  function sourceTopicFor(item: EditableWorkflow): string {
    if (item.trigger.type === "subscription") return item.trigger.source_topic;
    if (item.trigger.type === "connection") return item.trigger.connection_slug;
    return "workspace.task.created";
  }

  function useConnectionTrigger(connectionSlug: string) {
    if (!connectionSlug) return;
    const connection = connections.find((item) => item.slug === connectionSlug);
    if (connection && !connectionTriggerSupported(connection)) {
      saveNotice = `${connection.slug} cannot start workflow triggers. Choose an event-capable connection.`;
      return;
    }
    if (connection) {
      selectedConnectorSlug = connection.connector_slug;
      selectedConnectorConnectionName = connection.slug;
    }
    updateCurrentWorkflow((item) => ({
      ...item,
      trigger: { type: "connection", connection_slug: connectionSlug, pattern: defaultPattern(item) }
    }));
  }

  function useConnectorTemplate(connector: WorkflowConnector, plannedConnectionName?: string | null) {
    const connectorConnections = connectionsForConnector(connector.connector_slug);
    const plannedSlug = plannedConnectionName?.trim();
    const existingConnection = plannedSlug
      ? connectorConnections.find((connection) => connection.slug === plannedSlug)
      : connectorConnections[0];
    const connectionSlug = plannedSlug || existingConnection?.slug || connectorConnectionHint(connector);
    const command = connectorConnectCommand(connector, connectionSlug);
    selectedConnectorSlug = connector.connector_slug;
    selectedConnectorConnectionName = connectionSlug;
    if (!connectorTriggerSupported(connector)) {
      saveNotice = `${connector.connector_slug} cannot start workflow triggers yet. ${command} is available for connector setup.`;
      return;
    }
    updateCurrentWorkflow((item) => ({
      ...item,
      trigger: { type: "connection", connection_slug: connectionSlug, pattern: defaultPattern(item) }
    }));
    if (!existingConnection) {
      saveNotice = `Run ${command} before enabling this workflow trigger.`;
    }
  }

  function createWorkflowDraftForConnection(connection: WorkflowConnection) {
    if (!connectionTriggerSupported(connection)) {
      saveNotice = `${connection.slug} cannot start workflow triggers. Choose an event-capable connection.`;
      return;
    }
    createWorkflowDraft({
      slugBase: `${connection.slug}-workflow`,
      name: `${titleFromSlug(connection.slug)} workflow`,
      connectionSlug: connection.slug,
      connectorSlug: connection.connector_slug,
      connectionName: connection.slug,
      saveMessage: `Created ${connection.slug}-backed workflow locally. Save to persist this workflow.`
    });
  }

  function createWorkflowDraftForConnector(
    connector: WorkflowConnector,
    plannedConnectionName?: string,
    pattern?: string
  ) {
    const connectorConnections = connectionsForConnector(connector.connector_slug);
    const matchingConnection = plannedConnectionName
      ? connectorConnections.find((connection) => connection.slug === plannedConnectionName)
      : null;
    const existingConnection =
      matchingConnection ?? (plannedConnectionName ? null : connectorConnections[0]);
    const connectionSlug = plannedConnectionName || existingConnection?.slug || connectorConnectionHint(connector);
    const command = connectorConnectCommand(connector, connectionSlug);
    selectedConnectorSlug = connector.connector_slug;
    selectedConnectorConnectionName = connectionSlug;
    if (!connectorTriggerSupported(connector)) {
      saveNotice = `${connector.connector_slug} cannot start workflow triggers yet. ${command} is available for connector setup.`;
      return;
    }
    createWorkflowDraft({
      slugBase: `${connectionSlug}-workflow`,
      name: `${titleFromSlug(connectionSlug)} workflow`,
      connectionSlug,
      connectorSlug: connector.connector_slug,
      connectionName: connectionSlug,
      pattern,
      saveMessage: existingConnection
        ? `Created ${connectionSlug}-backed workflow locally. Save to persist this workflow.`
        : `Created ${connectionSlug}-backed workflow locally. Run ${command} before enabling it.`
    });
  }

  function createWorkflowDraftForSelectedConnector() {
    if (!selectedConnector || selectedConnectorConnectionInvalid) return;
    createWorkflowDraftForConnector(
      selectedConnector,
      selectedConnectorConnectionName,
      selectedConnectorDraftPattern
    );
  }

  async function createAppendWorkflowForSelectedConnector() {
    if (!selectedConnector) return;
    if (connectorCommandRunnerBusy()) return;
    const connectionSlug = selectedConnectorConnectionName.trim();
    const path = selectedConnectorAppendPath.trim();
    if (!connectorTriggerSupported(selectedConnector)) {
      saveNotice = `${selectedConnector.connector_slug} cannot start workflow triggers yet.`;
      return;
    }
    if (selectedConnectorConnectionInvalid) {
      saveNotice = "Connection names must use lowercase letters, digits, and hyphens.";
      return;
    }
    if (!appendPathValid(path)) {
      saveNotice = "Append paths must be relative or under /tmp.";
      return;
    }
    creatingWorkflowBinding = true;
    error = null;
    const pattern = normalizedPattern(selectedConnectorDraftPattern);
    const slug = appendBindingSlug(connectionSlug, path);
    saveNotice = `Creating ${slug}...`;
    try {
      const next = await createWorkflowBinding({
        slug,
        description: `Append ${connectionSlug} messages to ${path}`,
        connection_slug: connectionSlug,
        connector_slug: selectedConnector.connector_slug,
        pattern: pattern || null,
        file_append_path: path,
        enabled: true
      });
      applyWorkflowSnapshot(next);
      saveNotice = `Created append workflow ${slug}.`;
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      error = message;
      saveNotice = `Could not create append workflow: ${message}`;
    } finally {
      creatingWorkflowBinding = false;
    }
  }

  async function createAppendWorkflowForConnector(connector: WorkflowConnector, plannedConnectionName?: string | null) {
    if (connectorCommandRunnerBusy()) return;
    if (!connectorTriggerSupported(connector)) {
      saveNotice = `${connector.connector_slug} cannot start workflow triggers yet.`;
      return;
    }
    const connectionSlug = connectorConnectionName(connector, plannedConnectionName);
    if (!connectionSlugValid(connectionSlug)) {
      saveNotice = "Connection names must use lowercase letters, digits, and hyphens.";
      return;
    }
    const intent = connectorAppendIntent(connector, connectionSlug);
    const path = intent.path;
    const pattern = intent.pattern;
    const slug = appendBindingSlug(connectionSlug, path);
    selectedConnectorSlug = connector.connector_slug;
    selectedConnectorConnectionName = connectionSlug;
    selectedConnectorDraftPattern = pattern ?? "";
    selectedConnectorAppendPath = path;
    creatingWorkflowBinding = true;
    creatingConnectorAppendFor = connector.connector_slug;
    error = null;
    saveNotice = `Creating ${slug}...`;
    try {
      const description = pattern
        ? `Append ${connectionSlug} messages matching ${pattern} to ${path}`
        : `Append ${connectionSlug} messages to ${path}`;
      const next = await createWorkflowBinding({
        slug,
        description,
        connection_slug: connectionSlug,
        connector_slug: connector.connector_slug,
        pattern,
        file_append_path: path,
        enabled: true
      });
      applyWorkflowSnapshot(next);
      saveNotice = `Created append workflow ${slug}.`;
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      error = message;
      saveNotice = `Could not create append workflow: ${message}`;
    } finally {
      creatingConnectorAppendFor = null;
      creatingWorkflowBinding = false;
    }
  }

  async function createAppendWorkflowForConnection(connection: WorkflowConnection) {
    if (connectorCommandRunnerBusy()) return;
    if (!connectionTriggerSupported(connection)) {
      saveNotice = `${connection.slug} cannot start workflow triggers. Choose an event-capable connection.`;
      return;
    }
    const intent = connectionAppendIntent(connection);
    const path = intent.path;
    const pattern = intent.pattern;
    const slug = appendBindingSlug(connection.slug, path);
    selectedConnectorSlug = connection.connector_slug;
    selectedConnectorConnectionName = connection.slug;
    selectedConnectorDraftPattern = pattern ?? "";
    selectedConnectorAppendPath = path;
    creatingWorkflowBinding = true;
    creatingConnectionAppendFor = connection.slug;
    error = null;
    saveNotice = `Creating ${slug}...`;
    try {
      const description = pattern
        ? `Append ${connection.slug} messages matching ${pattern} to ${path}`
        : `Append ${connection.slug} messages to ${path}`;
      const next = await createWorkflowBinding({
        slug,
        description,
        connection_slug: connection.slug,
        connector_slug: connection.connector_slug,
        pattern,
        file_append_path: path,
        enabled: true
      });
      applyWorkflowSnapshot(next);
      saveNotice = `Created append workflow ${slug}.`;
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      error = message;
      saveNotice = `Could not create append workflow: ${message}`;
    } finally {
      creatingConnectionAppendFor = null;
      creatingWorkflowBinding = false;
    }
  }

  function workflowDraftCommand(connectionSlug: string, pattern?: string | null): string {
    const slug = connectionSlug.trim();
    if (!slug) return "";
    const normalized = normalizedPattern(pattern);
    const base = `/workflows new ${slug}-workflow ${slug}`;
    return normalized ? `${base} ${quoteWorkflowArg(normalized)}` : base;
  }

  function quoteWorkflowArg(value: string): string {
    if (/^[a-zA-Z0-9_./:@%+=,-]+$/.test(value)) return value;
    return `'${value.replaceAll("'", "'\\''")}'`;
  }

  function connectionDraftCommand(connection: WorkflowConnection): string {
    return workflowDraftCommand(connection.slug);
  }

  function connectorDraftCommand(connector: WorkflowConnector, plannedConnectionName?: string): string {
    const existingConnection = plannedConnectionName
      ? connectionsForConnector(connector.connector_slug).find((connection) => connection.slug === plannedConnectionName)
      : connectionsForConnector(connector.connector_slug)[0];
    return workflowDraftCommand(plannedConnectionName || existingConnection?.slug || connectorConnectionHint(connector));
  }

  function workflowAppendCommand(
    connectionSlug: string,
    path: string,
    pattern?: string | null,
    connectorSlug?: string | null
  ): string {
    const slug = connectionSlug.trim();
    const target = path.trim();
    if (!slug || !target) return "";
    const normalized = normalizedPattern(pattern);
    const withTarget = `/workflows append ${slug} ${quoteWorkflowArg(target)}`;
    const withPattern = normalized ? `${withTarget} ${quoteWorkflowArg(normalized)}` : withTarget;
    return connectorSlug ? `${withPattern} --connector ${connectorSlug}` : withPattern;
  }

  function connectionAppendCommand(connection: WorkflowConnection): string {
    const intent = connectionAppendIntent(connection);
    return workflowAppendCommand(connection.slug, intent.path, intent.pattern);
  }

  function connectionAppendIntent(connection: WorkflowConnection): AppendQueryIntent {
    const fallback = {
      path: `/tmp/${connection.slug}.log`,
      pattern: null
    };
    if (!connectorQueryNamesConnection(connection)) return fallback;
    const path = connectorQueryAppendPath();
    if (!path) return fallback;
    return {
      path,
      pattern: connectorQueryAppendPattern(connection, path)
    };
  }

  function connectorQueryNamesConnection(connection: WorkflowConnection): boolean {
    const needles = new Set([
      connection.slug.toLowerCase(),
      connection.connector_slug.toLowerCase()
    ]);
    return connectorQueryRawTokens.some((token) => needles.has(token.toLowerCase()));
  }

  function connectorQueryAppendPath(): string | null {
    return connectorQueryRawTokens.find((token) => looksLikeAppendPath(token) && appendPathValid(token)) ?? null;
  }

  function connectorQueryAppendPattern(connection: WorkflowConnection, path: string): string | null {
    return connectorQueryAppendPatternForTerms(path, new Set([
      connection.slug.toLowerCase(),
      connection.connector_slug.toLowerCase()
    ]));
  }

  function connectorAppendIntent(connector: WorkflowConnector, connectionName: string): AppendQueryIntent {
    const fallback = {
      path: `/tmp/${connectionName}.log`,
      pattern: null
    };
    const path = connectorQueryAppendPath();
    if (!path) return fallback;
    return {
      path,
      pattern: connectorQueryAppendPatternForTerms(
        path,
        connectorIdentityTerms(connector, connectionName)
      )
    };
  }

  function connectorIdentityTerms(connector: WorkflowConnector, connectionName?: string | null): Set<string> {
    const values = [
      connector.connector_slug,
      connectorConnectionHint(connector),
      connectionName ?? ""
    ];
    return new Set(
      values
        .flatMap((value) => [value, ...value.split(/[^a-zA-Z0-9]+/)])
        .map((value) => value.trim().toLowerCase())
        .filter(Boolean)
    );
  }

  function connectorQueryNamesConnector(connector: WorkflowConnector): boolean {
    const identity = connectorIdentityTerms(connector);
    return connectorQueryRawTokens.some((token) => identity.has(token.toLowerCase()));
  }

  function connectorAppendQueryMatches(connector: WorkflowConnector): boolean {
    return connectorQueryAppendPath() !== null && connectorQueryNamesConnector(connector);
  }

  function connectorQueryAppendPatternForTerms(path: string, identityTerms: Set<string>): string | null {
    const pattern = connectorQueryRawTokens
      .filter((token) => {
        const lower = token.toLowerCase();
        return token !== path
          && !appendQueryStopWords.has(lower)
          && !identityTerms.has(lower)
          && !looksLikeAppendPath(token);
      })
      .join(" ")
      .trim();
    return pattern || null;
  }

  function queryTokens(query: string): string[] {
    return query
      .trim()
      .split(/\s+/)
      .map(stripQueryTokenQuotes)
      .filter(Boolean);
  }

  function connectorQueryConnectionName(tokens: string[]): string | null {
    if (tokens.length < 2) return null;
    const candidate = tokens[tokens.length - 1]?.trim() ?? "";
    if (
      !candidate
      || looksLikeAppendPath(candidate)
      || !connectionSlugValid(candidate)
      || connectorQueryFallbackStopWords.has(candidate.toLowerCase())
    ) return null;
    return candidate;
  }

  function connectorQueryConnectorFallbackTerms(tokens: string[], connectionName: string | null): string[] {
    if (!connectionName || tokens.length < 2) return [];
    const connectorTerm = tokens
      .slice(0, -1)
      .map((token) => token.trim().toLowerCase())
      .find((token) => token && !connectorQueryFallbackStopWords.has(token));
    return connectorTerm ? [connectorTerm] : [];
  }

  function stripQueryTokenQuotes(value: string): string {
    if (value.length >= 2 && value.startsWith("'") && value.endsWith("'")) {
      return value.slice(1, -1);
    }
    if (value.length >= 2 && value.startsWith("\"") && value.endsWith("\"")) {
      return value.slice(1, -1);
    }
    return value;
  }

  function looksLikeAppendPath(value: string): boolean {
    return value.startsWith("/")
      || value.startsWith("./")
      || value.startsWith("../")
      || value.includes("/")
      || value.includes(".");
  }

  function connectorAppendCommand(
    connector: WorkflowConnector,
    plannedConnectionName?: string,
    path?: string | null,
    pattern?: string | null
  ): string {
    const connectionName = connectorConnectionName(connector, plannedConnectionName);
    const existingConnection = connectionsForConnector(connector.connector_slug).find(
      (connection) => connection.slug === connectionName
    );
    const target = path?.trim() || `/tmp/${connectionName}.log`;
    return workflowAppendCommand(
      connectionName,
      target,
      pattern,
      existingConnection ? null : connector.connector_slug
    );
  }

  function connectorConnectionName(connector: WorkflowConnector, plannedConnectionName?: string | null): string {
    return plannedConnectionName?.trim() || connectorConnectionHint(connector);
  }

  function connectorConnectionHint(connector: WorkflowConnector): string {
    return connector.suggested_connection_slug || connector.connector_slug;
  }

  function connectorConnectCommand(
    connector: WorkflowConnector,
    connectionName = connectorConnectionHint(connector)
  ): string {
    const name = connectionName.trim();
    if (name === connectorConnectionHint(connector) && connector.connect_command) return connector.connect_command;
    return `/connect ${connector.connector_slug} ${name}`;
  }

  function connectionSlugValid(value: string): boolean {
    return /^[a-z0-9-]+$/.test(value.trim());
  }

  function appendPathValid(value: string): boolean {
    const path = value.trim();
    const segments = path.split("/").filter(Boolean);
    if (!path || path.startsWith("~/") || segments.includes("..")) return false;
    return !path.startsWith("/") || path.startsWith("/tmp/");
  }

  function appendBindingSlug(connectionSlug: string, path: string): string {
    const leaf = path.split("/").filter(Boolean).at(-1) ?? "events";
    return `append-${connectionSlug}-${slugFragment(leaf)}`;
  }

  function slugFragment(value: string): string {
    const slug = value.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 48);
    return slug || "events";
  }

  function appendWorkflowPreview(): string {
    if (!selectedConnector || selectedConnectorConnectionInvalid) return "Enter a valid connection name.";
    if (selectedConnectorAppendPathInvalid) return "Enter a relative path or /tmp path.";
    const pattern = normalizedPattern(selectedConnectorDraftPattern);
    return `${selectedConnectorConnectionName.trim()} ${pattern || ".*"} -> ${selectedConnectorAppendPath.trim()}`;
  }

  function updateSelectedConnectorConnectionName(value: string) {
    selectedConnectorConnectionName = value;
    if (!selectedConnector || !connectorTriggerSupported(selectedConnector) || !connectionSlugValid(value)) {
      return;
    }
    updateCurrentWorkflow((item) => ({
      ...item,
      trigger: { type: "connection", connection_slug: value.trim(), pattern: defaultPattern(item) }
    }));
  }

  function updateSelectedConnectorDraftPattern(value: string) {
    selectedConnectorDraftPattern = value;
  }

  function updateSelectedConnectorAppendPath(value: string) {
    selectedConnectorAppendPath = value;
  }

  async function copyWorkflowCommand(command: string) {
    if (!command.trim()) return;
    try {
      await navigator.clipboard.writeText(command.trim());
      saveNotice = `Copied ${command}.`;
    } catch (err) {
      saveNotice = "Clipboard unavailable. Select and copy the command manually.";
    }
  }

  async function copySelectedConnectorCommand() {
    const command = selectedConnectorCommand.trim();
    if (selectedConnectorConnectionInvalid) {
      saveNotice = "Connection names must use lowercase letters, digits, and hyphens.";
      return;
    }
    await copyWorkflowCommand(command);
  }

  async function copySelectedConnectorDraftCommand() {
    const command = selectedConnectorDraftCommand.trim();
    if (selectedConnectorConnectionInvalid) {
      saveNotice = "Connection names must use lowercase letters, digits, and hyphens.";
      return;
    }
    await copyWorkflowCommand(command);
  }

  async function copySelectedConnectorAppendCommand() {
    const command = selectedConnectorAppendCommand.trim();
    if (selectedConnectorConnectionInvalid) {
      saveNotice = "Connection names must use lowercase letters, digits, and hyphens.";
      return;
    }
    if (selectedConnectorAppendPathInvalid) {
      saveNotice = "Append paths must be relative or under /tmp.";
      return;
    }
    await copyWorkflowCommand(command);
  }

  async function runSelectedConnectorCommand() {
    const command = selectedConnectorCommand.trim();
    if (selectedConnectorConnectionInvalid) {
      saveNotice = "Connection names must use lowercase letters, digits, and hyphens.";
      return;
    }
    if (!command || connectorCommandRunnerBusy() || !onRunWorkflowCommand) return;
    connectorCommandRunning = true;
    try {
      const started = await onRunWorkflowCommand(command);
      saveNotice = started === false ? `Could not start ${command}.` : `Started ${command} in an agent session.`;
    } catch (err) {
      saveNotice = `Could not start ${command}.`;
    } finally {
      connectorCommandRunning = false;
    }
  }

  async function runSelectedWorkflowCommand(kind: "draft" | "append", command: string) {
    const trimmed = command.trim();
    if (selectedConnectorConnectionInvalid) {
      saveNotice = "Connection names must use lowercase letters, digits, and hyphens.";
      return;
    }
    if (kind === "append" && selectedConnectorAppendPathInvalid) {
      saveNotice = "Append paths must be relative or under /tmp.";
      return;
    }
    if (!trimmed || connectorCommandRunnerBusy() || !onRunWorkflowCommand) return;
    selectedWorkflowCommandRunningFor = kind;
    try {
      const started = await onRunWorkflowCommand(trimmed);
      saveNotice = started === false ? `Could not start ${trimmed}.` : `Started ${trimmed} in an agent session.`;
    } catch (err) {
      saveNotice = `Could not start ${trimmed}.`;
    } finally {
      selectedWorkflowCommandRunningFor = null;
    }
  }

  async function runSelectedConnectorDraftCommand() {
    await runSelectedWorkflowCommand("draft", selectedConnectorDraftCommand);
  }

  async function runSelectedConnectorAppendCommand() {
    await runSelectedWorkflowCommand("append", selectedConnectorAppendCommand);
  }

  async function runConnectorSetupCommand(connector: WorkflowConnector, plannedConnectionName?: string | null) {
    const connectionName = plannedConnectionName?.trim() || connectorConnectionHint(connector);
    const command = connectorConnectCommand(connector, connectionName);
    if (connectorCommandRunnerBusy() || !onRunWorkflowCommand) return;
    selectedConnectorSlug = connector.connector_slug;
    selectedConnectorConnectionName = connectionName;
    connectorCommandRunningFor = connector.connector_slug;
    try {
      const started = await onRunWorkflowCommand(command);
      saveNotice = started === false
        ? `Could not start ${command}.`
        : `Started ${command} in an agent session.`;
    } catch (err) {
      saveNotice = `Could not start ${command}.`;
    } finally {
      connectorCommandRunningFor = null;
    }
  }

  function connectionMonitorSupported(connection: WorkflowConnection): boolean {
    if (connection.monitor_command !== undefined) return Boolean(connection.monitor_command);
    return connectionTriggerSupported(connection);
  }

  function connectionMonitorCommand(connection: WorkflowConnection): string {
    return connection.monitor_command || `/monitor ${connection.slug}`;
  }

  function connectionConnectCommand(connection: WorkflowConnection): string {
    return connection.connect_command || `/connect ${connection.connector_slug} ${connection.slug}`;
  }

  function connectorCommandRunnerBusy(): boolean {
    return connectorCommandRunning
      || connectorCommandRunningFor !== null
      || selectedWorkflowCommandRunningFor !== null
      || connectionCommandRunningFor !== null
      || monitorCommandRunningFor !== null
      || monitorTaskCommandRunningFor !== null
      || creatingWorkflowBinding
      || togglingWorkflowSlug !== null;
  }

  async function runConnectionConnectCommand(connection: WorkflowConnection) {
    const command = connectionConnectCommand(connection);
    if (connectorCommandRunnerBusy() || !onRunWorkflowCommand) return;
    connectionCommandRunningFor = connection.slug;
    try {
      const started = await onRunWorkflowCommand(command);
      saveNotice = started === false
        ? `Could not start ${command}.`
        : `Started ${command} in an agent session.`;
    } catch (err) {
      saveNotice = `Could not start ${command}.`;
    } finally {
      connectionCommandRunningFor = null;
    }
  }

  async function runConnectionMonitorCommand(connection: WorkflowConnection) {
    const command = connectionMonitorCommand(connection);
    if (!connectionMonitorSupported(connection) || connectorCommandRunnerBusy() || !onRunWorkflowCommand) return;
    monitorCommandRunningFor = connection.slug;
    try {
      const started = await onRunWorkflowCommand(command);
      saveNotice = started === false
        ? `Could not start ${command}.`
        : `Started ${command} in an agent session.`;
    } catch (err) {
      saveNotice = `Could not start ${command}.`;
    } finally {
      monitorCommandRunningFor = null;
    }
  }

  function monitorTaskIgnored(task: WorkflowMonitorTask): boolean {
    return task.ignored === true;
  }

  function monitorBindingLabel(binding: WorkflowBinding): string {
    return binding.description?.trim() || binding.slug;
  }

  function monitorBindingStatus(binding: WorkflowBinding): string {
    if (binding.status?.trim()) return binding.status;
    return binding.enabled ? "enabled" : "paused";
  }

  function monitorBindingToggleLabel(binding: WorkflowBinding): string {
    return `${binding.enabled ? "Pause" : "Resume"} ${binding.slug}`;
  }

  function workflowBindingBusy(binding: WorkflowBinding): boolean {
    return (
      togglingWorkflowSlug !== null ||
      deletingWorkflowBindingSlug !== null ||
      creatingWorkflowBinding ||
      binding.slug === deletingWorkflowBindingSlug
    );
  }

  async function toggleMonitorBinding(binding: WorkflowBinding) {
    if (workflowBindingBusy(binding)) return;
    const enabled = !binding.enabled;
    togglingWorkflowSlug = binding.slug;
    error = null;
    saveNotice = `${enabled ? "Resuming" : "Pausing"} ${binding.slug}...`;
    try {
      const next = await toggleWorkflow(binding.slug, enabled);
      applyWorkflowSnapshot(next);
      saveNotice = `${enabled ? "Resumed" : "Paused"} ${binding.slug}.`;
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      error = message;
      saveNotice = `Could not toggle ${binding.slug}: ${message}`;
    } finally {
      togglingWorkflowSlug = null;
    }
  }

  async function deleteWorkflowBindingRow(binding: WorkflowBinding) {
    if (workflowBindingBusy(binding)) return;
    deletingWorkflowBindingSlug = binding.slug;
    error = null;
    saveNotice = `Deleting ${binding.slug}...`;
    try {
      const next = await deleteWorkflowBinding(binding.slug);
      applyWorkflowSnapshot(next);
      saveNotice = `Deleted ${binding.slug}.`;
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      error = message;
      saveNotice = `Could not delete ${binding.slug}: ${message}`;
    } finally {
      deletingWorkflowBindingSlug = null;
    }
  }

  function monitorTaskActions(task: WorkflowMonitorTask): WorkflowMonitorTaskAction[] {
    return task.actions ?? [];
  }

  function monitorTaskIgnoreReasons(task: WorkflowMonitorTask): string[] {
    return task.possible_ignore_reasons ?? [];
  }

  function monitorTaskShowCommand(task: WorkflowMonitorTask): string {
    return `/tasks show ${task.task_id}`;
  }

  function monitorTaskIgnoreCommand(task: WorkflowMonitorTask, reason?: string): string {
    const trimmed = reason?.trim();
    return trimmed ? `/tasks ignore ${task.task_id} ${trimmed}` : `/tasks ignore ${task.task_id}`;
  }

  function monitorTaskActionPrompt(task: WorkflowMonitorTask, action: WorkflowMonitorTaskAction): string {
    return [
      `Act on monitored task ${task.task_id}: ${task.subject}`,
      "",
      "Task description:",
      task.description,
      "",
      `Selected action: ${action.name}`,
      "",
      action.prompt,
      "",
      `When the action is fully handled, update task ${task.task_id} with TaskUpdate status=completed. If you need more context, inspect the connector or ask the user.`
    ].join("\n");
  }

  async function runMonitorTaskCommand(
    task: WorkflowMonitorTask,
    command: string,
    startedMessage: string
  ) {
    if (!command.trim() || connectorCommandRunnerBusy() || !onRunWorkflowCommand) return;
    monitorTaskCommandRunningFor = task.task_id;
    try {
      const started = await onRunWorkflowCommand(command);
      saveNotice = started === false ? `Could not start ${task.task_id}.` : startedMessage;
    } catch (err) {
      saveNotice = `Could not start ${task.task_id}.`;
    } finally {
      monitorTaskCommandRunningFor = null;
    }
  }

  async function runMonitorTaskShowCommand(task: WorkflowMonitorTask) {
    await runMonitorTaskCommand(task, monitorTaskShowCommand(task), `Opened ${task.task_id} in an agent session.`);
  }

  async function runMonitorTaskIgnoreCommand(task: WorkflowMonitorTask, reason?: string) {
    await runMonitorTaskCommand(task, monitorTaskIgnoreCommand(task, reason), `Started ignore flow for ${task.task_id}.`);
  }

  async function runMonitorTaskAction(task: WorkflowMonitorTask, action: WorkflowMonitorTaskAction) {
    await runMonitorTaskCommand(
      task,
      monitorTaskActionPrompt(task, action),
      `Started ${action.name} for ${task.task_id}.`
    );
  }

  function connectorBySlug(slug: string | null | undefined): WorkflowConnector | undefined {
    if (!slug) return undefined;
    return connectors.find((connector) => connector.connector_slug === slug);
  }

  function connectorTriggerSupported(connector: WorkflowConnector | undefined): boolean {
    if (!connector) return false;
    return connector.can_trigger_workflow ?? connector.can_subscribe;
  }

  function connectorActionSlugs(
    connector: WorkflowConnector | undefined,
    terms: string[],
    limit: number | null = 3
  ): string[] {
    const actions = connector?.action_slugs ?? [];
    const matching = terms.length === 0 ? [] : actions.filter((action) => matchesSearchTerms(terms, action.toLowerCase()));
    const visible = matching.length > 0 ? matching : actions;
    return limit === null ? visible : visible.slice(0, limit);
  }

  function connectorHiddenActionCount(connector: WorkflowConnector | undefined, visibleActions: string[]): number {
    return Math.max(0, (connector?.action_slugs.length ?? 0) - visibleActions.length);
  }

  function connectionTriggerSupported(connection: WorkflowConnection): boolean {
    return connection.can_trigger_workflow ?? connectorTriggerSupported(connectorBySlug(connection.connector_slug));
  }

  function connectionsForConnector(slug: string): WorkflowConnection[] {
    return connectionsByConnector.get(slug) ?? [];
  }

  function activeConnectionSlug(item: EditableWorkflow | null): string | null {
    if (!item || item.trigger.type !== "connection") return null;
    return item.trigger.connection_slug;
  }

  function connectionExists(slug: string): boolean {
    return connections.some((connection) => connection.slug === slug);
  }

  function connectionOptionLabel(connection: WorkflowConnection): string {
    const label = `${connection.slug} (${connection.connector_slug})`;
    return connectionTriggerSupported(connection) ? label : `${label} - no trigger`;
  }

  function searchTerms(query: string): string[] {
    return query.trim().toLowerCase().split(/\s+/).filter(Boolean);
  }

  function buildSearchText(parts: Array<string | null | undefined>): string {
    return parts
      .map((part) => (part ?? "").trim())
      .filter(Boolean)
      .join(" ")
      .toLowerCase();
  }

  function matchesSearchTerms(terms: string[], searchText: string): boolean {
    return terms.length === 0 || terms.every((term) => searchText.includes(term));
  }

  function groupConnectionsByConnector(items: WorkflowConnection[]): Map<string, WorkflowConnection[]> {
    const groups = new Map<string, WorkflowConnection[]>();
    for (const connection of items) {
      const group = groups.get(connection.connector_slug) ?? [];
      group.push(connection);
      groups.set(connection.connector_slug, group);
    }
    return groups;
  }

  function indexConnectors(
    items: WorkflowConnector[],
    existingConnections: Map<string, WorkflowConnection[]>
  ): ConnectorSearchRow[] {
    return items.map((connector) => {
      const connectorConnections = existingConnections.get(connector.connector_slug) ?? [];
      return {
        connector,
        searchText: buildSearchText([
          connector.connector_slug,
          connector.description,
          connector.skill,
          connectorRuntimeHints(connector).join(" "),
          connector.connect_command,
          connector.suggested_connection_slug,
          connectorCapabilitySearchText(connector),
          connectorTriggerSupported(connector) ? `${connectorDraftCommand(connector)} draft workflow new` : undefined,
          connectorTriggerSupported(connector) ? `${connectorAppendCommand(connector)} append file save workflow` : undefined,
          connectorConnections.map((connection) => `${connection.slug} ${connection.description}`).join(" "),
          "connect setup",
          connector.action_slugs.join(" ")
        ])
      };
    });
  }

  function connectorCapabilitySearchText(connector: WorkflowConnector): string {
    const terms = [];
    if (connector.requires_auth) terms.push("auth");
    if (connector.can_subscribe) terms.push("events", "subscribe");
    if (connector.can_proxy_agent) terms.push("proxy", "agent proxy");
    if (connector.action_slugs.length > 0) terms.push("actions", "has-actions");
    if (connectorTriggerSupported(connector)) {
      terms.push("trigger", "trigger-ready", "append", "file", "save");
    } else {
      terms.push("no trigger", "no-trigger", "setup-only");
    }
    return terms.join(" ");
  }

  function connectorRuntimeHints(connector: WorkflowConnector | undefined): string[] {
    return connector?.runtime_hints ?? [];
  }

  function connectorPresetActive(preset: ConnectorFilterPreset): boolean {
    return connectorQuery.trim().toLowerCase() === preset.query;
  }

  function indexConnections(items: WorkflowConnection[], catalog: ConnectorSearchRow[]): ConnectionSearchRow[] {
    const catalogBySlug = new Map(catalog.map((row) => [row.connector.connector_slug, row.connector]));
    return items.map((connection) => {
      const connector = catalogBySlug.get(connection.connector_slug);
      return {
        connection,
        searchText: buildSearchText([
          connection.slug,
          connection.description,
          connection.connector_slug,
          connection.state,
          connection.connect_command,
          connection.monitor_command,
          connectionTriggerSupported(connection) ? `${connectionDraftCommand(connection)} draft workflow new` : undefined,
          connectionTriggerSupported(connection) ? `${connectionAppendCommand(connection)} append file save workflow` : undefined,
          "connect repair reconnect",
          connection.has_consumer ? "consumer active active" : "consumer idle idle",
          connectionMonitorSupported(connection) ? "monitor monitorable" : undefined,
          connectionTriggerSupported(connection) ? "trigger trigger-ready" : "no trigger no-trigger setup-only",
          connector?.description,
          connector?.skill,
          connectorRuntimeHints(connector).join(" "),
          connector && connector.action_slugs.length > 0 ? "actions has-actions" : undefined,
          connector?.action_slugs.join(" ")
        ])
      };
    });
  }

  function indexWorkflowBindings(items: WorkflowBinding[], scope: string): WorkflowBindingSearchRow[] {
    return items.map((binding) => ({
      binding,
      searchText: buildSearchText([
        scope,
        binding.slug,
        binding.description,
        binding.connection_slug,
        binding.connector_slug,
        binding.status,
        binding.enabled ? "enabled active" : "paused disabled",
        binding.action_type,
        binding.action_path,
        binding.action_format,
        binding.filter_pattern,
        binding.monitor_memory_path,
        workflowBindingDeleteCommand(binding),
        "delete remove cleanup"
      ])
    }));
  }

  function workflowBindingDeleteCommand(binding: WorkflowBinding): string {
    return `/workflows delete ${binding.slug}`;
  }

  function indexMonitorTasks(items: WorkflowMonitorTask[]): MonitorTaskSearchRow[] {
    return items.map((task) => ({
      task,
      searchText: buildSearchText([
        "monitor task",
        task.task_id,
        task.subject,
        task.description,
        task.status,
        task.monitor_connection,
        task.monitor_connector,
        task.monitor_memory_path,
        monitorTaskActions(task).map((action) => `${action.name} ${action.prompt}`).join(" "),
        monitorTaskIgnoreReasons(task).join(" ")
      ])
    }));
  }

  function filterWorkflowBindings(rows: WorkflowBindingSearchRow[], terms: string[]): WorkflowBinding[] {
    return rows.filter((row) => matchesSearchTerms(terms, row.searchText)).map((row) => row.binding);
  }

  function filterConnections(rows: ConnectionSearchRow[], terms: string[]): WorkflowConnection[] {
    return rows.filter((row) => matchesSearchTerms(terms, row.searchText)).map((row) => row.connection);
  }

  function filterMonitorTasks(rows: MonitorTaskSearchRow[], terms: string[]): WorkflowMonitorTask[] {
    return rows.filter((row) => matchesSearchTerms(terms, row.searchText)).map((row) => row.task);
  }

  function filterConnectors(
    rows: ConnectorSearchRow[],
    terms: string[],
    fallbackTerms: string[] = [],
    plannedConnectionName: string | null = null
  ): WorkflowConnector[] {
    const directMatches = rows.filter((row) => matchesSearchTerms(terms, row.searchText));
    if (directMatches.length > 0 || !plannedConnectionName || fallbackTerms.length === 0) {
      if (directMatches.length > 0) return directMatches.map((row) => row.connector);
      const appendMatches = rows.filter((row) => connectorAppendQueryMatches(row.connector));
      if (appendMatches.length > 0) return appendMatches.map((row) => row.connector);
      return [];
    }
    return rows
      .filter((row) => matchesSearchTerms(fallbackTerms, row.searchText))
      .map((row) => row.connector);
  }

  function connectorPlannedConnectionName(connector: WorkflowConnector): string | null {
    if (!connectorQueryPlannedConnectionName || connectorQueryFallbackTerms.length === 0) return null;
    const hasDirectMatches = connectorSearchRows.some((row) => matchesSearchTerms(connectorQueryTerms, row.searchText));
    if (hasDirectMatches) return null;
    const row = connectorSearchRows.find((item) => item.connector.connector_slug === connector.connector_slug);
    if (!row || !matchesSearchTerms(connectorQueryFallbackTerms, row.searchText)) return null;
    return connectorQueryPlannedConnectionName;
  }

  function indexWorkflows(items: EditableWorkflow[]): WorkflowSearchRow[] {
    return items.map((item) => {
      const latest = workflowLatestRun(item.slug);
      return {
        workflow: item,
        searchText: buildSearchText([
          "workflow",
          item.slug,
          item.pipeline.name,
          item.pipeline.working_dir,
          item.enabled ? "enabled active" : "disabled paused",
          dirtyWorkflowSlugs.includes(item.slug) ? "dirty unsaved" : undefined,
          triggerSearchText(item),
          item.pipeline.nodes
            .map((node) =>
              [
                node.id,
                node.type,
                node.agent,
                node.model,
                node.prompt,
                (node.tools ?? []).join(" "),
                (node.depends_on ?? []).join(" ")
              ].join(" ")
            )
            .join(" "),
          latest ? workflowRunSearchText(latest) : undefined
        ])
      };
    });
  }

  function filterWorkflows(rows: WorkflowSearchRow[], terms: string[]): EditableWorkflow[] {
    return rows.filter((row) => matchesSearchTerms(terms, row.searchText)).map((row) => row.workflow);
  }

  function triggerSearchText(item: EditableWorkflow | WorkflowDefinition): string {
    if (item.trigger.type === "cron") return `cron ${item.trigger.cron}`;
    if (item.trigger.type === "connection") {
      return buildSearchText(["connection trigger", item.trigger.connection_slug, item.trigger.pattern]);
    }
    return buildSearchText(["subscription trigger", item.trigger.source_topic, item.trigger.pattern]);
  }

  function workflowRunSearchText(item: WorkflowRun): string {
    return buildSearchText([
      "run",
      String(item.idx),
      item.workflow_slug,
      item.status,
      item.error,
      JSON.stringify(item.trigger),
      item.trigger_key,
      item.nodes
        .map((node) => [node.id, node.status, node.output, node.error].join(" "))
        .join(" ")
    ]);
  }

  function indexRuns(items: WorkflowRun[]): WorkflowRunSearchRow[] {
    return items.map((item) => ({
      run: item,
      searchText: workflowRunSearchText(item)
    }));
  }

  function filterRuns(rows: WorkflowRunSearchRow[], terms: string[]): WorkflowRun[] {
    return rows.filter((row) => matchesSearchTerms(terms, row.searchText)).map((row) => row.run);
  }

  function updateNode(id: string, patch: Partial<EditablePipelineNode>) {
    updateCurrentWorkflow((item) => ({
      ...item,
      pipeline: {
        ...item.pipeline,
        nodes: item.pipeline.nodes.map((node) => (node.id === id ? { ...node, ...patch } : node))
      }
    }));
  }

  function changeProvider(id: string, provider: AgentProvider) {
    const meta = providerMeta(provider);
    updateNode(id, {
      type: provider,
      agent: meta.defaultAgent,
      model: meta.defaultModel,
      tools: defaultTools(provider),
      prompt: selectedNode?.prompt || defaultPrompt(provider)
    });
  }

  function selectNodeProvider(provider: AgentProvider) {
    if (!selectedNode) return;
    changeProvider(selectedNode.id, provider);
  }

  function focusProviderButton(provider: AgentProvider) {
    document.querySelector<HTMLButtonElement>(`[data-workflow-provider="${provider}"]`)?.focus();
  }

  function moveProviderSelection(provider: AgentProvider, offset: number) {
    const idx = providerOptions.findIndex((item) => item.id === provider);
    if (idx < 0) return;
    const next = providerOptions[(idx + offset + providerOptions.length) % providerOptions.length].id;
    selectNodeProvider(next);
    setTimeout(() => focusProviderButton(next), 0);
  }

  function handleProviderKeydown(event: KeyboardEvent, provider: AgentProvider) {
    if (event.key === "ArrowRight" || event.key === "ArrowDown") {
      event.preventDefault();
      moveProviderSelection(provider, 1);
    } else if (event.key === "ArrowLeft" || event.key === "ArrowUp") {
      event.preventDefault();
      moveProviderSelection(provider, -1);
    } else if (event.key === "Home") {
      event.preventDefault();
      const first = providerOptions[0].id;
      selectNodeProvider(first);
      setTimeout(() => focusProviderButton(first), 0);
    } else if (event.key === "End") {
      event.preventDefault();
      const last = providerOptions[providerOptions.length - 1].id;
      selectNodeProvider(last);
      setTimeout(() => focusProviderButton(last), 0);
    }
  }

  function addAgent(provider: AgentProvider) {
    if (!workflow) return;
    const meta = providerMeta(provider);
    const id = uniqueNodeId(provider);
    const dependency = selectedNodeId ?? workflow.pipeline.nodes.at(-1)?.id;
    const node: EditablePipelineNode = {
      id,
      type: provider,
      agent: meta.defaultAgent,
      model: meta.defaultModel,
      tools: defaultTools(provider),
      depends_on: dependency ? [dependency] : [],
      prompt: defaultPrompt(provider)
    };
    updateCurrentWorkflow((item) => ({
      ...item,
      pipeline: { ...item.pipeline, nodes: [...item.pipeline.nodes, node] }
    }));
    selectedNodeId = id;
  }

  function addToolNode(tool: ToolName = "Bash") {
    if (!workflow) return;
    const id = uniqueNodeId(tool.toLowerCase());
    const dependency = selectedNodeId ?? workflow.pipeline.nodes.at(-1)?.id;
    const node: EditablePipelineNode = {
      id,
      type: "tool",
      agent: `${tool} call`,
      tools: [tool],
      depends_on: dependency ? [dependency] : [],
      prompt: defaultToolPrompt(tool)
    };
    updateCurrentWorkflow((item) => ({
      ...item,
      pipeline: { ...item.pipeline, nodes: [...item.pipeline.nodes, node] }
    }));
    selectedNodeId = id;
  }

  function addMergeNode() {
    if (!workflow) return;
    const id = uniqueNodeId("merge");
    const lastTwo = workflow.pipeline.nodes.slice(-2).map((node) => node.id);
    const node: EditablePipelineNode = {
      id,
      type: "merge",
      agent: "Merge",
      tools: [],
      depends_on: lastTwo,
      prompt: ""
    };
    updateCurrentWorkflow((item) => ({
      ...item,
      pipeline: { ...item.pipeline, nodes: [...item.pipeline.nodes, node] }
    }));
    selectedNodeId = id;
  }

  function addFanoutNode() {
    if (!workflow) return;
    const id = uniqueNodeId("fanout");
    const dependency = selectedNodeId ?? workflow.pipeline.nodes.at(-1)?.id;
    const node: EditablePipelineNode = {
      id,
      type: "fanout",
      agent: "Fanout",
      tools: [],
      depends_on: dependency ? [dependency] : [],
      prompt: ""
    };
    updateCurrentWorkflow((item) => ({
      ...item,
      pipeline: { ...item.pipeline, nodes: [...item.pipeline.nodes, node] }
    }));
    selectedNodeId = id;
  }

  function defaultToolPrompt(tool: ToolName): string {
    if (tool === "Bash") return "echo \"hello from workflow\"";
    if (tool === "Read" || tool === "Write" || tool === "Edit") return "{ \"file_path\": \"\" }";
    if (tool === "Grep") return "{ \"pattern\": \"\", \"path\": \".\" }";
    if (tool === "Glob") return "{ \"pattern\": \"**/*\" }";
    if (tool === "WebFetch") return "{ \"url\": \"\", \"prompt\": \"\" }";
    if (tool === "WebSearch") return "{ \"query\": \"\" }";
    return "{}";
  }

  function nodeToolName(node: EditablePipelineNode): ToolName | "" {
    const value = node.tools?.[0];
    if (value && (TOOL_NAMES as readonly string[]).includes(value)) return value as ToolName;
    return "";
  }

  function setNodeToolName(id: string, value: ToolName) {
    updateNode(id, { tools: [value], agent: `${value} call`, prompt: defaultToolPrompt(value) });
  }

  function removeSelectedNode() {
    if (!workflow || !selectedNode) return;
    const removeId = selectedNode.id;
    updateCurrentWorkflow((item) => {
      const nodes = item.pipeline.nodes
        .filter((node) => node.id !== removeId)
        .map((node) => ({
          ...node,
          depends_on: (node.depends_on ?? []).filter((dep) => dep !== removeId)
        }));
      return { ...item, pipeline: { ...item.pipeline, nodes } };
    });
    selectedNodeId = null;
  }

  function toggleDependency(targetId: string, dependencyId: string, checked: boolean) {
    const target = workflow?.pipeline.nodes.find((node) => node.id === targetId);
    if (!target || target.id === dependencyId) return;
    const deps = new Set(target.depends_on ?? []);
    if (checked) deps.add(dependencyId);
    else deps.delete(dependencyId);
    updateNode(targetId, { depends_on: Array.from(deps) });
  }

  function createGraphEdge(fromId: string, toId: string) {
    if (!workflow || fromId === toId || toId === "trigger") return;
    const target = workflow.pipeline.nodes.find((node) => node.id === toId);
    if (!target) return;
    if (fromId === "trigger") {
      updateNode(toId, { depends_on: [] });
      selectedNodeId = toId;
      return;
    }
    if (!workflow.pipeline.nodes.some((node) => node.id === fromId)) return;
    const deps = new Set(target.depends_on ?? []);
    deps.add(fromId);
    updateNode(toId, { depends_on: Array.from(deps) });
    selectedNodeId = toId;
  }

  function connectSelectedTo(targetId: string) {
    if (!selectedNode || selectedNode.id === targetId) return;
    toggleDependency(targetId, selectedNode.id, true);
  }

  function dependsOn(node: EditablePipelineNode, dependencyId: string): boolean {
    return (node.depends_on ?? []).includes(dependencyId);
  }

  function toolsText(node: EditablePipelineNode): string {
    return (node.tools ?? []).join(", ");
  }

  function setTools(id: string, value: string) {
    updateNode(id, {
      tools: value
        .split(",")
        .map((part) => part.trim())
        .filter(Boolean)
    });
  }

  function uniqueNodeId(seed: string): string {
    const existing = new Set(editorWorkflows.flatMap((item) => item.pipeline.nodes.map((node) => node.id)));
    const base = seed === "claude"
      ? "claude-review"
      : seed === "puffer"
        ? "puffer-handoff"
        : seed === "codex"
          ? "codex-task"
          : `${seed}-step`;
    let id = base;
    let i = 2;
    while (existing.has(id)) id = `${base}-${i++}`;
    return id;
  }

  function nodesFor(item: EditableWorkflow): GraphNode[] {
    return [
      {
        id: "trigger",
        type: "trigger",
        title: triggerTitle(item),
        subtitle: item.enabled ? "enabled" : "disabled"
      },
      ...item.pipeline.nodes.map((node): GraphNode => {
        if (node.type === "tool") {
          const tool = nodeToolName(node) || "Tool";
          return {
            id: node.id,
            type: "tool",
            title: node.agent ?? `${tool} call`,
            subtitle: `tool · ${tool}`,
            node
          };
        }
        if (node.type === "merge") {
          return {
            id: node.id,
            type: "merge",
            title: node.agent ?? "Merge",
            subtitle: `merge · ${(node.depends_on ?? []).length} inputs`,
            node
          };
        }
        if (node.type === "fanout") {
          return {
            id: node.id,
            type: "fanout",
            title: node.agent ?? "Fanout",
            subtitle: "fanout · split branches",
            node
          };
        }
        return {
          id: node.id,
          type: "agent",
          title: node.agent ?? node.id,
          subtitle: `${providerMeta(node.type).short} · ${node.model ?? "default"}`,
          provider: node.type,
          node
        };
      })
    ];
  }

  function edgesFor(item: EditableWorkflow): GraphEdge[] {
    const edges: GraphEdge[] = [];
    for (const node of item.pipeline.nodes) {
      const deps = (node.depends_on ?? []).filter((dep) => item.pipeline.nodes.some((candidate) => candidate.id === dep));
      if (deps.length === 0) {
        edges.push({ from: "trigger", to: node.id });
      } else {
        for (const dep of deps) edges.push({ from: dep, to: node.id, label: "after" });
      }
    }
    return edges;
  }

  function triggerTitle(item: EditableWorkflow | WorkflowDefinition): string {
    if (item.trigger.type === "cron") return item.trigger.cron;
    if (item.trigger.type === "connection") return item.trigger.connection_slug;
    return item.trigger.source_topic;
  }

  function workflowLatestRun(slug: string): WorkflowRun | undefined {
    return snapshot.runs.find((item) => item.workflow_slug === slug);
  }

  function workflowRunStats(slug: string): WorkflowRunStats {
    const items = snapshot.runs.filter((item) => item.workflow_slug === slug);
    return {
      runs: items.length,
      success: items.filter((item) => item.status === "completed").length,
      failure: items.filter((item) => item.status === "failed").length
    };
  }

  function workflowPreviewNodes(item: EditableWorkflow): GraphNode[] {
    return nodesFor(item).slice(0, 7);
  }

  function workflowHiddenNodeCount(item: EditableWorkflow): number {
    return Math.max(0, nodesFor(item).length - workflowPreviewNodes(item).length);
  }

  function previewNodeLabel(node: GraphNode): string {
    if (node.type === "trigger") return "Trigger";
    if (node.provider) return providerMeta(node.provider).short;
    if (node.type === "tool" && node.node) return nodeToolName(node.node) || "Tool";
    if (node.type === "merge") return "Merge";
    if (node.type === "fanout") return "Fanout";
    return node.title;
  }

  function previewNodeStyle(node: GraphNode): string {
    return node.provider ? `--provider-accent: ${providerMeta(node.provider).accent};` : "";
  }

  function defaultNodePosition(id: string): NodePosition {
    const idx = Math.max(0, graphNodes.findIndex((node) => node.id === id));
    return { x: PAD_L + idx * COL_W, y: PAD_T };
  }

  function workflowNodePositions(): Record<string, NodePosition> {
    return nodePositions[workflow?.slug ?? ""] ?? {};
  }

  function nodePosition(id: string): NodePosition {
    return workflowNodePositions()[id] ?? defaultNodePosition(id);
  }

  function nodeXY(id: string) {
    const { x, y } = nodePosition(id);
    return { x, y, cx: x + NODE_W / 2, cy: y + NODE_H / 2 };
  }

  function graphBoundsFor(): { width: number; height: number } {
    let maxX = PAD_L + Math.max(0, graphNodes.length - 1) * COL_W + NODE_W;
    let maxY = PAD_T + NODE_H;
    for (const node of graphNodes) {
      const pos = nodePosition(node.id);
      maxX = Math.max(maxX, pos.x + NODE_W);
      maxY = Math.max(maxY, pos.y + NODE_H);
    }
    return {
      width: Math.max(PAD_L * 2 + NODE_W, maxX + PAD_L),
      height: Math.max(PAD_T * 2 + NODE_H, maxY + PAD_T)
    };
  }

  function setNodePosition(nodeId: string, position: NodePosition) {
    const slug = workflow?.slug;
    if (!slug) return;
    nodePositions = {
      ...nodePositions,
      [slug]: {
        ...(nodePositions[slug] ?? {}),
        [nodeId]: {
          x: Math.max(PAD_L, Math.round(position.x)),
          y: Math.max(PAD_T, Math.round(position.y))
        }
      }
    };
  }

  function graphPoint(event: PointerEvent): NodePosition {
    const rect = wrapEl?.getBoundingClientRect();
    if (!rect) return { x: PAD_L, y: PAD_T };
    return {
      x: Math.max(0, (event.clientX - rect.left) / Math.max(scale, 0.01)),
      y: Math.max(0, (event.clientY - rect.top) / Math.max(scale, 0.01))
    };
  }

  function pathFor(fromId: string, toId: string) {
    const pa = nodeXY(fromId);
    const pb = nodeXY(toId);
    const x1 = pa.x + NODE_W - 4;
    const y1 = pa.cy;
    const x2 = pb.x + 4;
    const y2 = pb.cy;
    const mx = (x1 + x2) / 2;
    return `M ${x1} ${y1} C ${mx} ${y1}, ${mx} ${y2}, ${x2} ${y2}`;
  }

  function draftPathFor(draft: EdgeDraftState): string {
    const pa = nodeXY(draft.fromId);
    const x1 = pa.x + NODE_W - 4;
    const y1 = pa.cy;
    const x2 = draft.x;
    const y2 = draft.y;
    const mx = (x1 + x2) / 2;
    return `M ${x1} ${y1} C ${mx} ${y1}, ${mx} ${y2}, ${x2} ${y2}`;
  }

  function edgeMidY(from: string, to: string) {
    const pa = nodeXY(from);
    const pb = nodeXY(to);
    return (pa.cy + pb.cy) / 2 - 16;
  }

  function edgeMidX(from: string, to: string) {
    const pa = nodeXY(from);
    const pb = nodeXY(to);
    const x1 = pa.x + NODE_W - 4;
    const x2 = pb.x + 4;
    return (x1 + x2) / 2;
  }

  function edgeClass(from: string, to: string) {
    const hasRunPath = visited.has(from) && visited.has(to);
    const selectedPath = selectedNodeId && (from === selectedNodeId || to === selectedNodeId);
    return "pf-pipe-edge" + (hasRunPath || selectedPath ? " on-path visited" : "");
  }

  function nodeState(node: GraphNode): string {
    if (node.id === selectedNodeId) return "active";
    if (node.id === "trigger") return run ? "visited" : "idle";
    const record = run?.nodes.find((item) => item.id === node.id);
    if (!record) return "idle";
    if (record.status === "running") return "active";
    if (record.status === "failed") return "failed";
    if (record.status === "skipped") return "blocked";
    if (record.status === "completed") return "visited";
    return visited.has(node.id) ? "queued" : "idle";
  }

  function handleNodePointerDown(event: PointerEvent, nodeId: string) {
    const target = event.target as HTMLElement | null;
    if (target?.closest("[data-edge-handle]")) {
      startEdgeDraft(event, nodeId);
      return;
    }
    startNodeDrag(event, nodeId);
  }

  function startNodeDrag(event: PointerEvent, nodeId: string) {
    event.preventDefault();
    event.stopPropagation();
    selectedNodeId = nodeId;
    nodeDrag = {
      nodeId,
      origin: nodePosition(nodeId),
      pointer: graphPoint(event)
    };
  }

  function startEdgeDraft(event: PointerEvent, fromId: string) {
    event.preventDefault();
    event.stopPropagation();
    const point = graphPoint(event);
    selectedNodeId = fromId;
    edgeDraft = { fromId, x: point.x, y: point.y };
  }

  function handleGraphPointerMove(event: PointerEvent) {
    if (nodeDrag) {
      const point = graphPoint(event);
      setNodePosition(nodeDrag.nodeId, {
        x: nodeDrag.origin.x + point.x - nodeDrag.pointer.x,
        y: nodeDrag.origin.y + point.y - nodeDrag.pointer.y
      });
    }
    if (edgeDraft) {
      const point = graphPoint(event);
      edgeDraft = { ...edgeDraft, x: point.x, y: point.y };
    }
  }

  function handleGraphPointerUp(event: PointerEvent) {
    if (edgeDraft) {
      const target = document
        .elementFromPoint(event.clientX, event.clientY)
        ?.closest<HTMLElement>("[data-node-id]");
      const toId = target?.dataset.nodeId;
      if (toId) createGraphEdge(edgeDraft.fromId, toId);
      edgeDraft = null;
    }
    nodeDrag = null;
  }

  function statusColor(status: WorkflowRunStatus): string {
    return (
      {
        pending: "var(--pf-run-skipped)",
        running: "var(--pf-run-running)",
        completed: "var(--pf-run-done)",
        failed: "var(--pf-run-failed)",
        skipped: "var(--pf-run-skipped)"
      } as const
    )[status];
  }

  function runElapsed(item: WorkflowRun): string {
    const end = item.ended_at_ms ?? Date.now();
    const seconds = Math.max(0, Math.round((end - item.started_at_ms) / 1000));
    if (seconds < 60) return `${seconds}s`;
    return `${Math.floor(seconds / 60)}m ${seconds % 60}s`;
  }

  function runWhen(item: WorkflowRun): string {
    return new Date(item.started_at_ms).toLocaleString([], {
      month: "short",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit"
    });
  }

  function nodeIcon(node: GraphNode): IconName {
    if (node.type === "trigger") {
      if (workflow?.trigger.type === "cron") return "clock";
      if (workflow?.trigger.type === "connection") return "plug";
      return "bolt";
    }
    if (node.type === "tool") return "terminal";
    if (node.type === "merge") return "link";
    if (node.type === "fanout") return "arrow";
    return "panel";
  }

  function stepLabel(node: WorkflowRunNode): string {
    if (node.status === "completed") return "completed";
    if (node.status === "failed") return "failed";
    if (node.status === "skipped") return "skipped";
    if (node.status === "running") return "running";
    return "pending";
  }
</script>

<svelte:window onpointermove={handleGraphPointerMove} onpointerup={handleGraphPointerUp} />

<div class="pf-pipe pf-pipe-editor">
  <div class="pf-pipe-top">
    <div class="pf-pipe-top-id">
      {#if workflowPage === "overview"}
        <strong>Workflows {workflows.length}</strong>
        {#if workflowQuery.trim()}
          <span class="pf-pipe-hash">{filteredWorkflows.length} shown</span>
        {/if}
      {:else}
        <span class="pf-pipe-chip">
          {workflowPage === "create" ? "Create workflow" : "Workflow detail"}
        </span>
        <strong>{workflow?.pipeline.name ?? "No workflow"}</strong>
      {/if}
      {#if workflow && workflowPage !== "overview"}
        <span class="pf-pipe-hash">{workflow.slug}</span>
      {/if}
      {#if workflowPage !== "overview"}
        <span class="pf-pipe-save-note">{saveNotice}</span>
      {/if}
    </div>
    <div class="pf-pipe-top-right">
      {#if workflowPage === "overview"}
        <label class="pf-workflow-top-search">
          <span class="pf-connector-searchbox">
            <Icon name="search" size={12} />
            <input
              aria-label="Search workflows"
              value={workflowQuery}
              placeholder="Search workflows"
              oninput={(event) => (workflowQuery = event.currentTarget.value)}
            />
          </span>
        </label>
      {/if}
      {#if workflowPage !== "overview"}
        <button
          type="button"
          class="sc-btn"
          data-variant="ghost"
          data-size="sm"
          aria-label="Back to workflows"
          onclick={backToWorkflowOverview}
        >
          <Icon name="chevL" size={12} />Back
        </button>
      {/if}
      {#if workflowPage === "overview"}
        <button
          type="button"
          class="sc-btn"
          data-variant="ghost"
          data-size="sm"
          aria-label="New workflow"
          onclick={() => createWorkflowDraft()}
        >
          <Icon name="plus" size={12} />New workflow
        </button>
      {/if}
      {#if workflowPage !== "overview"}
        <button
          type="button"
          class="sc-btn"
          data-variant="ghost"
          data-size="sm"
          aria-label={workflow?.enabled ? "Pause workflow" : "Resume workflow"}
          aria-busy={togglingWorkflowSlug === workflow?.slug}
          disabled={!workflow || workflowDirty || savingWorkflowSlug !== null || togglingWorkflowSlug !== null}
          title={workflowDirty ? "Save local edits before toggling" : workflow?.enabled ? "Pause workflow" : "Resume workflow"}
          onclick={toggleCurrentWorkflowEnabled}
        >
          <Icon name={workflow?.enabled ? "pause2" : "play"} size={12} />{workflow?.enabled ? "Pause" : "Resume"}
        </button>
        <button
          type="button"
          class="sc-btn"
          data-variant="ghost"
          data-size="sm"
          aria-label="Save workflow"
          aria-busy={savingWorkflowSlug === workflow?.slug}
          disabled={!workflow || !workflowDirty || savingWorkflowSlug !== null}
          onclick={saveCurrentWorkflow}
        >
          <Icon name="check" size={12} />{savingWorkflowSlug === workflow?.slug ? "Saving" : workflowDirty ? "Save" : "Saved"}
        </button>
      {/if}
    </div>
  </div>

  {#if workflowPage === "overview"}
    <div class="pf-workflow-overview" aria-label="Workflow overview">
      <section class="pf-workflow-panel pf-workflow-list-panel" aria-label="Workflow list">
        <div class="pf-workflow-table">
          {#if loading}
            <div class="pf-pipe-empty">Loading workflows...</div>
          {:else if filteredWorkflows.length === 0}
            <div class="pf-workflow-empty">No matching workflows.</div>
          {:else}
            {#each filteredWorkflows as item (item.slug)}
              {@const latest = workflowLatestRun(item.slug)}
              {@const stats = workflowRunStats(item.slug)}
              {@const preview = workflowPreviewNodes(item)}
              {@const hiddenCount = workflowHiddenNodeCount(item)}
              <button
                type="button"
                class="pf-workflow-row"
                data-selected={item.slug === workflow?.slug}
                onclick={() => openWorkflowDetail(item.slug)}
              >
                <span class="pf-run-pip {latest?.status ?? 'pending'}"></span>
                <span class="pf-workflow-row-main">
                  <strong>{item.pipeline.name}</strong>
                  <small>{item.slug} · {triggerTitle(item)}</small>
                </span>
                <span class="pf-workflow-pipeline-preview" aria-label="Pipeline preview">
                  {#each preview as node, index (node.id)}
                    <span
                      class="pf-workflow-node-pill"
                      data-type={node.type}
                      style={previewNodeStyle(node)}
                      title={node.title}
                    >
                      {previewNodeLabel(node)}
                    </span>
                    {#if index < preview.length - 1}
                      <span class="pf-workflow-node-arrow">-&gt;</span>
                    {/if}
                  {/each}
                  {#if hiddenCount > 0}
                    <span class="pf-workflow-node-more">+{hiddenCount}</span>
                  {/if}
                </span>
                <span class="pf-workflow-row-stats" aria-label="Workflow run stats">
                  <span class="pf-workflow-stat-inline">
                    <strong>{stats.runs}</strong>
                    <small>runs</small>
                  </span>
                  <span class="pf-workflow-stat-inline" data-kind="success">
                    <strong>{stats.success}</strong>
                    <small>success</small>
                  </span>
                  <span class="pf-workflow-stat-inline" data-kind="failure">
                    <strong>{stats.failure}</strong>
                    <small>failure</small>
                  </span>
                </span>
                <span class="pf-workflow-row-state" data-enabled={item.enabled}>{item.enabled ? "enabled" : "disabled"}</span>
                <Icon name="chevR" size={14} />
              </button>
            {/each}
          {/if}
        </div>
      </section>
    </div>
  {:else}
  <div class="pf-pipe-body pf-pipe-canvas-body" data-page={workflowPage}>
    <div class="pf-pipe-main pf-canvas-main">
      {#if workflow}
        <div class="pf-canvas-stage">
        <div class="pf-canvas-toolbar" role="group" aria-label="Add node">
          {#each providerOptions as provider (provider.id)}
            <button
              type="button"
              class="sc-btn"
              data-variant="outline"
              data-size="sm"
              aria-label={`Add ${provider.short} agent`}
              onclick={() => addAgent(provider.id)}
            >
              <Icon name="plus" size={12} />{provider.short}
            </button>
          {/each}
          <button
            type="button"
            class="sc-btn"
            data-variant="outline"
            data-size="sm"
            aria-label="Add tool call node"
            onclick={() => addToolNode()}
          >
            <Icon name="plus" size={12} />Tool
          </button>
          <button
            type="button"
            class="sc-btn"
            data-variant="outline"
            data-size="sm"
            aria-label="Add merge node"
            onclick={addMergeNode}
          >
            <Icon name="plus" size={12} />Merge
          </button>
          <button
            type="button"
            class="sc-btn"
            data-variant="outline"
            data-size="sm"
            aria-label="Add fanout node"
            onclick={addFanoutNode}
          >
            <Icon name="plus" size={12} />Fanout
          </button>
        </div>
        <div class="pf-pipe-graph-wrap pf-canvas-graph-wrap">
          <div bind:this={wrapEl} class="pf-pipe-graph-scaler" style="height: {graphHeight * scale}px;">
            <div
              class="pf-pipe-graph"
              style="width: {graphWidth}px; height: {graphHeight}px; transform: scale({scale}); transform-origin: top left;"
            >
              <svg class="pf-pipe-graph-svg" viewBox="0 0 {graphWidth} {graphHeight}" width={graphWidth} height={graphHeight}>
                <defs>
                  <marker id="pf-pipe-arr" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto">
                    <path d="M 0 0 L 10 5 L 0 10 z" fill="currentColor" />
                  </marker>
                </defs>
                {#each graphEdges as edge, i (i)}
                  {@const d = pathFor(edge.from, edge.to)}
                  {@const midY = edgeMidY(edge.from, edge.to)}
                  {@const midX = edgeMidX(edge.from, edge.to)}
                  <g class={edgeClass(edge.from, edge.to)}>
                    <path d={d} fill="none" stroke-width="1.6" />
                    <path d={d} fill="none" stroke-width="1.2" class="arr-head" marker-end="url(#pf-pipe-arr)" />
                    {#if edge.label}
                      <g transform="translate({midX}, {midY})">
                        <rect x="-32" y="-9" width="64" height="18" rx="9" fill="var(--background)" class="pf-pipe-edge-pill"></rect>
                        <text x="0" y="4" text-anchor="middle" font-size="10.5" font-family="var(--font-sans)" class="pf-pipe-edge-text">
                          {edge.label}
                        </text>
                      </g>
                    {/if}
                  </g>
                {/each}
                {#if edgeDraft}
                  <path class="pf-pipe-edge-draft" d={draftPathFor(edgeDraft)} fill="none" stroke-width="1.6" marker-end="url(#pf-pipe-arr)" />
                {/if}
              </svg>

              {#each graphNodes as node (node.id)}
                {@const p = nodeXY(node.id)}
                {@const st = nodeState(node)}
                <button
                  type="button"
                  class="pf-pipe-node pf-editor-node"
                  style="left: {p.x}px; top: {p.y}px; width: {NODE_W}px; height: {NODE_H}px; --provider-accent: {node.provider ? providerMeta(node.provider).accent : 'var(--puffer-accent)'};"
                  data-type={node.type}
                  data-provider={node.provider ?? "trigger"}
                  data-state={st}
                  data-node-id={node.id}
                  data-dragging={nodeDrag?.nodeId === node.id}
                  aria-pressed={node.id === selectedNodeId}
                  onpointerdown={(event) => handleNodePointerDown(event, node.id)}
                >
                  <div class="pf-pipe-node-head">
                    {#if node.type === "agent"}
                      <span class="pf-provider-avatar" data-provider={node.provider}>{providerMeta(node.provider ?? "puffer").short.slice(0, 1)}</span>
                    {:else}
                      <span class="pf-pipe-node-ico">
                        <Icon name={nodeIcon(node)} size={12} />
                      </span>
                    {/if}
                    <div class="pf-pipe-node-meta">
                      <span class="name">{node.title}</span>
                      <span class="sub">{node.subtitle}</span>
                    </div>
                  </div>
                  <span class="pf-pipe-node-stub left"></span>
                  <span
                    class="pf-pipe-node-stub right"
                    data-edge-handle="out"
                    title="Drag to connect"
                  ></span>
                </button>
              {/each}
            </div>
          </div>
        </div>

        </div>

        <div class="pf-canvas-selected" aria-label="Selected node">
          <section class="pf-canvas-section pf-canvas-section-agent">
            <div class="pf-editor-panel-head">
              <Icon name="panel" size={13} />
              <span>{triggerSelected ? "Trigger & workflow" : "Selected node"}</span>
              {#if selectedNodeId !== null}
                <button
                  type="button"
                  class="sc-btn pf-canvas-selected-close"
                  data-variant="ghost"
                  data-size="sm"
                  aria-label="Close selected node panel"
                  onclick={() => (selectedNodeId = null)}
                >
                  <Icon name="x" size={12} />Close
                </button>
              {/if}
            </div>
            {#if selectedNodeId === null}
              <div class="pf-pipe-empty">Click the trigger to set it up, click any node to edit it, or add one from the toolbar at the top of the canvas.</div>
            {/if}
            {#if triggerSelected}
              <div class="pf-canvas-selected-grid">
                <label>
                  <span>Workflow name</span>
                  <input value={workflow.pipeline.name} oninput={(event) => updateWorkflowField("name", event.currentTarget.value)} />
                </label>
                <label>
                  <span>Slug</span>
                  <input value={workflow.slug} oninput={(event) => updateWorkflowField("slug", event.currentTarget.value)} />
                </label>
                <label>
                  <span>Working directory</span>
                  <input value={workflow.pipeline.working_dir ?? ""} oninput={(event) => updateWorkflowField("working_dir", event.currentTarget.value)} />
                </label>
                <label class="pf-editor-inline">
                  <span>Enabled</span>
                  <input type="checkbox" checked={workflow.enabled} onchange={(event) => updateWorkflowField("enabled", event.currentTarget.checked)} />
                </label>
                <label>
                  <span>Trigger type</span>
                  <select value={workflow.trigger.type} onchange={(event) => setTriggerType(event.currentTarget.value as TriggerMode)}>
                    <option value="connection">Connection</option>
                    <option value="cron">Cron</option>
                  </select>
                </label>
                {#if workflow.trigger.type === "cron"}
                  <label class="pf-canvas-selected-wide">
                    <span>Cron</span>
                    <input value={workflow.trigger.cron} oninput={(event) => updateTriggerField("cron", event.currentTarget.value)} />
                  </label>
                {:else if workflow.trigger.type === "connection"}
                  <label>
                    <span>Connection</span>
                    <select
                      aria-label="Workflow connection"
                      value={workflow.trigger.connection_slug}
                      onchange={(event) => useConnectionTrigger(event.currentTarget.value)}
                    >
                      {#if !connectionExists(workflow.trigger.connection_slug)}
                        <option value={workflow.trigger.connection_slug}>{workflow.trigger.connection_slug} (planned)</option>
                      {/if}
                      {#each connections as connection (connection.slug)}
                        <option value={connection.slug} disabled={!connectionTriggerSupported(connection)}>
                          {connectionOptionLabel(connection)}
                        </option>
                      {/each}
                    </select>
                  </label>
                  <label>
                    <span>Pattern</span>
                    <input placeholder=".*" value={workflow.trigger.pattern ?? ""} oninput={(event) => updateTriggerField("pattern", event.currentTarget.value)} />
                  </label>
                {:else}
                  <label>
                    <span>Source topic</span>
                    <input value={workflow.trigger.source_topic} oninput={(event) => updateTriggerField("source_topic", event.currentTarget.value)} />
                  </label>
                  <label>
                    <span>Pattern</span>
                    <input placeholder=".*" value={workflow.trigger.pattern ?? ""} oninput={(event) => updateTriggerField("pattern", event.currentTarget.value)} />
                  </label>
                {/if}
              </div>
            {:else if selectedNode}
              <div class="pf-canvas-selected-grid">
              {#if selectedNode.type === "tool"}
                <label>
                  <span>Node id</span>
                  <input value={selectedNode.id} disabled />
                </label>
                <label>
                  <span>Tool</span>
                  <select
                    value={nodeToolName(selectedNode) || "Bash"}
                    onchange={(event) => setNodeToolName(selectedNode.id, event.currentTarget.value as ToolName)}
                  >
                    {#each TOOL_NAMES as tool (tool)}
                      <option value={tool}>{tool}</option>
                    {/each}
                  </select>
                </label>
                <label>
                  <span>Label</span>
                  <input value={selectedNode.agent ?? ""} oninput={(event) => updateNode(selectedNode.id, { agent: event.currentTarget.value })} />
                </label>
                <label class="pf-canvas-selected-wide">
                  <span>Input{nodeToolName(selectedNode) === "Bash" ? " (bash command)" : " (JSON)"}</span>
                  <textarea
                    rows="4"
                    value={selectedNode.prompt}
                    oninput={(event) => updateNode(selectedNode.id, { prompt: event.currentTarget.value })}
                  ></textarea>
                </label>
              {:else if selectedNode.type === "merge" || selectedNode.type === "fanout"}
                <label>
                  <span>Node id</span>
                  <input value={selectedNode.id} disabled />
                </label>
                <label>
                  <span>Label</span>
                  <input value={selectedNode.agent ?? ""} oninput={(event) => updateNode(selectedNode.id, { agent: event.currentTarget.value })} />
                </label>
                <div class="pf-pipe-empty pf-canvas-selected-wide">
                  {selectedNode.type === "merge"
                    ? "Merge collects all dependency outputs into one JSON array."
                    : "Fanout passes its upstream output through (parallel branching not yet implemented)."}
                </div>
              {:else}
                <div class="pf-provider-switcher pf-canvas-selected-wide" role="radiogroup" aria-label="Agent provider">
                  {#each providerOptions as provider (provider.id)}
                    <button
                      type="button"
                      role="radio"
                      data-selected={selectedNode.type === provider.id}
                      data-workflow-provider={provider.id}
                      aria-checked={selectedNode.type === provider.id}
                      tabindex={selectedNode.type === provider.id ? 0 : -1}
                      onclick={() => selectNodeProvider(provider.id)}
                      onkeydown={(event) => handleProviderKeydown(event, provider.id)}
                    >
                      {provider.label}
                    </button>
                  {/each}
                </div>
                <label>
                  <span>Node id</span>
                  <input value={selectedNode.id} disabled />
                </label>
                <label>
                  <span>Agent name</span>
                  <input value={selectedNode.agent ?? ""} oninput={(event) => updateNode(selectedNode.id, { agent: event.currentTarget.value })} />
                </label>
                <label>
                  <span>Model</span>
                  <input value={selectedNode.model ?? ""} oninput={(event) => updateNode(selectedNode.id, { model: event.currentTarget.value })} />
                </label>
                <label>
                  <span>Tools</span>
                  <input value={toolsText(selectedNode)} oninput={(event) => setTools(selectedNode.id, event.currentTarget.value)} />
                </label>
                <label class="pf-canvas-selected-wide">
                  <span>Prompt</span>
                  <textarea rows="4" value={selectedNode.prompt} oninput={(event) => updateNode(selectedNode.id, { prompt: event.currentTarget.value })}></textarea>
                </label>
              {/if}
              </div>
              {#if workflow.pipeline.nodes.length > 1}
                <div class="pf-canvas-wiring" aria-label="Node wiring">
                  <span class="pf-canvas-wiring-label">Inputs from</span>
                  <div class="pf-canvas-wiring-chips">
                    {#each workflow.pipeline.nodes.filter((node) => node.id !== selectedNode?.id) as node (node.id)}
                      <label class="pf-canvas-wiring-chip">
                        <input
                          type="checkbox"
                          checked={(selectedNode.depends_on ?? []).includes(node.id)}
                          onchange={(event) => toggleDependency(selectedNode.id, node.id, event.currentTarget.checked)}
                        />
                        <span class="pf-wire-provider" data-provider={node.type}>{nodeKindShort(node.type)}</span>
                        <span>{node.agent ?? node.id}</span>
                      </label>
                    {/each}
                  </div>
                </div>
              {/if}
            {/if}
            {#if selectedNode}
              <div class="pf-canvas-selected-actions">
                <button type="button" class="sc-btn" data-variant="ghost" data-size="sm" onclick={removeSelectedNode} disabled={workflow.pipeline.nodes.length === 0}>
                  <Icon name="x" size={12} />Remove node
                </button>
              </div>
            {/if}
          </section>
        </div>

        <div class="pf-canvas-runs-sheet" data-open={runsSheetOpen}>
          <button
            type="button"
            class="pf-canvas-runs-toggle"
            aria-expanded={runsSheetOpen}
            onclick={() => (runsSheetOpen = !runsSheetOpen)}
          >
            <Icon name="chevD" size={12} />
            <span>Runs</span>
            <span class="pf-pipe-traj-count">{runs.length}</span>
          </button>
          {#if runsSheetOpen}
            <div class="pf-canvas-runs-body">
            {#if runs.length === 0}
              <div class="pf-pipe-empty">No runs yet for this workflow.</div>
            {:else}
          <div class="pf-pipe-traj pf-editor-runs">
            <div class="pf-pipe-traj-head">
              <Icon name="terminal" size={12} />
              <span>Runs</span>
              <span class="pf-pipe-traj-count">{filteredRuns.length}/{runs.length}</span>
            </div>
            <label class="pf-run-search">
              <span class="pf-connector-searchbox">
                <Icon name="search" size={12} />
                <input
                  aria-label="Search workflow runs"
                  value={runQuery}
                  placeholder="Search runs"
                  oninput={(event) => (runQuery = event.currentTarget.value)}
                />
              </span>
            </label>
            <div class="pf-run-result-summary" aria-label="Workflow run search results">
              {filteredRuns.length}/{runs.length} runs
            </div>
            <div class="pf-pipe-run-list" aria-label="Workflow runs">
              {#if filteredRuns.length === 0}
                <div class="pf-pipe-empty">No matching runs.</div>
              {/if}
              {#each filteredRuns as item (item.idx)}
                <button
                  type="button"
                  class="pf-run-row"
                  data-selected={item.idx === run?.idx}
                  data-state={item.status}
                  onclick={() => selectRun(item.idx)}
                >
                  <div class="pf-run-head">
                    <span class="pf-run-pip {item.status}"></span>
                    <span class="pf-run-label">#{item.idx}</span>
                    <span class="pf-run-when">{runWhen(item)}</span>
                  </div>
                  <div class="pf-run-title">{item.status}</div>
                  <div class="pf-run-meta">
                    <span class="mono">{runElapsed(item)}</span>
                    <span class="sep">·</span>
                    <span>{item.nodes.length} steps</span>
                  </div>
                </button>
              {/each}
            </div>

            {#if run}
              <div class="pf-pipe-traj-head">
                <Icon name="terminal" size={12} />
                <span>Trajectory</span>
                <span class="pf-pipe-traj-count">{run.nodes.length} steps</span>
                <span style="flex: 1;"></span>
                <button
                  type="button"
                  class="sc-btn"
                  data-variant="ghost"
                  data-size="sm"
                  disabled={currentStepIndex <= 0}
                  onclick={() => stepToIdx(Math.max(0, currentStepIndex - 1))}
                  aria-label="Previous step"
                >
                  <Icon name="chevL" size={12} />
                </button>
                <button
                  type="button"
                  class="sc-btn"
                  data-variant="ghost"
                  data-size="sm"
                  disabled={currentStepIndex >= run.nodes.length - 1}
                  onclick={() => stepToIdx(Math.min(run.nodes.length - 1, currentStepIndex + 1))}
                  aria-label="Next step"
                >
                  <Icon name="chevR" size={12} />
                </button>
              </div>

              <div class="pf-pipe-traj-list">
                {#each run.nodes as node, i (i)}
                  <button
                    type="button"
                    class="pf-pipe-traj-row"
                    data-step={i}
                    data-current={i === currentStepIndex}
                    data-past={i < currentStepIndex}
                    data-status={node.status}
                    onclick={() => stepToIdx(i)}
                  >
                    <span class="t">#{i + 1}</span>
                    <span class="rail">
                      <span class="dot" data-kind="agent" data-status={node.status}></span>
                    </span>
                    <span><span class="lane-chip agent">{node.id}</span></span>
                    <span class="body">
                      <span class="body-title">{stepLabel(node)}</span>
                      {#if node.output}
                        <span class="body-arg">{node.output}</span>
                      {:else if node.error}
                        <span class="body-arg">{node.error}</span>
                      {/if}
                    </span>
                    <span class="status {node.status}">
                      {#if node.status === "running"}
                        <span class="dot-live"></span>running
                      {:else}
                        {node.status}
                      {/if}
                    </span>
                  </button>
                {/each}
              </div>
            {/if}
          </div>
        {/if}
            </div>
          {/if}
        </div>
      {:else}
        <div class="pf-pipe-empty">Create a workflow to start wiring agents.</div>
      {/if}
    </div>
  </div>
  {/if}
</div>

<style>
  .pf-pipe-empty {
    color: var(--muted-foreground);
    font-size: 12px;
    padding: 14px;
  }

  .pf-pipe-save-note {
    color: var(--muted-foreground);
    font-size: 11.5px;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .pf-pipe-canvas-body {
    grid-template-columns: 230px minmax(0, 1fr);
  }

  .pf-pipe-canvas-body[data-page="detail"],
  .pf-pipe-canvas-body[data-page="create"] {
    grid-template-columns: minmax(0, 1fr);
  }

  .pf-workflow-overview {
    flex: 1;
    min-height: 0;
    display: flex;
    padding: 0;
    overflow: hidden;
    background: color-mix(in oklab, var(--background) 96%, var(--muted));
  }

  .pf-workflow-panel {
    min-width: 0;
    min-height: 0;
    background: var(--background);
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .pf-workflow-list-panel {
    flex: 1;
  }

  .pf-workflow-top-search {
    min-width: min(360px, 42vw);
  }

  .pf-workflow-top-search .pf-connector-searchbox {
    width: 100%;
  }

  .pf-workflow-table {
    display: grid;
    gap: 0;
    padding: 0;
    overflow: auto;
  }

  .pf-workflow-row {
    all: unset;
    box-sizing: border-box;
    width: 100%;
    border-bottom: 1px solid var(--border);
    background: transparent;
    color: var(--foreground);
    cursor: pointer;
    min-width: 0;
  }

  .pf-workflow-row {
    display: grid;
    grid-template-columns: auto minmax(180px, 0.85fr) minmax(260px, 1.4fr) auto auto auto;
    align-items: center;
    gap: 12px;
    padding: 12px;
  }

  .pf-workflow-row:hover,
  .pf-workflow-row:focus-visible {
    background: color-mix(in oklab, var(--puffer-accent) 7%, transparent);
  }

  .pf-workflow-row-main {
    display: grid;
    gap: 2px;
    min-width: 0;
  }

  .pf-workflow-row-main strong {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 13px;
  }

  .pf-workflow-row-main small {
    color: var(--muted-foreground);
    font-size: 11px;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .pf-workflow-row-state {
    justify-self: end;
    color: var(--muted-foreground);
    font-size: 11px;
    font-weight: 700;
    white-space: nowrap;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .pf-workflow-row-state[data-enabled="true"] {
    color: var(--puffer-accent);
  }

  .pf-workflow-pipeline-preview {
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 6px;
    overflow: hidden;
  }

  .pf-workflow-node-pill,
  .pf-workflow-node-more {
    min-width: 0;
    max-width: 118px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    padding: 0;
    color: var(--foreground);
    font-size: 11px;
    font-weight: 700;
    line-height: 1.2;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    flex: 0 1 auto;
  }

  .pf-workflow-node-pill[data-type="agent"] {
    color: var(--provider-accent, var(--puffer-accent));
  }

  .pf-workflow-node-pill[data-type="trigger"] {
    color: var(--muted-foreground);
  }

  .pf-workflow-node-arrow {
    color: var(--muted-foreground);
    font-family: var(--font-mono);
    font-size: 10px;
    flex: 0 0 auto;
    opacity: 0.65;
  }

  .pf-workflow-node-more {
    color: var(--muted-foreground);
    flex: 0 0 auto;
  }

  .pf-workflow-row-stats {
    display: grid;
    grid-template-columns: repeat(3, minmax(56px, auto));
    align-items: center;
    justify-items: end;
    gap: 8px;
  }

  .pf-workflow-stat-inline {
    display: grid;
    gap: 1px;
    text-align: right;
    line-height: 1.1;
  }

  .pf-workflow-stat-inline strong {
    color: var(--foreground);
    font-size: 13px;
    font-weight: 700;
  }

  .pf-workflow-stat-inline small {
    color: var(--muted-foreground);
    font-size: 10px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .pf-workflow-stat-inline[data-kind="success"] strong {
    color: var(--pf-run-done);
  }

  .pf-workflow-stat-inline[data-kind="failure"] strong {
    color: var(--pf-run-failed);
  }

  .pf-workflow-empty {
    color: var(--muted-foreground);
    font-size: 12px;
    padding: 14px;
  }

  .pf-provider-avatar {
    width: 24px;
    height: 24px;
    border-radius: 8px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    font-size: 11px;
    font-weight: 700;
    color: var(--provider-accent, var(--puffer-accent));
    background: color-mix(in oklab, var(--provider-accent, var(--puffer-accent)) 14%, var(--background));
    border: 1px solid color-mix(in oklab, var(--provider-accent, var(--puffer-accent)) 30%, transparent);
  }

  .pf-provider-avatar[data-provider="codex"] { --provider-accent: oklch(0.58 0.18 245); }
  .pf-provider-avatar[data-provider="claude"] { --provider-accent: oklch(0.64 0.17 55); }
  .pf-provider-avatar[data-provider="puffer"] { --provider-accent: var(--puffer-accent); }

  .pf-editor-graph-wrap {
    overflow-x: auto;
  }

  .pf-editor-node {
    text-align: left;
    cursor: grab;
  }

  .pf-editor-node[data-provider="codex"] {
    border-color: color-mix(in oklab, oklch(0.58 0.18 245) 42%, var(--border));
  }

  .pf-editor-node[data-provider="claude"] {
    border-color: color-mix(in oklab, oklch(0.64 0.17 55) 42%, var(--border));
  }

  .pf-editor-node[data-provider="puffer"] {
    border-color: color-mix(in oklab, var(--puffer-accent) 42%, var(--border));
  }

  .pf-editor-node[data-state="active"] {
    border-color: var(--provider-accent, var(--puffer-accent));
    box-shadow: 0 1px 2px rgb(0 0 0 / 0.05);
  }

  .pf-editor-lower {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: minmax(190px, 0.8fr) minmax(260px, 1.2fr) minmax(220px, 0.9fr);
    gap: 10px;
    padding: 10px;
    overflow: auto;
    background: color-mix(in oklab, var(--background) 96%, var(--muted));
  }

  .pf-editor-panel {
    min-width: 0;
    border: 1px solid var(--border);
    border-radius: 12px;
    background: var(--background);
    padding: 10px;
    display: flex;
    flex-direction: column;
    gap: 9px;
    box-shadow: 0 8px 24px -22px rgb(0 0 0 / 0.3);
  }

  .pf-editor-panel-head {
    display: flex;
    align-items: center;
    gap: 7px;
    min-height: 28px;
    color: var(--muted-foreground);
    font-size: 11px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.06em;
  }

  .pf-editor-panel-head .sc-btn {
    margin-left: auto;
  }

  .pf-editor-panel label {
    display: flex;
    flex-direction: column;
    gap: 5px;
    color: var(--muted-foreground);
    font-size: 11px;
    font-weight: 600;
  }

  .pf-editor-panel input,
  .pf-editor-panel select,
  .pf-editor-panel textarea {
    width: 100%;
    box-sizing: border-box;
    border: 1px solid var(--border);
    background: var(--card);
    color: var(--foreground);
    border-radius: 8px;
    padding: 7px 9px;
    font: inherit;
    font-size: 12px;
    outline: none;
  }

  .pf-editor-panel textarea {
    resize: vertical;
    min-height: 92px;
    line-height: 1.45;
  }

  .pf-editor-panel input:focus,
  .pf-editor-panel select:focus,
  .pf-editor-panel textarea:focus {
    border-color: var(--puffer-accent);
    box-shadow: 0 0 0 2px color-mix(in oklab, var(--puffer-accent) 18%, transparent);
  }

  .pf-editor-inline {
    flex-direction: row !important;
    align-items: center;
    justify-content: space-between;
  }

  .pf-editor-inline input {
    width: auto;
  }

  .pf-provider-switcher {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 3px;
    padding: 3px;
    border: 1px solid color-mix(in oklab, var(--border) 78%, transparent);
    border-radius: 8px;
    background: color-mix(in oklab, var(--background) 84%, var(--muted));
  }

  .pf-provider-switcher button {
    border: 1px solid transparent;
    background: transparent;
    color: var(--muted-foreground);
    border-radius: 6px;
    padding: 6px 7px;
    cursor: pointer;
    font: inherit;
    font-size: 11px;
    font-weight: 600;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .pf-provider-switcher button[data-selected="true"] {
    border-color: color-mix(in oklab, var(--puffer-accent) 20%, var(--border));
    color: var(--foreground);
    background: var(--background);
    font-weight: 700;
    box-shadow: 0 1px 2px rgb(0 0 0 / 0.04);
  }

  .pf-provider-switcher button:hover {
    color: var(--foreground);
    background: color-mix(in oklab, var(--background) 72%, var(--muted));
  }

  .pf-wire-target {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 8px 9px;
    border-radius: 9px;
    background: var(--muted);
    font-size: 11px;
    color: var(--muted-foreground);
  }

  .pf-wire-target strong {
    color: var(--foreground);
    font-size: 12px;
  }

  .pf-wire-target.outbound {
    margin-top: 6px;
  }

  .pf-wire-row {
    display: grid !important;
    grid-template-columns: auto auto 1fr;
    align-items: center;
    gap: 8px !important;
    padding: 7px 8px;
    border: 1px solid var(--border);
    border-radius: 9px;
    color: var(--foreground) !important;
  }

  .pf-wire-row input {
    width: auto;
  }

  .pf-wire-provider {
    display: inline-flex;
    justify-content: center;
    min-width: 44px;
    border-radius: 999px;
    border: 1px solid color-mix(in oklab, var(--border) 78%, transparent);
    padding: 1px 6px;
    font-size: 10px;
    color: var(--muted-foreground);
    background: var(--background);
  }

  .pf-wire-provider[data-provider="codex"] { color: oklch(0.58 0.18 245); }
  .pf-wire-provider[data-provider="claude"] { color: oklch(0.58 0.16 55); }
  .pf-wire-provider[data-provider="puffer"] { color: var(--puffer-accent); }

  .pf-wire-connect {
    border: 1px solid var(--border);
    background: var(--card);
    color: var(--foreground);
    border-radius: 8px;
    padding: 7px 9px;
    font: inherit;
    font-size: 12px;
    cursor: pointer;
    display: flex;
    align-items: center;
    gap: 7px;
    text-align: left;
  }

  .pf-wire-connect:hover {
    border-color: transparent;
    background: var(--pf-selected-bg-hover);
    font-weight: 700;
  }

  .pf-wire-connect:disabled {
    cursor: default;
    color: var(--muted-foreground);
    border-color: color-mix(in oklab, var(--border) 80%, var(--puffer-accent));
    background: color-mix(in oklab, var(--puffer-accent) 8%, var(--card));
    font-weight: 600;
  }

  .pf-wire-connect:disabled::after {
    content: "connected";
    margin-left: auto;
    color: var(--puffer-accent);
    font-size: 10px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  .pf-connector-picker {
    border-top: 1px solid var(--border);
    margin-top: 2px;
    padding-top: 9px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .pf-connector-search {
    gap: 5px !important;
  }

  .pf-connector-result-summary {
    color: var(--muted-foreground);
    font-size: 11px;
    line-height: 1.3;
    padding: 0 1px 1px;
  }

  .pf-connector-searchbox {
    display: grid;
    grid-template-columns: auto 1fr;
    align-items: center;
    gap: 6px;
    border: 1px solid var(--border);
    background: var(--card);
    color: var(--muted-foreground);
    border-radius: 8px;
    padding: 0 8px;
  }

  .pf-connector-searchbox:focus-within {
    border-color: var(--puffer-accent);
    box-shadow: 0 0 0 2px color-mix(in oklab, var(--puffer-accent) 18%, transparent);
  }

  .pf-connector-searchbox input {
    border: 0;
    box-shadow: none;
    background: transparent;
    padding: 7px 0;
  }

  .pf-connector-searchbox input:focus {
    border: 0;
    box-shadow: none;
  }

  .pf-connector-filters {
    display: flex;
    flex-wrap: wrap;
    gap: 5px;
  }

  .pf-connector-filters button {
    border: 1px solid var(--border);
    border-radius: 999px;
    background: var(--card);
    color: var(--muted-foreground);
    font: inherit;
    font-size: 10.5px;
    font-weight: 700;
    line-height: 1;
    padding: 5px 8px;
    cursor: pointer;
  }

  .pf-connector-filters button:hover,
  .pf-connector-filters button[aria-pressed="true"] {
    border-color: color-mix(in oklab, var(--puffer-accent) 34%, var(--border));
    background: color-mix(in oklab, var(--puffer-accent) 11%, var(--card));
    color: var(--foreground);
  }

  .pf-connection-list,
  .pf-connector-catalog {
    display: grid;
    gap: 5px;
  }

  .pf-connector-catalog {
    max-height: 164px;
    overflow: auto;
  }

  .pf-monitor-workflows,
  .pf-monitor-tasks {
    display: grid;
    gap: 5px;
    border-top: 1px solid var(--border);
    padding-top: 6px;
  }

  .pf-monitor-tasks-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    color: var(--muted-foreground);
    font-size: 10.5px;
    font-weight: 800;
    text-transform: uppercase;
    letter-spacing: 0;
  }

  .pf-monitor-tasks-head span {
    display: inline-flex;
    align-items: center;
    gap: 5px;
  }

  .pf-monitor-tasks-head small {
    color: var(--muted-foreground);
    font-size: 10.5px;
  }

  .pf-monitor-workflow-row,
  .pf-monitor-task-row {
    display: grid;
    gap: 5px;
    border: 1px solid color-mix(in oklab, var(--puffer-accent) 22%, var(--border));
    border-radius: 8px;
    background: color-mix(in oklab, var(--puffer-accent) 5%, var(--card));
    padding: 5px;
  }

  .pf-monitor-workflow-row {
    grid-template-columns: minmax(0, 1fr) auto;
    align-items: center;
  }

  .pf-monitor-workflow-row[data-enabled="false"] {
    border-color: var(--border);
    background: var(--card);
  }

  .pf-monitor-workflow-main {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    gap: 8px;
    align-items: start;
    min-width: 0;
  }

  .pf-monitor-task-main {
    all: unset;
    box-sizing: border-box;
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    gap: 8px;
    align-items: start;
    cursor: pointer;
  }

  .pf-monitor-task-main:hover:not(:disabled) strong,
  .pf-monitor-task-main:focus-visible strong {
    color: var(--puffer-accent);
  }

  .pf-monitor-task-main:disabled {
    opacity: 0.64;
    cursor: default;
  }

  .pf-monitor-task-detail {
    color: var(--muted-foreground);
    font-size: 10.7px;
    line-height: 1.35;
    overflow: hidden;
    display: -webkit-box;
    line-clamp: 2;
    -webkit-line-clamp: 2;
    -webkit-box-orient: vertical;
  }

  .pf-monitor-task-actions {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 5px;
  }

  .pf-monitor-row-actions {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    flex-wrap: wrap;
    gap: 5px;
    min-width: 0;
  }

  .pf-monitor-action-btn {
    all: unset;
    box-sizing: border-box;
    min-height: 24px;
    max-width: 100%;
    display: inline-flex;
    align-items: center;
    gap: 4px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--card);
    color: var(--foreground);
    padding: 3px 7px;
    font-size: 10.5px;
    font-weight: 700;
    cursor: pointer;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .pf-monitor-action-btn:hover:not(:disabled),
  .pf-monitor-action-btn:focus-visible {
    border-color: color-mix(in oklab, var(--puffer-accent) 34%, var(--border));
    background: color-mix(in oklab, var(--puffer-accent) 10%, var(--card));
  }

  .pf-monitor-delete-btn:hover:not(:disabled),
  .pf-monitor-delete-btn:focus-visible {
    border-color: color-mix(in oklab, var(--danger, oklch(0.55 0.2 30)) 38%, var(--border));
    background: color-mix(in oklab, var(--danger, oklch(0.55 0.2 30)) 9%, var(--card));
    color: color-mix(in oklab, var(--danger, oklch(0.55 0.2 30)) 75%, var(--foreground));
  }

  .pf-monitor-action-btn:disabled {
    opacity: 0.56;
    cursor: default;
  }

  .pf-connection-row-group,
  .pf-connector-row-group {
    display: grid;
    align-items: stretch;
    gap: 5px;
  }

  .pf-connection-row-group {
    grid-template-columns: minmax(0, 1fr) 30px 30px 30px 30px;
  }

  .pf-connector-row-group {
    grid-template-columns: minmax(0, 1fr) 30px 30px;
  }

  .pf-connection-row {
    border: 1px solid var(--border);
    background: var(--card);
    color: var(--foreground);
    border-radius: 8px;
    padding: 7px 8px;
    font: inherit;
    cursor: pointer;
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    gap: 8px;
    min-width: 0;
    overflow: hidden;
    text-align: left;
  }

  .pf-connection-row-group .pf-icon-btn,
  .pf-connector-row-group .pf-icon-btn {
    width: 30px;
    height: auto;
    min-height: 100%;
    border-radius: 8px;
  }

  .pf-connection-row:hover,
  .pf-connection-row[data-selected="true"] {
    border-color: transparent;
    background: var(--pf-selected-bg-hover);
  }

  .pf-connection-row:disabled {
    cursor: not-allowed;
    opacity: 0.58;
  }

  .pf-connection-row:disabled:hover {
    border-color: var(--border);
    background: var(--card);
  }

  .pf-connection-row[data-selected="true"] {
    box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--puffer-accent) 45%, transparent);
  }

  .pf-connector-row {
    border: 1px solid var(--border);
    background: color-mix(in oklab, var(--card) 72%, transparent);
    color: var(--foreground);
    border-radius: 8px;
    padding: 7px 8px;
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    gap: 8px;
    align-items: start;
    font: inherit;
    text-align: left;
    cursor: pointer;
    min-width: 0;
    overflow: hidden;
  }

  .pf-connector-row:hover,
  .pf-connector-row[data-selected="true"] {
    border-color: transparent;
    background: var(--pf-selected-bg-hover);
  }

  .pf-connector-row[data-selected="true"] {
    box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--puffer-accent) 45%, transparent);
  }

  .pf-connector-row[data-supported="false"] {
    color: var(--muted-foreground);
  }

  .pf-connector-row[data-supported="false"]:hover,
  .pf-connector-row[data-supported="false"][data-selected="true"] {
    border-color: color-mix(in oklab, var(--puffer-accent) 22%, var(--border));
    background: color-mix(in oklab, var(--puffer-accent) 6%, var(--card));
  }

  .pf-connector-main {
    min-width: 0;
    display: grid;
    gap: 2px;
  }

  .pf-connector-main strong {
    color: var(--foreground);
    font-size: 12px;
    line-height: 1.2;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .pf-connector-main small {
    color: var(--muted-foreground);
    font-size: 10.7px;
    line-height: 1.32;
  }

  .pf-connection-state,
  .pf-connector-tags {
    display: inline-flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: 4px;
    min-width: 0;
    color: var(--muted-foreground);
    font-size: 10px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0;
  }

  .pf-connection-state,
  .pf-connector-tags span {
    border: 1px solid var(--border);
    border-radius: 999px;
    padding: 2px 6px;
    background: var(--muted);
  }

  .pf-connection-state[data-state="active"],
  .pf-connection-state[data-state="authenticated"] {
    color: var(--puffer-accent);
    background: color-mix(in oklab, var(--puffer-accent) 10%, var(--card));
    border-color: color-mix(in oklab, var(--puffer-accent) 28%, var(--border));
  }

  .pf-connection-state[data-state="degraded"] {
    color: var(--pf-run-failed);
    background: color-mix(in oklab, var(--pf-run-failed) 10%, var(--card));
    border-color: color-mix(in oklab, var(--pf-run-failed) 28%, var(--border));
  }

  .pf-connector-empty {
    color: var(--muted-foreground);
    font-size: 11px;
    padding: 8px;
    border: 1px dashed var(--border);
    border-radius: 8px;
    background: var(--card);
  }

  .pf-connector-setup {
    display: grid;
    gap: 6px;
  }

  .pf-connector-detail {
    border: 1px solid var(--border);
    border-radius: 8px;
    background: color-mix(in oklab, var(--card) 82%, transparent);
    padding: 7px 8px;
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    gap: 8px;
    align-items: start;
    min-width: 0;
  }

  .pf-connector-name {
    display: grid !important;
    grid-template-columns: minmax(0, 0.62fr) minmax(0, 1fr);
    align-items: center;
    gap: 7px !important;
  }

  .pf-connector-name input[aria-invalid="true"] {
    border-color: var(--pf-run-failed);
    box-shadow: 0 0 0 2px color-mix(in oklab, var(--pf-run-failed) 14%, transparent);
  }

  .pf-connector-validation {
    color: var(--pf-run-failed);
    font-size: 10.5px;
    font-weight: 600;
  }

  .pf-connector-command {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto;
    align-items: center;
    gap: 6px;
    border: 1px solid color-mix(in oklab, var(--puffer-accent) 28%, var(--border));
    background: color-mix(in oklab, var(--puffer-accent) 8%, var(--card));
    color: var(--muted-foreground);
    border-radius: 8px;
    padding: 7px 8px;
  }

  .pf-connector-command code {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--foreground);
    font-size: 11px;
  }

  .pf-connector-command-actions {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .pf-icon-btn {
    all: unset;
    box-sizing: border-box;
    width: 24px;
    height: 24px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border-radius: 6px;
    border: 1px solid var(--border);
    background: var(--card);
    color: var(--muted-foreground);
    cursor: pointer;
  }

  .pf-icon-btn:hover:not(:disabled),
  .pf-icon-btn:focus-visible {
    color: var(--foreground);
    border-color: color-mix(in oklab, var(--puffer-accent) 32%, var(--border));
    background: color-mix(in oklab, var(--puffer-accent) 10%, var(--card));
  }

  .pf-icon-btn:disabled {
    opacity: 0.56;
    cursor: default;
  }

  .pf-editor-runs {
    max-height: 260px;
    border-top: 1px solid var(--border);
  }

  .pf-run-search {
    display: block;
    padding: 0 10px 4px;
  }

  .pf-run-result-summary {
    color: var(--muted-foreground);
    font-size: 11px;
    line-height: 1.3;
    padding: 0 10px 4px;
  }

  .pf-pipe-run-list {
    display: grid;
    gap: 8px;
    padding: 10px;
  }

  .pf-pipe-traj-row {
    all: unset;
    display: grid;
    grid-template-columns: 54px 22px 88px 1fr auto;
    align-items: center;
    gap: 10px;
    padding: 5px 16px;
    font-size: 12.5px;
    cursor: pointer;
    border-left: 2px solid transparent;
    transition: background 100ms;
    min-height: 30px;
  }

  @media (max-width: 1120px) {
    .pf-pipe-canvas-body { grid-template-columns: 190px minmax(0, 1fr); }
    .pf-pipe-canvas-body[data-page="detail"],
    .pf-pipe-canvas-body[data-page="create"] {
      grid-template-columns: minmax(0, 1fr);
    }
    .pf-editor-lower { grid-template-columns: 1fr; }
    .pf-workflow-row {
      grid-template-columns: auto minmax(160px, 0.9fr) minmax(220px, 1.2fr) auto auto;
    }
    .pf-workflow-row-state { display: none; }
  }

  @media (max-width: 880px) {
    .pf-pipe-canvas-body { grid-template-columns: minmax(0, 1fr); }
    .pf-pipe-save-note { display: none; }
    .pf-editor-lower { padding: 8px; }
    .pf-workflow-top-search { min-width: 0; width: min(320px, 48vw); }
    .pf-workflow-overview { padding: 0; }
    .pf-workflow-row {
      grid-template-columns: auto minmax(0, 1fr) auto;
      align-items: start;
    }
    .pf-workflow-pipeline-preview {
      grid-column: 2 / -1;
      flex-wrap: wrap;
      overflow: visible;
    }
    .pf-workflow-row-stats {
      grid-column: 2 / -1;
      justify-content: start;
      justify-items: start;
    }
  }
</style>

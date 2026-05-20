<script lang="ts">
  import "../design/pipeline.css";

  import { onMount } from "svelte";
  import { loadWorkflowSnapshot } from "../api/desktop";
  import Icon, { type IconName } from "../design/Icon.svelte";
  import Puffer from "../design/Puffer.svelte";
  import type {
    WorkflowDefinition,
    WorkflowPipelineNode,
    WorkflowRun,
    WorkflowRunNode,
    WorkflowRunStatus,
    WorkflowSnapshot
  } from "../types";

  type AgentProvider = "codex" | "claude" | "puffer";
  type EditablePipelineNode = WorkflowPipelineNode & { type: AgentProvider };
  type EditableWorkflow = Omit<WorkflowDefinition, "pipeline"> & {
    pipeline: Omit<WorkflowDefinition["pipeline"], "nodes"> & { nodes: EditablePipelineNode[] };
  };

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
    type: "trigger" | "agent";
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

  let snapshot = $state<WorkflowSnapshot>({ workflows: [], runs: [] });
  let editorWorkflows = $state<EditableWorkflow[]>([starterWorkflow()]);
  let workflowSlug = $state("agent-review-pipeline");
  let selectedNodeId = $state<string | null>("codex-implement");
  let runIdx = $state<number | null>(null);
  let stepIdx = $state<number | null>(null);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let refreshGeneration = 0;
  let dirtyWorkflowSlugs = $state<string[]>([]);
  let saveNotice = $state("Draft changes are local until workflow save lands in the daemon.");

  let workflows = $derived(editorWorkflows);
  let workflow = $derived(
    workflows.find((item) => item.slug === workflowSlug) ?? workflows[0] ?? null
  );
  let runs = $derived(
    workflow ? snapshot.runs.filter((run) => run.workflow_slug === workflow.slug) : []
  );
  let run = $derived(runs.find((item) => item.idx === runIdx) ?? runs[0] ?? null);
  let graphNodes = $derived(workflow ? nodesFor(workflow) : []);
  let graphEdges = $derived(workflow ? edgesFor(workflow) : []);
  let graphWidth = $derived(PAD_L * 2 + Math.max(0, graphNodes.length - 1) * COL_W + NODE_W);
  let graphHeight = $derived(PAD_T * 2 + NODE_H);
  let currentStepIndex = $derived(
    run ? (stepIdx === null ? Math.max(0, run.nodes.length - 1) : Math.min(stepIdx, run.nodes.length - 1)) : 0
  );
  let currentNode = $derived(run?.nodes[currentStepIndex] ?? null);
  let visited = $derived(new Set((run?.nodes ?? []).slice(0, currentStepIndex + 1).map((node) => node.id)));
  let activeNode = $derived(currentNode?.id ?? "");
  let isLive = $derived(run?.status === "running" && stepIdx === null);
  let selectedNode = $derived(
    workflow?.pipeline.nodes.find((node) => node.id === selectedNodeId) ?? workflow?.pipeline.nodes[0] ?? null
  );

  let wrapEl = $state<HTMLDivElement | undefined>();
  let scale = $state(0.8);

  function starterWorkflow(): EditableWorkflow {
    return {
      schema: "puffer.workflow.v1",
      slug: "agent-review-pipeline",
      enabled: true,
      trigger: { type: "subscription", source_topic: "workspace.task.created", pattern: "review|implement|ship" },
      pipeline: {
        name: "Agent review pipeline",
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

  function defaultTools(provider: AgentProvider): string[] {
    if (provider === "claude") return ["read", "diff", "bash"];
    if (provider === "puffer") return ["workflow", "memory", "git"];
    return ["read", "edit", "bash", "mcp"];
  }

  function defaultPrompt(provider: AgentProvider): string {
    if (provider === "claude") return "Review the upstream result and call out risks before the pipeline proceeds.";
    if (provider === "puffer") return "Coordinate local runtime state and produce a clean handoff summary.";
    return "Implement the assigned pipeline step with a focused patch and verification notes.";
  }

  function measure() {
    if (!wrapEl || graphWidth <= 0) return;
    const cw = wrapEl.clientWidth;
    if (!cw) return;
    scale = Math.min(1, cw / graphWidth);
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
    stepIdx = null;
  });

  $effect(() => {
    workflow;
    graphWidth;
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
        runs: [...next.runs].sort((a, b) => b.idx - a.idx)
      };
      editorWorkflows = merged;
      if (!workflowSlug || !editorWorkflows.some((item) => item.slug === workflowSlug)) {
        workflowSlug = editorWorkflows[0]?.slug ?? "agent-review-pipeline";
      }
      const activeWorkflow = editorWorkflows.find((item) => item.slug === workflowSlug) ?? editorWorkflows[0];
      if (!activeWorkflow?.pipeline.nodes.some((node) => node.id === selectedNodeId)) {
        selectedNodeId = activeWorkflow?.pipeline.nodes[0]?.id ?? null;
      }
    } catch (err) {
      if (generation !== refreshGeneration) return;
      error = err instanceof Error ? err.message : String(err);
      if (editorWorkflows.length === 0) editorWorkflows = [starterWorkflow()];
      workflowSlug = editorWorkflows[0].slug;
      selectedNodeId = editorWorkflows[0].pipeline.nodes[0]?.id ?? null;
    } finally {
      if (generation === refreshGeneration) {
        loading = false;
        setTimeout(measure, 0);
      }
    }
  }

  function selectWorkflow(slug: string) {
    workflowSlug = slug;
    const next = editorWorkflows.find((item) => item.slug === slug);
    selectedNodeId = next?.pipeline.nodes[0]?.id ?? null;
  }

  function selectRun(idx: number) {
    runIdx = idx;
    stepIdx = null;
  }

  function stepToIdx(i: number) {
    stepIdx = i;
  }

  function updateCurrentWorkflow(mutator: (item: EditableWorkflow) => EditableWorkflow) {
    if (!workflow) return;
    const dirtySlug = workflow.slug;
    editorWorkflows = editorWorkflows.map((item) => (item.slug === dirtySlug ? mutator(item) : item));
    if (!dirtyWorkflowSlugs.includes(dirtySlug)) {
      dirtyWorkflowSlugs = [...dirtyWorkflowSlugs, dirtySlug];
    }
    saveNotice = "Edited locally. Save/export wiring can use this workflow shape.";
  }

  function updateWorkflowField(field: "slug" | "enabled" | "name" | "working_dir" | "concurrency", value: string | boolean | number | null) {
    if (!workflow) return;
    const oldSlug = workflow.slug;
    updateCurrentWorkflow((item) => {
      if (field === "slug") return { ...item, slug: String(value || "pipeline") };
      if (field === "enabled") return { ...item, enabled: Boolean(value) };
      if (field === "name") return { ...item, pipeline: { ...item.pipeline, name: String(value) } };
      if (field === "working_dir") return { ...item, pipeline: { ...item.pipeline, working_dir: String(value) } };
      return { ...item, pipeline: { ...item.pipeline, concurrency: Number(value) || 1 } };
    });
    if (field === "slug") workflowSlug = String(value || oldSlug);
  }

  function updateTriggerField(field: "source_topic" | "pattern" | "cron", value: string) {
    updateCurrentWorkflow((item) => {
      if (item.trigger.type === "cron") return { ...item, trigger: { type: "cron", cron: value || "0 * * * *" } };
      return {
        ...item,
        trigger: {
          ...item.trigger,
          [field === "cron" ? "source_topic" : field]: value
        }
      };
    });
  }

  function setTriggerType(type: "subscription" | "cron") {
    updateCurrentWorkflow((item) => ({
      ...item,
      trigger:
        type === "cron"
          ? { type: "cron", cron: "0 9 * * 1-5" }
          : { type: "subscription", source_topic: "workspace.task.created", pattern: "*" }
    }));
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
    document.querySelector<HTMLButtonElement>(`[data-pipeline-provider="${provider}"]`)?.focus();
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
    selectedNodeId = workflow.pipeline.nodes.find((node) => node.id !== removeId)?.id ?? null;
  }

  function toggleDependency(targetId: string, dependencyId: string, checked: boolean) {
    const target = workflow?.pipeline.nodes.find((node) => node.id === targetId);
    if (!target || target.id === dependencyId) return;
    const deps = new Set(target.depends_on ?? []);
    if (checked) deps.add(dependencyId);
    else deps.delete(dependencyId);
    updateNode(targetId, { depends_on: Array.from(deps) });
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

  function uniqueNodeId(provider: AgentProvider): string {
    const existing = new Set(editorWorkflows.flatMap((item) => item.pipeline.nodes.map((node) => node.id)));
    const base = `${provider}-${provider === "claude" ? "review" : provider === "puffer" ? "handoff" : "task"}`;
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
      ...item.pipeline.nodes.map((node) => ({
        id: node.id,
        type: "agent" as const,
        title: node.agent ?? node.id,
        subtitle: `${providerMeta(node.type).short} · ${node.model ?? "default"}`,
        provider: node.type,
        node
      }))
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
    return item.trigger.source_topic;
  }

  function workflowLatestRun(slug: string): WorkflowRun | undefined {
    return snapshot.runs.find((item) => item.workflow_slug === slug);
  }

  function nodeXY(id: string) {
    const idx = Math.max(0, graphNodes.findIndex((node) => node.id === id));
    const x = PAD_L + idx * COL_W;
    return { x, y: PAD_T, cx: x + NODE_W / 2, cy: PAD_T + NODE_H / 2 };
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
      return workflow?.trigger.type === "cron" ? "clock" : "bolt";
    }
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

<div class="pf-pipe pf-pipe-editor">
  <div class="pf-pipe-top">
    <div class="pf-pipe-top-id">
      <span class="pf-pipe-chip">Pipeline editor</span>
      <strong>{workflow?.pipeline.name ?? "No pipeline"}</strong>
      {#if workflow}
        <span class="pf-pipe-hash">{workflow.slug}</span>
      {/if}
      <span class="pf-pipe-save-note">{saveNotice}</span>
    </div>
    <div class="pf-pipe-top-right">
      {#each providerOptions as provider (provider.id)}
        <button type="button" class="sc-btn" data-variant="ghost" data-size="sm" onclick={() => addAgent(provider.id)}>
          <Icon name="plus" size={12} />{provider.short}
        </button>
      {/each}
      <button
        type="button"
        class="sc-btn"
        data-variant="ghost"
        data-size="sm"
        aria-label="Refresh workflows"
        aria-busy={loading}
        disabled={loading}
        onclick={refresh}
      >
        <Icon name="refresh" size={12} />{loading ? "Refreshing" : "Refresh"}
      </button>
    </div>
  </div>

  <div class="pf-pipe-body pf-pipe-editor-body">
    <div class="pf-pipe-runs pf-pipe-workflows">
      <div class="pf-pipe-runs-head">
        <span>Workflows</span>
        <span class="count">{workflows.length}</span>
      </div>
      {#if loading}
        <div class="pf-pipe-empty">Loading workflows...</div>
      {:else if error}
        <div class="pf-pipe-empty">Daemon workflow list unavailable. Editing a local draft.</div>
      {/if}

      {#each workflows as item (item.slug)}
        {@const latest = workflowLatestRun(item.slug)}
        <button
          type="button"
          class="pf-run-row"
          data-selected={item.slug === workflow?.slug}
          data-state={latest?.status ?? "pending"}
          onclick={() => selectWorkflow(item.slug)}
        >
          <div class="pf-run-head">
            <span class="pf-run-pip {latest?.status ?? 'pending'}"></span>
            <span class="pf-run-label">{item.slug}</span>
            <span class="pf-run-when">{item.enabled ? "enabled" : "disabled"}</span>
          </div>
          <div class="pf-run-title">{item.pipeline.name}</div>
          <div class="pf-run-meta">
            <span>{triggerTitle(item)}</span>
            <span class="sep">·</span>
            <span class="mono">{item.pipeline.nodes.length} nodes</span>
          </div>
        </button>
      {/each}

      <div class="pf-provider-palette">
        <div class="pf-pipe-runs-head compact">
          <span>Agent lanes</span>
        </div>
        {#each providerOptions as provider (provider.id)}
          <button type="button" class="pf-provider-card" data-provider={provider.id} onclick={() => addAgent(provider.id)}>
            <span class="pf-provider-mark" style:--provider-accent={provider.accent}>{provider.short.slice(0, 1)}</span>
            <span>
              <strong>{provider.label}</strong>
              <small>{provider.description}</small>
            </span>
          </button>
        {/each}
      </div>
    </div>

    <div class="pf-pipe-main">
      {#if workflow}
        <div class="pf-run-header pf-editor-header">
          <span class="pf-run-header-pip" style="background: {run ? statusColor(run.status) : 'var(--puffer-accent)'};"></span>
          <span class="pf-run-header-label">{workflow.pipeline.name}</span>
          <span class="pf-run-header-state" data-state={workflow.enabled ? "done" : "skipped"}>{workflow.enabled ? "enabled" : "disabled"}</span>
          <span class="pf-run-header-title">Wire Codex, Claude Code, and Puffer into one handoff graph.</span>
          <span class="pf-run-header-meta-group">
            <span class="pf-run-header-dim"><Icon name="wrench" size={11} /><span class="mono">{workflow.pipeline.nodes.length} nodes</span></span>
            <span class="pf-run-header-dim"><Icon name="link" size={11} /><span class="mono">{graphEdges.length} wires</span></span>
          </span>
        </div>

        <div class="pf-pipe-graph-wrap pf-editor-graph-wrap">
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
                  aria-pressed={node.id === selectedNodeId}
                  onclick={() => node.node ? (selectedNodeId = node.id) : null}
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
                    {#if node.id === selectedNodeId}
                      <span class="pf-pipe-node-halo"></span>
                    {/if}
                  </div>
                  <span class="pf-pipe-node-stub left"></span>
                  <span class="pf-pipe-node-stub right"></span>
                </button>
              {/each}
            </div>
          </div>
        </div>

        <div class="pf-editor-lower">
          <section class="pf-editor-panel pf-editor-config">
            <div class="pf-editor-panel-head">
              <Icon name="settings" size={13} />
              <span>Pipeline</span>
            </div>
            <label>
              <span>Name</span>
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
              <select value={workflow.trigger.type} onchange={(event) => setTriggerType(event.currentTarget.value as "subscription" | "cron")}>
                <option value="subscription">Subscription</option>
                <option value="cron">Cron</option>
              </select>
            </label>
            {#if workflow.trigger.type === "cron"}
              <label>
                <span>Cron</span>
                <input value={workflow.trigger.cron} oninput={(event) => updateTriggerField("cron", event.currentTarget.value)} />
              </label>
            {:else}
              <label>
                <span>Source topic</span>
                <input value={workflow.trigger.source_topic} oninput={(event) => updateTriggerField("source_topic", event.currentTarget.value)} />
              </label>
              <label>
                <span>Pattern</span>
                <input value={workflow.trigger.pattern ?? ""} oninput={(event) => updateTriggerField("pattern", event.currentTarget.value)} />
              </label>
            {/if}
          </section>

          <section class="pf-editor-panel pf-editor-inspector">
            <div class="pf-editor-panel-head">
              <Icon name="panel" size={13} />
              <span>Selected agent</span>
              {#if selectedNode}
                <button type="button" class="sc-btn" data-variant="ghost" data-size="sm" onclick={removeSelectedNode} disabled={workflow.pipeline.nodes.length <= 1}>
                  <Icon name="x" size={12} />Remove
                </button>
              {/if}
            </div>
            {#if selectedNode}
              <div class="pf-provider-switcher" role="radiogroup" aria-label="Agent provider">
                {#each providerOptions as provider (provider.id)}
                  <button
                    type="button"
                    role="radio"
                    data-selected={selectedNode.type === provider.id}
                    data-pipeline-provider={provider.id}
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
              <label>
                <span>Prompt</span>
                <textarea rows="5" value={selectedNode.prompt} oninput={(event) => updateNode(selectedNode.id, { prompt: event.currentTarget.value })}></textarea>
              </label>
            {:else}
              <div class="pf-pipe-empty">Select an agent node to edit it.</div>
            {/if}
          </section>

          <section class="pf-editor-panel pf-editor-wiring">
            <div class="pf-editor-panel-head">
              <Icon name="link" size={13} />
              <span>Wiring</span>
            </div>
            {#if selectedNode}
              <div class="pf-wire-target">
                <span>Inputs into</span>
                <strong>{selectedNode.agent ?? selectedNode.id}</strong>
              </div>
              {#each workflow.pipeline.nodes.filter((node) => node.id !== selectedNode?.id) as node (node.id)}
                <label class="pf-wire-row">
                  <input
                    type="checkbox"
                    checked={(selectedNode.depends_on ?? []).includes(node.id)}
                    onchange={(event) => toggleDependency(selectedNode.id, node.id, event.currentTarget.checked)}
                  />
                  <span class="pf-wire-provider" data-provider={node.type}>{providerMeta(node.type).short}</span>
                  <span>{node.agent ?? node.id}</span>
                </label>
              {/each}

              <div class="pf-wire-target outbound">
                <span>Send selected output to</span>
              </div>
              {#each workflow.pipeline.nodes.filter((node) => node.id !== selectedNode?.id) as node (node.id)}
                {@const alreadyConnected = dependsOn(node, selectedNode.id)}
                <button
                  type="button"
                  class="pf-wire-connect"
                  onclick={() => connectSelectedTo(node.id)}
                  disabled={alreadyConnected}
                  aria-pressed={alreadyConnected}
                >
                  <Icon name="link" size={12} />
                  {node.agent ?? node.id}
                </button>
              {/each}
            {:else}
              <div class="pf-pipe-empty">Select a node to wire dependencies.</div>
            {/if}
          </section>
        </div>

        {#if runs.length > 0}
          <div class="pf-pipe-traj pf-editor-runs">
            <div class="pf-pipe-traj-head">
              <Icon name="terminal" size={12} />
              <span>Runs</span>
              <span class="pf-pipe-traj-count">{runs.length}</span>
            </div>
            <div class="pf-pipe-run-list">
              {#each runs as item (item.idx)}
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
      {:else}
        <div class="pf-pipe-empty">Create a pipeline to start wiring agents.</div>
      {/if}
    </div>
  </div>
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

  .pf-pipe-editor-body {
    grid-template-columns: 230px minmax(0, 1fr);
  }

  .pf-pipe-workflows {
    padding-bottom: 16px;
  }

  .pf-provider-palette {
    border-top: 1px solid var(--border);
    margin-top: 10px;
    padding-top: 8px;
  }

  .pf-pipe-runs-head.compact {
    padding-bottom: 6px;
  }

  .pf-provider-card {
    all: unset;
    box-sizing: border-box;
    display: flex;
    gap: 9px;
    width: 100%;
    padding: 9px 10px;
    margin-bottom: 4px;
    border: 1px solid transparent;
    border-radius: 10px;
    cursor: pointer;
  }

  .pf-provider-card:hover {
    background: var(--pf-selected-bg-hover);
    border-color: transparent;
  }

  .pf-provider-mark,
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

  .pf-provider-card strong {
    display: block;
    font-size: 12px;
    line-height: 1.2;
  }

  .pf-provider-card small {
    display: block;
    color: var(--muted-foreground);
    font-size: 10.8px;
    line-height: 1.35;
    margin-top: 2px;
  }

  .pf-editor-header {
    min-height: 52px;
  }

  .pf-editor-graph-wrap {
    overflow-x: auto;
  }

  .pf-editor-node {
    text-align: left;
    cursor: pointer;
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
    box-shadow:
      0 0 0 3px color-mix(in oklab, var(--provider-accent, var(--puffer-accent)) 20%, transparent),
      0 12px 26px -16px color-mix(in oklab, var(--provider-accent, var(--puffer-accent)) 60%, transparent);
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
    gap: 6px;
  }

  .pf-provider-switcher button {
    border: 1px solid var(--border);
    background: var(--card);
    color: var(--muted-foreground);
    border-radius: 8px;
    padding: 7px 8px;
    cursor: pointer;
    font: inherit;
    font-size: 11.5px;
    font-weight: 600;
  }

  .pf-provider-switcher button[data-selected="true"] {
    border-color: transparent;
    color: var(--foreground);
    background: var(--pf-selected-bg);
    font-weight: 700;
  }

  .pf-provider-switcher button:hover {
    border-color: transparent;
    color: var(--foreground);
    background: var(--pf-selected-bg-hover);
    font-weight: 700;
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
    min-width: 48px;
    border-radius: 999px;
    border: 1px solid var(--border);
    padding: 2px 7px;
    font-size: 10px;
    color: var(--muted-foreground);
    background: var(--muted);
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

  .pf-editor-runs {
    max-height: 260px;
    border-top: 1px solid var(--border);
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
    .pf-pipe-editor-body { grid-template-columns: 190px minmax(0, 1fr); }
    .pf-editor-lower { grid-template-columns: 1fr; }
  }

  @media (max-width: 880px) {
    .pf-pipe-editor-body { grid-template-columns: minmax(0, 1fr); }
    .pf-pipe-workflows { display: none; }
    .pf-pipe-save-note { display: none; }
    .pf-editor-lower { padding: 8px; }
  }
</style>

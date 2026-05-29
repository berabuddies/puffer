<script lang="ts">
  import "../design/tasks.css";

  import { onMount } from "svelte";
  import { createMonitor, ignoreMonitorTask, loadWorkflowSnapshot, saveMonitorMemory } from "../api/desktop";
  import Icon from "../design/Icon.svelte";
  import type {
    WorkflowBinding,
    WorkflowConnection,
    WorkflowFilterRule,
    WorkflowMonitorMemory,
    WorkflowMonitorTask,
    WorkflowMonitorTaskAction,
    WorkflowSnapshot,
    WorkflowTask,
    WorkflowTaskSource
  } from "../types";

  type Props = {
    onRunTaskCommand?: (command: string) => boolean | Promise<boolean>;
  };

  type SourceFilter = "all" | "ignored" | WorkflowTaskSource;

  let { onRunTaskCommand }: Props = $props();

  let snapshot = $state<WorkflowSnapshot>({
    workflows: [],
    runs: [],
    tasks: [],
    monitor_tasks: [],
    monitor_memories: [],
    task_error: null,
    monitor_task_error: null,
    monitor_memory_error: null,
    monitor_ignore_filter_error: null
  });
  let loading = $state(false);
  let error = $state<string | null>(null);
  let notice = $state("");
  let query = $state("");
  let sourceFilter = $state<SourceFilter>("all");
  let statusFilter = $state("all");
  let commandRunningFor = $state<string | null>(null);
  let ignoreMenuTaskId = $state<string | null>(null);
  let showTaskConfig = $state(false);
  let selectedMonitorConnection = $state("");
  let creatingMonitor = $state(false);
  let configMemoryPath = $state("");
  let selectedFilterBindingSlug = $state("");
  let memoryDraft = $state("");
  let savingMemoryPath = $state<string | null>(null);
  let refreshGeneration = 0;

  let tasks = $derived(normalizedTasks());
  let searchTerms = $derived(query.trim().toLowerCase().split(/\s+/).filter(Boolean));
  let statusOptions = $derived([
    "all",
    ...Array.from(
      new Set(
        tasks
          .filter((task) => sourceFilter === "ignored" ? taskIgnored(task) : !taskIgnored(task))
          .map(taskStatusValue)
      )
    ).sort()
  ]);
  let visibleTasks = $derived(filteredTasks());
  let nonIgnoredCount = $derived(tasks.filter((task) => !taskIgnored(task)).length);
  let agentCount = $derived(tasks.filter((task) => task.source === "agent" && !taskIgnored(task)).length);
  let monitorCount = $derived(tasks.filter((task) => task.source === "monitor" && !taskIgnored(task)).length);
  let ignoredCount = $derived(tasks.filter(taskIgnored).length);
  let activeCount = $derived(tasks.filter((task) => !taskIgnored(task) && !taskTerminal(task)).length);
  let monitorMemories = $derived(snapshot.monitor_memories ?? []);
  let monitorFilterBindings = $derived(
    (snapshot.workflow_bindings ?? []).filter((binding) => binding.monitor)
  );
  let selectedConfigMemory = $derived(
    monitorMemories.find((memory) => memory.path === configMemoryPath) ?? monitorMemories[0] ?? null
  );
  let selectedFilterBinding = $derived(
    monitorFilterBindings.find((binding) => binding.slug === selectedFilterBindingSlug)
      ?? monitorFilterBindings[0]
      ?? null
  );
  let monitorConnections = $derived((snapshot.connections ?? []).filter(canCreateMonitor));
  let monitorConnectionWarnings = $derived(warningMonitorConnections());
  let selectedMonitorConnectionRecord = $derived(
    monitorConnections.find((connection) => connection.slug === selectedMonitorConnection) ?? null
  );
  let selectedMonitorNeedsRepair = $derived(
    selectedMonitorConnectionRecord ? connectionNeedsRepair(selectedMonitorConnectionRecord) : false
  );

  onMount(() => {
    void refresh();
  });

  $effect(() => {
    if (!showTaskConfig) return;
    if (monitorConnections.some((connection) => connection.slug === selectedMonitorConnection)) return;
    selectedMonitorConnection = monitorConnections[0]?.slug ?? "";
  });

  $effect(() => {
    if (!showTaskConfig) return;
    if (monitorMemories.length === 0) {
      configMemoryPath = "";
      memoryDraft = "";
      return;
    }
    if (monitorMemories.some((memory) => memory.path === configMemoryPath)) return;
    chooseConfigMemory(monitorMemories[0].path);
  });

  $effect(() => {
    if (!showTaskConfig) return;
    if (monitorFilterBindings.some((binding) => binding.slug === selectedFilterBindingSlug)) return;
    selectedFilterBindingSlug = monitorFilterBindings[0]?.slug ?? "";
  });

  $effect(() => {
    if (!statusOptions.includes(statusFilter)) {
      statusFilter = "all";
    }
  });

  async function refresh() {
    if (loading) return;
    const generation = ++refreshGeneration;
    loading = true;
    error = null;
    try {
      const next = await loadWorkflowSnapshot();
      if (generation !== refreshGeneration) return;
      applySnapshot(next);
      notice = "Task snapshot refreshed.";
    } catch (err) {
      if (generation !== refreshGeneration) return;
      const message = err instanceof Error ? err.message : String(err);
      error = message;
      notice = `Could not load tasks: ${message}`;
    } finally {
      if (generation === refreshGeneration) loading = false;
    }
  }

  function applySnapshot(next: WorkflowSnapshot) {
    snapshot = {
      ...next,
      tasks: next.tasks ?? [],
      monitor_tasks: next.monitor_tasks ?? [],
      monitor_memories: next.monitor_memories ?? [],
      connections: next.connections ?? [],
      task_error: next.task_error ?? null,
      monitor_task_error: next.monitor_task_error ?? null,
      monitor_memory_error: next.monitor_memory_error ?? null,
      monitor_ignore_filter_error: next.monitor_ignore_filter_error ?? null
    };
    ignoreMenuTaskId = null;
  }

  function normalizedTasks(): WorkflowTask[] {
    const explicit = snapshot.tasks ?? [];
    const rows = explicit.length > 0
      ? explicit
      : (snapshot.monitor_tasks ?? []).map(taskFromMonitor);
    return [...rows].sort((left, right) => taskSortTime(right) - taskSortTime(left) || left.task_id.localeCompare(right.task_id));
  }

  function taskFromMonitor(task: WorkflowMonitorTask): WorkflowTask {
    return {
      ...task,
      source: "monitor",
      task_scope: "monitor",
      task_scope_label: "monitors",
      task_type: "task",
      active_form: task.subject
    };
  }

  function filteredTasks(): WorkflowTask[] {
    return tasks.filter((task) => {
      const ignored = taskIgnored(task);
      if (sourceFilter === "ignored") {
        if (!ignored) return false;
      } else {
        if (ignored) return false;
        if (sourceFilter !== "all" && task.source !== sourceFilter) return false;
      }
      if (statusFilter !== "all" && taskStatusValue(task) !== statusFilter) return false;
      if (searchTerms.length === 0) return true;
      const haystack = [
        task.task_id,
        task.subject,
        task.description,
        task.status,
        task.source,
        task.task_type,
        task.owner,
        task.command,
        task.monitor_connection,
        task.monitor_connector,
        task.monitor_memory_path,
        (task.actions ?? []).map((action) => `${action.name} ${action.prompt}`).join(" "),
        (task.possible_ignore_reasons ?? []).join(" ")
      ]
        .filter(Boolean)
        .join(" ")
        .toLowerCase();
      return searchTerms.every((term) => haystack.includes(term));
    });
  }

  function canCreateMonitor(connection: WorkflowConnection): boolean {
    if (connection.monitor_command !== undefined) return Boolean(connection.monitor_command);
    return connection.can_trigger_workflow === true;
  }

  function warningMonitorConnections(): WorkflowConnection[] {
    const monitoredSlugs = new Set(
      (snapshot.workflow_bindings ?? [])
        .filter((binding) => binding.monitor && binding.enabled)
        .map((binding) => binding.connection_slug)
    );
    return (snapshot.connections ?? []).filter(
      (connection) => monitoredSlugs.has(connection.slug) && connectionNeedsRepair(connection)
    );
  }

  function connectionNeedsRepair(connection: WorkflowConnection): boolean {
    const state = connection.state?.toLowerCase();
    return connection.auth_failure_notified === true
      || state === "degraded"
      || state === "disabled"
      || state === "created"
      || state === "authenticating";
  }

  function connectionRepairCommand(connection: WorkflowConnection): string {
    return connection.connect_command || `/connect ${connection.connector_slug} ${connection.slug}`;
  }

  function monitorConnectionLabel(connection: WorkflowConnection): string {
    const description = connection.description?.trim();
    if (description && description !== connection.slug) {
      return `${connection.slug} - ${description}`;
    }
    return connection.slug;
  }

  function monitorConnectionStateLabel(connection: WorkflowConnection): string {
    return connectionNeedsRepair(connection) ? "repair auth" : connection.state;
  }

  async function createSelectedMonitor(event?: SubmitEvent) {
    event?.preventDefault();
    if (!selectedMonitorConnection || selectedMonitorNeedsRepair || creatingMonitor) return;
    const connection = monitorConnections.find((item) => item.slug === selectedMonitorConnection);
    creatingMonitor = true;
    try {
      const next = await createMonitor(selectedMonitorConnection);
      applySnapshot(next);
      showTaskConfig = false;
      notice = `Monitor created for ${connection?.slug ?? selectedMonitorConnection}.`;
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      notice = `Could not create monitor: ${message}`;
    } finally {
      creatingMonitor = false;
    }
  }

  async function reconnectConnection(connection: WorkflowConnection) {
    if (!onRunTaskCommand || commandRunningFor !== null) return;
    commandRunningFor = `connection:${connection.slug}`;
    try {
      const started = await onRunTaskCommand(connectionRepairCommand(connection));
      notice = started === false ? `Could not reconnect ${connection.slug}.` : `Reconnect started for ${connection.slug}.`;
    } catch (err) {
      notice = `Could not reconnect ${connection.slug}.`;
    } finally {
      commandRunningFor = null;
    }
  }

  function taskSortTime(task: WorkflowTask): number {
    return task.updated_at_ms ?? task.started_at_ms ?? 0;
  }

  function taskTerminal(task: WorkflowTask): boolean {
    const status = taskStatusValue(task);
    return status === "completed" || status === "failed" || status === "stopped" || status === "deleted" || status === "ignored";
  }

  function taskIgnored(task: WorkflowTask): boolean {
    return task.ignored === true;
  }

  function taskStatusValue(task: WorkflowTask): string {
    return taskIgnored(task) ? "ignored" : (task.status || "pending").toLowerCase();
  }

  function taskSourceLabel(task: WorkflowTask): string {
    return task.source === "monitor" ? "Monitor" : "Agent";
  }

  function taskKindLabel(task: WorkflowTask): string {
    if (task.source === "monitor") return task.monitor_connector ?? "monitor";
    const kind = task.task_type?.trim();
    return kind && kind !== "task" ? kind : "task";
  }

  function taskOwnerLabel(task: WorkflowTask): string {
    if (task.source === "monitor") return task.monitor_connection || task.monitor_connector || "monitor";
    return task.owner || task.command || task.output_file || "agent";
  }

  function taskScopeLabel(task: WorkflowTask): string | null {
    const label = task.task_scope_label?.trim();
    if (!label || label === "workspace" || label === "monitors") return null;
    return label;
  }

  function taskWhen(task: WorkflowTask): string {
    const ms = task.updated_at_ms ?? task.started_at_ms;
    if (!ms) return "no timestamp";
    return new Intl.DateTimeFormat(undefined, {
      month: "short",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit"
    }).format(new Date(ms));
  }

  function memorySummary(memory: WorkflowMonitorMemory): string {
    const ignored = memory.content.match(/^## Ignored Task:/gm)?.length ?? 0;
    return ignored === 1 ? "1 ignored example" : `${ignored} ignored examples`;
  }

  function bindingFilterSummary(binding: WorkflowBinding): string {
    const ignoreCount = binding.ignore_filters?.length ?? 0;
    const triggerCount = binding.filter_pattern ? 1 : 0;
    const count = ignoreCount + triggerCount;
    return count === 1 ? "1 rule" : `${count} rules`;
  }

  function bindingFilterLabel(binding: WorkflowBinding): string {
    const status = binding.enabled ? "active" : "paused";
    return `${binding.connection_slug} - ${bindingFilterSummary(binding)} (${status})`;
  }

  function scalarRuleEntries(rule: WorkflowFilterRule): string[] {
    return Object.entries(rule)
      .filter(([, value]) => value === null || ["string", "number", "boolean"].includes(typeof value))
      .map(([key, value]) => `${key}=${JSON.stringify(value)}`);
  }

  function filterRuleTitle(rule: WorkflowFilterRule): string {
    if (rule.type === "regex") return "Regex ignore";
    if (rule.type === "jq") return "Expression ignore";
    const entries = scalarRuleEntries(rule);
    return entries.length > 0 ? "Payload ignore" : "Ignore rule";
  }

  function filterRuleSummary(rule: WorkflowFilterRule): string {
    if (rule.type === "regex" && typeof rule.pattern === "string") {
      const casing = rule.case_insensitive === false ? "case-sensitive" : "case-insensitive";
      return `${rule.pattern} (${casing})`;
    }
    if (rule.type === "jq" && typeof rule.expression === "string") {
      return rule.expression;
    }
    const entries = scalarRuleEntries(rule);
    return entries.length > 0 ? entries.join("  ") : JSON.stringify(rule);
  }

  function openTaskConfig() {
    showTaskConfig = true;
    if (!configMemoryPath && monitorMemories.length > 0) {
      chooseConfigMemory(monitorMemories[0].path);
    }
  }

  function closeTaskConfig() {
    if (savingMemoryPath !== null || creatingMonitor) return;
    showTaskConfig = false;
  }

  function chooseConfigMemory(path: string) {
    configMemoryPath = path;
    const memory = monitorMemories.find((item) => item.path === path) ?? null;
    memoryDraft = memory?.content ?? "";
  }

  function onConfigMemoryChange(event: Event) {
    chooseConfigMemory((event.currentTarget as HTMLSelectElement).value);
  }

  async function saveConfiguredMemory(event: SubmitEvent) {
    event.preventDefault();
    const memory = selectedConfigMemory;
    if (!memory || memory.truncated || savingMemoryPath !== null) return;
    savingMemoryPath = memory.path;
    try {
      const next = await saveMonitorMemory(memory.connection_slug, memoryDraft);
      applySnapshot(next);
      showTaskConfig = false;
      notice = `Saved memory for ${memory.connection_slug}.`;
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      notice = `Could not save memory for ${memory.connection_slug}: ${message}`;
    } finally {
      savingMemoryPath = null;
    }
  }

  function taskDescription(task: WorkflowTask): string {
    return task.description?.trim() || task.active_form?.trim() || task.command?.trim() || "No task detail.";
  }

  function taskShowCommand(task: WorkflowTask): string {
    return `/tasks show ${task.task_id}`;
  }

  function taskActionPrompt(task: WorkflowTask, action: WorkflowMonitorTaskAction): string {
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

  async function runTaskCommand(task: WorkflowTask, command: string, startedMessage: string) {
    if (!command.trim() || !onRunTaskCommand || commandRunningFor !== null) return;
    commandRunningFor = task.task_id;
    try {
      const started = await onRunTaskCommand(command);
      notice = started === false ? `Could not start ${task.task_id}.` : startedMessage;
    } catch (err) {
      notice = `Could not start ${task.task_id}.`;
    } finally {
      commandRunningFor = null;
    }
  }

  async function openTask(task: WorkflowTask) {
    await runTaskCommand(task, taskShowCommand(task), `Opened ${task.task_id} in an agent session.`);
  }

  async function runTaskAction(task: WorkflowTask, action: WorkflowMonitorTaskAction) {
    await runTaskCommand(task, taskActionPrompt(task, action), `Started ${action.name} for ${task.task_id}.`);
  }

  function ignoreReasons(task: WorkflowTask): string[] {
    return (task.possible_ignore_reasons ?? []).filter((reason) => reason.trim().length > 0);
  }

  function toggleIgnoreMenu(task: WorkflowTask) {
    if (task.source !== "monitor" || task.ignored || commandRunningFor !== null) return;
    if (ignoreReasons(task).length === 0) {
      void ignoreTask(task);
      return;
    }
    ignoreMenuTaskId = ignoreMenuTaskId === task.task_id ? null : task.task_id;
  }

  async function ignoreTask(task: WorkflowTask, reason?: string) {
    if (task.source !== "monitor" || task.ignored || commandRunningFor !== null) return;
    commandRunningFor = task.task_id;
    try {
      const next = await ignoreMonitorTask(task.task_id, reason);
      applySnapshot(next);
      notice = `Ignored ${task.task_id}; analysis agent started.`;
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      notice = `Could not ignore ${task.task_id}: ${message}`;
    } finally {
      commandRunningFor = null;
    }
  }
</script>

<div class="pf-tasks">
  <div class="pf-tasks-top">
    <div class="pf-tasks-title">
      <h1>Tasks {tasks.length}</h1>
      <span>{notice}</span>
    </div>
    <div class="pf-tasks-top-right">
      <button
        type="button"
        class="sc-btn"
        data-variant="outline"
        data-size="sm"
        aria-haspopup="dialog"
        aria-expanded={showTaskConfig}
        onclick={() => openTaskConfig()}
      >
        <Icon name="settings" size={12} />Configure
      </button>
      <label class="pf-tasks-search">
        <Icon name="search" size={12} />
        <input
          aria-label="Search tasks"
          placeholder="Search tasks"
          bind:value={query}
        />
      </label>
      <button
        type="button"
        class="sc-btn"
        data-variant="ghost"
        data-size="sm"
        aria-label="Refresh tasks"
        aria-busy={loading}
        disabled={loading}
        onclick={() => void refresh()}
      >
        <Icon name="refresh" size={12} />{loading ? "Refreshing" : "Refresh"}
      </button>
    </div>
  </div>

  <div class="pf-tasks-summary" aria-label="Task summary">
    <button type="button" data-active={sourceFilter === "all"} onclick={() => (sourceFilter = "all")}>
      <strong>{nonIgnoredCount}</strong>
      <span>all</span>
    </button>
    <button type="button" data-active={sourceFilter === "agent"} onclick={() => (sourceFilter = "agent")}>
      <strong>{agentCount}</strong>
      <span>agent</span>
    </button>
    <button type="button" data-active={sourceFilter === "monitor"} onclick={() => (sourceFilter = "monitor")}>
      <strong>{monitorCount}</strong>
      <span>monitor</span>
    </button>
    <button type="button" data-active={sourceFilter === "ignored"} onclick={() => (sourceFilter = "ignored")}>
      <strong>{ignoredCount}</strong>
      <span>ignored</span>
    </button>
    <div>
      <strong>{activeCount}</strong>
      <span>active</span>
    </div>
    <label>
      <span>Status</span>
      <select bind:value={statusFilter} aria-label="Filter tasks by status">
        {#each statusOptions as status (status)}
          <option value={status}>{status === "all" ? "All statuses" : status}</option>
        {/each}
      </select>
    </label>
  </div>

  {#if monitorConnectionWarnings.length > 0}
    <div class="pf-tasks-warning">
      {#each monitorConnectionWarnings as connection (connection.slug)}
        <span><strong>{connection.slug}</strong> auth is degraded. New monitor tasks will not appear.</span>
        <button
          type="button"
          class="sc-btn"
          data-variant="outline"
          data-size="sm"
          disabled={!onRunTaskCommand || commandRunningFor !== null}
          onclick={() => void reconnectConnection(connection)}
        >
          Reconnect
        </button>
      {/each}
    </div>
  {/if}

  {#if error || snapshot.connector_error || snapshot.workflow_binding_error || snapshot.task_error || snapshot.monitor_task_error || snapshot.monitor_memory_error || snapshot.monitor_ignore_filter_error}
    <div class="pf-tasks-error">
      {error ?? snapshot.connector_error ?? snapshot.workflow_binding_error ?? snapshot.task_error ?? snapshot.monitor_task_error ?? snapshot.monitor_memory_error ?? snapshot.monitor_ignore_filter_error}
    </div>
  {/if}

  <div class="pf-tasks-list" aria-label="Task list">
    {#if loading && tasks.length === 0}
      <div class="pf-tasks-empty">Loading tasks...</div>
    {:else if visibleTasks.length === 0}
      <div class="pf-tasks-empty">
        {tasks.length === 0 ? "No agent or monitor tasks yet." : sourceFilter === "ignored" ? "No ignored tasks." : "No tasks match the current filters."}
      </div>
    {:else}
      {#each visibleTasks as task ((task.task_scope ?? task.source) + ":" + task.task_id)}
        <article class="pf-task-row" data-source={task.source} data-terminal={taskTerminal(task)}>
          <div class="pf-task-row-main">
            <div class="pf-task-row-title">
              <span class="pf-task-source">{taskSourceLabel(task)}</span>
              <strong>{task.subject || task.task_id}</strong>
              <span class="pf-task-status" data-status={taskStatusValue(task)}>{taskStatusValue(task)}</span>
            </div>
            <p>{taskDescription(task)}</p>
            <div class="pf-task-meta">
              <code>{task.task_id}</code>
              <span>{taskKindLabel(task)}</span>
              <span>{taskOwnerLabel(task)}</span>
              {#if taskScopeLabel(task)}
                <span>{taskScopeLabel(task)}</span>
              {/if}
              <span>{taskWhen(task)}</span>
            </div>
          </div>
          <div class="pf-task-actions">
            {#if task.actions?.length}
              {#each (task.actions ?? []).slice(0, 2) as action (action.name)}
                <button
                  type="button"
                  class="sc-btn"
                  data-variant="outline"
                  data-size="sm"
                  disabled={commandRunningFor !== null}
                  onclick={() => void runTaskAction(task, action)}
                >
                  {action.name}
                </button>
              {/each}
            {/if}
            {#if task.source === "monitor" && !task.ignored}
              <div class="pf-task-ignore-menu">
                <button
                  type="button"
                  class="sc-btn pf-task-ignore"
                  data-variant="ghost"
                  data-size="sm"
                  aria-haspopup={ignoreReasons(task).length > 0 ? "menu" : undefined}
                  aria-expanded={ignoreMenuTaskId === task.task_id}
                  disabled={commandRunningFor !== null}
                  onclick={() => toggleIgnoreMenu(task)}
                >
                  <Icon name="x" size={12} />Ignore
                  {#if ignoreReasons(task).length > 0}
                    <Icon name="chevD" size={12} />
                  {/if}
                </button>
                {#if ignoreReasons(task).length > 0 && ignoreMenuTaskId === task.task_id}
                  <div class="pf-task-ignore-options" role="menu" aria-label={`Ignore ${task.task_id}`}>
                    <button type="button" role="menuitem" onclick={() => void ignoreTask(task)}>
                      Ignore task
                    </button>
                    {#each ignoreReasons(task) as reason (reason)}
                      <button type="button" role="menuitem" onclick={() => void ignoreTask(task, reason)}>
                        {reason}
                      </button>
                    {/each}
                  </div>
                {/if}
              </div>
            {/if}
            <button
              type="button"
              class="sc-btn"
              data-variant="ghost"
              data-size="sm"
              disabled={commandRunningFor !== null || !onRunTaskCommand}
              onclick={() => void openTask(task)}
            >
              <Icon name="external" size={12} />Open
            </button>
          </div>
        </article>
      {/each}
    {/if}
  </div>

  {#if showTaskConfig}
    <div class="pf-task-config-backdrop" role="presentation">
      <div
        class="pf-task-config"
        role="dialog"
        aria-modal="true"
        aria-labelledby="pf-task-config-title"
      >
        <header class="pf-task-config-head">
          <div>
            <h2 id="pf-task-config-title">Task configuration</h2>
            <span>Monitors, filter rules, and memory</span>
          </div>
          <button
            type="button"
            class="sc-btn"
            data-variant="ghost"
            data-size="sm"
            aria-label="Close task configuration"
            disabled={creatingMonitor || savingMemoryPath !== null}
            onclick={closeTaskConfig}
          >
            <Icon name="x" size={12} />
          </button>
        </header>

        <form class="pf-task-config-section" onsubmit={(event) => void createSelectedMonitor(event)}>
          <div class="pf-task-config-section-head">
            <strong>New monitor</strong>
            <span>Create a monitor from a trigger-ready connection.</span>
          </div>
          <div class="pf-task-config-row">
            <label>
              <span>Connection</span>
              <select
                bind:value={selectedMonitorConnection}
                aria-label="Connection to monitor"
                disabled={monitorConnections.length === 0 || creatingMonitor}
              >
                {#each monitorConnections as connection (connection.slug)}
                  <option value={connection.slug} disabled={connectionNeedsRepair(connection)}>
                    {monitorConnectionLabel(connection)} ({connection.connector_slug}, {monitorConnectionStateLabel(connection)})
                  </option>
                {/each}
              </select>
            </label>
            <button
              type="submit"
              class="sc-btn"
              data-variant="solid"
              data-size="sm"
              disabled={!selectedMonitorConnection || selectedMonitorNeedsRepair || creatingMonitor}
            >
              <Icon name="plus" size={12} />{creatingMonitor ? "Creating" : "Create"}
            </button>
          </div>
          {#if monitorConnections.length === 0}
            <p>No trigger-ready connections.</p>
          {/if}
        </form>

        <section class="pf-task-config-section">
          <div class="pf-task-config-section-head">
            <strong>Filter rules</strong>
            <span>Check installed trigger and ignore rules.</span>
          </div>
          {#if monitorFilterBindings.length > 0 && selectedFilterBinding}
            <label class="pf-task-config-memory-select">
              <span>Monitor</span>
              <select
                bind:value={selectedFilterBindingSlug}
                aria-label="Monitor filter rules"
              >
                {#each monitorFilterBindings as binding (binding.slug)}
                  <option value={binding.slug}>{bindingFilterLabel(binding)}</option>
                {/each}
              </select>
            </label>
            <div class="pf-task-filter-list">
              {#if selectedFilterBinding.filter_pattern}
                <div class="pf-task-filter-rule">
                  <span>Trigger</span>
                  <code>{selectedFilterBinding.filter_pattern}</code>
                </div>
              {/if}
              {#each selectedFilterBinding.ignore_filters ?? [] as rule, index (`${selectedFilterBinding.slug}:${index}`)}
                <div class="pf-task-filter-rule">
                  <span>Ignore {index + 1} - {filterRuleTitle(rule)}</span>
                  <code>{filterRuleSummary(rule)}</code>
                </div>
              {/each}
              {#if !selectedFilterBinding.filter_pattern && (selectedFilterBinding.ignore_filters?.length ?? 0) === 0}
                <p>No filter rules installed for {selectedFilterBinding.connection_slug}.</p>
              {/if}
            </div>
          {:else}
            <p>No monitor filter rules yet.</p>
          {/if}
        </section>

        <form class="pf-task-config-section" onsubmit={(event) => void saveConfiguredMemory(event)}>
          <div class="pf-task-config-section-head">
            <strong>Monitor memory</strong>
            <span>Edit the ignore context used before monitor tasks are created.</span>
          </div>
          {#if monitorMemories.length > 0 && selectedConfigMemory}
            <label class="pf-task-config-memory-select">
              <span>Memory</span>
              <select
                value={configMemoryPath}
                aria-label="Monitor memory file"
                disabled={savingMemoryPath !== null}
                onchange={onConfigMemoryChange}
              >
                {#each monitorMemories as memory (memory.path)}
                  <option value={memory.path}>{memory.connection_slug} - {memorySummary(memory)}</option>
                {/each}
              </select>
            </label>
            <code class="pf-task-config-memory-path">{selectedConfigMemory.path}</code>
            <textarea
              aria-label={`Edit monitor memory for ${selectedConfigMemory.connection_slug}`}
              bind:value={memoryDraft}
              disabled={selectedConfigMemory.truncated || savingMemoryPath !== null}
              spellcheck="false"
            ></textarea>
            {#if selectedConfigMemory.truncated}
              <p>Snapshot truncated. Open the file directly to edit safely.</p>
            {/if}
            <div class="pf-task-config-actions">
              <button
                type="button"
                class="sc-btn"
                data-variant="ghost"
                data-size="sm"
                disabled={savingMemoryPath !== null}
                onclick={() => chooseConfigMemory(selectedConfigMemory.path)}
              >
                Reset
              </button>
              <button
                type="submit"
                class="sc-btn"
                data-variant="solid"
                data-size="sm"
                disabled={selectedConfigMemory.truncated || savingMemoryPath !== null || memoryDraft === selectedConfigMemory.content}
              >
                <Icon name="check" size={12} />{savingMemoryPath === selectedConfigMemory.path ? "Saving" : "Save memory"}
              </button>
            </div>
          {:else}
            <p>No monitor memory files yet.</p>
          {/if}
        </form>
      </div>
    </div>
  {/if}
</div>

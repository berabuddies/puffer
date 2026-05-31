<script lang="ts">
  import "../design/tasks.css";

  import { onMount } from "svelte";
  import {
    createMonitor,
    deleteMonitor,
    ignoreMonitorTask,
    listProviderModels,
    loadMonitorHistory,
    loadSettingsSnapshot,
    loadWorkflowSnapshot,
    saveMonitorMemory,
    type ModelDescriptorInfo
  } from "../api/desktop";
  import Icon from "../design/Icon.svelte";
  import { providerIsAvailableForAgent, providerIdsEquivalent } from "../providerIds";
  import type {
    ProviderSummary,
    SettingsSnapshot,
    WorkflowActionUsage,
    WorkflowBinding,
    WorkflowConnection,
    WorkflowFilterRule,
    WorkflowMonitorHistoryAction,
    WorkflowMonitorHistoryMessage,
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
  type TaskModelOption = {
    selector: string;
    label: string;
  };

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
  let selectedTaskKey = $state<string | null>(null);
  let commandRunningFor = $state<string | null>(null);
  let ignoreMenuTaskId = $state<string | null>(null);
  let showTaskConfig = $state(false);
  let showTaskHistory = $state(false);
  let historyLoading = $state(false);
  let historyError = $state<string | null>(null);
  let historyMessages = $state<WorkflowMonitorHistoryMessage[]>([]);
  let selectedHistoryIdx = $state<number | null>(null);
  let selectedMonitorConnection = $state("");
  let selectedMonitorModel = $state("");
  let creatingMonitor = $state(false);
  let configMemoryPath = $state("");
  let selectedFilterBindingSlug = $state("");
  let memoryDraft = $state("");
  let savingMemoryPath = $state<string | null>(null);
  let deletingMonitorSlug = $state<string | null>(null);
  let confirmDeleteMonitorSlug = $state<string | null>(null);
  let settingsSnapshot = $state<SettingsSnapshot | null>(null);
  let taskModelOptions = $state<TaskModelOption[]>([]);
  let taskModelLoading = $state(false);
  let taskModelLoadError = $state<string | null>(null);
  let refreshGeneration = 0;
  let taskModelGeneration = 0;

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
  let selectedTask = $derived(selectedTaskValue());
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
  let selectedMonitorBinding = $derived(
    monitorFilterBindings.find((binding) => binding.connection_slug === selectedMonitorConnection) ?? null
  );
  let selectedMonitorNeedsRepair = $derived(
    selectedMonitorConnectionRecord ? connectionNeedsRepair(selectedMonitorConnectionRecord) : false
  );
  let selectedHistoryMessage = $derived(
    historyMessages.find((message) => message.idx === selectedHistoryIdx) ?? historyMessages[0] ?? null
  );
  let selectedHistoryTriageActions = $derived(
    selectedHistoryMessage ? historyTriageActions(selectedHistoryMessage) : []
  );
  let selectedHistoryIgnoreTasks = $derived(
    selectedHistoryMessage ? ignoreTasksForHistory(selectedHistoryMessage) : []
  );

  onMount(() => {
    void refresh();
  });

  $effect(() => {
    if (!showTaskHistory) return;
    const timer = window.setInterval(() => {
      void refreshHistory();
    }, 3_000);
    return () => window.clearInterval(timer);
  });

  $effect(() => {
    if (!showTaskConfig) return;
    if (monitorConnections.some((connection) => connection.slug === selectedMonitorConnection)) return;
    selectedMonitorConnection = monitorConnections[0]?.slug ?? "";
  });

  $effect(() => {
    if (!showTaskConfig) return;
    selectedMonitorModel = selectedMonitorBinding?.model ?? "";
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

  $effect(() => {
    if (visibleTasks.length === 0) {
      selectedTaskKey = null;
      return;
    }
    if (selectedTaskKey && !visibleTasks.some((task) => taskKey(task) === selectedTaskKey)) {
      selectedTaskKey = null;
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

  async function openTaskHistory() {
    showTaskHistory = true;
    await refreshHistory();
  }

  async function refreshHistory() {
    if (historyLoading) return;
    historyLoading = true;
    historyError = null;
    try {
      historyMessages = await loadMonitorHistory();
      if (
        selectedHistoryIdx === null ||
        !historyMessages.some((message) => message.idx === selectedHistoryIdx)
      ) {
        selectedHistoryIdx = historyMessages[0]?.idx ?? null;
      }
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      historyError = message;
      notice = `Could not load task history: ${message}`;
    } finally {
      historyLoading = false;
    }
  }

  function closeTaskHistory() {
    if (historyLoading) return;
    showTaskHistory = false;
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
    if (confirmDeleteMonitorSlug && !(next.workflow_bindings ?? []).some((binding) => binding.slug === confirmDeleteMonitorSlug)) {
      confirmDeleteMonitorSlug = null;
    }
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

  function taskKey(task: WorkflowTask): string {
    return `${task.task_scope ?? task.source}:${task.task_id}`;
  }

  function selectTask(task: WorkflowTask) {
    selectedTaskKey = taskKey(task);
    ignoreMenuTaskId = null;
  }

  function closeSelectedTask() {
    selectedTaskKey = null;
    ignoreMenuTaskId = null;
  }

  function selectedTaskValue(): WorkflowTask | null {
    if (!selectedTaskKey) return null;
    return visibleTasks.find((task) => taskKey(task) === selectedTaskKey) ?? null;
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

  function modelSupportsAgentTools(model: ModelDescriptorInfo): boolean {
    return model.supportsTools !== false;
  }

  function providerSort(left: ProviderSummary, right: ProviderSummary): number {
    const defaultProvider = settingsSnapshot?.config.defaultProvider ?? "";
    const leftDefault = providerIdsEquivalent(left.id, defaultProvider);
    const rightDefault = providerIdsEquivalent(right.id, defaultProvider);
    if (leftDefault !== rightDefault) return leftDefault ? -1 : 1;
    return left.displayName.localeCompare(right.displayName);
  }

  async function loadTaskModelOptions() {
    if (taskModelLoading) return;
    const generation = ++taskModelGeneration;
    taskModelLoading = true;
    taskModelLoadError = null;
    try {
      const settings = await loadSettingsSnapshot();
      if (generation !== taskModelGeneration) return;
      settingsSnapshot = settings;
      const authenticatedProviderIds = settings.auth.map((entry) => entry.providerId);
      const providers = settings.providers
        .filter((provider) => providerIsAvailableForAgent(provider, authenticatedProviderIds))
        .sort(providerSort);
      const loaded = await Promise.allSettled(
        providers.map(async (provider) => ({
          provider,
          models: await listProviderModels(provider.id)
        }))
      );
      if (generation !== taskModelGeneration) return;
      const seen = new Set<string>();
      const nextOptions: TaskModelOption[] = [];
      const failed: string[] = [];
      for (const result of loaded) {
        if (result.status === "rejected") {
          failed.push("provider");
          continue;
        }
        const provider = result.value.provider;
        const providerLabel = provider.displayName || provider.id;
        for (const model of result.value.models.filter(modelSupportsAgentTools)) {
          const modelId = model.id.trim();
          if (!modelId) continue;
          const modelProvider = model.provider?.trim() || provider.id;
          const selector = `${modelProvider}/${modelId}`;
          if (seen.has(selector)) continue;
          seen.add(selector);
          nextOptions.push({
            selector,
            label: `${model.displayName || modelId} (${providerLabel})`
          });
        }
      }
      taskModelOptions = nextOptions;
      taskModelLoadError = failed.length > 0 && nextOptions.length === 0
        ? "Could not load task model suggestions."
        : null;
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      taskModelLoadError = `Could not load task models: ${message}`;
    } finally {
      if (generation === taskModelGeneration) {
        taskModelLoading = false;
      }
    }
  }

  function monitorModelLabel(binding: WorkflowBinding): string {
    return binding.model?.trim() || "default";
  }

  async function createSelectedMonitor(event?: SubmitEvent) {
    event?.preventDefault();
    if (!selectedMonitorConnection || selectedMonitorNeedsRepair || creatingMonitor) return;
    const connection = monitorConnections.find((item) => item.slug === selectedMonitorConnection);
    const wasUpdate = selectedMonitorBinding !== null;
    const selectedModel = selectedMonitorModel.trim();
    creatingMonitor = true;
    try {
      const next = await createMonitor(selectedMonitorConnection, selectedModel || null);
      applySnapshot(next);
      showTaskConfig = false;
      const action = wasUpdate ? "updated" : "created";
      const model = selectedModel || "default model";
      notice = `Monitor ${action} for ${connection?.slug ?? selectedMonitorConnection} using ${model}.`;
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      notice = `Could not configure monitor: ${message}`;
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

  function historyWhen(message: WorkflowMonitorHistoryMessage): string {
    const ms = message.received_at_ms ?? message.started_at_ms;
    if (!ms) return "no timestamp";
    return new Intl.DateTimeFormat(undefined, {
      month: "short",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit"
    }).format(new Date(ms));
  }

  function historySourceLabel(message: WorkflowMonitorHistoryMessage): string {
    return message.connection_slug || message.connector_slug || message.topic || "monitor";
  }

  function historyDeliveryLabel(message: WorkflowMonitorHistoryMessage): string | null {
    const source = message.payload?.delivery_source;
    if (source === "catch_up") return "catch-up";
    if (source === "live") return "live";
    return null;
  }

  function historyLagLabel(message: WorkflowMonitorHistoryMessage): string | null {
    const dateMs = numericPayloadValue(message.payload, "date_ms");
    const receivedMs = message.received_at_ms;
    if (!dateMs || !receivedMs) return null;
    const lagSeconds = Math.max(0, Math.round((receivedMs - dateMs) / 1000));
    if (lagSeconds < 2) return "lag <2s";
    if (lagSeconds < 60) return `lag ${lagSeconds}s`;
    const minutes = Math.floor(lagSeconds / 60);
    const seconds = lagSeconds % 60;
    return seconds > 0 ? `lag ${minutes}m ${seconds}s` : `lag ${minutes}m`;
  }

  function historyMetaLabel(message: WorkflowMonitorHistoryMessage): string {
    return [
      message.kind ?? "message",
      historyDeliveryLabel(message),
      historyLagLabel(message),
      `#${message.idx}`
    ].filter(Boolean).join(" · ");
  }

  function numericPayloadValue(payload: Record<string, unknown> | null | undefined, key: string): number | null {
    const value = payload?.[key];
    return typeof value === "number" && Number.isFinite(value) ? value : null;
  }

  function historyTriageActions(message: WorkflowMonitorHistoryMessage): WorkflowMonitorHistoryAction[] {
    return (message.action_log ?? []).filter((action) =>
      action.action === "triage_agent" || action.action.startsWith("monitor_")
    );
  }

  function historyActionLabel(action: WorkflowMonitorHistoryAction): string {
    if (action.action === "triage_agent") return "Triage agent";
    if (action.action === "monitor_ignore_filter") return "Ignore filter";
    if (action.action === "monitor_muted_skip") return "Muted notification";
    if (action.action === "monitor_filter_skip") return "Trigger filter";
    if (action.action === "monitor_classifier_skip") return "Classifier";
    return action.action.replaceAll("_", " ");
  }

  function historyActionStatusLabel(action: WorkflowMonitorHistoryAction): string {
    return action.status === "running" ? "processing" : action.status;
  }

  function ignoreTasksForHistory(message: WorkflowMonitorHistoryMessage): WorkflowTask[] {
    if (!message.envelope_id) return [];
    return tasks.filter(
      (task) => task.monitor_envelope_id && task.monitor_envelope_id === message.envelope_id
        && task.source === "monitor"
        && taskIgnored(task)
    );
  }

  function ignoreAnalysisText(task: WorkflowTask): string {
    if (task.ignore_analysis_result?.trim()) return task.ignore_analysis_result;
    if (task.ignore_analysis_error?.trim()) return task.ignore_analysis_error;
    if (task.ignore_analysis_started) return "Ignore analysis is running.";
    return "No ignore analysis recorded.";
  }

  function tokenUsageLabel(usage: WorkflowActionUsage | null | undefined): string {
    if (!usage) return "tokens n/a";
    const spent = usage.spent_tokens
      ?? Math.max(0, (usage.input_tokens ?? 0) - (usage.cache_read_tokens ?? 0))
        + (usage.output_tokens ?? 0);
    return `${spent.toLocaleString()} tokens`;
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

  function monitorCountLabel(count: number): string {
    return count === 1 ? "1 configured monitor" : `${count} configured monitors`;
  }

  function monitorRowSummary(binding: WorkflowBinding): string {
    const connector = binding.connector_slug?.trim() || "connector";
    const status = binding.enabled ? "active" : "paused";
    return `${connector} - ${monitorModelLabel(binding)} - ${bindingFilterSummary(binding)} - ${status}`;
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
    confirmDeleteMonitorSlug = null;
    if (taskModelOptions.length === 0) {
      void loadTaskModelOptions();
    }
    if (!configMemoryPath && monitorMemories.length > 0) {
      chooseConfigMemory(monitorMemories[0].path);
    }
  }

  function closeTaskConfig() {
    if (savingMemoryPath !== null || creatingMonitor || deletingMonitorSlug !== null) return;
    showTaskConfig = false;
    confirmDeleteMonitorSlug = null;
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

  function requestDeleteMonitor(binding: WorkflowBinding) {
    if (savingMemoryPath !== null || creatingMonitor || deletingMonitorSlug !== null) return;
    confirmDeleteMonitorSlug = confirmDeleteMonitorSlug === binding.slug ? null : binding.slug;
  }

  async function deleteConfiguredMonitor(binding: WorkflowBinding) {
    if (savingMemoryPath !== null || creatingMonitor || deletingMonitorSlug !== null) return;
    deletingMonitorSlug = binding.slug;
    try {
      const next = await deleteMonitor(binding.slug);
      applySnapshot(next);
      if (selectedFilterBindingSlug === binding.slug) {
        selectedFilterBindingSlug = "";
      }
      if (selectedMonitorConnection === binding.connection_slug) {
        selectedMonitorModel = "";
      }
      notice = `Deleted monitor for ${binding.connection_slug}.`;
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      notice = `Could not delete monitor for ${binding.connection_slug}: ${message}`;
    } finally {
      deletingMonitorSlug = null;
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
      <button
        type="button"
        class="sc-btn"
        data-variant="outline"
        data-size="sm"
        aria-haspopup="dialog"
        aria-expanded={showTaskHistory}
        onclick={() => void openTaskHistory()}
      >
        <Icon name="clock" size={12} />History
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

  <div class="pf-tasks-workspace" data-inspector={selectedTask !== null}>
    <section class="pf-tasks-list-panel">
      <div class="pf-tasks-list-head">
        <strong>{visibleTasks.length} shown</strong>
        <span>{sourceFilter === "ignored" ? "ignored tasks" : statusFilter === "all" ? "latest first" : statusFilter}</span>
      </div>
      <div class="pf-tasks-list" aria-label="Task list">
        {#if loading && tasks.length === 0}
          <div class="pf-tasks-empty">Loading tasks...</div>
        {:else if visibleTasks.length === 0}
          <div class="pf-tasks-empty">
            {tasks.length === 0 ? "No agent or monitor tasks yet." : sourceFilter === "ignored" ? "No ignored tasks." : "No tasks match the current filters."}
          </div>
        {:else}
          {#each visibleTasks as task (taskKey(task))}
            <article
              class="pf-task-row"
              data-source={task.source}
              data-terminal={taskTerminal(task)}
              data-selected={selectedTaskKey === taskKey(task)}
            >
              <button
                type="button"
                class="pf-task-row-main"
                aria-pressed={selectedTaskKey === taskKey(task)}
                onclick={() => selectTask(task)}
              >
                <span class="pf-task-row-title">
                  <span class="pf-task-source">{taskSourceLabel(task)}</span>
                  <strong>{task.subject || task.task_id}</strong>
                  <span class="pf-task-status" data-status={taskStatusValue(task)}>{taskStatusValue(task)}</span>
                </span>
                <span class="pf-task-row-summary">{taskDescription(task)}</span>
                <span class="pf-task-meta">
                  <code>{task.task_id}</code>
                  <span>{taskKindLabel(task)}</span>
                  <span>{taskOwnerLabel(task)}</span>
                  {#if taskScopeLabel(task)}
                    <span>{taskScopeLabel(task)}</span>
                  {/if}
                  <span>{taskWhen(task)}</span>
                </span>
              </button>
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
    </section>

    {#if selectedTask}
      {@const detailTask = selectedTask}
      <aside class="pf-task-detail" aria-label="Selected task">
        <header class="pf-task-detail-head">
          <div class="pf-task-detail-titlebar">
            <div class="pf-task-detail-kicker">
              <span class="pf-task-source">{taskSourceLabel(detailTask)}</span>
              <span class="pf-task-status" data-status={taskStatusValue(detailTask)}>{taskStatusValue(detailTask)}</span>
            </div>
            <button
              type="button"
              class="sc-btn"
              data-variant="ghost"
              data-size="sm"
              aria-label="Close selected task"
              onclick={closeSelectedTask}
            >
              <Icon name="x" size={12} />
            </button>
          </div>
          <h2>{detailTask.subject || detailTask.task_id}</h2>
          <p>{taskDescription(detailTask)}</p>
        </header>

        <section class="pf-task-detail-section">
          <div class="pf-task-detail-section-head">
            <strong>Context</strong>
            <span>{taskWhen(detailTask)}</span>
          </div>
          <dl class="pf-task-detail-meta">
            <div>
              <dt>ID</dt>
              <dd><code>{detailTask.task_id}</code></dd>
            </div>
            <div>
              <dt>Owner</dt>
              <dd>{taskOwnerLabel(detailTask)}</dd>
            </div>
            <div>
              <dt>Kind</dt>
              <dd>{taskKindLabel(detailTask)}</dd>
            </div>
            {#if taskScopeLabel(detailTask)}
              <div>
                <dt>Scope</dt>
                <dd>{taskScopeLabel(detailTask)}</dd>
              </div>
            {/if}
            {#if detailTask.monitor_memory_path}
              <div>
                <dt>Memory</dt>
                <dd><code>{detailTask.monitor_memory_path}</code></dd>
              </div>
            {/if}
          </dl>
        </section>

        {#if detailTask.ignored}
          <section class="pf-task-detail-section">
            <div class="pf-task-detail-section-head">
              <strong>Ignore analysis</strong>
              <span>{detailTask.ignore_analysis_status ?? (detailTask.ignore_analysis_started ? "running" : "not started")}</span>
            </div>
            {#if detailTask.ignore_reason}
              <p class="pf-task-detail-copy">Reason: {detailTask.ignore_reason}</p>
            {/if}
            <p class="pf-task-detail-copy">{ignoreAnalysisText(detailTask)}</p>
            <span class="pf-task-detail-usage">{tokenUsageLabel(detailTask.ignore_analysis_usage)}</span>
          </section>
        {/if}

      </aside>
    {/if}
  </div>

  {#if showTaskHistory}
    <div class="pf-task-config-backdrop" role="presentation">
      <div
        class="pf-task-history"
        role="dialog"
        aria-modal="true"
        aria-labelledby="pf-task-history-title"
      >
        <header class="pf-task-config-head">
          <div>
            <h2 id="pf-task-history-title">Task history</h2>
            <span>Received messages, triage results, and ignore analysis</span>
          </div>
          <div class="pf-task-history-head-actions">
            <button
              type="button"
              class="sc-btn"
              data-variant="ghost"
              data-size="sm"
              aria-busy={historyLoading}
              disabled={historyLoading}
              onclick={() => void refreshHistory()}
            >
              <Icon name="refresh" size={12} />Refresh
            </button>
            <button
              type="button"
              class="sc-btn"
              data-variant="ghost"
              data-size="sm"
              aria-label="Close task history"
              disabled={historyLoading}
              onclick={closeTaskHistory}
            >
              <Icon name="x" size={12} />
            </button>
          </div>
        </header>
        <div class="pf-task-history-body">
          <aside class="pf-task-history-sidebar" aria-label="Received messages">
            {#if historyLoading && historyMessages.length === 0}
              <div class="pf-tasks-empty">Loading history...</div>
            {:else if historyError}
              <div class="pf-tasks-error">{historyError}</div>
            {:else if historyMessages.length === 0}
              <div class="pf-tasks-empty">No received monitor messages yet.</div>
            {:else}
              {#each historyMessages as message (message.idx)}
                <button
                  type="button"
                  class="pf-task-history-message"
                  data-selected={selectedHistoryMessage?.idx === message.idx}
                  onclick={() => (selectedHistoryIdx = message.idx)}
                >
                  <span class="pf-task-history-message-top">
                    <strong>{historySourceLabel(message)}</strong>
                    <small>{historyWhen(message)}</small>
                  </span>
                  <span>{message.summary || message.text || "Received message"}</span>
                  <small>{historyMetaLabel(message)}</small>
                </button>
              {/each}
            {/if}
          </aside>

          <section class="pf-task-history-detail" aria-label="Agent history">
            {#if selectedHistoryMessage}
              <div class="pf-task-history-selected">
                <div>
                  <span>{historySourceLabel(selectedHistoryMessage)} · {historyMetaLabel(selectedHistoryMessage)}</span>
                  <strong>{selectedHistoryMessage.summary || "Received message"}</strong>
                </div>
                <code>{selectedHistoryMessage.envelope_id ?? selectedHistoryMessage.run_id}</code>
              </div>

              <section class="pf-task-history-agent-card">
                <div class="pf-task-config-section-head">
                  <strong>Triage agent</strong>
                  <span>{selectedHistoryTriageActions.length} outcome{selectedHistoryTriageActions.length === 1 ? "" : "s"}</span>
                </div>
                {#if selectedHistoryTriageActions.length === 0}
                  <p>No triage outcome recorded for this message.</p>
                {:else}
                  {#each selectedHistoryTriageActions as action, index (`${selectedHistoryMessage.idx}:${action.action}:${index}`)}
                    <article class="pf-task-history-agent-result" data-status={action.status}>
                      <div>
                        <strong>{historyActionLabel(action)}</strong>
                        <span>{historyActionStatusLabel(action)} · {tokenUsageLabel(action.usage)}</span>
                      </div>
                      <p>{action.summary}</p>
                    </article>
                  {/each}
                {/if}
              </section>

              <section class="pf-task-history-agent-card">
                <div class="pf-task-config-section-head">
                  <strong>Ignore agents</strong>
                  <span>{selectedHistoryIgnoreTasks.length} task{selectedHistoryIgnoreTasks.length === 1 ? "" : "s"}</span>
                </div>
                {#if selectedHistoryIgnoreTasks.length === 0}
                  <p>No ignored task or ignore analysis is linked to this message.</p>
                {:else}
                  {#each selectedHistoryIgnoreTasks as task ((task.task_scope ?? task.source) + ":history:" + task.task_id)}
                    <article class="pf-task-history-agent-result" data-status={task.ignore_analysis_status ?? "pending"}>
                      <div>
                        <strong>{task.subject || task.task_id}</strong>
                        <span>{task.ignore_analysis_status ?? (task.ignore_analysis_started ? "running" : "not started")} · {tokenUsageLabel(task.ignore_analysis_usage)}</span>
                      </div>
                      {#if task.ignore_reason}
                        <small>Reason: {task.ignore_reason}</small>
                      {/if}
                      <p>{ignoreAnalysisText(task)}</p>
                    </article>
                  {/each}
                {/if}
              </section>

              <section class="pf-task-history-agent-card">
                <div class="pf-task-config-section-head">
                  <strong>Message payload</strong>
                  <span>{selectedHistoryMessage.kind ?? "event"}</span>
                </div>
                {#if selectedHistoryMessage.text}
                  <p>{selectedHistoryMessage.text}</p>
                {/if}
                <pre>{JSON.stringify(selectedHistoryMessage.payload ?? {}, null, 2)}</pre>
              </section>
            {:else}
              <div class="pf-tasks-empty">Select a received message.</div>
            {/if}
          </section>
        </div>
      </div>
    </div>
  {/if}

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
            disabled={creatingMonitor || savingMemoryPath !== null || deletingMonitorSlug !== null}
            onclick={closeTaskConfig}
          >
            <Icon name="x" size={12} />
          </button>
        </header>

        <form class="pf-task-config-section" onsubmit={(event) => void createSelectedMonitor(event)}>
          <div class="pf-task-config-section-head">
            <strong>Monitor agent</strong>
            <span>Create or update the task agent for a trigger-ready connection.</span>
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
            <label>
              <span>Model</span>
              <input
                list="pf-task-model-options"
                bind:value={selectedMonitorModel}
                aria-label="Task agent model"
                placeholder={taskModelLoading ? "Loading models" : "default model"}
                disabled={creatingMonitor}
                spellcheck="false"
              />
              <datalist id="pf-task-model-options">
                {#each taskModelOptions as option (option.selector)}
                  <option value={option.selector}>{option.label}</option>
                {/each}
              </datalist>
            </label>
            <button
              type="submit"
              class="sc-btn"
              data-variant="solid"
              data-size="sm"
              disabled={!selectedMonitorConnection || selectedMonitorNeedsRepair || creatingMonitor}
            >
              <Icon name={selectedMonitorBinding ? "check" : "plus"} size={12} />
              {creatingMonitor ? (selectedMonitorBinding ? "Updating" : "Creating") : (selectedMonitorBinding ? "Update" : "Create")}
            </button>
          </div>
          {#if monitorConnections.length === 0}
            <p>No trigger-ready connections.</p>
          {/if}
          {#if selectedMonitorBinding}
            <p>Current model: {monitorModelLabel(selectedMonitorBinding)}</p>
          {/if}
          {#if taskModelLoadError}
            <p>{taskModelLoadError}</p>
          {/if}
        </form>

        <section class="pf-task-config-section">
          <div class="pf-task-config-section-head">
            <strong>Active monitors</strong>
            <span>{monitorCountLabel(monitorFilterBindings.length)}</span>
          </div>
          {#if monitorFilterBindings.length > 0}
            <div class="pf-task-monitor-list">
              {#each monitorFilterBindings as binding (binding.slug)}
                <div class="pf-task-monitor-row" data-enabled={binding.enabled}>
                  <div class="pf-task-monitor-main">
                    <strong>{binding.connection_slug}</strong>
                    <span>{monitorRowSummary(binding)}</span>
                  </div>
                  <div class="pf-task-monitor-actions">
                    {#if confirmDeleteMonitorSlug === binding.slug}
                      <button
                        type="button"
                        class="sc-btn"
                        data-variant="ghost"
                        data-size="sm"
                        disabled={deletingMonitorSlug === binding.slug}
                        onclick={() => (confirmDeleteMonitorSlug = null)}
                      >
                        Cancel
                      </button>
                      <button
                        type="button"
                        class="sc-btn"
                        data-variant="destructive"
                        data-size="sm"
                        disabled={creatingMonitor || savingMemoryPath !== null || deletingMonitorSlug !== null}
                        onclick={() => void deleteConfiguredMonitor(binding)}
                      >
                        <Icon name="trash" size={12} />{deletingMonitorSlug === binding.slug ? "Deleting" : "Confirm"}
                      </button>
                    {:else}
                      <button
                        type="button"
                        class="sc-btn"
                        data-variant="ghost"
                        data-size="sm"
                        aria-label={`Delete monitor for ${binding.connection_slug}`}
                        disabled={creatingMonitor || savingMemoryPath !== null || deletingMonitorSlug !== null}
                        onclick={() => requestDeleteMonitor(binding)}
                      >
                        <Icon name="trash" size={12} />Delete
                      </button>
                    {/if}
                  </div>
                </div>
              {/each}
            </div>
          {:else}
            <p>No active monitors.</p>
          {/if}
        </section>

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
              <div class="pf-task-filter-rule">
                <span>Model</span>
                <code>{monitorModelLabel(selectedFilterBinding)}</code>
              </div>
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

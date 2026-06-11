<script lang="ts">
  import "../design/tasks.css";

  import { onMount } from "svelte";
  import MonitorRulesEditor from "./tasks/MonitorRulesEditor.svelte";
  import {
    addMonitorRule,
    createMonitor,
    deleteMonitorRule,
    deleteMonitor,
    ignoreMonitorTask,
    listProviderModels,
    loadContacts,
    loadMonitorHistory,
    loadSettingsSnapshot,
    loadWorkflowSnapshot,
    saveMonitorMemory,
    type ModelDescriptorInfo
  } from "../api/desktop";
  import { normalizeContactIds } from "../contactIds";
  import Icon from "../design/Icon.svelte";
  import { providerIsAvailableForAgent, providerIdsEquivalent } from "../providerIds";
  import type {
    ContactsSnapshot,
    ConnectorContact,
    MonitorRuleAddRequest,
    MonitorRuleMode,
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
  type ContactChoice = {
    key: string;
    label: string;
    description: string;
    ids: string[];
    saved: boolean;
  };
  type TaskConfigTab = "monitor" | "contacts" | "rules";

  const TASK_PAGE_SIZE = 40;

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
  let taskConfigTab = $state<TaskConfigTab>("monitor");
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
  let contactSnapshot = $state<ContactsSnapshot>({ contacts: [], candidates: [], proposals: [] });
  let contactsLoading = $state(false);
  let selectedMonitorContactIds = $state<string[]>([]);
  let visibleTaskCount = $state(TASK_PAGE_SIZE);
  let taskListSentinel: HTMLDivElement | null = $state(null);
  let taskWindowKey = "";
  let contactScopeBindingKey = "";
  let savingMonitorRuleMode = $state<MonitorRuleMode | null>(null);
  let deletingMonitorRuleKey = $state<string | null>(null);
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
  let renderedTasks = $derived(visibleTasks.slice(0, visibleTaskCount));
  let hasMoreTasks = $derived(renderedTasks.length < visibleTasks.length);
  let remainingTaskCount = $derived(Math.max(0, visibleTasks.length - renderedTasks.length));
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
  let selectedFilterConnection = $derived(
    (snapshot.connections ?? []).find((connection) => connection.slug === selectedFilterBinding?.connection_slug)
      ?? null
  );
  let selectedRuleSchema = $derived(
    selectedFilterBinding?.monitor_rule_schema ?? selectedFilterConnection?.monitor_rule_schema ?? null
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
  let selectedMonitorRepairRunning = $derived(
    selectedMonitorConnectionRecord
      ? commandRunningFor === `connection:${selectedMonitorConnectionRecord.slug}`
      : false
  );
  let selectedMonitorPrimaryDisabled = $derived(
    !selectedMonitorConnection
      || creatingMonitor
      || selectedMonitorRepairRunning
      || (selectedMonitorNeedsRepair && !onRunTaskCommand)
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
  let contactChoices = $derived(contactScopeChoices());

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
    const nextKey = selectedMonitorBinding?.slug ?? `connection:${selectedMonitorConnection}`;
    if (contactScopeBindingKey === nextKey) return;
    contactScopeBindingKey = nextKey;
    selectedMonitorContactIds = selectedMonitorBinding?.contact_ids ?? [];
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
    const nextKey = taskRenderWindowKey(visibleTasks);
    if (nextKey === taskWindowKey) return;
    taskWindowKey = nextKey;
    visibleTaskCount = initialTaskRenderCount();
  });

  $effect(() => {
    const sentinel = taskListSentinel;
    if (!sentinel || !hasMoreTasks || typeof IntersectionObserver === "undefined") return;
    const root = sentinel.closest(".pf-tasks-list");
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) {
          loadMoreTasks();
        }
      },
      { root, rootMargin: "360px 0px", threshold: 0.01 }
    );
    observer.observe(sentinel);
    return () => observer.disconnect();
  });

  $effect(() => {
    if (visibleTaskCount <= visibleTasks.length) return;
    visibleTaskCount = Math.max(TASK_PAGE_SIZE, visibleTasks.length);
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

  function taskRenderWindowKey(rows: WorkflowTask[]): string {
    return [
      sourceFilter,
      statusFilter,
      searchTerms.join("\u0001"),
      rows.map(taskKey).join("\u0001")
    ].join("\u0002");
  }

  function initialTaskRenderCount(): number {
    const selectedIndex = selectedTaskKey
      ? visibleTasks.findIndex((task) => taskKey(task) === selectedTaskKey)
      : -1;
    const minimum = selectedIndex >= 0 ? selectedIndex + 1 : TASK_PAGE_SIZE;
    return Math.min(Math.max(TASK_PAGE_SIZE, minimum), visibleTasks.length);
  }

  function loadMoreTasks() {
    visibleTaskCount = Math.min(visibleTasks.length, visibleTaskCount + TASK_PAGE_SIZE);
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
    if (connection.monitor_command?.trim()) return true;
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

  function monitorPrimaryActionLabel(): string {
    if (selectedMonitorNeedsRepair) {
      return selectedMonitorRepairRunning ? "Reconnecting" : "Reconnect";
    }
    if (creatingMonitor) {
      return selectedMonitorBinding ? "Updating" : "Adding";
    }
    return selectedMonitorBinding ? "Update" : "Add";
  }

  function monitorRepairNotice(connection: WorkflowConnection): string {
    return `${connection.slug} needs auth repair before it can start new monitor tasks.`;
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

  async function loadContactScopeOptions() {
    if (contactsLoading) return;
    contactsLoading = true;
    try {
      contactSnapshot = await loadContacts(80);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      notice = `Could not load contacts: ${message}`;
    } finally {
      contactsLoading = false;
    }
  }

  async function createSelectedMonitor(event?: SubmitEvent) {
    event?.preventDefault();
    if (creatingMonitor || selectedMonitorRepairRunning) return;
    if (!selectedMonitorConnection) {
      notice = "Choose an account before adding a monitor.";
      return;
    }
    if (selectedMonitorNeedsRepair) {
      if (!selectedMonitorConnectionRecord) return;
      await reconnectConnection(selectedMonitorConnectionRecord);
      return;
    }
    const connection = monitorConnections.find((item) => item.slug === selectedMonitorConnection);
    const wasUpdate = selectedMonitorBinding !== null;
    const selectedModel = selectedMonitorModel.trim();
    creatingMonitor = true;
    try {
      const next = await createMonitor(
        selectedMonitorConnection,
        selectedModel || null,
        normalizeContactIds(selectedMonitorContactIds)
      );
      applySnapshot(next);
      showTaskConfig = false;
      const action = wasUpdate ? "updated" : "created";
      const model = selectedModel || "default model";
      notice = `Monitor ${action} for ${connection?.slug ?? selectedMonitorConnection} using ${model} and ${monitorContactScopeLabel(selectedMonitorContactIds)}.`;
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      notice = `Could not configure monitor: ${message}`;
    } finally {
      creatingMonitor = false;
    }
  }

  async function reconnectConnection(connection: WorkflowConnection) {
    if (!onRunTaskCommand) {
      notice = `Run ${connectionRepairCommand(connection)} to reconnect ${connection.slug}.`;
      return;
    }
    if (commandRunningFor !== null) return;
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

  function monitorMemoryForBinding(binding: WorkflowBinding | null): WorkflowMonitorMemory | null {
    if (!binding) return selectedConfigMemory;
    return monitorMemories.find((memory) => memory.connection_slug === binding.connection_slug) ?? selectedConfigMemory;
  }

  function onRulesMonitorChange(event: Event) {
    const slug = (event.currentTarget as HTMLSelectElement).value;
    selectedFilterBindingSlug = slug;
    const binding = monitorFilterBindings.find((item) => item.slug === slug) ?? null;
    const memory = monitorMemoryForBinding(binding);
    if (memory) chooseConfigMemory(memory.path);
  }

  async function addConfiguredRule(request: MonitorRuleAddRequest) {
    if (!selectedFilterBinding || savingMonitorRuleMode !== null) return;
    savingMonitorRuleMode = request.mode;
    try {
      const next = await addMonitorRule(request);
      applySnapshot(next);
      const action = request.mode === "include" ? "task" : "skip";
      notice = `Added ${action} rule for ${selectedFilterBinding.connection_slug}.`;
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      notice = `Could not add monitor rule: ${message}`;
    } finally {
      savingMonitorRuleMode = null;
    }
  }

  async function deleteConfiguredRule(mode: MonitorRuleMode, rule: WorkflowFilterRule, key: string) {
    if (!selectedFilterBinding || deletingMonitorRuleKey !== null) return;
    deletingMonitorRuleKey = key;
    try {
      const next = await deleteMonitorRule(
        selectedFilterBinding.connection_slug,
        mode,
        rule
      );
      applySnapshot(next);
      notice = `Removed monitor rule for ${selectedFilterBinding.connection_slug}.`;
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      notice = `Could not remove monitor rule: ${message}`;
    } finally {
      deletingMonitorRuleKey = null;
    }
  }

  function contactScopeChoices(): ContactChoice[] {
    const usedIds = new Set<string>();
    const choices: ContactChoice[] = [];
    for (const contact of contactSnapshot.contacts) {
      const ids = normalizeContactIds(contact.contact_ids);
      if (ids.length === 0) continue;
      ids.forEach((id) => usedIds.add(id));
      choices.push({
        key: `saved:${contact.id}`,
        label: contact.name,
        description: contact.description || ids.join(", "),
        ids,
        saved: true
      });
    }
    for (const candidate of contactSnapshot.candidates.slice(0, 40)) {
      const ids = normalizeContactIds([candidate.id]);
      if (ids.length === 0 || usedIds.has(ids[0])) continue;
      choices.push({
        key: `candidate:${candidate.id}`,
        label: contactCandidateLabel(candidate),
        description: candidate.context?.[0]?.text?.trim() || candidate.id,
        ids,
        saved: false
      });
    }
    return choices;
  }

  function contactCandidateLabel(candidate: ConnectorContact): string {
    return candidate.name?.trim() || candidate.id;
  }

  function monitorContactScopeLabel(ids: string[]): string {
    const count = normalizeContactIds(ids).length;
    if (count === 0) return "all contacts";
    return count === 1 ? "1 contact id" : `${count} contact ids`;
  }

  function contactChoiceSelected(choice: ContactChoice): boolean {
    const selected = new Set(selectedMonitorContactIds);
    return choice.ids.length > 0 && choice.ids.every((id) => selected.has(id));
  }

  function toggleContactChoice(choice: ContactChoice, checked: boolean) {
    const next = new Set(selectedMonitorContactIds);
    for (const id of choice.ids) {
      if (checked) {
        next.add(id);
      } else {
        next.delete(id);
      }
    }
    selectedMonitorContactIds = normalizeContactIds(Array.from(next));
  }

  function clearContactScope() {
    selectedMonitorContactIds = [];
  }

  function openTaskConfig() {
    showTaskConfig = true;
    taskConfigTab = "monitor";
    confirmDeleteMonitorSlug = null;
    contactScopeBindingKey = "";
    if (taskModelOptions.length === 0) {
      void loadTaskModelOptions();
    }
    if (contactSnapshot.contacts.length === 0 && contactSnapshot.candidates.length === 0) {
      void loadContactScopeOptions();
    }
    if (!configMemoryPath && monitorMemories.length > 0) {
      chooseConfigMemory(monitorMemories[0].path);
    }
  }

  function closeTaskConfig() {
    if (
      savingMemoryPath !== null
      || creatingMonitor
      || deletingMonitorSlug !== null
      || savingMonitorRuleMode !== null
      || deletingMonitorRuleKey !== null
    ) return;
    showTaskConfig = false;
    confirmDeleteMonitorSlug = null;
  }

  function chooseConfigMemory(path: string) {
    configMemoryPath = path;
    const memory = monitorMemories.find((item) => item.path === path) ?? null;
    memoryDraft = memory?.content ?? "";
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
          {#each renderedTasks as task (taskKey(task))}
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
          {#if hasMoreTasks}
            <div class="pf-task-lazy-sentinel" bind:this={taskListSentinel}>
              <button
                type="button"
                class="sc-btn"
                data-variant="outline"
                data-size="sm"
                onclick={loadMoreTasks}
              >
                Load {remainingTaskCount} more task{remainingTaskCount === 1 ? "" : "s"}
              </button>
            </div>
          {/if}
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
            <h2 id="pf-task-config-title">Task settings</h2>
            <span>Watch messages, ignore noise, and remember context.</span>
          </div>
          <button
            type="button"
            class="sc-btn"
            data-variant="ghost"
            data-size="sm"
            aria-label="Close task configuration"
            disabled={
              creatingMonitor
                || savingMemoryPath !== null
                || deletingMonitorSlug !== null
                || savingMonitorRuleMode !== null
                || deletingMonitorRuleKey !== null
            }
            onclick={closeTaskConfig}
          >
            <Icon name="x" size={12} />
          </button>
        </header>

        <div class="pf-task-settings-body">
          <nav class="pf-task-settings-nav" aria-label="Task settings sections">
            <button
              type="button"
              class="pf-task-settings-nav-item"
              data-active={taskConfigTab === "monitor"}
              onclick={() => (taskConfigTab = "monitor")}
            >
              Monitor
            </button>
            <button
              type="button"
              class="pf-task-settings-nav-item"
              data-active={taskConfigTab === "contacts"}
              onclick={() => (taskConfigTab = "contacts")}
            >
              Contacts
            </button>
            <button
              type="button"
              class="pf-task-settings-nav-item"
              data-active={taskConfigTab === "rules"}
              onclick={() => (taskConfigTab = "rules")}
            >
              Rules and memory
            </button>
          </nav>

          <div class="pf-task-settings-pane">
            {#if taskConfigTab === "monitor"}
              <form class="pf-task-settings-section" onsubmit={(event) => void createSelectedMonitor(event)}>
                <div class="pf-task-settings-section-head">
                  <strong>Monitor</strong>
                  <span>Choose which message source can start tasks.</span>
                </div>

                <div class="pf-task-settings-monitor-form">
                  <label>
                    <span>Account</span>
                    <select
                      bind:value={selectedMonitorConnection}
                      aria-label="Connection to monitor"
                      disabled={monitorConnections.length === 0 || creatingMonitor}
                    >
                      {#if monitorConnections.length === 0}
                        <option value="">Choose an account</option>
                      {:else}
                        {#each monitorConnections as connection (connection.slug)}
                          <option value={connection.slug}>
                            {monitorConnectionLabel(connection)}
                          </option>
                        {/each}
                      {/if}
                    </select>
                  </label>
                  <label>
                    <span>Model</span>
                    <input
                      list="pf-task-model-options"
                      bind:value={selectedMonitorModel}
                      aria-label="Task agent model"
                      placeholder={taskModelLoading ? "Loading models" : "Choose a model"}
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
                    class="sc-btn pf-task-settings-primary-button pf-task-settings-compact-button"
                    data-variant="solid"
                    data-size="sm"
                    disabled={selectedMonitorPrimaryDisabled}
                  >
                    {monitorPrimaryActionLabel()}
                  </button>
                </div>

                {#if selectedMonitorConnectionRecord && selectedMonitorNeedsRepair}
                  <p>{monitorRepairNotice(selectedMonitorConnectionRecord)}</p>
                {/if}
                {#if monitorConnections.length === 0}
                  <p>No trigger-ready accounts.</p>
                {/if}
                {#if taskModelLoadError}
                  <p>{taskModelLoadError}</p>
                {/if}

                <div class="pf-task-settings-monitor-list" aria-label="Configured monitors">
                  {#if monitorFilterBindings.length > 0}
                    {#each monitorFilterBindings as binding (binding.slug)}
                      <div class="pf-task-settings-monitor-row" data-enabled={binding.enabled}>
                        <label>
                          <span>Account</span>
                          <select
                            value={binding.connection_slug}
                            aria-label={`Account for ${binding.connection_slug}`}
                            disabled
                          >
                            <option>{binding.connection_slug}</option>
                          </select>
                        </label>
                        <label>
                          <span>Model</span>
                          <input
                            value={monitorModelLabel(binding)}
                            aria-label={`Model for ${binding.connection_slug}`}
                            disabled
                          />
                        </label>
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
                            {deletingMonitorSlug === binding.slug ? "Deleting" : "Confirm"}
                          </button>
                        {:else}
                          <button
                            type="button"
                            class="sc-btn pf-task-settings-icon-button"
                            data-variant="ghost"
                            data-size="sm"
                            aria-label={`Delete monitor for ${binding.connection_slug}`}
                            disabled={creatingMonitor || savingMemoryPath !== null || deletingMonitorSlug !== null}
                            onclick={() => requestDeleteMonitor(binding)}
                          >
                            <Icon name="x" size={14} />
                          </button>
                        {/if}
                      </div>
                    {/each}
                  {:else}
                    <p>No active monitors.</p>
                  {/if}
                </div>
              </form>
            {:else if taskConfigTab === "contacts"}
              <section class="pf-task-settings-section">
                <div class="pf-task-settings-section-toolbar">
                  <div class="pf-task-settings-section-head">
                    <strong>Contacts</strong>
                    <span>Choose which contact can trigger tasks.</span>
                  </div>
                  <button
                    type="button"
                    class="sc-btn"
                    data-variant="ghost"
                    data-size="sm"
                    disabled={contactsLoading}
                    onclick={() => void loadContactScopeOptions()}
                  >
                    <Icon name="refresh" size={14} />{contactsLoading ? "Loading" : "Refresh"}
                  </button>
                </div>

                <div class="pf-task-contact-table" aria-label="Monitor contacts">
                  <div class="pf-task-contact-table-head">
                    <span aria-hidden="true"></span>
                    <strong>Contact</strong>
                    <strong>Description</strong>
                    <strong>Action</strong>
                  </div>
                  <label class="pf-task-contact-table-row">
                    <span class="pf-task-contact-check">
                      <input
                        type="checkbox"
                        checked={selectedMonitorContactIds.length === 0}
                        onchange={(event) => {
                          if ((event.currentTarget as HTMLInputElement).checked) clearContactScope();
                        }}
                      />
                    </span>
                    <span class="pf-task-contact-person">
                      <span class="pf-task-contact-avatar">All</span>
                      <span>
                        <strong>All contacts</strong>
                        <small>{selectedMonitorConnection || "monitor account"}</small>
                      </span>
                    </span>
                    <span class="pf-task-contact-description">Allow any contact from the selected monitor source to trigger tasks.</span>
                    <span class="pf-task-contact-action" aria-hidden="true"></span>
                  </label>
                  {#if contactsLoading && contactChoices.length === 0}
                    <div class="pf-task-contact-table-empty">Loading contacts...</div>
                  {:else if contactChoices.length === 0}
                    <div class="pf-task-contact-table-empty">No saved contacts or connector candidates are available.</div>
                  {:else}
                    {#each contactChoices as choice (choice.key)}
                      <label class="pf-task-contact-table-row" data-saved={choice.saved}>
                        <span class="pf-task-contact-check">
                          <input
                            type="checkbox"
                            checked={contactChoiceSelected(choice)}
                            onchange={(event) => toggleContactChoice(choice, (event.currentTarget as HTMLInputElement).checked)}
                          />
                        </span>
                        <span class="pf-task-contact-person">
                          <span class="pf-task-contact-avatar">{choice.label.slice(0, 2).toUpperCase()}</span>
                          <span>
                            <strong>{choice.label}</strong>
                            <small>{choice.ids[0] ?? choice.key}</small>
                          </span>
                        </span>
                        <span class="pf-task-contact-description">{choice.description}</span>
                        <span class="pf-task-contact-action">
                          <button
                            type="button"
                            class="pf-task-contact-edit-button"
                            aria-label={`Edit ${choice.label}`}
                            disabled
                          >
                            <Icon name="edit" size={15} />
                          </button>
                        </span>
                      </label>
                    {/each}
                  {/if}
                </div>
              </section>
            {:else}
              <form class="pf-task-settings-section" onsubmit={(event) => void saveConfiguredMemory(event)}>
                <div class="pf-task-settings-section-head">
                  <strong>Monitor rules</strong>
                  <span>Rules, notes, and priorities this monitor uses before creating tasks.</span>
                </div>

                <label class="pf-task-config-memory-select">
                  <span>Monitor</span>
                  <select
                    value={selectedFilterBindingSlug}
                    aria-label="Monitor rules"
                    onchange={onRulesMonitorChange}
                  >
                    {#if monitorFilterBindings.length === 0}
                      <option value="">No active monitors</option>
                    {:else}
                      {#each monitorFilterBindings as binding (binding.slug)}
                        <option value={binding.slug}>{binding.connection_slug}</option>
                      {/each}
                    {/if}
                  </select>
                </label>

                <div class="pf-task-settings-rules-block">
                  <span class="pf-task-settings-field-label">Filter rules</span>
                  <MonitorRulesEditor
                    binding={selectedFilterBinding}
                    schema={selectedRuleSchema}
                    savingMode={savingMonitorRuleMode}
                    deletingKey={deletingMonitorRuleKey}
                    onAddRule={addConfiguredRule}
                    onDeleteRule={deleteConfiguredRule}
                  />
                </div>

                <div class="pf-task-settings-memory-block">
                  <span class="pf-task-settings-field-label">Rules and memory</span>
                  {#if monitorMemories.length > 0 && selectedConfigMemory}
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
                        class="sc-btn pf-task-settings-primary-button pf-task-settings-save-button"
                        data-variant="solid"
                        data-size="sm"
                        disabled={selectedConfigMemory.truncated || savingMemoryPath !== null}
                      >
                        {savingMemoryPath === selectedConfigMemory.path ? "Saving" : "Save memory"}
                      </button>
                    </div>
                  {:else}
                    <div class="pf-task-settings-empty">No monitor memory files yet.</div>
                  {/if}
                </div>
              </form>
            {/if}
          </div>
        </div>
      </div>
    </div>
  {/if}
</div>

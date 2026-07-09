<script lang="ts">
  import { onMount } from "svelte";
  import {
    activateAutomationRecord,
    deleteAutomationRecord,
    executeConnectorActionDraft,
    getAutomationPendingAction,
    loadAutomationCatalog,
    loadAutomationRunHistory,
    listAutomationPendingActions,
    listAutomations,
    rejectAutomationPendingAction,
    runAutomationPreview,
    saveAutomationRecord,
    syncAutomationPreview
  } from "../api/desktop";
  import type {
    AutomationCatalogAction,
    AutomationCatalogInput,
    AutomationCatalogResult,
    AutomationCatalogTrigger,
    AutomationNodeRef,
    AutomationPendingActionDetail,
    AutomationPendingActionListItem,
    AutomationRecordDto,
    AutomationRunLocation,
    AutomationRuntimeSyncResult,
    AutomationRunHistoryRecord,
    AutomationSpec,
    AutomationSource,
    AutomationStatus,
    AutomationStepSpec,
    SettingsSnapshot,
    AutomationTriggerSpec
  } from "../types";
  import "../design/chat.css";
  import Icon, { type IconName } from "../design/Icon.svelte";

  type AutomationItem = {
    id: string;
    title: string;
    description: string;
    status: string;
    source: string;
    updated: string;
    when: string;
    then: string;
    review: string;
    recent: string[];
    icon: IconName;
    prompt?: string;
    trigger?: AutomationTrigger | null;
    tools?: SelectedAutomationTool[];
    enabled?: boolean;
    owner?: string;
    history?: AutomationRun[];
    revision?: number;
    record?: AutomationRecordDto;
  };

  type AutomationRun = {
    id: string;
    title: string;
    status: string;
    started: string;
    duration: string;
    summary: string;
    error?: string | null;
    compiled?: boolean;
    runtimeStatus?: string;
    input?: unknown;
    result?: unknown;
  };

  type AutomationStarter = {
    id: string;
    title: string;
    description: string;
    icon: IconName;
    name: string;
    prompt: string;
    trigger: AutomationTrigger;
  };

  type AutomationTrigger = {
    icon: IconName;
    leading: string;
    target?: string;
    actorPrefix?: string;
    actor?: string;
    catalog?: AutomationCatalogTrigger;
    config?: Record<string, string>;
  };

  type AutomationApp = {
    id: string;
    title: string;
    description: string;
    icon: IconName;
    capabilities: AutomationCapability[];
  };

  type AutomationCapability = {
    id: string;
    title: string;
    description: string;
    targetLabel?: string;
    targetOptions?: string[];
    defaultTarget?: string;
    action: AutomationCatalogAction;
  };

  type VisibleAutomationApp = AutomationApp & {
    visibleCapabilities: AutomationCapability[];
  };

  // "Tool" is the user-facing umbrella. A selected tool can be backed by an
  // AgentEnv node or by a Puffer-owned connector action step.
  type SelectedAutomationTool = {
    id: string;
    appId: string;
    appTitle: string;
    icon: IconName;
    title: string;
    targetLabel?: string;
    targetOptions: string[];
    target: string | null;
    action: AutomationCatalogAction;
  };

  type AutomationDraft = {
    name: string;
    prompt: string;
    trigger: AutomationTrigger | null;
    tools: SelectedAutomationTool[];
  };

  type Props = {
    settingsSnapshot?: SettingsSnapshot | null;
    onOpenAutomationRuntimeSettings?: () => void;
  };

  let props: Props = $props();

  const blankAutomationName = "Untitled automation";

  const automations: AutomationItem[] = [
    {
      id: "review-inbox",
      title: "Review inbox",
      description: "Review drafts before they go out.",
      status: "Needs review",
      source: "GitHub, Telegram",
      updated: "Today 09:12",
      when: "A pull request, issue, or message needs a response.",
      then: "Puffer gathers the relevant context and prepares a concise draft.",
      review: "You edit, approve, or reject the draft from the detail pane.",
      recent: ["4 drafts waiting", "2 new since this morning", "Last approved yesterday"],
      icon: "listTodo"
    },
    {
      id: "pr-review",
      title: "PR review assistant",
      description: "Summarize code changes and draft a review note.",
      status: "Ready",
      source: "GitHub",
      updated: "Starter",
      when: "A new pull request is opened or marked ready for review.",
      then: "Puffer reads the diff, checks test signals, and writes a short review draft.",
      review: "You decide whether to post, edit, or keep the note for later.",
      recent: ["Template ready", "Works best with linked repos", "Draft style: concise"],
      icon: "git"
    },
    {
      id: "calendar-rsvp",
      title: "Calendar RSVP",
      description: "Prepare RSVP suggestions with meeting context.",
      status: "Needs setup",
      source: "Calendar",
      updated: "Starter",
      when: "A new invite arrives or a meeting time changes.",
      then: "Puffer checks conflicts and drafts an accept, decline, or tentative response.",
      review: "You approve the RSVP after checking the guest list and conflicts.",
      recent: ["Choose calendars", "Set default response tone", "Keep final approval on"],
      icon: "clock"
    },
    {
      id: "release-watch",
      title: "Release watch",
      description: "Watch a release branch and surface changes that need attention.",
      status: "Paused",
      source: "GitHub Actions",
      updated: "Every 15 min",
      when: "A release check fails, recovers, or waits for a manual step.",
      then: "Puffer summarizes the change and suggests the next owner-facing update.",
      review: "You review the summary before sending it to the team.",
      recent: ["Last run yesterday", "No failures in latest run", "Paused by user"],
      icon: "rocket"
    },
    {
      id: "morning-digest",
      title: "Morning digest",
      description: "Collect overnight updates into a short start-of-day brief.",
      status: "Ready",
      source: "Slack, Calendar",
      updated: "Daily 09:00",
      when: "Your workday starts.",
      then: "Puffer groups overnight updates, upcoming meetings, and waiting reviews.",
      review: "You skim the digest and open anything that needs action.",
      recent: ["3 sources selected", "Digest length: short", "Weekdays only"],
      icon: "logs"
    }
  ];

  const automationTemplates: AutomationStarter[] = [
    {
      id: "pr-review",
      title: "Review PRs",
      description: "Prepare a concise review draft when code changes need attention.",
      icon: "git",
      name: "PR review draft",
      prompt: "When a pull request opens, summarize the changes and prepare a review note for me.",
      trigger: {
        icon: "git",
        leading: "PR opened in",
        target: "Select repos",
        actorPrefix: "by",
        actor: "Anyone"
      }
    },
    {
      id: "reply-drafts",
      title: "Reply drafts",
      description: "Turn incoming messages into replies you can edit before sending.",
      icon: "edit",
      name: "Reply draft",
      prompt: "When a message needs a response, gather context and prepare a reply draft.",
      trigger: {
        icon: "edit",
        leading: "Message arrives from",
        target: "Trusted contacts"
      }
    },
    {
      id: "calendar-rsvp",
      title: "Calendar RSVP",
      description: "Check meeting conflicts and prepare an RSVP suggestion.",
      icon: "clock",
      name: "Calendar RSVP",
      prompt: "When a calendar invite arrives, check conflicts and prepare an RSVP suggestion.",
      trigger: {
        icon: "clock",
        leading: "Invite arrives on",
        target: "Calendar",
        actorPrefix: "for",
        actor: "Any meeting"
      }
    },
    {
      id: "morning-digest",
      title: "Morning digest",
      description: "Collect overnight updates into a short start-of-day brief.",
      icon: "logs",
      name: "Morning digest",
      prompt: "Every weekday morning, summarize overnight updates and anything waiting for me.",
      trigger: {
        icon: "clock",
        leading: "Weekdays at",
        target: "09:00",
        actorPrefix: "from",
        actor: "Selected sources"
      }
    }
  ];

  const baseUserAutomations: AutomationItem[] = [];
  const everyDayTrigger: AutomationTrigger = {
    icon: "clock",
    leading: "Every day at",
    target: "09:00"
  };
  const customScheduleTrigger: AutomationTrigger = {
    icon: "clock",
    leading: "Custom schedule",
    target: "Cron"
  };
  const prOpenedTrigger: AutomationTrigger = {
    icon: "git",
    leading: "PR opened in",
    target: "Select repos",
    actorPrefix: "by",
    actor: "Anyone"
  };
  const draftOpenedTrigger: AutomationTrigger = {
    icon: "git",
    leading: "Draft opened in",
    target: "Select repos"
  };
  const commentAddedTrigger: AutomationTrigger = {
    icon: "git",
    leading: "Comment added in",
    target: "Select repos"
  };
  const labelChangeTrigger: AutomationTrigger = {
    icon: "git",
    leading: "Label changes in",
    target: "Select repos"
  };
  type AutomationLibraryTab = "your" | "templates";
  type AutomationDetailTab = "settings" | "history";

  let screenMode = $state<"home" | "new" | "detail">("home");
  let activeAutomationLibraryTab = $state<AutomationLibraryTab>("your");
  let activeAutomationDetailTab = $state<AutomationDetailTab>("settings");
  let savedAutomations = $state<AutomationItem[]>([]);
  let savedAutomationSequence = $state(0);
  let savedRunSequence = $state(0);
  let automationLoadError = $state<string | null>(null);
  let automationCatalogError = $state<string | null>(null);
  let automationReviewError = $state<string | null>(null);
  let automationPendingActions = $state<AutomationPendingActionListItem[]>([]);
  let selectedPendingAction = $state<AutomationPendingActionDetail | null>(null);
  let pendingActionMessage = $state("");
  let pendingActionRejectReason = $state("");
  let pendingActionLoading = $state(false);
  let pendingActionSubmitting = $state(false);
  let automationSaving = $state(false);
  let automationStatusChanging = $state(false);
  let automationRunning = $state(false);
  let triggerCatalog = $state<AutomationCatalogTrigger[]>([]);
  let commonApps = $state<AutomationApp[]>([]);
  let userAutomations = $derived([...savedAutomations, ...baseUserAutomations]);
  let selectedAutomationId = $state<string | null>(null);
  let selectedAutomation = $derived(userAutomations.find((item) => item.id === selectedAutomationId) ?? null);
  let homePrompt = $state("");
  let automationName = $state(blankAutomationName);
  let automationPrompt = $state("");
  let automationSource = $state<AutomationSource>({ type: "blank" });
  let automationRunLocation = $state<AutomationRunLocation>(defaultAutomationRunLocation());
  let automationTrigger = $state<AutomationTrigger | null>(null);
  let selectedTools = $state<SelectedAutomationTool[]>([]);
  let automationEnabled = $state(false);
  let automationTestInputText = $state(defaultAutomationTestInputText(""));
  let triggerMenuOpen = $state(false);
  let toolMenuOpen = $state(false);
  let automationActionMenuOpen = $state(false);
  let editingToolId = $state<string | null>(null);
  let toolSearchQuery = $state("");
  let visibleToolApps = $derived(visibleAppsForSearch(toolSearchQuery));

  function defaultAutomationRunLocation(): AutomationRunLocation {
    return props.settingsSnapshot?.workflowBackend.mode ?? "local";
  }

  function openAutomationRuntimeSettings() {
    props.onOpenAutomationRuntimeSettings?.();
  }

  onMount(() => {
    void refreshAutomations();
    void refreshAutomationCatalog();
    void refreshAutomationPendingActions();
  });

  async function refreshAutomations() {
    try {
      const snapshot = await listAutomations();
      savedAutomations = snapshot.automations
        .filter((record) => record.status !== "archived")
        .map(automationItemFromRecord);
      automationLoadError = null;
    } catch (error) {
      automationLoadError = errorMessage(error);
    }
  }

  async function refreshAutomationCatalog() {
    try {
      const catalog = await loadAutomationCatalog();
      triggerCatalog = catalog.triggers;
      commonApps = appsFromCatalog(catalog);
      automationCatalogError = [catalog.trigger_error, catalog.action_error]
        .filter(Boolean)
        .join(" | ") || null;
    } catch (error) {
      triggerCatalog = [];
      commonApps = [];
      automationCatalogError = errorMessage(error);
    }
  }

  async function refreshAutomationPendingActions() {
    try {
      const result = await listAutomationPendingActions();
      automationPendingActions = result.drafts;
      automationReviewError = null;
      if (
        selectedPendingAction &&
        !result.drafts.some((draft) => draft.draft_id === selectedPendingAction?.draft_id)
      ) {
        snoozePendingAction();
      }
    } catch (error) {
      automationPendingActions = [];
      automationReviewError = errorMessage(error);
    }
  }

  async function openPendingAction(action: AutomationPendingActionListItem) {
    pendingActionLoading = true;
    automationReviewError = null;
    try {
      const result = await getAutomationPendingAction(action.draft_id, action.version);
      selectedPendingAction = result.draft;
      pendingActionMessage = result.draft.message;
      pendingActionRejectReason = "";
    } catch (error) {
      automationReviewError = errorMessage(error);
    } finally {
      pendingActionLoading = false;
    }
  }

  async function approvePendingAction() {
    if (!selectedPendingAction || pendingActionSubmitting) return;
    pendingActionSubmitting = true;
    automationReviewError = null;
    try {
      // send_message drafts approve an edited message; exact_action drafts with
      // an editable body field approve an edited input (body only — the daemon
      // pins the destination). Otherwise the exact action is approved as-is.
      let approvedMessage: string | undefined;
      let approvedInput: Record<string, unknown> | undefined;
      if (selectedPendingAction.message_editable) {
        approvedMessage = pendingActionMessage;
      } else if (selectedPendingAction.message_field) {
        approvedInput = {
          ...(selectedPendingAction.input ?? {}),
          [selectedPendingAction.message_field]: pendingActionMessage
        };
      }
      await executeConnectorActionDraft({
        draftId: selectedPendingAction.draft_id,
        version: selectedPendingAction.version,
        approvedMessage,
        approvedInput,
        clientRequestId: pendingActionClientRequestId(selectedPendingAction)
      });
      snoozePendingAction();
      await refreshAutomationPendingActions();
    } catch (error) {
      automationReviewError = errorMessage(error);
    } finally {
      pendingActionSubmitting = false;
    }
  }

  async function rejectPendingAction() {
    if (!selectedPendingAction || pendingActionSubmitting) return;
    const reason = pendingActionRejectReason.trim();
    if (!reason) {
      automationReviewError = "Add a short rejection reason.";
      return;
    }
    pendingActionSubmitting = true;
    automationReviewError = null;
    try {
      await rejectAutomationPendingAction({
        draftId: selectedPendingAction.draft_id,
        version: selectedPendingAction.version,
        reason
      });
      snoozePendingAction();
      await refreshAutomationPendingActions();
    } catch (error) {
      automationReviewError = errorMessage(error);
    } finally {
      pendingActionSubmitting = false;
    }
  }

  function snoozePendingAction() {
    selectedPendingAction = null;
    pendingActionMessage = "";
    pendingActionRejectReason = "";
  }

  function pendingActionClientRequestId(action: AutomationPendingActionDetail): string {
    const random = globalThis.crypto?.randomUUID?.() ?? `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
    return `automation-review-${action.draft_id}-${action.version}-${random}`;
  }

  function applyStarter(starter: AutomationStarter) {
    automationName = starter.name;
    automationPrompt = starter.prompt;
    automationSource = { type: "template", template_id: starter.id };
    automationRunLocation = defaultAutomationRunLocation();
    automationTrigger = copyTrigger(starter.trigger);
    selectedTools = [];
    automationEnabled = false;
    selectedAutomationId = null;
    triggerMenuOpen = false;
    toolMenuOpen = false;
    automationActionMenuOpen = false;
    editingToolId = null;
    toolSearchQuery = "";
  }

  function openBlankAutomation(prompt = "") {
    automationName = blankAutomationName;
    automationPrompt = prompt.trim();
    automationSource = { type: "blank" };
    automationRunLocation = defaultAutomationRunLocation();
    automationTrigger = null;
    selectedTools = [];
    automationEnabled = false;
    selectedAutomationId = null;
    triggerMenuOpen = false;
    toolMenuOpen = false;
    automationActionMenuOpen = false;
    editingToolId = null;
    toolSearchQuery = "";
    screenMode = "new";
  }

  function appsFromCatalog(catalog: AutomationCatalogResult): AutomationApp[] {
    const groups = new Map<string, AutomationApp>();
    for (const action of catalog.actions.filter(isSupportedAutomationToolAction)) {
      const appId = action.connector_slug ?? action.kind;
      const title = appTitle(appId);
      const group = groups.get(appId) ?? {
        id: appId,
        title,
        description: action.kind === "agentenv_node" ? "Local runtime tools." : "Connector actions and tools.",
        icon: iconName(action.icon),
        capabilities: []
      };
      const targetOptions = targetOptionsFromCatalogInputs(action);
      group.capabilities.push({
        id: action.id,
        title: action.label,
        description: actionSummary(action),
        targetLabel: targetLabelForAction(action),
        targetOptions,
        defaultTarget: targetOptions[0],
        action
      });
      groups.set(appId, group);
    }
    return Array.from(groups.values()).sort((a, b) => a.title.localeCompare(b.title));
  }

  function isSupportedAutomationToolAction(action: AutomationCatalogAction): boolean {
    return action.node_ref.node_type !== "tool_capability";
  }

  function appTitle(id: string): string {
    if (id === "github") return "GitHub";
    if (id === "gmail") return "Gmail";
    if (id === "google-calendar" || id === "gcal-browser") return "Google Calendar";
    if (id === "agentenv_node") return "Local Runtime";
    return id
      .split(/[-_]/)
      .filter(Boolean)
      .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
      .join(" ");
  }

  function actionSummary(action: AutomationCatalogAction): string {
    const connection = action.connection_state ? `Connection: ${connectionStateLabel(action.connection_state)}.` : "";
    const permission = action.permission_summary ? ` ${action.permission_summary}` : "";
    return `${action.summary || "Runtime action."} ${connection}${permission}`.trim();
  }

  function targetLabelForAction(action: AutomationCatalogAction): string | undefined {
    if (action.external_side_effect) return "as";
    if (action.connection_slug) return "via";
    return undefined;
  }

  function targetOptionsFromCatalogInputs(action: AutomationCatalogAction): string[] {
    const targetInput = action.required_inputs.find((input) => input.id === "target" && input.options?.length);
    return targetInput?.options ?? [];
  }

  function iconName(value: string | null | undefined): IconName {
    const allowed = new Set<IconName>(["bolt", "clock", "edit", "git", "listTodo", "logs", "rocket"]);
    return allowed.has(value as IconName) ? (value as IconName) : "bolt";
  }

  function catalogTriggerById(id: string): AutomationCatalogTrigger | null {
    return triggerCatalog.find((trigger) => trigger.id === id) ?? null;
  }

  function firstTriggerMatch(match: (trigger: AutomationCatalogTrigger) => boolean): AutomationTrigger | null {
    const entry = triggerCatalog.find(match);
    return entry ? triggerFromCatalog(entry) : null;
  }

  function defaultInputValues(inputs: AutomationCatalogInput[]): Record<string, string> {
    return Object.fromEntries(
      inputs.map((input) => [input.id, input.default == null ? "" : String(input.default)])
    );
  }

  function triggerFromCatalog(entry: AutomationCatalogTrigger): AutomationTrigger {
    const config = defaultInputValues(entry.required_inputs);
    return {
      icon: iconName(entry.icon),
      leading: entry.label,
      target: triggerTarget(entry, config),
      actorPrefix: entry.connection_state ? "status" : undefined,
      actor: entry.connection_state ? connectionStateLabel(entry.connection_state) : undefined,
      catalog: entry,
      config
    };
  }

  function triggerTarget(entry: AutomationCatalogTrigger, config: Record<string, string>): string | undefined {
    if (entry.kind === "schedule") {
      if (config.mode === "cron") return config.cron || "Cron";
      return config.time || "09:00";
    }
    return config.repo || config.source || entry.connection_slug || undefined;
  }

  function connectionStateLabel(state: string): string {
    switch (state) {
      case "active":
      case "authenticated":
      case "ready":
        return "Ready";
      case "degraded":
        return "Needs repair";
      case "disabled":
        return "Disabled";
      case "not_connected":
      case "created":
      case "authenticating":
        return "Needs connection";
      default:
        return state.replace(/_/g, " ");
    }
  }

  function appById(id: string): AutomationApp | null {
    return commonApps.find((app) => app.id === id) ?? null;
  }

  function selectedToolFrom(app: AutomationApp, capability: AutomationCapability): SelectedAutomationTool {
    return {
      id: `${app.id}:${capability.id}`,
      appId: app.id,
      appTitle: app.title,
      icon: app.icon,
      title: capability.title,
      targetLabel: capability.targetLabel,
      targetOptions: capability.targetOptions ?? [],
      target: capability.defaultTarget ?? capability.targetOptions?.[0] ?? null,
      action: capability.action
    };
  }

  function toolById(appId: string, capabilityId: string): SelectedAutomationTool | null {
    const app = appById(appId);
    const capability = app?.capabilities.find((candidate) => candidate.id === capabilityId);
    if (!app || !capability) return null;
    return selectedToolFrom(app, capability);
  }

  function toolBySelectedId(toolId: string): SelectedAutomationTool | null {
    for (const app of commonApps) {
      const capability = app.capabilities.find((candidate) => toolIdFor(app, candidate) === toolId);
      if (capability) return selectedToolFrom(app, capability);
    }
    return null;
  }

  function toolsById(ids: Array<[string, string]>): SelectedAutomationTool[] {
    return ids
      .map(([appId, capabilityId]) => toolById(appId, capabilityId))
      .filter((tool): tool is SelectedAutomationTool => tool !== null);
  }

  function copyTrigger(trigger: AutomationTrigger | null): AutomationTrigger | null {
    return trigger ? { ...trigger, config: { ...(trigger.config ?? {}) } } : null;
  }

  function copySelectedTools(tools: SelectedAutomationTool[]): SelectedAutomationTool[] {
    return tools.map((tool) => ({
      ...tool,
      targetOptions: [...tool.targetOptions]
    }));
  }

  function errorMessage(error: unknown): string {
    return error instanceof Error ? error.message : String(error);
  }

  function automationIsActive(record: AutomationRecordDto): boolean {
    return record.status === "enabled" && record.runtime.status === "deployed";
  }

  function statusLabel(record: AutomationRecordDto): string {
    if (record.status === "archived") return "Archived";
    if (automationIsActive(record)) return "Active";
    return "Paused";
  }

  function iconForTrigger(trigger: AutomationTrigger | null): IconName {
    return trigger?.icon ?? "bolt";
  }

  function triggerFromSpec(trigger: AutomationTriggerSpec | undefined): AutomationTrigger | null {
    if (!trigger) return null;
    if (trigger.type === "agent_env_node") {
      const target =
        typeof trigger.node.config?.time === "string"
          ? trigger.node.config.time
          : typeof trigger.node.config?.target === "string"
            ? trigger.node.config.target
            : undefined;
      const leading = triggerLeadingFromNode(trigger, target);
      return {
        icon: trigger.node.node_type.includes("schedule") ? "clock" : "bolt",
        leading,
        target,
        config: stringifyConfig(trigger.node.config)
      };
    }
    return {
      icon: trigger.connector_slug?.includes("git") || trigger.connection_slug.includes("git") ? "git" : "bolt",
      leading: trigger.summary ?? `When ${trigger.connection_slug} receives an event`,
      target: trigger.connection_slug
    };
  }

  function triggerLeadingFromNode(trigger: Extract<AutomationTriggerSpec, { type: "agent_env_node" }>, target: string | undefined): string {
    if (trigger.node.node_type.includes("schedule")) {
      if (trigger.summary && target && trigger.summary.endsWith(target)) {
        return trigger.summary.slice(0, -target.length).trim();
      }
      if (trigger.node.name === "Schedule") return "Every day at";
      if (trigger.node.name === "Cron schedule") return "Custom schedule";
    }
    return trigger.node.name || trigger.summary || trigger.node.node_type;
  }

  function toolFromStep(step: AutomationStepSpec): SelectedAutomationTool | null {
    if (step.type !== "agent_env_node" || step.id === "agent") return null;
    const connectorSlug = typeof step.node.config?.connector_slug === "string" ? step.node.config.connector_slug : null;
    const connectorAction = typeof step.node.config?.action === "string" ? step.node.config.action : null;
    if (connectorSlug && connectorAction) {
      const connectionSlug = typeof step.node.config?.connection_slug === "string" ? step.node.config.connection_slug : null;
      const app = appById(connectorSlug);
      const capability = app?.capabilities.find((candidate) => {
        const action = candidate.action;
        return (
          action.connector_slug === connectorSlug &&
          action.action === connectorAction &&
          (connectionSlug == null || action.connection_slug === connectionSlug)
        );
      });
      const tool = app && capability ? selectedToolFrom(app, capability) : null;
      if (tool) return tool;
    }
    const toolId = typeof step.node.config?.tool_id === "string" ? step.node.config.tool_id : "";
    const tool = toolBySelectedId(toolId);
    if (!tool) return null;
    const target = typeof step.node.config?.target === "string" ? step.node.config.target : tool.target;
    return { ...tool, target };
  }

  function stringifyConfig(config: Record<string, unknown> | undefined): Record<string, string> {
    if (!config) return {};
    return Object.fromEntries(
      Object.entries(config)
        .filter(([, value]) => value == null || ["string", "number", "boolean"].includes(typeof value))
        .map(([key, value]) => [key, value == null ? "" : String(value)])
    );
  }

  function automationItemFromRecord(record: AutomationRecordDto): AutomationItem {
    const trigger = triggerFromSpec(record.spec.triggers[0]);
    const tools = record.spec.flow.steps
      .map(toolFromStep)
      .filter((tool): tool is SelectedAutomationTool => tool !== null);
    const title = record.spec.name.trim() || blankAutomationName;
    const description = record.spec.description?.trim() || record.spec.instructions.trim() || "Ready to configure.";
    return {
      id: record.id,
      title,
      description,
      status: statusLabel(record),
      source: "Puffer",
      updated: formatUpdated(record.updated_at_ms),
      when: triggerSummary(trigger),
      then: record.spec.instructions,
      review: record.spec.review.human_approval_required
        ? "You can review results before any action is sent."
        : "Runs without a required review gate.",
      recent: [`Revision ${record.revision}`, runtimeStatusLabel(record.runtime.status)],
      icon: iconForTrigger(trigger),
      prompt: record.spec.instructions,
      trigger,
      tools,
      enabled: automationIsActive(record),
      owner: "You",
      history: [],
      revision: record.revision,
      record
    };
  }

  function defaultAutomationTestInputText(instructions: string, trigger: AutomationTrigger | null = null): string {
    const text =
      instructions.trim() ||
      "Alice says checkout rollout is blocked because Stripe webhooks are intermittently failing in staging. Bob suspects a recent env var change. They need a decision before tomorrow's launch freeze. Lunch plans are unrelated.";
    return JSON.stringify(
      {
        text,
        trigger: triggerSummary(trigger)
      },
      null,
      2
    );
  }

  function runtimeStatusLabel(status: string): string {
    switch (status) {
      case "draft_synced":
        return "Runtime synced";
      case "deployed":
        return "Runtime deployed";
      case "stale":
        return "Runtime stale";
      case "error":
        return "Runtime error";
      default:
        return "Not compiled";
    }
  }

  function formatUpdated(value: number): string {
    if (!Number.isFinite(value) || value <= 0) return "Saved";
    const delta = Date.now() - value;
    if (delta < 60_000) return "Just now";
    if (delta < 3_600_000) return `${Math.max(1, Math.round(delta / 60_000))} min ago`;
    if (delta < 86_400_000) return `${Math.max(1, Math.round(delta / 3_600_000))} hr ago`;
    return new Date(value).toLocaleDateString();
  }

  function pendingActionTimeLabel(action: AutomationPendingActionListItem | AutomationPendingActionDetail): string {
    if (typeof action.created_at_ms === "number") return formatUpdated(action.created_at_ms);
    if (action.created_at) {
      const parsed = Date.parse(action.created_at);
      if (Number.isFinite(parsed)) return formatUpdated(parsed);
      return action.created_at;
    }
    return "Queued";
  }

  function pendingActionConnectorLabel(action: AutomationPendingActionListItem | AutomationPendingActionDetail): string {
    return [appTitle(action.connector_slug), actionLabel(action.action)].filter(Boolean).join(" / ");
  }

  function actionLabel(value: string): string {
    if (!value) return "";
    return value
      .split(/[-_]/)
      .filter(Boolean)
      .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
      .join(" ");
  }

  function pendingActionRecipientLabel(action: AutomationPendingActionListItem | AutomationPendingActionDetail): string {
    return action.recipient_label ?? action.recipient ?? action.recipient_stable_id ?? "Destination";
  }

  function pendingActionApprovalLabel(action: AutomationPendingActionListItem | AutomationPendingActionDetail): string {
    return action.message_editable ? "Editable message" : "Exact action";
  }

  function pendingActionDestinationEntries(action: AutomationPendingActionDetail): { key: string; value: string }[] {
    return Object.entries(action.destination_metadata ?? {})
      .filter(([, value]) => value !== null && value !== undefined)
      .slice(0, 8)
      .map(([key, value]) => ({ key: actionLabel(key), value: pendingActionMetadataValue(value) }));
  }

  function pendingActionMetadataValue(value: unknown): string {
    if (typeof value === "string") return value;
    if (typeof value === "number" || typeof value === "boolean") return String(value);
    try {
      return JSON.stringify(value);
    } catch {
      return String(value);
    }
  }

  function slugifyAutomationName(name: string): string {
    const base = name
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "")
      .slice(0, 48);
    return base || "automation";
  }

  function nextAutomationId(name: string): string {
    const nextSequence = savedAutomationSequence + 1;
    savedAutomationSequence = nextSequence;
    const base = slugifyAutomationName(name);
    const candidate = `${base}-${Date.now().toString(36)}-${nextSequence.toString(36)}`;
    return candidate.replace(/-+/g, "-");
  }

  function triggerSpecFromUi(trigger: AutomationTrigger | null): AutomationTriggerSpec {
    const summary = triggerSummary(trigger);
    if (!trigger) {
      throw new Error("Choose a trigger before saving the automation.");
    }
    if (trigger.catalog) {
      const spec = JSON.parse(JSON.stringify(trigger.catalog.spec_template)) as AutomationTriggerSpec;
      if (spec.type === "agent_env_node") {
        spec.node = {
          ...spec.node,
          config: {
            ...(spec.node.config ?? {}),
            ...(trigger.config ?? {}),
            target: trigger.target ?? trigger.config?.time ?? trigger.config?.cron ?? null
          }
        };
        spec.summary = summary;
      } else if (spec.type === "puffer_connection") {
        const filter = trigger.config?.filter?.trim();
        spec.summary = summary;
        if (filter) {
          spec.filter = { type: "regex", pattern: filter, case_insensitive: true };
        }
      }
      return spec;
    }
    if (trigger.icon === "git") {
      return {
        type: "puffer_connection",
        id: "trigger-1",
        connection_slug: "github",
        connector_slug: "github",
        summary
      };
    }
    return {
      type: "agent_env_node",
      id: "trigger-1",
      node: {
        node_type: trigger.icon === "clock" ? "schedule" : "event",
        name: trigger.leading,
        trusted: true,
        config: {
          target: trigger.target ?? null,
          actor: trigger.actor ?? null
        }
      },
      summary
    };
  }

  function stepIdFromTool(tool: SelectedAutomationTool, index: number): string {
    return `tool-${index + 1}-${tool.id.replace(/[^a-zA-Z0-9_-]+/g, "-")}`;
  }

  function isGeneratedTrigger(trigger: AutomationTriggerSpec): boolean {
    if (trigger.type === "agent_env_node") {
      return trigger.id === "trigger-1" && ["schedule", "event", "webhook"].includes(trigger.node.node_type);
    }
    return (
      trigger.id === "trigger-1" &&
      trigger.connection_slug === "github" &&
      (trigger.connector_slug == null || trigger.connector_slug === "github") &&
      trigger.filter == null &&
      (trigger.ignore_filters == null || trigger.ignore_filters.length === 0) &&
      (trigger.contact_ids == null || trigger.contact_ids.length === 0)
    );
  }

  function isGeneratedStep(step: AutomationStepSpec, index: number): boolean {
    if (step.type !== "agent_env_node") return false;
    if (index === 0) return step.id === "agent" && ["puffer_agent", "transform_js"].includes(step.node.node_type);
    return toolFromStep(step) !== null;
  }

  function isUiRoundTrippableSpec(spec: AutomationSpec): boolean {
    return (
      spec.triggers.length === 1 &&
      isGeneratedTrigger(spec.triggers[0]) &&
      spec.flow.steps.length > 0 &&
      spec.flow.steps.every(isGeneratedStep)
    );
  }

  function patchUnsupportedSpec(existing: AutomationSpec, title: string, description: string): AutomationSpec {
    return {
      ...existing,
      name: title,
      description,
      instructions: description
    };
  }

  function detailSpecForSave(selected: AutomationItem, title: string, description: string): AutomationSpec {
    const existing = selected.record?.spec;
    if (!existing) return automationSpecFromUi(title, description, automationSource);
    if (!isUiRoundTrippableSpec(existing)) return patchUnsupportedSpec(existing, title, description);
    return automationSpecFromUi(title, description, existing.source);
  }

  function automationSpecFromUi(
    title: string,
    description: string,
    source: AutomationSource = automationSource
  ): AutomationSpec {
    return {
      spec_version: 1,
      name: title,
      description,
      source,
      instructions: description,
      run_location: automationRunLocation,
      triggers: [triggerSpecFromUi(automationTrigger)],
      flow: {
        steps: [
          {
            type: "agent_env_node",
            id: "agent",
            node: {
              node_type: "puffer_agent",
              name: "Agent",
              trusted: true,
              config: {
                instructions: description,
                tools: selectedTools.map(productToolConfig),
                permissions: {}
              }
            },
            summary: "Run the Agent"
          },
          ...selectedTools.map((tool, index): AutomationStepSpec => ({
            type: "agent_env_node",
            id: stepIdFromTool(tool, index),
            node: nodeRefForTool(tool),
            summary: selectedToolLabel(tool)
          }))
        ]
      },
      review: {
        human_approval_required: true
      }
    };
  }

  function productToolConfig(tool: SelectedAutomationTool): Record<string, unknown> {
    return {
      id: tool.id,
      app_id: tool.appId,
      title: tool.title,
      target: tool.target,
      action: tool.action.action ?? null,
      connector_slug: tool.action.connector_slug ?? null,
      connection_slug: tool.action.connection_slug ?? null
    };
  }

  function nodeRefForTool(tool: SelectedAutomationTool): AutomationNodeRef {
    return {
      ...tool.action.node_ref,
      config: {
        ...(tool.action.node_ref.config ?? {}),
        tool_id: tool.id,
        app_id: tool.appId,
        capability: tool.title,
        target: tool.target,
        human_approval_required: Boolean(tool.action.external_side_effect)
      }
    };
  }

  function upsertAutomationRecord(record: AutomationRecordDto) {
    const item = automationItemFromRecord(record);
    savedAutomations = [
      item,
      ...savedAutomations.filter((candidate) => candidate.id !== record.id)
    ];
    selectedAutomationId = record.id;
  }

  function capabilityMatchesSearch(capability: AutomationCapability, query: string): boolean {
    return [capability.title, capability.description, capability.targetLabel, ...(capability.targetOptions ?? [])].some(
      (value) => value?.toLowerCase().includes(query)
    );
  }

  function visibleAppsForSearch(query: string): VisibleAutomationApp[] {
    const normalizedQuery = query.trim().toLowerCase();
    return commonApps
      .map((app) => {
        const appMatches =
          !normalizedQuery ||
          [app.title, app.description].some((value) => value.toLowerCase().includes(normalizedQuery));
        const visibleCapabilities = appMatches
          ? app.capabilities
          : app.capabilities.filter((capability) => capabilityMatchesSearch(capability, normalizedQuery));
        return {
          ...app,
          visibleCapabilities
        };
      })
      .filter((app) => app.visibleCapabilities.length > 0);
  }

  function draftFromPrompt(prompt: string): AutomationDraft {
    const trimmedPrompt = prompt.trim();
    const lowerPrompt = trimmedPrompt.toLowerCase();
    if (/\bpr\b|pull request/.test(lowerPrompt)) {
      return {
        name: "PR review draft",
        prompt: trimmedPrompt,
        trigger: firstTriggerMatch((trigger) => trigger.connector_slug?.includes("github") || /pull request|github/i.test(trigger.label)) ?? prOpenedTrigger,
        tools: firstToolsByAction((action) => action.action === "comment-on-pull-request" || /comment on pull request/i.test(action.label), 1)
      };
    }
    if (/calendar|invite|rsvp|meeting/.test(lowerPrompt)) {
      return {
        name: "Calendar RSVP",
        prompt: trimmedPrompt,
        trigger: firstTriggerMatch((trigger) => /calendar|gcal|invite/i.test(`${trigger.connector_slug} ${trigger.label}`)) ?? automationTemplates.find((template) => template.id === "calendar-rsvp")?.trigger ?? null,
        tools: firstToolsByAction((action) => /calendar|gcal|rsvp|accept|deny/i.test(`${action.connector_slug} ${action.label}`), 1)
      };
    }
    if (/gmail|email|mail/.test(lowerPrompt)) {
      return {
        name: "Email reply draft",
        prompt: trimmedPrompt,
        trigger: firstTriggerMatch((trigger) => /gmail|email|mail/i.test(`${trigger.connector_slug} ${trigger.label}`)) ?? {
          icon: "edit",
          leading: "Email arrives in",
          target: "Gmail"
        },
        tools: firstToolsByAction((action) => /gmail|email|draft|send_email/i.test(`${action.connector_slug} ${action.label}`), 1)
      };
    }
    if (/slack|message|reply/.test(lowerPrompt)) {
      return {
        name: "Reply draft",
        prompt: trimmedPrompt,
        trigger: firstTriggerMatch((trigger) => /slack|telegram|message|lark|wechat/i.test(`${trigger.connector_slug} ${trigger.label}`)) ?? automationTemplates.find((template) => template.id === "reply-drafts")?.trigger ?? null,
        tools: firstToolsByAction((action) => /slack|telegram|message|reply|send/i.test(`${action.connector_slug} ${action.label}`), 1)
      };
    }
    if (/daily|weekday|morning|digest|every/.test(lowerPrompt)) {
      return {
        name: "Morning digest",
        prompt: trimmedPrompt,
        trigger: firstTriggerMatch((trigger) => trigger.kind === "schedule") ?? everyDayTrigger,
        tools: firstToolsByAction((action) => /read|list|history|calendar|slack/i.test(`${action.connector_slug} ${action.label}`), 2)
      };
    }
    return {
      name: blankAutomationName,
      prompt: trimmedPrompt,
      trigger: null,
      tools: []
    };
  }

  function firstToolsByAction(match: (action: AutomationCatalogAction) => boolean, limit: number): SelectedAutomationTool[] {
    const selected: SelectedAutomationTool[] = [];
    for (const app of commonApps) {
      for (const capability of app.capabilities) {
        if (capability.action && match(capability.action)) {
          selected.push(selectedToolFrom(app, capability));
          if (selected.length >= limit) return selected;
        }
      }
    }
    return selected;
  }

  function openPromptAutomation(prompt: string) {
    const trimmedPrompt = prompt.trim();
    if (!trimmedPrompt) {
      openBlankAutomation();
      return;
    }
    const draft = draftFromPrompt(trimmedPrompt);
    automationName = draft.name;
    automationPrompt = draft.prompt;
    automationSource = { type: "natural_language", prompt: trimmedPrompt };
    automationRunLocation = defaultAutomationRunLocation();
    automationTrigger = copyTrigger(draft.trigger);
    selectedTools = copySelectedTools(draft.tools);
    automationEnabled = false;
    selectedAutomationId = null;
    triggerMenuOpen = false;
    toolMenuOpen = false;
    automationActionMenuOpen = false;
    editingToolId = null;
    toolSearchQuery = "";
    screenMode = "new";
  }

  function openTemplateAutomation(starter: AutomationStarter) {
    applyStarter(starter);
    screenMode = "new";
  }

  function openExistingAutomation(item: AutomationItem) {
    selectedAutomationId = item.id;
    automationName = item.title;
    automationPrompt = item.prompt ?? item.description;
    automationSource = item.record?.spec.source ?? { type: "blank" };
    automationRunLocation = item.record?.spec.run_location ?? defaultAutomationRunLocation();
    automationTrigger = copyTrigger(item.trigger ?? {
      icon: item.icon,
      leading: item.when
    });
    selectedTools = copySelectedTools(item.tools ?? []);
    automationEnabled = item.enabled ?? item.status !== "Paused";
    automationTestInputText = defaultAutomationTestInputText(item.prompt ?? item.description, item.trigger ?? null);
    activeAutomationDetailTab = "settings";
    triggerMenuOpen = false;
    toolMenuOpen = false;
    automationActionMenuOpen = false;
    editingToolId = null;
    toolSearchQuery = "";
    screenMode = "detail";
    void refreshSelectedRunHistory();
  }

  function selectTrigger(trigger: AutomationTrigger) {
    automationTrigger = copyTrigger(trigger);
    triggerMenuOpen = false;
  }

  function selectCatalogTrigger(trigger: AutomationCatalogTrigger) {
    selectTrigger(triggerFromCatalog(trigger));
  }

  function updateTriggerConfig(key: string, value: string) {
    if (!automationTrigger) return;
    const config = {
      ...(automationTrigger.config ?? {}),
      [key]: value
    };
    automationTrigger = {
      ...automationTrigger,
      config,
      target: automationTrigger.catalog ? triggerTarget(automationTrigger.catalog, config) : automationTrigger.target
    };
  }

  function removeTrigger() {
    automationTrigger = null;
    triggerMenuOpen = false;
  }

  function openTriggerEditor() {
    triggerMenuOpen = true;
  }

  function openToolPickerForAdd() {
    editingToolId = null;
    toolSearchQuery = "";
    toolMenuOpen = !toolMenuOpen;
  }

  function openToolPickerForEdit(toolId: string) {
    editingToolId = toolId;
    toolSearchQuery = "";
    toolMenuOpen = true;
  }

  function cancelCreate() {
    triggerMenuOpen = false;
    toolMenuOpen = false;
    automationActionMenuOpen = false;
    editingToolId = null;
    toolSearchQuery = "";
    selectedAutomationId = null;
    screenMode = "home";
  }

  function triggerSummary(trigger: AutomationTrigger | null): string {
    if (!trigger) return "No trigger selected.";
    return [trigger.leading, trigger.target, trigger.actorPrefix, trigger.actor].filter(Boolean).join(" ");
  }

  async function saveAutomation() {
    if (automationSaving) return;
    const title = automationName.trim() || blankAutomationName;
    const description = automationPrompt.trim() || "Ready to configure.";
    automationSaving = true;
    try {
      const record = await saveAutomationRecord({
        id: nextAutomationId(title),
        spec: automationSpecFromUi(title, description, automationSource)
      });
      upsertAutomationRecord(record);
      automationLoadError = null;
    } catch (error) {
      automationLoadError = errorMessage(error);
      automationSaving = false;
      return;
    }
    automationSaving = false;
    activeAutomationLibraryTab = "your";
    homePrompt = "";
    triggerMenuOpen = false;
    toolMenuOpen = false;
    automationActionMenuOpen = false;
    editingToolId = null;
    toolSearchQuery = "";
    selectedAutomationId = null;
    screenMode = "home";
  }

  async function saveAutomationDetail() {
    if (!selectedAutomationId) return;
    if (automationSaving) return;
    try {
      await persistSelectedAutomationDetail();
    } catch (error) {
      automationLoadError = errorMessage(error);
    }
  }

  async function persistSelectedAutomationDetail(options: { status?: AutomationStatus } = {}): Promise<AutomationRecordDto> {
    const id = selectedAutomationId;
    if (!id) throw new Error("Automation is no longer selected; refresh before saving.");
    const title = automationName.trim() || blankAutomationName;
    const description = automationPrompt.trim() || "Ready to configure.";
    const selected = selectedAutomation;
    if (!selected) {
      throw new Error("Automation is no longer loaded; refresh before saving.");
    }
    const expectedRevision = selected?.revision;
    if (expectedRevision === undefined) {
      throw new Error("Automation revision is missing; refresh before saving.");
    }
    automationSaving = true;
    try {
      const status = options.status ?? (automationEnabled ? undefined : "paused");
      const request = {
        id,
        expectedRevision,
        spec: detailSpecForSave(selected, title, description)
      };
      const record = await saveAutomationRecord(
        status === undefined
          ? request
          : {
              ...request,
              status
            }
      );
      upsertAutomationRecord(record);
      automationLoadError = null;
      automationName = title;
      automationPrompt = description;
      automationEnabled = automationIsActive(record);
      triggerMenuOpen = false;
      toolMenuOpen = false;
      automationActionMenuOpen = false;
      editingToolId = null;
      toolSearchQuery = "";
      return record;
    } finally {
      automationSaving = false;
    }
  }

  function applyAutomationRuntimeSync(sync: AutomationRuntimeSyncResult) {
    savedAutomations = savedAutomations.map((item) => {
      if (item.id !== sync.id) return item;
      const record = item.record
        ? {
            ...item.record,
            status: sync.status ?? item.record.status,
            revision: sync.revision,
            runtime: sync.runtime
          }
        : item.record;
      const active = record ? automationIsActive(record) : sync.status === "enabled" && sync.runtime.status === "deployed";
      return {
        ...item,
        status: active ? "Active" : "Paused",
        enabled: active,
        revision: sync.revision,
        record,
        recent: [`Revision ${sync.revision}`, runtimeStatusLabel(sync.runtime.status)]
      };
    });
    if (selectedAutomationId === sync.id) {
      if (sync.status !== undefined) {
        automationEnabled = sync.status === "enabled" && sync.runtime.status === "deployed";
      } else if (sync.runtime.status !== "deployed") {
        automationEnabled = false;
      }
    }
  }

  async function runTestAutomation() {
    if (!selectedAutomationId) return;
    if (automationRunning || automationSaving) return;
    const nextRunSequence = savedRunSequence + 1;
    savedRunSequence = nextRunSequence;
    const runningRun: AutomationRun = {
      id: `test-${nextRunSequence}`,
      title: "Test run",
      status: "Running",
      started: "Just now",
      duration: "-",
      summary: "Puffer is running the current configuration through daemon preview."
    };
    savedAutomations = savedAutomations.map((item) =>
      item.id === selectedAutomationId
        ? {
            ...item,
            updated: "Just now",
            recent: ["Test run started", ...item.recent.filter((entry) => entry !== "Test run started")],
            history: [runningRun, ...(item.history ?? [])]
          }
        : item
    );
    activeAutomationDetailTab = "history";
    automationRunning = true;
    triggerMenuOpen = false;
    toolMenuOpen = false;
    automationActionMenuOpen = false;
    editingToolId = null;
    toolSearchQuery = "";
    let previewRequested = false;
    const testInput = previewInput();
    try {
      const saved = await persistSelectedAutomationDetail();
      const sync = await syncAutomationPreview(saved.id, saved.revision);
      applyAutomationRuntimeSync(sync);
      previewRequested = true;
      const preview = await runAutomationPreview(saved.id, testInput);
      const run: AutomationRun = {
        id: `test-${nextRunSequence}`,
        title: "Test run",
        status: preview.status === "completed" ? "Completed" : preview.status,
        started: "Just now",
        duration: "-",
        summary: preview.summary || summarizePreviewResult(preview.result),
        compiled: preview.compiled,
        runtimeStatus: preview.runtime.status,
        input: testInput,
        result: preview.result
      };
      applyRunToSelected(run);
      automationLoadError = null;
    } catch (error) {
      const run: AutomationRun = {
        id: `test-${nextRunSequence}`,
        title: "Test run",
        status: "Error",
        started: "Just now",
        duration: "-",
        summary: errorMessage(error),
        error: errorMessage(error),
        input: testInput
      };
      applyRunToSelected(run);
      automationLoadError = errorMessage(error);
    } finally {
      automationRunning = false;
      if (previewRequested) {
        await refreshSelectedRunHistory();
      }
    }
  }

  function previewInput(): Record<string, unknown> {
    return parsePreviewInputText(automationTestInputText);
  }

  function parsePreviewInputText(value: string): Record<string, unknown> {
    const trimmed = value.trim();
    if (!trimmed) return {};
    try {
      const parsed = JSON.parse(trimmed);
      if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
        return parsed as Record<string, unknown>;
      }
      return { value: parsed };
    } catch {
      return { text: trimmed };
    }
  }

  function previewValueText(value: unknown): string {
    if (typeof value === "string") return value;
    if (value == null) return "";
    try {
      return JSON.stringify(value, null, 2);
    } catch {
      return String(value);
    }
  }

  function summarizePreviewResult(result: unknown): string {
    if (typeof result === "string") return result;
    if (result == null) return "Preview completed.";
    try {
      return JSON.stringify(result).slice(0, 220);
    } catch {
      return "Preview completed.";
    }
  }

  function applyRunToSelected(run: AutomationRun) {
    if (!selectedAutomationId) return;
    savedAutomations = savedAutomations.map((item) =>
      item.id === selectedAutomationId
        ? {
            ...item,
            history: [run, ...(item.history ?? []).filter((candidate) => candidate.id !== run.id)],
            recent: [run.status === "Error" ? "Test run failed" : "Test run completed", ...item.recent.slice(0, 2)]
          }
        : item
    );
  }

  async function refreshSelectedRunHistory() {
    if (!selectedAutomationId) return;
    try {
      const history = await loadAutomationRunHistory(selectedAutomationId);
      const runs = history.runs.map(runFromHistoryRecord);
      savedAutomations = savedAutomations.map((item) =>
        item.id === selectedAutomationId ? { ...item, history: runs } : item
      );
    } catch (error) {
      automationLoadError = errorMessage(error);
    }
  }

  function runFromHistoryRecord(record: AutomationRunHistoryRecord): AutomationRun {
    const waitingForReview = record.approval?.required && record.approval.status.includes("review");
    return {
      id: record.id,
      title: record.title,
      status: waitingForReview ? "Waiting for review" : record.status === "completed" ? "Completed" : record.status === "error" ? "Error" : record.status,
      started: formatUpdated(record.started_at_ms),
      duration: formatDuration(record.duration_ms),
      summary: record.error || record.summary,
      error: record.error,
      compiled: record.compiled,
      runtimeStatus: record.runtime_status,
      result: record.result
    };
  }

  function formatDuration(value: number): string {
    if (!Number.isFinite(value) || value <= 0) return "-";
    if (value < 1000) return `${Math.round(value)} ms`;
    return `${(value / 1000).toFixed(1)} s`;
  }

  function returnToAutomationHome() {
    triggerMenuOpen = false;
    toolMenuOpen = false;
    automationActionMenuOpen = false;
    editingToolId = null;
    toolSearchQuery = "";
    selectedAutomationId = null;
    screenMode = "home";
  }

  function selectAutomationDetailTab(tab: AutomationDetailTab) {
    activeAutomationDetailTab = tab;
    triggerMenuOpen = false;
    toolMenuOpen = false;
    automationActionMenuOpen = false;
    editingToolId = null;
    toolSearchQuery = "";
  }

  function toggleAutomationActionMenu() {
    automationActionMenuOpen = !automationActionMenuOpen;
    triggerMenuOpen = false;
    toolMenuOpen = false;
  }

  async function deleteSelectedAutomation() {
    if (!selectedAutomationId) return;
    const id = selectedAutomationId;
    try {
      await deleteAutomationRecord(id);
      automationLoadError = null;
    } catch (error) {
      automationLoadError = errorMessage(error);
      return;
    }
    savedAutomations = savedAutomations.filter((item) => item.id !== selectedAutomationId);
    activeAutomationLibraryTab = "your";
    returnToAutomationHome();
  }

  async function setSelectedAutomationActive(active: boolean) {
    if (!selectedAutomationId || automationSaving || automationRunning || automationStatusChanging) return;
    const previous = automationEnabled;
    automationEnabled = active;
    automationStatusChanging = true;
    try {
      const saved = await persistSelectedAutomationDetail({ status: "paused" });
      if (active) {
        const activated = await activateAutomationRecord(saved.id, saved.revision);
        applyAutomationRuntimeSync(activated);
      } else {
        automationEnabled = false;
      }
      automationLoadError = null;
    } catch (error) {
      automationEnabled = previous;
      automationLoadError = errorMessage(error);
    } finally {
      automationStatusChanging = false;
    }
  }

  function closeFloatingMenusFromOutside(event: MouseEvent) {
    if (!toolMenuOpen && !triggerMenuOpen) return;
    const target = event.target;
    const insideToolPicker =
      target instanceof Element &&
      (target.closest(".pf-automation-tool-menu-wrap") || target.closest(".pf-automation-tool-config-row"));
    const insideTriggerPicker =
      target instanceof Element &&
      (target.closest(".pf-automation-trigger-menu-wrap") ||
        target.closest(".pf-automation-trigger-row") ||
        target.closest('[aria-label="Edit trigger"]'));

    if (toolMenuOpen && !insideToolPicker) {
      toolMenuOpen = false;
      editingToolId = null;
      toolSearchQuery = "";
    }
    if (triggerMenuOpen && !insideTriggerPicker) {
      triggerMenuOpen = false;
    }
  }

  function replaceSelectedTool(tool: SelectedAutomationTool) {
    if (editingToolId) {
      selectedTools = selectedTools
        .map((selected) => (selected.id === editingToolId ? tool : selected))
        .filter((selected, index, tools) => tools.findIndex((candidate) => candidate.id === selected.id) === index);
      editingToolId = null;
      toolSearchQuery = "";
      toolMenuOpen = false;
      return;
    }

    const alreadySelected = selectedTools.some((selected) => selected.id === tool.id);
    selectedTools = alreadySelected
      ? selectedTools.filter((selected) => selected.id !== tool.id)
      : [...selectedTools, tool];
  }

  function removeTool(toolId: string) {
    selectedTools = selectedTools.filter((selected) => selected.id !== toolId);
    if (editingToolId === toolId) {
      editingToolId = null;
      toolMenuOpen = false;
    }
  }

  function toolSelected(toolId: string): boolean {
    return selectedTools.some((tool) => tool.id === toolId);
  }

  function toggleToolCapability(app: AutomationApp, capability: AutomationCapability) {
    replaceSelectedTool(selectedToolFrom(app, capability));
  }

  function cycleToolTarget(toolId: string) {
    selectedTools = selectedTools.map((tool) => {
      if (tool.id !== toolId || tool.targetOptions.length === 0) return tool;
      const currentIndex = Math.max(0, tool.targetOptions.findIndex((option) => option === tool.target));
      const nextTarget = tool.targetOptions[(currentIndex + 1) % tool.targetOptions.length];
      return {
        ...tool,
        target: nextTarget
      };
    });
  }

  function selectedToolLabel(tool: SelectedAutomationTool): string {
    if (!tool.targetLabel || !tool.target) return tool.title;
    return `${tool.title} ${tool.targetLabel} ${tool.target}`;
  }

  function capabilityLabel(app: AutomationApp, capability: AutomationCapability): string {
    if (!capability.targetLabel || !capability.defaultTarget) return capability.title;
    return `${capability.title} ${capability.targetLabel} ${capability.defaultTarget}`;
  }

  function toolIdFor(app: AutomationApp, capability: AutomationCapability): string {
    return `${app.id}:${capability.id}`;
  }

  function stopButtonEvent(event: MouseEvent) {
    event.stopPropagation();
  }
</script>

{#snippet runLocationSection()}
  <section class="pf-automation-builder-config">
    <div class="pf-automation-section-title-row">
      <h2>Run location</h2>
      <button
        type="button"
        class="pf-automation-runtime-settings-link"
        onclick={openAutomationRuntimeSettings}
      >
        Configure Runtime
      </button>
    </div>
    <div class="pf-automation-run-location" role="radiogroup" aria-label="Run location">
      <label>
        <input
          type="radio"
          name="automation-run-location"
          value="local"
          checked={automationRunLocation === "local"}
          onchange={() => (automationRunLocation = "local")}
        />
        <span>
          <strong>Local</strong>
          <small>Puffer starts and configures the local runtime when needed.</small>
        </span>
      </label>
      <label>
        <input
          type="radio"
          name="automation-run-location"
          value="agent_env_cloud"
          checked={automationRunLocation === "agent_env_cloud"}
          onchange={() => (automationRunLocation = "agent_env_cloud")}
        />
        <span>
          <strong>AgentEnv Cloud</strong>
          <small>Use a cloud runtime for this automation.</small>
        </span>
      </label>
    </div>
  </section>
{/snippet}

<svelte:window onclick={closeFloatingMenusFromOutside} />

<div class="pf-screen-top">
  <div class="pf-screen-top-left">
    <span class="pf-screen-top-title">Automation</span>
    <span class="pf-screen-top-sub">Set up repeated work as editable drafts.</span>
  </div>
</div>

{#if screenMode === "new"}
  <section class="pf-automation-builder-page" aria-label="New automation page">
    <header class="pf-automation-builder-page-head">
      <div>
        <nav class="pf-automation-breadcrumb" aria-label="Automation path">
          <button type="button" aria-label="Back to automations" onclick={returnToAutomationHome}>Automations</button>
          <Icon name="chevR" size={12} />
          <span>Create New</span>
        </nav>
        <h1 class="pf-automation-sr-only">New automation</h1>
      </div>
      <div class="pf-automation-builder-page-actions">
        <button type="button" class="sc-btn" data-variant="outline" data-size="sm" onclick={cancelCreate}>Cancel</button>
        <button type="button" class="sc-btn" data-variant="default" data-size="sm" disabled={automationSaving} onclick={saveAutomation}>Save</button>
      </div>
    </header>

    <div class="pf-automation-builder-page-body">
      <main class="pf-automation-builder-main">
        {#if automationLoadError}
          <div class="pf-automation-error" role="alert">{automationLoadError}</div>
        {/if}
        <section class="pf-automation-builder-field">
          <input
            id="automation-name"
            class="pf-automation-name-input"
            aria-label="Name"
            bind:value={automationName}
          />
        </section>

        <section class="pf-automation-builder-config" aria-label="Automation rule">
          <h2>Triggers</h2>
          <div class="pf-automation-trigger-panel">
            {#if automationTrigger}
              <div class="pf-automation-config-row">
                <button type="button" class="pf-automation-trigger-row" onclick={openTriggerEditor}>
                  <Icon name={automationTrigger.icon} size={13} />
                  <span>{automationTrigger.leading}</span>
                  {#if automationTrigger.target}
                    <span class="pf-automation-token">{automationTrigger.target}</span>
                  {/if}
                  {#if automationTrigger.actorPrefix}
                    <span>{automationTrigger.actorPrefix}</span>
                  {/if}
                  {#if automationTrigger.actor}
                    <span class="pf-automation-token">{automationTrigger.actor}</span>
                  {/if}
                </button>
                <span class="pf-automation-row-actions">
                  <button type="button" class="pf-automation-row-action" aria-label="Edit trigger" onclick={openTriggerEditor}>
                    <Icon name="edit" size={12} />
                  </button>
                  <button type="button" class="pf-automation-row-action" aria-label="Remove trigger" onclick={removeTrigger}>
                    <Icon name="trash" size={12} />
                  </button>
                </span>
              </div>
              {#if automationTrigger.catalog?.required_inputs?.length}
                <div class="pf-automation-trigger-fields" aria-label="Trigger configuration">
                  {#each automationTrigger.catalog.required_inputs as input (input.id)}
                    <label>
                      <span>{input.label}</span>
                      {#if input.kind === "select" && input.options?.length}
                        <select
                          value={automationTrigger.config?.[input.id] ?? ""}
                          onchange={(event) => updateTriggerConfig(input.id, event.currentTarget.value)}
                        >
                          {#each input.options as option}
                            <option value={option}>{option}</option>
                          {/each}
                        </select>
                      {:else}
                        <input
                          type={input.kind === "time" ? "time" : "text"}
                          value={automationTrigger.config?.[input.id] ?? ""}
                          placeholder={input.default == null ? "" : String(input.default)}
                          oninput={(event) => updateTriggerConfig(input.id, event.currentTarget.value)}
                        />
                      {/if}
                    </label>
                  {/each}
                </div>
              {/if}
            {/if}

            <div class="pf-automation-trigger-menu-wrap">
              <button
                type="button"
                class="pf-automation-add-row"
                onclick={() => (triggerMenuOpen = !triggerMenuOpen)}
              >
                <Icon name="plus" size={13} />
                Add Trigger
              </button>

              {#if triggerMenuOpen}
                <div class="pf-automation-trigger-menu" role="menu" aria-label="Add trigger">
                  <label>
                    <Icon name="search" size={12} />
                    <input type="search" placeholder="Search triggers..." />
                  </label>
                  {#if triggerCatalog.length}
                    <span>Triggers</span>
                    {#each triggerCatalog as trigger (trigger.id)}
                      <button type="button" role="menuitem" title={trigger.summary} onclick={() => selectCatalogTrigger(trigger)}>
                        <Icon name={iconName(trigger.icon)} size={12} />
                        {trigger.label}
                        {#if trigger.connection_state}
                          <small>{trigger.kind === "connector_event" ? "Pull request" : connectionStateLabel(trigger.connection_state)}</small>
                        {/if}
                      </button>
                    {/each}
                  {:else}
                    <span>Scheduled</span>
                    <button type="button" role="menuitem" onclick={() => selectTrigger(everyDayTrigger)}>
                      <Icon name="clock" size={12} />
                      Every...
                      <Icon name="chevR" size={11} />
                    </button>
                    <button type="button" role="menuitem" onclick={() => selectTrigger(customScheduleTrigger)}><Icon name="clock" size={12} /> Custom (cron)</button>
                    <span>GitHub / GitLab</span>
                    <button type="button" role="menuitem" onclick={() => selectTrigger(draftOpenedTrigger)}><Icon name="git" size={12} /> Draft opened</button>
                    <button type="button" role="menuitem" onclick={() => selectTrigger(prOpenedTrigger)}>
                      <Icon name="git" size={12} />
                      Pull request...
                      <Icon name="chevR" size={11} />
                    </button>
                    <button type="button" role="menuitem" onclick={() => selectTrigger(commentAddedTrigger)}><Icon name="git" size={12} /> Comment added</button>
                    <button type="button" role="menuitem" onclick={() => selectTrigger(labelChangeTrigger)}><Icon name="git" size={12} /> Label change</button>
                  {/if}
                </div>
              {/if}
            </div>
          </div>
        </section>

        <section class="pf-automation-builder-prompt">
          <h2>Instructions</h2>
          <div class="pf-automation-instructions-box">
            <textarea
              id="automation-prompt"
              aria-label="Instructions"
              rows="5"
              bind:value={automationPrompt}
              placeholder="Enter prompt text... (type @ for tools & MCPs, / for skills and commands)"
            ></textarea>
            <button type="button" class="pf-automation-model-row">
              Codex 5.3 High
              <Icon name="chevD" size={12} />
            </button>
          </div>
          <p class="pf-automation-warning">Some tools might not be configured yet</p>
        </section>

        <section class="pf-automation-builder-config">
          <h2>Tools</h2>
          <div class="pf-automation-stack-panel">
            <div class="pf-automation-config-row">
              <button type="button" class="pf-automation-tool-row" aria-label="Memories tool">
                <span class="pf-automation-tool-main"><Icon name="logs" size={13} /> Memories</span>
                <span class="pf-automation-tool-capabilities">
                  <span class="pf-automation-token">Read context</span>
                </span>
              </button>
            </div>
            {#each selectedTools as tool (tool.id)}
              <div class="pf-automation-config-row pf-automation-tool-config-row">
                <button
                  type="button"
                  class="pf-automation-tool-row"
                  aria-label={`${tool.title} tool`}
                  title={selectedToolLabel(tool)}
                  onclick={() => openToolPickerForEdit(tool.id)}
                >
                  <span class="pf-automation-tool-main"><Icon name={tool.icon} size={13} /> {tool.title}</span>
                </button>
                {#if tool.targetLabel && tool.target}
                  <span class="pf-automation-tool-target">
                    <span>{tool.targetLabel}</span>
                    <button
                      type="button"
                      class="pf-automation-target-chip"
                      aria-label={`${tool.title} target`}
                      onclick={(event) => {
                        stopButtonEvent(event);
                        cycleToolTarget(tool.id);
                      }}
                    >
                      {tool.target}
                      <Icon name="chevD" size={10} />
                    </button>
                  </span>
                {/if}
                {#if tool.action}
                  <span class="pf-automation-tool-status">
                    <span>{connectionStateLabel(tool.action.connection_state ?? "ready")}</span>
                    <span>{tool.action.external_side_effect ? "Approval required" : (tool.action.permission_state ?? "Ready")}</span>
                  </span>
                {/if}
                <span class="pf-automation-row-actions">
                  <button type="button" class="pf-automation-row-action" aria-label={`Edit ${tool.title} tool`} onclick={() => openToolPickerForEdit(tool.id)}>
                    <Icon name="edit" size={12} />
                  </button>
                  <button type="button" class="pf-automation-row-action" aria-label={`Remove ${tool.title} tool`} onclick={() => removeTool(tool.id)}>
                    <Icon name="trash" size={12} />
                  </button>
                </span>
              </div>
            {/each}
            <div class="pf-automation-tool-menu-wrap">
              <button
                type="button"
                class="pf-automation-add-row"
                aria-expanded={toolMenuOpen}
                aria-haspopup="menu"
                onclick={openToolPickerForAdd}
              >
                <Icon name="plus" size={13} />
                Add Tool or MCP
              </button>
              {#if toolMenuOpen}
                <div class="pf-automation-app-menu" role="menu" aria-label="Common apps">
                  <label class="pf-automation-app-search">
                    <Icon name="search" size={12} />
                    <input type="search" placeholder="Search tools and APIs..." bind:value={toolSearchQuery} />
                  </label>
                  <span>Common apps</span>
                  {#each visibleToolApps as app (app.id)}
                    <div class="pf-automation-app-group" role="group" aria-label={`${app.title} API capabilities`}>
                      <div class="pf-automation-app-heading">
                        <Icon name={app.icon} size={13} />
                        <span>
                          <strong>{app.title}</strong>
                          <small>{app.description}</small>
                        </span>
                      </div>
                      <div class="pf-automation-app-capabilities">
                        {#each app.visibleCapabilities as capability}
                          <button
                            type="button"
                            role="menuitemcheckbox"
                            aria-checked={toolSelected(toolIdFor(app, capability))}
                            data-selected={toolSelected(toolIdFor(app, capability))}
                            title={capabilityLabel(app, capability)}
                            onclick={() => toggleToolCapability(app, capability)}
                          >
                            <Icon name={app.icon} size={13} />
                            <span>
                              <strong>{capability.title}</strong>
                              <small>{capability.description}</small>
                            </span>
                            {#if capability.targetLabel && capability.defaultTarget}
                              <span class="pf-automation-app-target-preview">
                                {capability.targetLabel} {capability.defaultTarget}
                              </span>
                            {/if}
                          </button>
                        {/each}
                      </div>
                    </div>
                  {:else}
                    <p class="pf-automation-app-empty">{commonApps.length ? "No matching apps." : "No catalog tools available."}</p>
                  {/each}
                </div>
              {/if}
            </div>
          </div>
        </section>

        {@render runLocationSection()}

      </main>
    </div>
  </section>
{:else if screenMode === "detail"}
  <section class="pf-automation-detail-page" aria-label="Automation detail page">
    <header class="pf-automation-detail-page-head">
      <nav class="pf-automation-breadcrumb" aria-label="Automation path">
        <button type="button" aria-label="Back to automations" onclick={returnToAutomationHome}>Automations</button>
        <Icon name="chevR" size={12} />
        <span>{automationName.trim() || blankAutomationName}</span>
      </nav>
      <div class="pf-automation-detail-actions">
        <button type="button" class="sc-btn" data-variant="outline" data-size="sm" onclick={runTestAutomation}>
          <Icon name="test" size={13} />
          <span>Test Run</span>
        </button>
        <button type="button" class="sc-btn" data-variant="default" data-size="sm" disabled={automationSaving} onclick={saveAutomationDetail}>Save</button>
        <div class="pf-automation-action-menu-wrap">
          <button
            type="button"
            class="pf-automation-icon-action"
            aria-label="More automation actions"
            aria-haspopup="menu"
            aria-expanded={automationActionMenuOpen}
            onclick={toggleAutomationActionMenu}
          >
            <Icon name="moreH" size={15} />
          </button>
          {#if automationActionMenuOpen}
            <div class="pf-automation-action-menu" role="menu" aria-label="Automation actions">
              <button type="button" role="menuitem" onclick={deleteSelectedAutomation}>
                <Icon name="trash" size={13} />
                Delete
              </button>
            </div>
          {/if}
        </div>
      </div>
    </header>

    <div class="pf-automation-detail-body">
      <main class="pf-automation-detail-main">
        {#if automationLoadError}
          <div class="pf-automation-error" role="alert">{automationLoadError}</div>
        {/if}
        <section class="pf-automation-detail-identity" aria-label="Automation identity">
          <input
            class="pf-automation-detail-name"
            aria-label="Automation name"
            bind:value={automationName}
          />
          <div class="pf-automation-detail-status">
            <label class="pf-automation-switch">
              <input
                type="checkbox"
                aria-label="Active"
                checked={automationEnabled}
                disabled={automationSaving || automationRunning || automationStatusChanging}
                onchange={(event) => {
                  const target = event.currentTarget;
                  if (target instanceof HTMLInputElement) {
                    void setSelectedAutomationActive(target.checked);
                  }
                }}
              />
              <span></span>
            </label>
            <span>{automationEnabled ? "Active" : "Paused"} | {selectedAutomation?.owner ?? "You"}</span>
          </div>
        </section>

        <div class="pf-automation-tabs pf-automation-detail-tabs" role="tablist" aria-label="Automation detail">
          <button
            id="automation-settings-tab"
            type="button"
            role="tab"
            aria-selected={activeAutomationDetailTab === "settings"}
            aria-controls="automation-settings-panel"
            tabindex={activeAutomationDetailTab === "settings" ? 0 : -1}
            onclick={() => selectAutomationDetailTab("settings")}
          >
            <span>Settings</span>
          </button>
          <button
            id="automation-history-tab"
            type="button"
            role="tab"
            aria-selected={activeAutomationDetailTab === "history"}
            aria-controls="automation-history-panel"
            tabindex={activeAutomationDetailTab === "history" ? 0 : -1}
            onclick={() => selectAutomationDetailTab("history")}
          >
            <span>Run History</span>
          </button>
        </div>

        {#if activeAutomationDetailTab === "settings"}
          <div
            id="automation-settings-panel"
            class="pf-automation-detail-settings"
            role="tabpanel"
            aria-labelledby="automation-settings-tab"
          >
            <section class="pf-automation-builder-config" aria-label="Automation rule">
              <h2>Triggers</h2>
              <div class="pf-automation-trigger-panel">
                {#if automationTrigger}
                  <div class="pf-automation-config-row">
                    <button type="button" class="pf-automation-trigger-row" onclick={openTriggerEditor}>
                      <Icon name={automationTrigger.icon} size={13} />
                      <span>{automationTrigger.leading}</span>
                      {#if automationTrigger.target}
                        <span class="pf-automation-token">{automationTrigger.target}</span>
                      {/if}
                      {#if automationTrigger.actorPrefix}
                        <span>{automationTrigger.actorPrefix}</span>
                      {/if}
                      {#if automationTrigger.actor}
                        <span class="pf-automation-token">{automationTrigger.actor}</span>
                      {/if}
                    </button>
                    <span class="pf-automation-row-actions">
                      <button type="button" class="pf-automation-row-action" aria-label="Edit trigger" onclick={openTriggerEditor}>
                        <Icon name="edit" size={12} />
                      </button>
                      <button type="button" class="pf-automation-row-action" aria-label="Remove trigger" onclick={removeTrigger}>
                        <Icon name="trash" size={12} />
                      </button>
                    </span>
                  </div>
                  {#if automationTrigger.catalog?.required_inputs?.length}
                    <div class="pf-automation-trigger-fields" aria-label="Trigger configuration">
                      {#each automationTrigger.catalog.required_inputs as input (input.id)}
                        <label>
                          <span>{input.label}</span>
                          {#if input.kind === "select" && input.options?.length}
                            <select
                              value={automationTrigger.config?.[input.id] ?? ""}
                              onchange={(event) => updateTriggerConfig(input.id, event.currentTarget.value)}
                            >
                              {#each input.options as option}
                                <option value={option}>{option}</option>
                              {/each}
                            </select>
                          {:else}
                            <input
                              type={input.kind === "time" ? "time" : "text"}
                              value={automationTrigger.config?.[input.id] ?? ""}
                              placeholder={input.default == null ? "" : String(input.default)}
                              oninput={(event) => updateTriggerConfig(input.id, event.currentTarget.value)}
                            />
                          {/if}
                        </label>
                      {/each}
                    </div>
                  {/if}
                {/if}

                <div class="pf-automation-trigger-menu-wrap">
                  <button
                    type="button"
                    class="pf-automation-add-row"
                    onclick={() => (triggerMenuOpen = !triggerMenuOpen)}
                  >
                    <Icon name="plus" size={13} />
                    Add Trigger
                  </button>

                  {#if triggerMenuOpen}
                    <div class="pf-automation-trigger-menu" role="menu" aria-label="Add trigger">
                      <label>
                        <Icon name="search" size={12} />
                        <input type="search" placeholder="Search triggers..." />
                      </label>
                      {#if triggerCatalog.length}
                        <span>Triggers</span>
                        {#each triggerCatalog as trigger (trigger.id)}
                          <button type="button" role="menuitem" title={trigger.summary} onclick={() => selectCatalogTrigger(trigger)}>
                            <Icon name={iconName(trigger.icon)} size={12} />
                            {trigger.label}
                            {#if trigger.connection_state}
                              <small>{trigger.kind === "connector_event" ? "Pull request" : connectionStateLabel(trigger.connection_state)}</small>
                            {/if}
                          </button>
                        {/each}
                      {:else}
                        <span>Scheduled</span>
                        <button type="button" role="menuitem" onclick={() => selectTrigger(everyDayTrigger)}>
                          <Icon name="clock" size={12} />
                          Every...
                          <Icon name="chevR" size={11} />
                        </button>
                        <button type="button" role="menuitem" onclick={() => selectTrigger(customScheduleTrigger)}><Icon name="clock" size={12} /> Custom (cron)</button>
                        <span>GitHub / GitLab</span>
                        <button type="button" role="menuitem" onclick={() => selectTrigger(draftOpenedTrigger)}><Icon name="git" size={12} /> Draft opened</button>
                        <button type="button" role="menuitem" onclick={() => selectTrigger(prOpenedTrigger)}>
                          <Icon name="git" size={12} />
                          Pull request...
                          <Icon name="chevR" size={11} />
                        </button>
                        <button type="button" role="menuitem" onclick={() => selectTrigger(commentAddedTrigger)}><Icon name="git" size={12} /> Comment added</button>
                        <button type="button" role="menuitem" onclick={() => selectTrigger(labelChangeTrigger)}><Icon name="git" size={12} /> Label change</button>
                      {/if}
                    </div>
                  {/if}
                </div>
              </div>
            </section>

            <section class="pf-automation-builder-prompt">
              <h2>Instructions</h2>
              <div class="pf-automation-instructions-box">
                <textarea
                  aria-label="Instructions"
                  rows="5"
                  bind:value={automationPrompt}
                  placeholder="Enter prompt text... (type @ for tools & MCPs, / for skills and commands)"
                ></textarea>
                <button type="button" class="pf-automation-model-row">
                  Codex 5.3 High
                  <Icon name="chevD" size={12} />
                </button>
              </div>
            </section>

            {@render runLocationSection()}

            <section class="pf-automation-builder-config">
              <h2>Tools</h2>
              <div class="pf-automation-stack-panel">
                <div class="pf-automation-config-row">
                  <button type="button" class="pf-automation-tool-row" aria-label="Memories tool">
                    <span class="pf-automation-tool-main"><Icon name="logs" size={13} /> Memories</span>
                    <span class="pf-automation-tool-capabilities">
                      <span class="pf-automation-token">Read context</span>
                    </span>
                  </button>
                </div>
                {#each selectedTools as tool (tool.id)}
                  <div class="pf-automation-config-row pf-automation-tool-config-row">
                    <button
                      type="button"
                      class="pf-automation-tool-row"
                      aria-label={`${tool.title} tool`}
                      title={selectedToolLabel(tool)}
                      onclick={() => openToolPickerForEdit(tool.id)}
                    >
                      <span class="pf-automation-tool-main"><Icon name={tool.icon} size={13} /> {tool.title}</span>
                    </button>
                    {#if tool.targetLabel && tool.target}
                      <span class="pf-automation-tool-target">
                        <span>{tool.targetLabel}</span>
                        <button
                          type="button"
                          class="pf-automation-target-chip"
                          aria-label={`${tool.title} target`}
                          onclick={(event) => {
                            stopButtonEvent(event);
                            cycleToolTarget(tool.id);
                          }}
                        >
                          {tool.target}
                          <Icon name="chevD" size={10} />
                        </button>
                    </span>
                  {/if}
                    {#if tool.action}
                      <span class="pf-automation-tool-status">
                        <span>{connectionStateLabel(tool.action.connection_state ?? "ready")}</span>
                        <span>{tool.action.external_side_effect ? "Approval required" : (tool.action.permission_state ?? "Ready")}</span>
                      </span>
                    {/if}
                    <span class="pf-automation-row-actions">
                      <button type="button" class="pf-automation-row-action" aria-label={`Edit ${tool.title} tool`} onclick={() => openToolPickerForEdit(tool.id)}>
                        <Icon name="edit" size={12} />
                      </button>
                      <button type="button" class="pf-automation-row-action" aria-label={`Remove ${tool.title} tool`} onclick={() => removeTool(tool.id)}>
                        <Icon name="trash" size={12} />
                      </button>
                    </span>
                  </div>
                {/each}
                <div class="pf-automation-tool-menu-wrap">
                  <button
                    type="button"
                    class="pf-automation-add-row"
                    aria-expanded={toolMenuOpen}
                    aria-haspopup="menu"
                    onclick={openToolPickerForAdd}
                  >
                    <Icon name="plus" size={13} />
                    Add Tool or MCP
                  </button>
                  {#if toolMenuOpen}
                    <div class="pf-automation-app-menu" role="menu" aria-label="Common apps">
                      <label class="pf-automation-app-search">
                        <Icon name="search" size={12} />
                        <input type="search" placeholder="Search tools and APIs..." bind:value={toolSearchQuery} />
                      </label>
                      <span>Common apps</span>
                      {#each visibleToolApps as app (app.id)}
                        <div class="pf-automation-app-group" role="group" aria-label={`${app.title} API capabilities`}>
                          <div class="pf-automation-app-heading">
                            <Icon name={app.icon} size={13} />
                            <span>
                              <strong>{app.title}</strong>
                              <small>{app.description}</small>
                            </span>
                          </div>
                          <div class="pf-automation-app-capabilities">
                            {#each app.visibleCapabilities as capability}
                              <button
                                type="button"
                                role="menuitemcheckbox"
                                aria-checked={toolSelected(toolIdFor(app, capability))}
                                data-selected={toolSelected(toolIdFor(app, capability))}
                                title={capabilityLabel(app, capability)}
                                onclick={() => toggleToolCapability(app, capability)}
                              >
                                <Icon name={app.icon} size={13} />
                                <span>
                                  <strong>{capability.title}</strong>
                                  <small>{capability.description}</small>
                                </span>
                                {#if capability.targetLabel && capability.defaultTarget}
                                  <span class="pf-automation-app-target-preview">
                                    {capability.targetLabel} {capability.defaultTarget}
                                  </span>
                                {/if}
                              </button>
                            {/each}
                          </div>
                        </div>
                      {:else}
                        <p class="pf-automation-app-empty">{commonApps.length ? "No matching apps." : "No catalog tools available."}</p>
                      {/each}
                    </div>
                  {/if}
                </div>
              </div>
            </section>
          </div>
        {:else}
          <div
            id="automation-history-panel"
            class="pf-automation-history-panel"
            role="tabpanel"
            aria-labelledby="automation-history-tab"
          >
            <section class="pf-automation-test-panel" aria-label="Test run input">
              <div class="pf-automation-test-head">
                <h2>Test input</h2>
                <span>{automationRunning ? "Running" : "Ready"}</span>
              </div>
              <textarea
                aria-label="Test input"
                rows="7"
                spellcheck="false"
                disabled={automationRunning}
                bind:value={automationTestInputText}
              ></textarea>
            </section>

            <section class="pf-automation-result-preview" aria-label="Test run result preview">
              <div class="pf-automation-test-head">
                <h2>Result preview</h2>
                {#if selectedAutomation?.history?.[0]}
                  <span>{selectedAutomation.history[0].status}</span>
                {:else}
                  <span>Idle</span>
                {/if}
              </div>
              {#if selectedAutomation?.history?.[0]}
                {@const latestRun = selectedAutomation.history[0]}
                <div class="pf-automation-result-summary">
                  <strong>{latestRun.title}</strong>
                  <span>{latestRun.summary}</span>
                </div>
                {#if latestRun.error}
                  <pre class="pf-automation-result-error">{latestRun.error}</pre>
                {:else if latestRun.result !== undefined}
                  <pre>{previewValueText(latestRun.result)}</pre>
                {:else}
                  <pre>{latestRun.summary}</pre>
                {/if}
              {:else}
                <div class="pf-automation-result-empty">
                  <span><Icon name="test" size={14} /></span>
                  <strong>No result yet</strong>
                </div>
              {/if}
            </section>

            {#if selectedAutomation && selectedAutomation.history && selectedAutomation.history.length > 0}
              <ul class="pf-automation-history-list" aria-label="Run history">
                {#each selectedAutomation.history as run (run.id)}
                  <li>
                    <span class="pf-automation-history-icon"><Icon name="test" size={13} /></span>
                    <span class="pf-automation-history-main">
                      <strong>{run.title}</strong>
                      <small>{run.summary}</small>
                    </span>
                    <span class="pf-automation-history-status">{run.status}</span>
                    <span class="pf-automation-history-meta">{run.started}</span>
                    <span class="pf-automation-history-meta">{run.duration}</span>
                  </li>
                {/each}
              </ul>
            {:else}
              <div class="pf-automation-history-empty">
                <span><Icon name="clock" size={14} /></span>
                <strong>No runs yet</strong>
              </div>
            {/if}
          </div>
        {/if}
      </main>
    </div>
  </section>
{:else}
  <section class="pf-automation-home" aria-label="Automation home">
    <section class="pf-automation-compose" aria-labelledby="automation-compose-title">
      <div class="pf-automation-compose-copy">
        <h1 id="automation-compose-title">Create an automation</h1>
        <p>Create an automation using natural language.</p>
      </div>

      <div class="pf-composer-wrap">
        <div class="pf-composer" role="group" aria-label="Message composer">
          <input
            class="pf-attachment-input"
            type="file"
            multiple
            tabindex="-1"
            data-testid="composer-file-input"
          />
          <textarea
            bind:value={homePrompt}
            placeholder="Tell Puffer what to automate, e.g. when a PR opens, prepare a review draft..."
          ></textarea>
          <div class="pf-composer-foot">
            <div class="pf-attachment-menu">
              <button
                type="button"
                class="pf-add-content-btn"
                aria-label="Add content"
                aria-haspopup="menu"
                aria-expanded="false"
                title="Add content"
              >
                <Icon name="plus" size={15} />
              </button>
            </div>
            <div class="picker">
              <button
                type="button"
                class="trigger"
                aria-haspopup="listbox"
                aria-expanded="false"
                title="OpenAI · gpt-5.5"
              >
                <Icon name="sparkles" size={11} color="var(--muted-foreground)" />
                <span class="model">gpt-5.5</span>
                <span class="provider">OpenAI</span>
                <Icon name="chevD" size={10} color="var(--muted-foreground)" />
              </button>
            </div>
            <label class="pf-toggle-chip" title="Fast mode">
              <input type="checkbox" />
              <Icon name="bolt" size={11} />
              <span>Fast</span>
            </label>
            <label class="pf-select-chip" title="Thinking level">
              <Icon name="cpu" size={11} />
              <select aria-label="Thinking level">
                <option value="">Default</option>
              </select>
            </label>
            <label class="pf-select-chip" title="Codex permissions">
              <Icon name="shield" size={11} />
              <select aria-label="Codex permissions">
                <option value="workspace-write">Workspace</option>
              </select>
            </label>
            <span class="spacer"></span>
            <span class="pf-composer-hint">⏎ to send · ⇧⏎ for newline</span>
            <button type="button" class="pf-send-btn" onclick={() => openPromptAutomation(homePrompt)} aria-label="Send">
              <Icon name="arrowUp" size={15} />
            </button>
          </div>
        </div>
      </div>
    </section>

    <section class="pf-automation-review" aria-label="Review inbox">
      <div class="pf-automation-review-head">
        <div>
          <h2>Review inbox</h2>
          <span>{automationPendingActions.length === 1 ? "1 pending draft" : `${automationPendingActions.length} pending drafts`}</span>
        </div>
        <button type="button" class="sc-btn" data-variant="outline" data-size="sm" onclick={refreshAutomationPendingActions}>
          <Icon name="refresh" size={13} />
          <span>Refresh</span>
        </button>
      </div>

      {#if automationReviewError}
        <div class="pf-automation-error" role="alert">{automationReviewError}</div>
      {/if}

      <div class="pf-automation-review-body">
        <div class="pf-automation-review-list-wrap">
          {#if automationPendingActions.length > 0}
            <ul class="pf-automation-review-list" aria-label="Pending automation drafts">
              {#each automationPendingActions as action (action.draft_id)}
                <li>
                  <button
                    type="button"
                    class="pf-automation-review-row"
                    data-selected={selectedPendingAction?.draft_id === action.draft_id}
                    onclick={() => openPendingAction(action)}
                  >
                    <span class="pf-automation-row-icon"><Icon name="listTodo" size={14} /></span>
                    <span class="pf-automation-review-row-main">
                      <strong>{action.automation_name}</strong>
                      <small>{pendingActionConnectorLabel(action)} | {pendingActionRecipientLabel(action)}</small>
                      <em>{action.preview}</em>
                    </span>
                    <span class="pf-automation-review-row-meta">
                      <small>{pendingActionApprovalLabel(action)}</small>
                      <span>{action.status.replace(/_/g, " ")}</span>
                      <small>{pendingActionTimeLabel(action)}</small>
                    </span>
                  </button>
                </li>
              {/each}
            </ul>
          {:else}
            <div class="pf-automation-review-empty">
              <span><Icon name="listTodo" size={14} /></span>
              <strong>No pending review</strong>
            </div>
          {/if}
        </div>

        <aside class="pf-automation-review-detail" aria-label="Automation draft detail">
          {#if pendingActionLoading}
            <div class="pf-automation-review-empty">
              <span><Icon name="clock" size={14} /></span>
              <strong>Loading draft</strong>
            </div>
          {:else if selectedPendingAction}
            <div class="pf-automation-review-detail-head">
              <div>
                <strong>{selectedPendingAction.automation_name}</strong>
                <span>{pendingActionConnectorLabel(selectedPendingAction)} | {pendingActionRecipientLabel(selectedPendingAction)}</span>
              </div>
              <small>{pendingActionApprovalLabel(selectedPendingAction)}</small>
            </div>

            {#if selectedPendingAction.message_editable}
              <textarea
                aria-label="Draft message"
                rows="8"
                bind:value={pendingActionMessage}
                disabled={pendingActionSubmitting}
              ></textarea>
            {:else}
              <div class="pf-automation-action-review" role="region" aria-label="Action review">
                <div>
                  <span>Action</span>
                  <strong>{pendingActionConnectorLabel(selectedPendingAction)}</strong>
                </div>
                <div>
                  <span>Destination</span>
                  <strong>{pendingActionRecipientLabel(selectedPendingAction)}</strong>
                </div>
                {#if selectedPendingAction.message_field}
                  <textarea
                    aria-label="Draft body"
                    rows="6"
                    bind:value={pendingActionMessage}
                    disabled={pendingActionSubmitting}
                  ></textarea>
                {:else if selectedPendingAction.message}
                  <p>{selectedPendingAction.message}</p>
                {/if}
                {#if pendingActionDestinationEntries(selectedPendingAction).length > 0}
                  <dl>
                    {#each pendingActionDestinationEntries(selectedPendingAction) as item}
                      <div>
                        <dt>{item.key}</dt>
                        <dd>{item.value}</dd>
                      </div>
                    {/each}
                  </dl>
                {/if}
              </div>
            {/if}

            <label class="pf-automation-review-reason">
              <span>Rejection reason</span>
              <input
                aria-label="Rejection reason"
                bind:value={pendingActionRejectReason}
                disabled={pendingActionSubmitting}
              />
            </label>

            <div class="pf-automation-review-actions">
              <button
                type="button"
                class="sc-btn"
                data-variant="default"
                data-size="sm"
                disabled={pendingActionSubmitting}
                onclick={approvePendingAction}
              >
                Approve
              </button>
              <button
                type="button"
                class="sc-btn"
                data-variant="outline"
                data-size="sm"
                disabled={pendingActionSubmitting}
                onclick={rejectPendingAction}
              >
                Reject
              </button>
              <button
                type="button"
                class="sc-btn"
                data-variant="ghost"
                data-size="sm"
                disabled={pendingActionSubmitting}
                onclick={snoozePendingAction}
              >
                Snooze
              </button>
            </div>
          {:else}
            <div class="pf-automation-review-empty">
              <span><Icon name="edit" size={14} /></span>
              <strong>Select a draft</strong>
            </div>
          {/if}
        </aside>
      </div>
    </section>

    <section class="pf-automations-section" aria-label="Automation library">
      {#if automationLoadError}
        <div class="pf-automation-error" role="alert">{automationLoadError}</div>
      {/if}
      {#if automationCatalogError}
        <div class="pf-automation-error" role="status">{automationCatalogError}</div>
      {/if}
      <div class="pf-automation-library-toolbar">
        <div class="pf-automation-tabs" role="tablist" aria-label="Automation library">
          <button
            id="your-automations-tab"
            type="button"
            role="tab"
            aria-selected={activeAutomationLibraryTab === "your"}
            aria-controls="your-automations-panel"
            tabindex={activeAutomationLibraryTab === "your" ? 0 : -1}
            onclick={() => (activeAutomationLibraryTab = "your")}
          >
            <span>Your automations</span>
            <small>{userAutomations.length}</small>
          </button>
          <button
            id="templates-tab"
            type="button"
            role="tab"
            aria-selected={activeAutomationLibraryTab === "templates"}
            aria-controls="templates-panel"
            tabindex={activeAutomationLibraryTab === "templates" ? 0 : -1}
            onclick={() => (activeAutomationLibraryTab = "templates")}
          >
            <span>Template Library</span>
            <small>{automationTemplates.length}</small>
          </button>
        </div>

        <button
          type="button"
          class="sc-btn pf-automation-new-button"
          data-variant="default"
          data-size="sm"
          onclick={() => openBlankAutomation()}
        >
          <Icon name="plus" size={13} />
          <span>new</span>
        </button>
      </div>

      <div class="pf-automation-library">
        {#if activeAutomationLibraryTab === "your"}
          <div
            id="your-automations-panel"
            class="pf-automation-group"
            role="tabpanel"
            aria-labelledby="your-automations-tab"
          >
            {#if userAutomations.length > 0}
              <ul class="pf-automation-grid" aria-label="Your automations">
                {#each userAutomations as item (item.id)}
                  <li>
                    <button type="button" class="pf-automation-card" onclick={() => openExistingAutomation(item)}>
                      <span class="pf-automation-row-icon"><Icon name={item.icon} size={14} /></span>
                      <span class="pf-automation-card-main">
                        <strong>{item.title}</strong>
                        <small>{item.description}</small>
                      </span>
                      <span class="pf-automation-card-meta">{item.status}</span>
                    </button>
                  </li>
                {/each}
              </ul>
            {:else}
              <div class="pf-automation-empty" aria-label="Your automations empty state">
                <span class="pf-automation-empty-icon"><Icon name="bolt" size={16} /></span>
                <strong>No automations yet</strong>
                <p>Create your first automation to handle repetitive workflows</p>
                <button type="button" class="sc-btn" data-variant="outline" data-size="sm" onclick={() => openBlankAutomation()}>
                  <Icon name="plus" size={13} />
                  <span>create automation</span>
                </button>
              </div>
            {/if}
          </div>
        {:else}
          <div
            id="templates-panel"
            class="pf-automation-group"
            role="tabpanel"
            aria-labelledby="templates-tab"
          >
            <ul class="pf-automation-grid" aria-label="Template Library">
              {#each automationTemplates as starter (starter.id)}
                <li>
                  <button type="button" class="pf-automation-card" onclick={() => openTemplateAutomation(starter)}>
                    <span class="pf-automation-row-icon"><Icon name={starter.icon} size={14} /></span>
                    <span class="pf-automation-card-main">
                      <strong>{starter.title}</strong>
                      <small>{starter.description}</small>
                    </span>
                    <span class="pf-automation-card-meta">Template</span>
                  </button>
                </li>
              {/each}
            </ul>
          </div>
        {/if}
      </div>
    </section>
  </section>
{/if}

<style>
  .pf-automation-home,
  .pf-automation-builder-page,
  .pf-automation-detail-page {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    padding: 16px;
    overflow: auto;
    background: var(--background);
  }

  .pf-automation-home {
    gap: 18px;
  }

  .pf-automation-compose,
  .pf-automation-review,
  .pf-automations-section,
  .pf-automation-builder-page-head {
    width: min(100%, 980px);
    margin: 0 auto;
  }

  .pf-automation-compose {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 12px;
    padding-top: 18px;
  }

  .pf-automation-compose-copy {
    text-align: center;
  }

  .pf-automation-compose h1 {
    margin: 0 0 6px;
    color: var(--foreground);
    font-size: 22px;
    letter-spacing: 0;
  }

  .pf-automation-compose p {
    margin: 0;
    color: var(--muted-foreground);
    font-size: 13px;
    line-height: 19px;
  }

  .pf-automation-name-input:focus,
  .pf-automation-detail-name:focus,
  .pf-automation-instructions-box:focus-within,
  .pf-automation-trigger-row:focus-visible,
  .pf-automation-tool-row:focus-visible,
  .pf-automation-add-row:focus-visible,
  .pf-automation-model-row:focus-visible,
  .pf-automation-card:focus-visible,
  .pf-automation-review-row:focus-visible,
  .pf-automation-icon-action:focus-visible {
    border-color: color-mix(in oklab, var(--puffer-accent) 55%, var(--border));
    box-shadow: 0 0 0 2px color-mix(in oklab, var(--puffer-accent) 14%, transparent);
  }

  .pf-composer-wrap {
    width: min(100%, 980px);
    border-top: 0;
    background: transparent;
    padding: 0;
    margin-bottom: 14px;
    flex-shrink: 0;
  }

  .pf-composer {
    max-width: 820px;
    margin: 0 auto;
    position: relative;
  }

  .pf-composer textarea {
    overflow-y: hidden;
  }

  .pf-attachment-input {
    display: none;
  }

  .pf-composer-foot .picker {
    min-width: 0;
  }

  .pf-composer-foot .trigger {
    height: 28px;
    max-width: 220px;
    background: var(--background);
  }

  .picker {
    position: relative;
    display: inline-block;
    flex-shrink: 0;
  }

  .trigger {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 4px 8px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--background);
    color: var(--foreground);
    cursor: pointer;
    font: inherit;
    font-size: 11.5px;
    line-height: 1.2;
    max-width: 240px;
    transition: background 120ms, border-color 120ms;
  }

  .trigger:hover {
    background: color-mix(in oklab, var(--background) 92%, var(--muted));
    border-color: color-mix(in oklab, var(--accent) 35%, var(--border));
  }

  .trigger .model {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-family: var(--font-mono);
    font-weight: 500;
  }

  .trigger .provider {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    border-left: 1px solid var(--border);
    padding-left: 6px;
    color: var(--muted-foreground);
    font-size: 10.5px;
  }

  .pf-toggle-chip,
  .pf-add-content-btn,
  .pf-select-chip {
    height: 28px;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 0 8px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--background);
    color: var(--muted-foreground);
    font-size: 11.5px;
    line-height: 1;
    white-space: nowrap;
  }

  .pf-attachment-menu {
    position: relative;
    flex: 0 0 auto;
  }

  .pf-add-content-btn {
    width: 28px;
    justify-content: center;
    padding: 0;
    cursor: pointer;
  }

  .pf-add-content-btn:hover {
    color: var(--foreground);
    background: var(--accent);
  }

  .pf-toggle-chip {
    cursor: pointer;
  }

  .pf-toggle-chip input {
    width: 12px;
    height: 12px;
    margin: 0;
    accent-color: var(--accent-foreground);
  }

  .pf-toggle-chip:has(input:checked) {
    border-color: color-mix(in oklab, var(--accent-foreground) 26%, var(--border));
    background: color-mix(in oklab, var(--accent) 70%, var(--background));
    color: var(--foreground);
  }

  .pf-select-chip select {
    border: 0;
    background: transparent;
    color: var(--foreground);
    font: inherit;
    font-size: 11.5px;
    padding: 0;
    outline: none;
  }

  .pf-select-chip:focus-within {
    border-color: color-mix(in oklab, var(--accent-foreground) 30%, var(--border));
  }

  .pf-composer-hint {
    color: var(--muted-foreground);
    font-family: var(--font-sans);
    font-size: var(--pf-chat-meta-size);
  }

  .pf-automations-section {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .pf-automation-review {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .pf-automation-review-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }

  .pf-automation-review-head > div {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .pf-automation-review-head h2 {
    margin: 0;
    color: var(--foreground);
    font-size: 13px;
    line-height: 18px;
    font-weight: 650;
    letter-spacing: 0;
  }

  .pf-automation-review-head span,
  .pf-automation-review-detail-head span,
  .pf-automation-review-row-main small,
  .pf-automation-review-row-main em,
  .pf-automation-review-row-meta,
  .pf-automation-review-reason span {
    color: var(--muted-foreground);
    font-size: 11.5px;
    line-height: 16px;
  }

  .pf-automation-review-head .sc-btn {
    flex: 0 0 auto;
    gap: 6px;
  }

  .pf-automation-review-body {
    min-width: 0;
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(280px, 360px);
    gap: 10px;
  }

  .pf-automation-review-list-wrap,
  .pf-automation-review-detail {
    min-width: 0;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: color-mix(in oklab, var(--background) 98%, var(--muted));
    overflow: hidden;
  }

  .pf-automation-review-list {
    display: flex;
    flex-direction: column;
    gap: 0;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .pf-automation-review-list li + li {
    border-top: 1px solid var(--border);
  }

  .pf-automation-review-row {
    width: 100%;
    min-width: 0;
    min-height: 76px;
    display: grid;
    grid-template-columns: 30px minmax(0, 1fr) auto;
    align-items: center;
    gap: 9px;
    border: 1px solid transparent;
    background: transparent;
    color: var(--foreground);
    padding: 10px;
    text-align: left;
    cursor: pointer;
  }

  .pf-automation-review-row:hover,
  .pf-automation-review-row[data-selected="true"] {
    background: var(--pf-selected-bg-hover);
  }

  .pf-automation-review-row-main {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 3px;
  }

  .pf-automation-review-row-main strong,
  .pf-automation-review-detail-head strong {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--foreground);
    font-size: 12.5px;
    line-height: 17px;
    font-weight: 650;
  }

  .pf-automation-review-row-main small,
  .pf-automation-review-row-main em {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-style: normal;
  }

  .pf-automation-review-row-meta {
    min-width: 96px;
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    gap: 3px;
    text-align: right;
  }

  .pf-automation-review-row-meta span {
    color: var(--foreground);
    font-family: var(--font-mono);
    font-size: 10.5px;
    font-weight: 650;
  }

  .pf-automation-review-row-meta small {
    color: var(--muted-foreground);
    font-size: 11px;
    line-height: 15px;
  }

  .pf-automation-review-detail {
    min-height: 188px;
    display: flex;
    flex-direction: column;
    gap: 9px;
    padding: 10px;
  }

  .pf-automation-review-detail-head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 10px;
  }

  .pf-automation-review-detail-head > div {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .pf-automation-review-detail-head small {
    flex: 0 0 auto;
    color: var(--muted-foreground);
    font-family: var(--font-mono);
    font-size: 10.5px;
    font-weight: 650;
    line-height: 16px;
  }

  .pf-automation-review-detail textarea,
  .pf-automation-review-reason input {
    width: 100%;
    min-width: 0;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--background);
    color: var(--foreground);
    font: inherit;
    font-size: 12px;
    line-height: 17px;
    padding: 8px;
    outline: none;
  }

  .pf-automation-review-detail textarea {
    min-height: 130px;
    resize: vertical;
  }

  .pf-automation-action-review {
    min-height: 130px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--background);
    padding: 9px;
  }

  .pf-automation-action-review > div {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .pf-automation-action-review span,
  .pf-automation-action-review dt {
    color: var(--muted-foreground);
    font-size: 11px;
    line-height: 15px;
  }

  .pf-automation-action-review strong,
  .pf-automation-action-review dd,
  .pf-automation-action-review p {
    margin: 0;
    color: var(--foreground);
    font-size: 12px;
    line-height: 17px;
    overflow-wrap: anywhere;
  }

  .pf-automation-action-review dl {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: 6px;
    margin: 0;
  }

  .pf-automation-review-detail textarea:focus,
  .pf-automation-review-reason input:focus {
    border-color: color-mix(in oklab, var(--puffer-accent) 42%, var(--border));
    box-shadow: 0 0 0 2px color-mix(in oklab, var(--puffer-accent) 12%, transparent);
  }

  .pf-automation-review-reason {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .pf-automation-review-actions {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 7px;
    flex-wrap: wrap;
  }

  .pf-automation-review-empty {
    min-height: 170px;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    color: var(--muted-foreground);
    padding: 18px;
    text-align: center;
  }

  .pf-automation-review-empty span {
    width: 24px;
    height: 24px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--background);
  }

  .pf-automation-review-empty strong {
    color: var(--foreground);
    font-size: 12px;
    line-height: 17px;
    font-weight: 650;
  }

  .pf-automation-error {
    border: 1px solid color-mix(in oklab, var(--destructive) 42%, var(--border));
    border-radius: 8px;
    padding: 9px 10px;
    color: var(--destructive);
    background: color-mix(in oklab, var(--destructive) 9%, var(--background));
    font-size: 12px;
    line-height: 17px;
  }

  .pf-automation-library-toolbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }

  .pf-automation-tabs {
    min-width: 0;
    display: inline-flex;
    align-items: center;
    gap: 4px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--muted);
    padding: 3px;
  }

  .pf-automation-tabs button {
    min-width: 0;
    min-height: 30px;
    display: inline-flex;
    align-items: center;
    gap: 7px;
    border: 0;
    border-radius: 6px;
    background: transparent;
    color: var(--muted-foreground);
    font: inherit;
    font-size: 12px;
    line-height: 16px;
    padding: 5px 10px;
    cursor: pointer;
  }

  .pf-automation-tabs button:hover {
    color: var(--foreground);
    background: var(--pf-selected-bg-hover);
  }

  .pf-automation-tabs button[aria-selected="true"] {
    color: var(--foreground);
    background: var(--background);
    box-shadow: var(--shadow-xs);
  }

  .pf-automation-tabs span {
    white-space: nowrap;
  }

  .pf-automation-tabs small {
    color: var(--muted-foreground);
    font-family: var(--font-mono);
    font-size: 10.5px;
    font-weight: 650;
  }

  .pf-automation-new-button {
    flex: 0 0 auto;
    gap: 6px;
  }

  .pf-automation-library {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .pf-automation-group {
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-width: 0;
  }

  .pf-automation-card-meta {
    color: var(--muted-foreground);
    font-family: var(--font-mono);
    font-size: 11px;
    font-weight: 600;
    white-space: nowrap;
  }

  .pf-automation-grid {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 10px;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .pf-automation-empty {
    min-height: 190px;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 9px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: color-mix(in oklab, var(--background) 98%, var(--muted));
    color: var(--foreground);
    padding: 24px;
    text-align: center;
  }

  .pf-automation-empty-icon {
    width: 32px;
    height: 32px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border: 1px solid color-mix(in oklab, var(--puffer-accent) 24%, var(--border));
    border-radius: 8px;
    color: var(--puffer-accent);
    background: color-mix(in oklab, var(--puffer-accent) 8%, var(--background));
  }

  .pf-automation-empty strong {
    color: var(--foreground);
    font-size: 13px;
    font-weight: 650;
  }

  .pf-automation-empty p {
    max-width: 300px;
    margin: -2px 0 3px;
    color: var(--muted-foreground);
    font-size: 12px;
    line-height: 17px;
  }

  .pf-automation-empty .sc-btn {
    gap: 6px;
  }

  .pf-automation-card {
    width: 100%;
    min-height: 104px;
    display: grid;
    grid-template-columns: 30px minmax(0, 1fr);
    grid-template-rows: auto 1fr;
    gap: 5px 9px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: color-mix(in oklab, var(--background) 98%, var(--muted));
    color: var(--foreground);
    padding: 12px;
    text-align: left;
    cursor: pointer;
  }

  .pf-automation-card:hover {
    border-color: color-mix(in oklab, var(--puffer-accent) 28%, var(--border));
    background: var(--pf-selected-bg-hover);
  }

  .pf-automation-row-icon {
    width: 30px;
    height: 30px;
    display: flex;
    align-items: center;
    justify-content: center;
    border: 1px solid color-mix(in oklab, var(--puffer-accent) 24%, var(--border));
    border-radius: 7px;
    color: var(--puffer-accent);
    background: color-mix(in oklab, var(--puffer-accent) 8%, var(--background));
    flex-shrink: 0;
  }

  .pf-automation-card .pf-automation-row-icon {
    grid-row: 1 / span 2;
  }

  .pf-automation-card-main {
    min-width: 0;
  }

  .pf-automation-card-main {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .pf-automation-card-main strong {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--foreground);
    font-size: 13px;
    font-weight: 650;
  }

  .pf-automation-card-main small {
    min-width: 0;
    color: var(--muted-foreground);
    font-size: 12px;
    line-height: 17px;
  }

  .pf-automation-card-meta {
    grid-column: 2;
  }

  .pf-automation-sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    padding: 0;
    margin: -1px;
    overflow: hidden;
    clip: rect(0, 0, 0, 0);
    white-space: nowrap;
    border: 0;
  }

  .pf-automation-builder-page,
  .pf-automation-detail-page {
    gap: 14px;
    padding-top: 10px;
  }

  .pf-automation-builder-page-head {
    width: min(100%, 760px);
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    flex-shrink: 0;
  }

  .pf-automation-breadcrumb {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    min-height: 28px;
    color: var(--muted-foreground);
    font-size: 12px;
    line-height: 18px;
  }

  .pf-automation-breadcrumb button {
    border: 0;
    background: transparent;
    color: var(--foreground);
    font: inherit;
    padding: 0;
    cursor: pointer;
  }

  .pf-automation-breadcrumb button:hover {
    color: var(--puffer-accent);
  }

  .pf-automation-builder-page-actions {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-shrink: 0;
  }

  .pf-automation-builder-page-body {
    width: min(100%, 760px);
    margin: 0 auto;
  }

  .pf-automation-detail-page-head,
  .pf-automation-detail-body {
    width: min(100%, 820px);
    margin: 0 auto;
  }

  .pf-automation-detail-page-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    flex-shrink: 0;
  }

  .pf-automation-detail-actions {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-shrink: 0;
  }

  .pf-automation-detail-actions .sc-btn {
    gap: 6px;
  }

  .pf-automation-action-menu-wrap {
    position: relative;
    display: inline-flex;
  }

  .pf-automation-icon-action {
    width: 30px;
    height: 30px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--background);
    color: var(--muted-foreground);
    padding: 0;
    cursor: pointer;
  }

  .pf-automation-icon-action:hover {
    background: var(--pf-selected-bg-hover);
    color: var(--foreground);
  }

  .pf-automation-action-menu {
    position: absolute;
    right: 0;
    top: calc(100% + 5px);
    z-index: 20;
    min-width: 138px;
    display: flex;
    flex-direction: column;
    border: 1px solid var(--border);
    border-radius: 7px;
    background: var(--popover, var(--background));
    box-shadow: var(--shadow-sm);
    padding: 5px;
  }

  .pf-automation-action-menu button {
    min-height: 30px;
    display: inline-flex;
    align-items: center;
    gap: 8px;
    border: 0;
    border-radius: 5px;
    background: transparent;
    color: var(--foreground);
    font: inherit;
    font-size: 12px;
    line-height: 17px;
    padding: 6px 8px;
    text-align: left;
    cursor: pointer;
  }

  .pf-automation-action-menu button:hover,
  .pf-automation-action-menu button:focus-visible {
    background: var(--pf-selected-bg-hover);
    color: var(--pf-run-failed, var(--foreground));
    outline: none;
  }

  .pf-automation-builder-main {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 20px;
  }

  .pf-automation-detail-main {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .pf-automation-detail-identity {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding-top: 2px;
  }

  .pf-automation-detail-name {
    width: 100%;
    min-width: 0;
    border: 1px solid transparent;
    border-radius: 5px;
    background: transparent;
    color: var(--foreground);
    font: inherit;
    font-size: 22px;
    font-weight: 650;
    line-height: 30px;
    letter-spacing: 0;
    padding: 2px 4px;
    outline: none;
  }

  .pf-automation-detail-name:hover {
    border-color: var(--border);
    background: color-mix(in oklab, var(--background) 98%, var(--muted));
  }

  .pf-automation-detail-status {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    color: var(--muted-foreground);
    font-size: 12px;
    line-height: 17px;
  }

  .pf-automation-detail-tabs {
    width: fit-content;
  }

  .pf-automation-detail-settings {
    display: flex;
    flex-direction: column;
    gap: 18px;
  }

  .pf-automation-builder-config {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .pf-automation-builder-prompt {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .pf-automation-builder-config h2,
  .pf-automation-builder-prompt h2 {
    margin: 0;
    color: var(--foreground);
    font-size: 12px;
    font-weight: 550;
    letter-spacing: 0;
  }

  .pf-automation-section-title-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
  }

  .pf-automation-runtime-settings-link {
    border: 0;
    background: transparent;
    color: var(--muted-foreground);
    font: inherit;
    font-size: 11px;
    line-height: 15px;
    padding: 0;
    cursor: pointer;
  }

  .pf-automation-runtime-settings-link:hover {
    color: var(--foreground);
    text-decoration: underline;
  }

  .pf-automation-name-input {
    width: 100%;
    height: 32px;
    border: 1px solid var(--border);
    border-radius: 3px;
    background: var(--background);
    color: var(--foreground);
    font: inherit;
    font-size: 15px;
    line-height: 20px;
    padding: 4px 8px;
    outline: none;
  }

  .pf-automation-trigger-panel,
  .pf-automation-stack-panel,
  .pf-automation-run-location,
  .pf-automation-instructions-box {
    border: 1px solid var(--border);
    border-radius: 5px;
    background: color-mix(in oklab, var(--background) 98%, var(--muted));
  }

  .pf-automation-trigger-panel,
  .pf-automation-stack-panel {
    display: flex;
    flex-direction: column;
  }

  .pf-automation-config-row {
    width: 100%;
    min-width: 0;
    display: flex;
    align-items: stretch;
  }

  .pf-automation-trigger-row,
  .pf-automation-add-row,
  .pf-automation-tool-row {
    width: 100%;
    min-height: 32px;
    display: flex;
    align-items: center;
    gap: 7px;
    border: 0;
    background: transparent;
    color: var(--foreground);
    font: inherit;
    font-size: 12px;
    line-height: 17px;
    padding: 7px 10px;
    text-align: left;
  }

  .pf-automation-config-row > .pf-automation-trigger-row,
  .pf-automation-config-row > .pf-automation-tool-row {
    flex: 1 1 auto;
    width: auto;
    min-width: 0;
  }

  .pf-automation-trigger-row,
  .pf-automation-tool-row,
  .pf-automation-add-row {
    cursor: pointer;
  }

  .pf-automation-add-row {
    color: var(--muted-foreground);
  }

  .pf-automation-trigger-row:hover,
  .pf-automation-tool-row:hover,
  .pf-automation-add-row:hover {
    background: var(--pf-selected-bg-hover);
    color: var(--foreground);
  }

  .pf-automation-token {
    display: inline-flex;
    align-items: center;
    min-height: 20px;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--background);
    color: var(--foreground);
    padding: 1px 6px;
    font-size: 11px;
  }

  .pf-automation-trigger-menu-wrap {
    position: relative;
    border-top: 1px solid var(--border);
  }

  .pf-automation-trigger-menu {
    position: absolute;
    left: 8px;
    top: calc(100% + 4px);
    z-index: 20;
    width: 236px;
    display: flex;
    flex-direction: column;
    gap: 2px;
    border: 1px solid var(--border);
    border-radius: 7px;
    background: var(--popover, var(--background));
    box-shadow: var(--shadow-sm);
    padding: 6px;
  }

  .pf-automation-trigger-menu label {
    height: 28px;
    display: flex;
    align-items: center;
    gap: 7px;
    border: 1px solid var(--border);
    border-radius: 5px;
    background: color-mix(in oklab, var(--background) 96%, var(--muted));
    color: var(--muted-foreground);
    padding: 0 7px;
  }

  .pf-automation-trigger-menu input {
    min-width: 0;
    width: 100%;
    border: 0;
    outline: 0;
    background: transparent;
    color: var(--foreground);
    font: inherit;
    font-size: 12px;
  }

  .pf-automation-trigger-menu > span {
    margin: 7px 4px 3px;
    color: var(--muted-foreground);
    font-size: 10px;
    font-weight: 650;
  }

  .pf-automation-trigger-menu button {
    min-height: 27px;
    display: grid;
    grid-template-columns: 16px minmax(0, 1fr) auto;
    align-items: center;
    gap: 7px;
    border: 0;
    border-radius: 5px;
    background: transparent;
    color: var(--foreground);
    font: inherit;
    font-size: 12px;
    line-height: 16px;
    padding: 4px 6px;
    text-align: left;
    cursor: pointer;
  }

  .pf-automation-trigger-menu button:hover {
    background: var(--pf-selected-bg-hover);
  }

  .pf-automation-trigger-fields {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
    gap: 8px;
    border-top: 1px solid var(--border);
    padding: 8px 10px 10px;
  }

  .pf-automation-trigger-fields label {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
    color: var(--muted-foreground);
    font-size: 11px;
    line-height: 15px;
  }

  .pf-automation-trigger-fields input,
  .pf-automation-trigger-fields select {
    min-width: 0;
    height: 28px;
    border: 1px solid var(--border);
    border-radius: 5px;
    background: var(--background);
    color: var(--foreground);
    font: inherit;
    font-size: 12px;
    padding: 0 7px;
    outline: none;
  }

  .pf-automation-instructions-box {
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .pf-automation-instructions-box textarea {
    width: 100%;
    min-height: 118px;
    resize: vertical;
    border: 0;
    outline: 0;
    background: transparent;
    color: var(--foreground);
    font: inherit;
    font-size: 12px;
    line-height: 18px;
    padding: 11px 10px;
  }

  .pf-automation-model-row {
    width: 100%;
    min-height: 28px;
    display: inline-flex;
    align-items: center;
    gap: 5px;
    border: 0;
    border-top: 1px solid var(--border);
    background: transparent;
    color: var(--muted-foreground);
    font: inherit;
    font-size: 12px;
    line-height: 18px;
    padding: 5px 10px;
    cursor: pointer;
  }

  .pf-automation-model-row:hover {
    color: var(--foreground);
  }

  .pf-automation-warning {
    margin: 0;
    color: oklch(0.62 0.12 75);
    font-size: 11px;
    line-height: 16px;
  }

  .pf-automation-tool-row {
    justify-content: flex-start;
    gap: 7px;
  }

  .pf-automation-stack-panel .pf-automation-config-row + .pf-automation-config-row,
  .pf-automation-tool-menu-wrap {
    border-top: 1px solid var(--border);
  }

  .pf-automation-tool-menu-wrap {
    position: relative;
  }

  .pf-automation-tool-menu-wrap .pf-automation-add-row {
    border-top: 0;
  }

  .pf-automation-tool-main {
    min-width: 0;
    flex: 0 0 auto;
    display: inline-flex;
    align-items: center;
    gap: 7px;
  }

  .pf-automation-tool-config-row {
    position: relative;
    align-items: center;
  }

  .pf-automation-tool-config-row > .pf-automation-tool-row {
    flex: 0 1 auto;
    width: auto;
    padding-right: 6px;
  }

  .pf-automation-row-actions {
    flex: 0 0 auto;
    display: inline-flex;
    align-items: center;
    gap: 2px;
    margin-left: auto;
    padding: 4px 7px 4px 0;
  }

  .pf-automation-row-action {
    width: 24px;
    height: 24px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border: 0;
    border-radius: 5px;
    background: transparent;
    color: var(--muted-foreground);
    padding: 0;
    cursor: pointer;
  }

  .pf-automation-tool-target {
    flex: 0 0 auto;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    color: var(--foreground);
    font-size: 12px;
    line-height: 17px;
  }

  .pf-automation-tool-status {
    flex: 0 1 auto;
    min-width: 0;
    display: inline-flex;
    align-items: center;
    gap: 5px;
    color: var(--muted-foreground);
    font-size: 11px;
    line-height: 16px;
  }

  .pf-automation-tool-status span {
    min-width: 0;
    max-width: 140px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--background);
    padding: 1px 5px;
  }

  .pf-automation-target-chip {
    height: 24px;
    display: inline-flex;
    align-items: center;
    gap: 4px;
    border: 0;
    border-radius: 6px;
    background: color-mix(in oklab, var(--muted) 58%, var(--background));
    color: var(--foreground);
    font: inherit;
    font-size: 11px;
    line-height: 16px;
    padding: 0 8px;
    cursor: pointer;
  }

  .pf-automation-target-chip:hover,
  .pf-automation-target-chip:focus-visible {
    background: var(--pf-selected-bg-hover);
  }

  .pf-automation-row-action:hover,
  .pf-automation-row-action:focus-visible {
    background: var(--pf-selected-bg-hover);
    color: var(--foreground);
  }

  .pf-automation-app-menu {
    position: absolute;
    left: 8px;
    top: calc(100% + 4px);
    z-index: 20;
    width: min(420px, calc(100vw - 48px));
    max-height: min(560px, calc(100vh - 220px));
    display: flex;
    flex-direction: column;
    gap: 4px;
    border: 1px solid var(--border);
    border-radius: 7px;
    background: var(--popover, var(--background));
    box-shadow: var(--shadow-sm);
    padding: 6px;
    overflow-y: auto;
  }

  .pf-automation-app-search {
    height: 28px;
    display: flex;
    align-items: center;
    gap: 7px;
    border: 1px solid var(--border);
    border-radius: 5px;
    background: color-mix(in oklab, var(--background) 96%, var(--muted));
    color: var(--muted-foreground);
    padding: 0 7px;
  }

  .pf-automation-app-search input {
    min-width: 0;
    width: 100%;
    border: 0;
    outline: 0;
    background: transparent;
    color: var(--foreground);
    font: inherit;
    font-size: 12px;
  }

  .pf-automation-app-menu > span {
    margin: 7px 4px 3px;
    color: var(--muted-foreground);
    font-size: 10px;
    font-weight: 650;
  }

  .pf-automation-app-group {
    display: flex;
    flex-direction: column;
    gap: 3px;
    padding-bottom: 4px;
  }

  .pf-automation-app-group + .pf-automation-app-group {
    border-top: 1px solid var(--border);
    padding-top: 5px;
  }

  .pf-automation-app-heading,
  .pf-automation-app-menu button {
    min-height: 44px;
    display: grid;
    grid-template-columns: 20px minmax(0, 1fr);
    align-items: center;
    gap: 8px;
    border: 0;
    border-radius: 5px;
    background: transparent;
    color: var(--foreground);
    font: inherit;
    padding: 6px;
    text-align: left;
  }

  .pf-automation-app-heading {
    color: var(--muted-foreground);
    padding: 5px 6px 2px;
  }

  .pf-automation-app-menu button {
    cursor: pointer;
  }

  .pf-automation-app-menu button:hover,
  .pf-automation-app-menu button[data-selected="true"] {
    background: var(--pf-selected-bg-hover);
  }

  .pf-automation-app-heading > span,
  .pf-automation-app-menu button > span {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .pf-automation-app-menu strong {
    color: var(--foreground);
    font-size: 12px;
    line-height: 16px;
    font-weight: 600;
  }

  .pf-automation-app-menu small {
    min-width: 0;
    color: var(--muted-foreground);
    font-size: 11px;
    line-height: 15px;
  }

  .pf-automation-app-capabilities {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 0 0 2px 28px;
  }

  .pf-automation-app-capabilities button {
    min-height: 36px;
    grid-template-columns: 18px minmax(0, 1fr) auto;
    padding: 5px 6px;
  }

  .pf-automation-app-capabilities button[data-selected="true"] {
    box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--puffer-accent) 34%, transparent);
  }

  .pf-automation-app-target-preview {
    min-width: 0;
    max-width: 140px;
    display: inline-flex;
    align-items: center;
    border-radius: 6px;
    background: color-mix(in oklab, var(--muted) 56%, var(--background));
    color: var(--muted-foreground);
    font-size: 10.5px;
    line-height: 15px;
    padding: 2px 6px;
  }

  .pf-automation-app-capabilities span,
  .pf-automation-app-target-preview {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .pf-automation-app-empty {
    margin: 4px;
    color: var(--muted-foreground);
    font-size: 12px;
    line-height: 17px;
  }

  .pf-automation-run-location {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 0;
  }

  .pf-automation-run-location label {
    min-width: 0;
    display: flex;
    gap: 8px;
    padding: 10px;
    cursor: pointer;
  }

  .pf-automation-run-location label + label {
    border-left: 1px solid var(--border);
  }

  .pf-automation-run-location input {
    margin-top: 2px;
  }

  .pf-automation-run-location strong {
    display: block;
    color: var(--foreground);
    font-size: 12px;
    line-height: 17px;
    font-weight: 600;
  }

  .pf-automation-run-location small {
    display: block;
    margin: 1px 0 0;
    color: var(--muted-foreground);
    font-size: 11px;
    line-height: 15px;
  }

  .pf-automation-history-panel {
    min-height: 220px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .pf-automation-test-panel,
  .pf-automation-result-preview {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 9px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: color-mix(in oklab, var(--background) 98%, var(--muted));
    padding: 10px;
  }

  .pf-automation-test-head {
    min-width: 0;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
  }

  .pf-automation-test-head h2 {
    margin: 0;
    color: var(--foreground);
    font-size: 12px;
    line-height: 17px;
    font-weight: 650;
  }

  .pf-automation-test-head span {
    color: var(--muted-foreground);
    font-size: 11px;
    line-height: 15px;
    white-space: nowrap;
  }

  .pf-automation-test-panel textarea {
    width: 100%;
    min-width: 0;
    resize: vertical;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--background);
    color: var(--foreground);
    font: 12px/17px ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    padding: 9px;
    outline: none;
  }

  .pf-automation-test-panel textarea:focus {
    border-color: color-mix(in oklab, var(--puffer-accent) 42%, var(--border));
    box-shadow: 0 0 0 2px color-mix(in oklab, var(--puffer-accent) 12%, transparent);
  }

  .pf-automation-test-panel textarea:disabled {
    opacity: 0.65;
  }

  .pf-automation-result-summary {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .pf-automation-result-summary strong {
    color: var(--foreground);
    font-size: 12px;
    line-height: 17px;
    font-weight: 650;
  }

  .pf-automation-result-summary span {
    color: var(--muted-foreground);
    font-size: 11px;
    line-height: 15px;
  }

  .pf-automation-result-preview pre {
    max-height: 220px;
    overflow: auto;
    margin: 0;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--background);
    color: var(--foreground);
    font: 11.5px/16px ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    padding: 9px;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }

  .pf-automation-result-preview .pf-automation-result-error {
    border-color: color-mix(in oklab, var(--destructive) 32%, var(--border));
    color: var(--destructive);
  }

  .pf-automation-result-empty {
    min-height: 72px;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    border: 1px dashed var(--border);
    border-radius: 6px;
    color: var(--muted-foreground);
  }

  .pf-automation-result-empty span {
    width: 24px;
    height: 24px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--background);
  }

  .pf-automation-result-empty strong {
    color: var(--foreground);
    font-size: 12px;
    line-height: 17px;
    font-weight: 650;
  }

  .pf-automation-history-list {
    display: flex;
    flex-direction: column;
    gap: 0;
    margin: 0;
    padding: 0;
    list-style: none;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: color-mix(in oklab, var(--background) 98%, var(--muted));
    overflow: hidden;
  }

  .pf-automation-history-list li {
    min-width: 0;
    display: grid;
    grid-template-columns: 26px minmax(0, 1fr) auto auto auto;
    align-items: center;
    gap: 10px;
    padding: 10px;
  }

  .pf-automation-history-list li + li {
    border-top: 1px solid var(--border);
  }

  .pf-automation-history-icon {
    width: 26px;
    height: 26px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border: 1px solid color-mix(in oklab, var(--puffer-accent) 24%, var(--border));
    border-radius: 6px;
    color: var(--puffer-accent);
    background: color-mix(in oklab, var(--puffer-accent) 8%, var(--background));
  }

  .pf-automation-history-main {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .pf-automation-history-main strong {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--foreground);
    font-size: 12px;
    line-height: 17px;
    font-weight: 650;
  }

  .pf-automation-history-main small {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--muted-foreground);
    font-size: 11px;
    line-height: 15px;
  }

  .pf-automation-history-status,
  .pf-automation-history-meta {
    color: var(--muted-foreground);
    font-size: 11px;
    line-height: 15px;
    white-space: nowrap;
  }

  .pf-automation-history-status {
    color: var(--foreground);
  }

  .pf-automation-history-empty {
    min-height: 190px;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 8px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: color-mix(in oklab, var(--background) 98%, var(--muted));
    color: var(--muted-foreground);
  }

  .pf-automation-history-empty span {
    width: 30px;
    height: 30px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--background);
  }

  .pf-automation-history-empty strong {
    color: var(--foreground);
    font-size: 12px;
    line-height: 17px;
    font-weight: 650;
  }

  .pf-automation-switch {
    position: relative;
    width: 28px;
    height: 16px;
    flex: 0 0 auto;
  }

  .pf-automation-switch input {
    position: absolute;
    inset: 0;
    opacity: 0;
    cursor: pointer;
  }

  .pf-automation-switch span {
    position: absolute;
    inset: 0;
    border-radius: 999px;
    background: var(--muted);
    border: 1px solid var(--border);
    transition: background 120ms, border-color 120ms;
  }

  .pf-automation-switch span::after {
    content: "";
    position: absolute;
    top: 2px;
    left: 2px;
    width: 10px;
    height: 10px;
    border-radius: 999px;
    background: var(--background);
    box-shadow: var(--shadow-xs);
    transition: transform 120ms;
  }

  .pf-automation-switch input:checked + span {
    border-color: color-mix(in oklab, var(--puffer-accent) 35%, var(--border));
    background: color-mix(in oklab, var(--puffer-accent) 72%, var(--background));
  }

  .pf-automation-switch input:checked + span::after {
    transform: translateX(12px);
  }

  @media (max-width: 640px) {
    .pf-automation-home,
    .pf-automation-builder-page,
    .pf-automation-detail-page {
      padding: 12px;
    }

    .pf-automation-builder-page-head,
    .pf-automation-detail-page-head {
      align-items: flex-start;
      flex-direction: column;
    }

    .pf-automation-builder-page-actions,
    .pf-automation-detail-actions {
      flex-wrap: wrap;
    }

    .pf-automation-library-toolbar {
      align-items: stretch;
      flex-direction: column;
    }

    .pf-automation-tabs {
      width: 100%;
    }

    .pf-automation-tabs button {
      flex: 1 1 0;
      justify-content: center;
    }

    .pf-automation-new-button {
      justify-content: center;
      width: 100%;
    }

    .pf-automation-review-head {
      align-items: stretch;
      flex-direction: column;
    }

    .pf-automation-review-head .sc-btn {
      justify-content: center;
      width: 100%;
    }

    .pf-automation-review-body {
      grid-template-columns: 1fr;
    }

    .pf-automation-review-row {
      grid-template-columns: 30px minmax(0, 1fr);
    }

    .pf-automation-review-row-meta {
      grid-column: 2;
      align-items: flex-start;
      text-align: left;
    }

    .pf-automation-grid {
      grid-template-columns: 1fr;
    }

    .pf-automation-run-location {
      grid-template-columns: 1fr;
      gap: 8px;
    }

    .pf-automation-run-location label + label {
      border-left: 0;
      border-top: 1px solid var(--border);
    }

    .pf-automation-history-list li {
      grid-template-columns: 26px minmax(0, 1fr);
      align-items: flex-start;
    }

    .pf-automation-history-status,
    .pf-automation-history-meta {
      grid-column: 2;
    }

  }
</style>

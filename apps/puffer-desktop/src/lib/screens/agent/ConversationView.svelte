<script lang="ts">
  import "../../design/chat.css";

  import { onDestroy, tick } from "svelte";
  import BrandLogo from "../../design/BrandLogo.svelte";
  import Icon, { type IconName } from "../../design/Icon.svelte";
  import MessageBody from "../../components/MessageBody.svelte";
  import ToolCard from "./ToolCard.svelte";
  import DiffCard from "./DiffCard.svelte";
  import Approval from "./Approval.svelte";
  import QuestionPrompt from "./QuestionPrompt.svelte";
  import ModelPicker from "./ModelPicker.svelte";
  import type {
    PermissionTimelineItem,
    SessionListItem,
    SettingsSnapshot,
    TimelineItem,
    ToolTimelineItem,
    DiffTimelineItem,
    MessageTimelineItem,
    UserQuestionTimelineItem
  } from "../../types";
  import type { AgentState } from "../../shell/tweaks";
  import {
    listProviderModels,
    type AgentPermissionMode,
    type AgentTurnOptions,
    type ModelDescriptorInfo
  } from "../../api/desktop";
  import {
    canonicalDaemonProviderId,
    providerIdCanRunAgent,
    providerIdInSet,
    providerIsAvailableForAgent,
    providerIdsEquivalent
  } from "../../providerIds";

  const ENGINEER_NAME = "Engineer";
  type SubmitMessageResult = boolean | void | Promise<boolean | void>;
  type ComposerRoutingPreference = {
    providerId: string | null;
    modelId: string | null;
  };

  type Props = {
    session: SessionListItem | null;
    agentState?: AgentState;
    userDisplayName?: string;
    timeline: TimelineItem[];
    pendingPermissions: PermissionTimelineItem[];
    pendingQuestions: UserQuestionTimelineItem[];
    resolvingPermissionIds?: string[];
    resolvingQuestionIds?: string[];
    loading: boolean;
    /** True while an agent turn is running on the current session. Flips
     *  the composer's send button into a red "Stop" so the user can
     *  interrupt a runaway loop. */
    turnRunning?: boolean;
    /** True once the daemon has returned the turn id required for cancel. */
    turnCancelable?: boolean;
    turnStartedAtMs?: number | null;
    turnThinking?: boolean;
    turnStatusHint?: string | null;
    settingsSnapshot?: SettingsSnapshot | null;
    backendConnected?: boolean;
    onSubmitMessage: (message: string, options?: AgentTurnOptions) => SubmitMessageResult;
    onResolvePermission: (permissionId: string, choice: string) => void;
    onResolveUserQuestion: (
      questionId: string,
      answers: Record<string, string | string[]>,
      annotations?: Record<string, Record<string, string>>
    ) => void;
    onCancelTurn?: () => void;
    onOpenFileLink?: (path: string, line?: number | null) => void;
    onDraftChange?: (hasDraft: boolean) => void;
  };

  let {
    session,
    agentState = "idle",
    userDisplayName = "Otter",
    timeline,
    pendingPermissions,
    pendingQuestions,
    resolvingPermissionIds = [],
    resolvingQuestionIds = [],
    loading,
    turnRunning = false,
    turnCancelable = true,
    turnStartedAtMs = null,
    turnThinking = false,
    turnStatusHint = null,
    settingsSnapshot = null,
    backendConnected = true,
    onSubmitMessage,
    onResolvePermission,
    onResolveUserQuestion,
    onCancelTurn,
    onOpenFileLink,
    onDraftChange
  }: Props = $props();

  let displayUserName = $derived(userDisplayName.trim() || "Otter");
  let userInitial = $derived(displayUserName.trim().charAt(0).toUpperCase() || "O");

  function scopedSessionItemId(itemId: string): string {
    return session?.id ? `${session.id}::${itemId}` : itemId;
  }

  function isPermissionResolving(item: PermissionTimelineItem): boolean {
    return resolvingPermissionIds.includes(scopedSessionItemId(item.id));
  }

  function isQuestionResolving(item: UserQuestionTimelineItem): boolean {
    return resolvingQuestionIds.includes(scopedSessionItemId(item.id));
  }

  let draft = $state("");
  let draftBySessionId = $state<Record<string, string>>({});
  let threadEl: HTMLDivElement | undefined;
  let lastSessionId: string | null = null;
  let nowMs = $state(Date.now());
  let expandedActivityIds = $state<string[]>([]);
  let selectedActivityChildren = $state<Record<string, string>>({});
  let fastMode = $state(false);
  let permissionMode = $state<AgentPermissionMode>("workspace-write");
  let routingSelectionKey = $state<string | null>(null);
  let selectedProviderId = $state<string | null>(null);
  let selectedModelId = $state<string | null>(null);
  let selectedThinkingOptionId = $state("");
  let submitInFlightSessionIds = $state<string[]>([]);
  const submitInFlightGuards = new Set<string>();
  let thinkingProviderId = $state<string | null>(null);
  let thinkingModels = $state<ModelDescriptorInfo[]>([]);
  let thinkingLoadError = $state<string | null>(null);
  let composerPreferencesSessionId = $state<string | null>(null);
  let loadedThinkingPreferenceKey = $state<string | null>(null);
  let routingBySessionId = $state<Record<string, ComposerRoutingPreference>>({});
  let submitInFlight = $derived(
    Boolean(session?.id && submitInFlightSessionIds.includes(session.id))
  );
  let displayedProviderId = $derived(
    selectedProviderId ?? session?.providerId ?? settingsSnapshot?.config.defaultProvider ?? null
  );
  let engineerName = $derived(`${ENGINEER_NAME} (${providerDisplayName(displayedProviderId)})`);

  let fastModeAvailable = $derived(modelSupportsFastMode(selectedModelId));
  let selectedProviderModelSourceId = $derived.by(() => {
    return providerModelSourceId(selectedProviderId);
  });
  let selectedModelInfo = $derived(
    selectedProviderModelSourceId &&
      providerIdsEquivalent(thinkingProviderId, selectedProviderModelSourceId)
      ? thinkingModels.find((model) => model.id === selectedModelId) ?? null
      : null
  );
  let selectedProviderModelsLoaded = $derived(
    !selectedProviderModelSourceId ||
      providerIdsEquivalent(thinkingProviderId, selectedProviderModelSourceId)
  );
  let selectedProviderModelsLoadFailed = $derived(
    Boolean(
      thinkingLoadError &&
        selectedProviderModelSourceId &&
        providerIdsEquivalent(thinkingProviderId, selectedProviderModelSourceId)
    )
  );
  let selectedModelReady = $derived.by(() => {
    const modelId = selectedModelId?.trim();
    if (!modelId) return false;
    if (!selectedProviderModelSourceId) return true;
    if (selectedProviderModelsLoadFailed) return true;
    if (!selectedProviderModelsLoaded) return false;
    const current = thinkingModels.find((model) => model.id === modelId);
    if (current) return modelSupportsAgentTools(current);
    if (isCustomModelId(modelId, selectedProviderModelSourceId)) return true;
    return false;
  });
  let selectedModelBlockedReason = $derived.by(() => {
    const label = providerDisplayName(selectedProviderId);
    const modelId = selectedModelId?.trim();
    if (!modelId) {
      if (selectedProviderModelsLoaded && thinkingModels.length > 0) {
        return `No ${label} models support agent tools.`;
      }
      return selectedProviderModelsLoaded
        ? `Pick a ${label} model before sending.`
        : `Loading ${label} models before sending.`;
    }
    if (!selectedProviderModelSourceId) return null;
    if (selectedProviderModelsLoadFailed) return null;
    if (!selectedProviderModelsLoaded) return `Loading ${label} models before sending.`;
    const current = thinkingModels.find((model) => model.id === modelId);
    if (current && !modelSupportsAgentTools(current)) {
      return `${current.displayName || current.id} does not support agent tools.`;
    }
    if (isCustomModelId(modelId, selectedProviderModelSourceId)) return null;
    if (thinkingModels.length === 0) return `No ${label} models available.`;
    return `Updating ${label} model before sending.`;
  });
  let thinkingOptions = $derived(selectedModelInfo?.thinkingOptions ?? []);
  let thinkingAvailable = $derived(thinkingOptions.length > 0);
  let conversationStarted = $derived(
    (session?.eventCount ?? 0) > 0 ||
      timeline.some((item) =>
        ["user", "assistant", "system", "tool", "command", "diff"].includes(item.kind)
      )
  );
  let allowProviderSwitch = $derived(Boolean(session) && !conversationStarted && !turnRunning);
  let authenticatedProviderIds = $derived((settingsSnapshot?.auth ?? []).map((entry) => entry.providerId));
  let availableAgentProviderIds = $derived(
    (settingsSnapshot?.providers ?? [])
      .filter((provider) => providerIsAvailableForAgent(provider, authenticatedProviderIds))
      .map((provider) => provider.id)
  );
  let selectedProviderAuthenticated = $derived(
    settingsSnapshot === null ||
      !selectedProviderId ||
      (providerIdCanRunAgent(selectedProviderId, settingsSnapshot?.providers ?? []) &&
        providerIdInSet(selectedProviderId, availableAgentProviderIds))
  );
  let providerSwitchCanRecover = $derived(
    allowProviderSwitch && availableAgentProviderIds.length > 0
  );
  let agentBusy = $derived(
    !turnRunning &&
      (agentState === "running" || agentState === "thinking" || agentState === "awaiting")
  );
  let composerDisabled = $derived(
    !session ||
      !backendConnected ||
      agentBusy ||
      (!selectedProviderAuthenticated && !providerSwitchCanRecover)
  );
  let modelPickerDisabled = $derived(
    turnRunning || (!selectedProviderAuthenticated && !providerSwitchCanRecover)
  );
  let composerBlockedReason = $derived(
    agentBusy
      ? agentState === "awaiting"
        ? "Respond to the pending request before starting another turn."
        : "Wait for the running agent turn to finish."
      : !backendConnected
        ? "Reconnect the Puffer backend before sending another message."
      : selectedProviderAuthenticated
      ? selectedModelReady
        ? null
        : selectedModelBlockedReason
      : providerSwitchCanRecover
        ? `Switch to a connected provider to continue this empty session.`
        : `Reconnect ${providerDisplayName(selectedProviderId)} to continue this session.`
  );
  let canSubmitPrompt = $derived(
    Boolean(
      draft.trim() &&
        session &&
        backendConnected &&
        !turnRunning &&
        !agentBusy &&
        !submitInFlight &&
        selectedProviderAuthenticated &&
        selectedModelReady
    )
  );
  let canCancelTurn = $derived(Boolean(turnRunning && turnCancelable && onCancelTurn));

  function modelSupportsFastMode(modelId: string | null | undefined): boolean {
    const normalized = modelId?.trim().toLowerCase();
    if (!normalized) return false;
    return ["gpt-5", "gpt-4.1", "o3", "o4-mini"].some(
      (prefix) => normalized === prefix || normalized.startsWith(prefix)
    );
  }

  function normalizeModelIdForProvider(
    providerId: string | null | undefined,
    modelId: string | null | undefined
  ): string | null {
    const trimmed = modelId?.trim();
    if (!trimmed) return null;
    const slashIndex = trimmed.indexOf("/");
    if (slashIndex <= 0 || slashIndex === trimmed.length - 1) return trimmed;
    const prefix = trimmed.slice(0, slashIndex);
    const model = trimmed.slice(slashIndex + 1).trim();
    const provider = providerId?.trim();
    const canonicalPrefix = canonicalDaemonProviderId(prefix).toLowerCase();
    const canonicalProvider = provider ? canonicalDaemonProviderId(provider).toLowerCase() : "";
    if (
      provider &&
      model &&
      canonicalPrefix === canonicalProvider &&
      shouldStripModelPrefix(canonicalProvider)
    ) {
      return model;
    }
    return trimmed;
  }

  function shouldStripModelPrefix(canonicalProviderId: string): boolean {
    return canonicalProviderId === "openai" || canonicalProviderId === "anthropic";
  }

  function isCustomModelId(
    modelId: string | null | undefined,
    providerId: string | null | undefined = null
  ): boolean {
    const trimmed = modelId?.trim();
    if (!trimmed) return false;
    if (providerIdsEquivalent(providerId, "openrouter") && trimmed === "openrouter/auto") {
      return true;
    }
    return trimmed.includes(":") || trimmed.startsWith("ft-");
  }

  function modelSupportsAgentTools(model: ModelDescriptorInfo): boolean {
    return model.supportsTools !== false;
  }

  function configuredProviderDisplayName(providerId: string): string | null {
    const providers = settingsSnapshot?.providers ?? [];
    const exact = providers.find((provider) => provider.id.trim().toLowerCase() === providerId);
    if (exact?.displayName.trim()) return exact.displayName.trim();
    const equivalent = providers.find((provider) => providerIdsEquivalent(provider.id, providerId));
    if (equivalent?.displayName.trim()) return equivalent.displayName.trim();
    return null;
  }

  function providerModelSourceId(providerId: string | null | undefined): string | null {
    const normalized = providerId?.trim().toLowerCase();
    if (!normalized) return null;
    const providers = settingsSnapshot?.providers ?? [];
    const exact = providers.find((provider) => provider.id.trim().toLowerCase() === normalized);
    if (exact) return exact.id;
    const equivalent = providers.find((provider) => providerIdsEquivalent(provider.id, normalized));
    return equivalent?.id ?? providerId ?? null;
  }

  function providerDisplayName(providerId: string | null | undefined): string {
    const normalized = providerId?.trim().toLowerCase();
    if (!normalized) return "Codex";
    if (normalized === "claude" || normalized === "anthropic") return "Claude";
    const configured = configuredProviderDisplayName(normalized);
    if (configured) return configured;
    if (normalized === "codex") return "Codex";
    if (normalized === "openai") return "OpenAI";
    if (normalized === "openrouter") return "OpenRouter";
    if (normalized === "puffer") return "Puffer";
    return normalized
      .split(/[-_\s]+/)
      .filter(Boolean)
      .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
      .join(" ") || "Codex";
  }

  function normalizePermissionMode(value: string | null): AgentPermissionMode {
    if (value === "read-only" || value === "workspace-write" || value === "full-access") {
      return value;
    }
    return "workspace-write";
  }

  function sessionPreferenceKey(sessionId: string, name: string): string {
    return `puffer-agent:session:${sessionId}:${name}`;
  }

  function routingPreferenceKey(sessionId: string): string {
    return sessionPreferenceKey(sessionId, "routing");
  }

  function draftPreferenceKey(sessionId: string): string {
    return sessionPreferenceKey(sessionId, "draft");
  }

  function readDraftForSession(sessionId: string): string {
    if (typeof window === "undefined") return "";
    try {
      return window.localStorage.getItem(draftPreferenceKey(sessionId)) ?? "";
    } catch {
      return "";
    }
  }

  function readRoutingPreference(sessionId: string): ComposerRoutingPreference | null {
    const cached = routingBySessionId[sessionId];
    if (cached) return cached;
    if (typeof window === "undefined") return null;
    try {
      const raw = window.localStorage.getItem(routingPreferenceKey(sessionId));
      if (!raw) return null;
      const parsed = JSON.parse(raw) as Partial<ComposerRoutingPreference> | null;
      if (!parsed || typeof parsed.providerId !== "string") return null;
      return {
        providerId: parsed.providerId,
        modelId: typeof parsed.modelId === "string" ? parsed.modelId : ""
      };
    } catch {
      return null;
    }
  }

  function rememberRoutingPreference(
    sessionId: string,
    providerId: string | null,
    modelId: string | null
  ) {
    const preference = {
      providerId,
      modelId: normalizeModelIdForProvider(providerId, modelId) ?? ""
    };
    routingBySessionId = { ...routingBySessionId, [sessionId]: preference };
    if (typeof window !== "undefined") {
      window.localStorage.setItem(routingPreferenceKey(sessionId), JSON.stringify(preference));
    }
  }

  function thinkingPreferenceKey(): string | null {
    const sessionId = session?.id;
    const providerId = selectedProviderModelSourceId ?? selectedProviderId;
    const modelId = selectedModelId?.trim();
    if (!sessionId || !providerId || !modelId) return null;
    return sessionPreferenceKey(sessionId, `thinking:${providerId}:${modelId}`);
  }

  function composerOptions(): AgentTurnOptions {
    return {
      providerId: selectedProviderId,
      modelId: selectedModelId,
      thinkingOptionId: thinkingAvailable ? selectedThinkingOptionId || null : null,
      fastMode: fastModeAvailable && fastMode,
      permissionMode
    };
  }

  function pickModel(providerId: string, modelId: string) {
    const normalizedModelId = normalizeModelIdForProvider(providerId, modelId) ?? "";
    selectedProviderId = providerId;
    selectedModelId = normalizedModelId;
    selectedThinkingOptionId = "";
    if (session?.id) {
      rememberRoutingPreference(session.id, providerId, normalizedModelId);
    }
  }

  function setSubmitInFlight(sessionId: string, inFlight: boolean) {
    if (inFlight) {
      submitInFlightGuards.add(sessionId);
      if (!submitInFlightSessionIds.includes(sessionId)) {
        submitInFlightSessionIds = [...submitInFlightSessionIds, sessionId];
      }
      return;
    }
    submitInFlightGuards.delete(sessionId);
    submitInFlightSessionIds = submitInFlightSessionIds.filter((id) => id !== sessionId);
  }

  function thinkingLabel(optionId: string | null | undefined): string {
    if (!optionId) return "Default";
    return thinkingOptions.find((option) => option.id === optionId)?.label ?? optionId;
  }

  // Rolled-up thread: agent activity stays attached to the final response,
  // while intermediate prose remains in chronological order with tool work.
  type RowKind =
    | { key: string; kind: "user"; item: MessageTimelineItem }
    | { key: string; kind: "system"; item: MessageTimelineItem }
    | {
        key: string;
        kind: "agent";
        item: MessageTimelineItem | null;
        children: ActivityChild[];
        approvals: PermissionTimelineItem[];
        questions: UserQuestionTimelineItem[];
      };
  type ActivityChild = ToolTimelineItem | DiffTimelineItem | MessageTimelineItem;

  function isActivityMessage(child: ActivityChild): child is MessageTimelineItem {
    return child.kind === "assistant" || child.kind === "command";
  }

  function isThinkingActivity(child: ActivityChild): child is ToolTimelineItem {
    return child.kind === "tool" && child.toolName.toLowerCase() === "thinking";
  }

  function normalizeLegacyActivityOrder(children: ActivityChild[]): ActivityChild[] {
    const firstMessageIndex = children.findIndex(isActivityMessage);
    if (firstMessageIndex <= 0) return children;

    const actions = children.slice(0, firstMessageIndex);
    const messages = children.slice(firstMessageIndex);
    if (
      actions.length === 0 ||
      messages.length < 2 ||
      actions.some(isActivityMessage) ||
      messages.some((child) => !isActivityMessage(child))
    ) {
      return children;
    }

    const reordered: ActivityChild[] = [];
    const finalMessage = messages[messages.length - 1];
    const intermediateMessages = messages.slice(0, -1);
    const pairCount = Math.max(actions.length, intermediateMessages.length);
    for (let index = 0; index < pairCount; index += 1) {
      if (intermediateMessages[index]) reordered.push(intermediateMessages[index]);
      if (actions[index]) reordered.push(actions[index]);
    }
    reordered.push(finalMessage);
    return reordered;
  }

  function agentRowHasVisibleText(row: Extract<RowKind, { kind: "agent" }>): boolean {
    if (row.item?.body.trim()) return true;
    return row.children.some((child) => isActivityMessage(child) && child.body.trim());
  }

  function stableTextHash(text: string): string {
    let hash = 2166136261;
    for (let index = 0; index < text.length; index += 1) {
      hash ^= text.charCodeAt(index);
      hash = Math.imul(hash, 16777619);
    }
    return (hash >>> 0).toString(36);
  }

  function timelineItemKeyBase(item: TimelineItem): string {
    if (item.id) return `${item.kind}:${item.id}`;
    return `${item.kind}:${stableTextHash(
      [item.title, item.summary, item.body].filter(Boolean).join("\n")
    )}`;
  }

  function nextRowKey(base: string, counts: Map<string, number>): string {
    const count = counts.get(base) ?? 0;
    counts.set(base, count + 1);
    return `${base}:${count}`;
  }

  function buildRows(items: TimelineItem[]): RowKind[] {
    const rows: RowKind[] = [];
    const keyCounts = new Map<string, number>();
    let lastUserKey: string | null = null;
    let current:
      | Extract<RowKind, { kind: "agent" }>
      | null = null;

    const startAgentRow = (seed: TimelineItem): Extract<RowKind, { kind: "agent" }> => ({
      key: nextRowKey(
        lastUserKey ? `agent-after:${lastUserKey}` : `agent:${timelineItemKeyBase(seed)}`,
        keyCounts
      ),
      kind: "agent",
      item: null,
      children: [],
      approvals: [],
      questions: []
    });

    const flushCurrent = () => {
      if (!current) return;
      current.children = normalizeLegacyActivityOrder(current.children);
      let finalIndex = -1;
      for (let index = current.children.length - 1; index >= 0; index -= 1) {
        const child = current.children[index];
        if (isActivityMessage(child)) {
          finalIndex = index;
          break;
        }
      }
      if (finalIndex >= 0) {
        current.item = current.children[finalIndex] as MessageTimelineItem;
        current.children = current.children.filter((_, index) => index !== finalIndex);
      }
      rows.push(current);
      current = null;
    };

    for (const item of items) {
      if (item.kind === "user") {
        flushCurrent();
        lastUserKey = nextRowKey(timelineItemKeyBase(item), keyCounts);
        rows.push({ key: lastUserKey, kind: "user", item: item as MessageTimelineItem });
      } else if (item.kind === "system") {
        flushCurrent();
        rows.push({
          key: nextRowKey(timelineItemKeyBase(item), keyCounts),
          kind: "system",
          item: item as MessageTimelineItem
        });
      } else if (item.kind === "assistant" || item.kind === "command") {
        if (!current) current = startAgentRow(item);
        current.children.push(item as MessageTimelineItem);
      } else if (item.kind === "tool") {
        if (!current) current = startAgentRow(item);
        current.children.push(item as ToolTimelineItem);
      } else if (item.kind === "diff") {
        if (!current) current = startAgentRow(item);
        current.children.push(item as DiffTimelineItem);
      } else if (item.kind === "question") {
        if (!current) current = startAgentRow(item);
        current.questions.push(item as UserQuestionTimelineItem);
      }
    }
    flushCurrent();
    return rows;
  }

  let rows = $derived(
    buildRows(
      timeline.filter(
        (i) =>
          i.kind !== "permission" &&
          !(i.kind === "question" && i.status === "pending")
      )
    )
  );

  function formatTime(ms: number | undefined): string {
    if (!ms) return "";
    const d = new Date(ms);
    const h = d.getHours();
    const m = d.getMinutes().toString().padStart(2, "0");
    const hh = h < 10 ? `0${h}` : `${h}`;
    return `${hh}:${m}`;
  }

  function formatElapsed(startedAtMs: number | null): string {
    if (!startedAtMs) return "";
    const elapsed = Math.max(0, nowMs - startedAtMs) / 1000;
    return elapsed < 10 ? `${elapsed.toFixed(1)}s` : `${Math.floor(elapsed)}s`;
  }

  function setDraftForSession(sessionId: string | null | undefined, value: string) {
    if (!sessionId) return;
    if (value.length > 0) {
      draftBySessionId = { ...draftBySessionId, [sessionId]: value };
      if (typeof window !== "undefined") {
        try {
          window.localStorage.setItem(draftPreferenceKey(sessionId), value);
        } catch {
          /* Draft persistence is best-effort. */
        }
      }
      return;
    }
    const { [sessionId]: _removed, ...rest } = draftBySessionId;
    draftBySessionId = rest;
    if (typeof window !== "undefined") {
      try {
        window.localStorage.removeItem(draftPreferenceKey(sessionId));
      } catch {
        /* Draft persistence is best-effort. */
      }
    }
  }

  function updateDraft(value: string) {
    draft = value;
    setDraftForSession(session?.id, value);
  }

  $effect(() => {
    // Keep unsent composer text isolated per session while switching threads.
    const nextSessionId = session?.id ?? null;
    if (nextSessionId !== lastSessionId) {
      draft = nextSessionId ? draftBySessionId[nextSessionId] ?? readDraftForSession(nextSessionId) : "";
      expandedActivityIds = [];
      selectedActivityChildren = {};
      lastSessionId = nextSessionId;
      void tick().then(() => threadEl?.scrollTo({ top: 0, behavior: "auto" }));
    }
  });

  $effect(() => {
    onDraftChange?.(draft.trim().length > 0);
  });

  onDestroy(() => {
    onDraftChange?.(false);
  });

  $effect(() => {
    const sessionId = session?.id ?? null;
    const saved =
      sessionId && (session?.eventCount ?? 0) === 0
        ? readRoutingPreference(sessionId)
        : null;
    const sessionHasRoute = Boolean(session?.providerId || session?.modelId);
    const source = saved ? "saved" : sessionHasRoute ? "session" : "default";
    const providerId = saved
      ? saved.providerId
      : session?.providerId ?? settingsSnapshot?.config.defaultProvider ?? null;
    const modelId = saved
      ? saved.modelId
      : session?.modelId ?? settingsSnapshot?.config.defaultModel ?? null;
    const nextSelectionKey = [
      sessionId ?? "",
      source,
      providerId ?? "",
      modelId ?? ""
    ].join("\0");
    if (nextSelectionKey === routingSelectionKey) return;
    routingSelectionKey = nextSelectionKey;
    selectedProviderId = providerId;
    selectedModelId = normalizeModelIdForProvider(
      providerId,
      modelId
    );
    selectedThinkingOptionId = "";
  });

  $effect(() => {
    const providerId = selectedProviderModelSourceId;
    if (!providerId) {
      thinkingProviderId = null;
      thinkingModels = [];
      return;
    }
    let canceled = false;
    thinkingLoadError = null;
    void listProviderModels(providerId)
      .then((models) => {
        if (canceled) return;
        thinkingProviderId = providerId;
        thinkingModels = models;
      })
      .catch((error) => {
        if (canceled) return;
        thinkingProviderId = providerId;
        thinkingModels = [];
        thinkingLoadError = String(error);
      });
    return () => {
      canceled = true;
    };
  });

  $effect(() => {
    if (
      !selectedProviderId ||
      !selectedProviderModelSourceId ||
      !providerIdsEquivalent(thinkingProviderId, selectedProviderModelSourceId) ||
      thinkingModels.length === 0
    ) {
      return;
    }
    if (
      selectedModelId &&
      thinkingModels.some((model) => model.id === selectedModelId && modelSupportsAgentTools(model))
    ) {
      return;
    }
    const selectedCatalogModel = selectedModelId
      ? thinkingModels.find((model) => model.id === selectedModelId)
      : null;
    const defaultModel = settingsSnapshot?.config.defaultModel ?? null;
    const defaultProvider = settingsSnapshot?.config.defaultProvider ?? null;
    const selectedCanonical = canonicalDaemonProviderId(selectedProviderId);
    const defaultCanonical = defaultProvider ? canonicalDaemonProviderId(defaultProvider) : null;
    const isDefaultFromOtherProvider =
      defaultModel &&
      defaultCanonical &&
      selectedModelId === normalizeModelIdForProvider(selectedProviderId, defaultModel) &&
      selectedCanonical !== defaultCanonical;
    if (
      !selectedCatalogModel &&
      !isDefaultFromOtherProvider &&
      isCustomModelId(selectedModelId, selectedProviderModelSourceId)
    ) {
      return;
    }
    const fallback =
      thinkingModels.find((model) => model.isDefault && modelSupportsAgentTools(model)) ??
      thinkingModels.find(modelSupportsAgentTools) ??
      null;
    const nextModelId = fallback?.id ?? "";
    selectedModelId = nextModelId;
    selectedThinkingOptionId = "";
    if (session?.id && selectedProviderId) {
      rememberRoutingPreference(session.id, selectedProviderId, nextModelId);
    }
  });

  $effect(() => {
    if (!thinkingAvailable) {
      selectedThinkingOptionId = "";
      loadedThinkingPreferenceKey = null;
      return;
    }
    const preferenceKey = thinkingPreferenceKey();
    if (typeof window !== "undefined" && preferenceKey && preferenceKey !== loadedThinkingPreferenceKey) {
      loadedThinkingPreferenceKey = preferenceKey;
      const saved = window.localStorage.getItem(preferenceKey);
      if (saved && thinkingOptions.some((option) => option.id === saved)) {
        selectedThinkingOptionId = saved;
        return;
      }
      selectedThinkingOptionId = "";
      return;
    }
    if (
      selectedThinkingOptionId &&
      thinkingOptions.some((option) => option.id === selectedThinkingOptionId)
    ) {
      return;
    }
    selectedThinkingOptionId = "";
  });

  $effect(() => {
    if (!turnRunning || !turnStartedAtMs) return;
    nowMs = Date.now();
    const interval = window.setInterval(() => {
      nowMs = Date.now();
    }, 100);
    return () => window.clearInterval(interval);
  });

  $effect(() => {
    if (typeof window === "undefined") return;
    const sessionId = session?.id ?? null;
    if (sessionId === composerPreferencesSessionId) return;
    composerPreferencesSessionId = sessionId;
    if (!sessionId) {
      fastMode = false;
      permissionMode = "workspace-write";
      return;
    }
    fastMode = window.localStorage.getItem(sessionPreferenceKey(sessionId, "fast-mode")) === "1";
    permissionMode = normalizePermissionMode(
      window.localStorage.getItem(sessionPreferenceKey(sessionId, "permission-mode"))
    );
  });

  $effect(() => {
    if (typeof window === "undefined") return;
    const sessionId = session?.id ?? null;
    if (!sessionId || composerPreferencesSessionId !== sessionId) return;
    window.localStorage.setItem(sessionPreferenceKey(sessionId, "fast-mode"), fastMode ? "1" : "0");
    window.localStorage.setItem(sessionPreferenceKey(sessionId, "permission-mode"), permissionMode);
  });

  $effect(() => {
    if (typeof window === "undefined") return;
    if (!thinkingAvailable) return;
    const preferenceKey = thinkingPreferenceKey();
    if (!preferenceKey) return;
    if (!selectedThinkingOptionId) {
      window.localStorage.removeItem(preferenceKey);
      return;
    }
    if (!thinkingOptions.some((option) => option.id === selectedThinkingOptionId)) return;
    window.localStorage.setItem(preferenceKey, selectedThinkingOptionId);
  });

  async function submit() {
    const v = draft.trim();
    if (!v || !canSubmitPrompt) return;
    const targetSessionId = session?.id;
    if (!targetSessionId) return;
    if (submitInFlightGuards.has(targetSessionId)) return;
    const previousDraft = draft;
    setSubmitInFlight(targetSessionId, true);
    draft = "";
    setDraftForSession(targetSessionId, "");
    try {
      const accepted = await onSubmitMessage(v, composerOptions());
      if (accepted === false) {
        setDraftForSession(targetSessionId, previousDraft);
        if ((session?.id ?? null) === targetSessionId && !draft.trim()) draft = previousDraft;
        return;
      }
      await tick();
      if ((session?.id ?? null) === targetSessionId) {
        threadEl?.scrollTo({ top: threadEl.scrollHeight, behavior: "smooth" });
      }
    } catch {
      setDraftForSession(targetSessionId, previousDraft);
      if ((session?.id ?? null) === targetSessionId && !draft.trim()) draft = previousDraft;
    } finally {
      setSubmitInFlight(targetSessionId, false);
    }
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.isComposing || e.keyCode === 229) return;
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      submit();
    }
  }

  // Distribute any pending permissions under the latest agent row so the
  // approval prompt sits with the tool call it's asking about.
  let distributedRows = $derived.by(() => {
    const out = [...rows];
    if (!pendingPermissions.length && !pendingQuestions.length) return out;
    // attach to the last agent row (or append a synthetic one)
    const lastAgentIdx = (() => {
      for (let i = out.length - 1; i >= 0; i--) if (out[i].kind === "agent") return i;
      return -1;
    })();
    if (lastAgentIdx >= 0 && out[lastAgentIdx].kind === "agent") {
      const prev = out[lastAgentIdx] as Extract<RowKind, { kind: "agent" }>;
      out[lastAgentIdx] = {
        ...prev,
        approvals: [...prev.approvals, ...pendingPermissions],
        questions: [...prev.questions, ...pendingQuestions]
      };
    } else {
      out.push({
        key: "agent-pending-prompts",
        kind: "agent",
        item: null,
        children: [],
        approvals: [...pendingPermissions],
        questions: [...pendingQuestions]
      });
    }
    return out;
  });

  type ActivityCategory = "thought" | "message" | "agent" | "write" | "read" | "browser" | "terminal" | "search" | "diff" | "other";

  type ActivitySummary = {
    icons: IconName[];
    text: string;
    failed: number;
  };

  const activityOrder: ActivityCategory[] = ["thought", "message", "agent", "write", "read", "browser", "terminal", "search", "diff", "other"];

  let activeTurnAgentRowIndex = $derived.by(() => {
    if (!turnRunning) return -1;
    let latestUserIndex = -1;
    for (let index = distributedRows.length - 1; index >= 0; index -= 1) {
      if (distributedRows[index].kind === "user") {
        latestUserIndex = index;
        break;
      }
    }
    if (latestUserIndex < 0) return -1;
    for (let index = distributedRows.length - 1; index > latestUserIndex; index -= 1) {
      if (distributedRows[index].kind === "agent") return index;
    }
    return -1;
  });

  let activeTurnHasVisibleText = $derived.by(() => {
    if (!turnRunning) return false;
    if (activeTurnAgentRowIndex >= 0) {
      const row = distributedRows[activeTurnAgentRowIndex];
      return row?.kind === "agent" && agentRowHasVisibleText(row);
    }
    for (let index = distributedRows.length - 1; index >= 0; index -= 1) {
      const row = distributedRows[index];
      if (row.kind === "agent") return agentRowHasVisibleText(row);
    }
    return false;
  });

  let typingLabel = $derived.by(() => {
    const elapsed = formatElapsed(turnStartedAtMs);
    const suffix = elapsed ? ` (${elapsed})` : "";
    if (turnRunning) {
      if (turnStatusHint) return `${turnStatusHint}${suffix}`;
      if (turnThinking) return `Thinking${suffix}`;
      if (activeTurnHasVisibleText) return null;
      return `Running${suffix}`;
    }
    if (agentState === "awaiting") return `${engineerName} paused - waiting for your response`;
    return null;
  });

  function shouldCollapseActivity(row: Extract<RowKind, { kind: "agent" }>, idx: number): boolean {
    const isActiveTurn = idx === activeTurnAgentRowIndex;
    return !isActiveTurn && row.children.length > 0 && Boolean(row.item?.body.trim());
  }

  function activityGroupId(row: Extract<RowKind, { kind: "agent" }>, idx: number): string {
    return row.key || row.item?.id || row.children[0]?.id || `activity-${idx}`;
  }

  function activityExpanded(id: string): boolean {
    return expandedActivityIds.includes(id);
  }

  function toggleActivity(id: string) {
    expandedActivityIds = activityExpanded(id)
      ? expandedActivityIds.filter((value) => value !== id)
      : [...expandedActivityIds, id];
  }

  function activityActionOrder(childIdx: number): number {
    return childIdx * 2;
  }

  function activityPanelOrder(childIdx: number): number {
    return activityActionOrder(childIdx) + 1;
  }

  function activityChildSelected(activityId: string, childId: string): boolean {
    return selectedActivityChildren[activityId] === childId;
  }

  function toggleActivityChild(activityId: string, childId: string) {
    const { [activityId]: current, ...rest } = selectedActivityChildren;
    selectedActivityChildren = current === childId ? rest : { ...rest, [activityId]: childId };
  }

  function selectedActivityChild(
    children: ActivityChild[],
    activityId: string
  ): { child: ActivityChild; idx: number } | null {
    const childId = selectedActivityChildren[activityId];
    if (!childId) return null;
    const idx = children.findIndex((child) => child.id === childId);
    if (idx < 0) return null;
    if (isThinkingActivity(children[idx])) return null;
    return { child: children[idx], idx };
  }

  function activityIcon(category: ActivityCategory): IconName {
    if (category === "thought") return "sparkles";
    if (category === "message") return "sparkles";
    if (category === "agent") return "plug";
    if (category === "write") return "edit";
    if (category === "read") return "file";
    if (category === "browser") return "globe";
    if (category === "terminal") return "terminal";
    if (category === "search") return "search";
    if (category === "diff") return "git";
    return "bolt";
  }

  function compactToolName(name: string | null | undefined): string {
    return (name ?? "").toLowerCase().replace(/[^a-z0-9]/g, "");
  }

  function isSubagentToolName(name: string | null | undefined): boolean {
    const compact = compactToolName(name);
    return (
      compact === "subagent" ||
      compact === "spawnagent" ||
      compact === "waitagent" ||
      compact === "sendinput" ||
      compact === "closeagent" ||
      compact === "resumeagent" ||
      compact.includes("collab")
    );
  }

  function subagentActivityName(name: string): string {
    const compact = compactToolName(name);
    if (compact === "spawnagent" || compact === "subagent") return "Spawn sub-agent";
    if (compact === "waitagent") return "Wait for sub-agent";
    if (compact === "sendinput") return "Message sub-agent";
    if (compact === "closeagent") return "Close sub-agent";
    if (compact === "resumeagent") return "Resume sub-agent";
    return "Sub-agent";
  }

  function childActivityCategory(child: ActivityChild): ActivityCategory {
    if (child.kind === "diff") return "diff";
    if (child.kind !== "tool") return "message";
    const name = child.toolName.toLowerCase();
    if (name === "thinking") return "thought";
    if (isSubagentToolName(child.toolName)) return "agent";
    if (name.includes("browser") || name.includes("web") || name.includes("fetch")) return "browser";
    if (name.includes("mcp__") && (name.includes("__list") || name.includes("__read"))) return "read";
    if (name.includes("edit") || name.includes("write") || name.includes("replace") || name.includes("patch")) return "write";
    if (name.includes("read") || name.includes("view")) return "read";
    if (name.includes("bash") || name.includes("shell") || name.includes("exec") || name.includes("terminal")) return "terminal";
    if (name.includes("grep") || name.includes("glob") || name.includes("search")) return "search";
    if (name.includes("git") || name.includes("diff")) return "diff";
    return "other";
  }

  function isTerminalActivity(child: ActivityChild): child is ToolTimelineItem {
    if (child.kind !== "tool") return false;
    const name = child.toolName.toLowerCase();
    return name === "bash" || name === "shell" || name === "powershell";
  }

  function activityActionSelected(activityId: string, child: ActivityChild): boolean {
    return activityChildSelected(activityId, child.id);
  }

  function handleActivityActionClick(activityId: string, child: ActivityChild) {
    toggleActivityChild(activityId, child.id);
  }

  function childFailed(child: ActivityChild): boolean {
    if (child.kind === "assistant" || child.kind === "command") return false;
    const status = (child.status ?? "").toLowerCase();
    return status.includes("err") || status.includes("fail");
  }

  function activityStatus(child: ActivityChild): string {
    if (child.kind === "assistant" || child.kind === "command") return "done";
    const status = (child.status ?? "").toLowerCase();
    if (status.includes("run") || status === "pending") return "running";
    if (status.includes("err") || status.includes("fail")) return "failed";
    return "done";
  }

  function activityCreatedAtMs(child: ActivityChild): number | null {
    return typeof child.createdAtMs === "number" && Number.isFinite(child.createdAtMs)
      ? child.createdAtMs
      : null;
  }

  function durationFromToolPayload(child: ToolTimelineItem): number | null {
    const input = parseInputObject(child);
    const output = parseOutputObject(child);
    const candidates = [
      input?.durationMs,
      input?.duration_ms,
      output?.durationMs,
      output?.duration_ms
    ];
    for (const candidate of candidates) {
      if (typeof candidate === "number" && Number.isFinite(candidate) && candidate >= 0) {
        return candidate;
      }
    }
    return null;
  }

  function formatThoughtDuration(ms: number | null): string {
    if (ms === null) return "a moment";
    const totalSeconds = Math.max(1, Math.round(ms / 1000));
    if (totalSeconds < 60) return `${totalSeconds} ${totalSeconds === 1 ? "second" : "seconds"}`;
    const minutes = Math.floor(totalSeconds / 60);
    const seconds = totalSeconds % 60;
    if (seconds === 0) return `${minutes} ${minutes === 1 ? "minute" : "minutes"}`;
    return `${minutes} ${minutes === 1 ? "minute" : "minutes"} ${seconds} ${seconds === 1 ? "second" : "seconds"}`;
  }

  function thoughtDurationLabel(
    child: ActivityChild,
    children: ActivityChild[],
    childIdx: number,
    finalMessage: MessageTimelineItem | null
  ): string {
    if (!isThinkingActivity(child)) return "";
    const explicitDuration = durationFromToolPayload(child);
    if (explicitDuration !== null) return formatThoughtDuration(explicitDuration);

    const start = activityCreatedAtMs(child);
    if (start === null) return formatThoughtDuration(null);
    const nextItem = children[childIdx + 1] ?? finalMessage;
    const end = nextItem ? activityCreatedAtMs(nextItem) : null;
    if (end === null || end <= start) return formatThoughtDuration(null);
    return formatThoughtDuration(end - start);
  }

  function parseInputObject(child: ToolTimelineItem): Record<string, unknown> | null {
    if (child.inputJson) return child.inputJson;
    try {
      const parsed = JSON.parse(child.input);
      return typeof parsed === "object" && parsed !== null ? (parsed as Record<string, unknown>) : null;
    } catch {
      return null;
    }
  }

  function parseOutputObject(child: ToolTimelineItem): Record<string, unknown> | null {
    try {
      const parsed = JSON.parse(child.output);
      return typeof parsed === "object" && parsed !== null ? (parsed as Record<string, unknown>) : null;
    } catch {
      return null;
    }
  }

  function inputString(input: Record<string, unknown> | null, names: string[]): string | null {
    if (!input) return null;
    for (const name of names) {
      const value = input[name];
      if (typeof value === "string" && value.trim()) return value;
    }
    return null;
  }

  function titleCaseAction(value: string | null): string {
    if (!value) return "Action";
    return value
      .split(/[_-]+/)
      .filter(Boolean)
      .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
      .join(" ")
      .replace(/\bMcp\b/g, "MCP")
      .replace(/\bCdp\b/g, "CDP")
      .replace(/\bJson\b/g, "JSON")
      .replace(/\bUrl\b/g, "URL")
      .replace(/\bUri\b/g, "URI")
      .replace(/\bId\b/g, "ID");
  }

  function valuePreview(value: unknown, maxLength = 96): string {
    const text =
      typeof value === "string"
        ? value
        : value === null || value === undefined
          ? ""
          : JSON.stringify(value);
    const compact = text.replace(/\s+/g, " ").trim();
    return compact.length > maxLength ? `${compact.slice(0, maxLength - 1)}...` : compact;
  }

  function inputRecord(input: Record<string, unknown> | null, name: string): Record<string, unknown> | null {
    const value = input?.[name];
    return typeof value === "object" && value !== null ? (value as Record<string, unknown>) : null;
  }

  function mcpParts(
    name: string,
    input: Record<string, unknown> | null
  ): { server: string; tool: string } | null {
    const server = inputString(input, ["server"]);
    const tool = inputString(input, ["tool"]);
    if (server || tool) return { server: server ?? "mcp", tool: tool ?? "tool" };
    const match = /^mcp__(.*?)__(.*)$/.exec(name);
    if (!match) return null;
    return { server: match[1] || "mcp", tool: match[2] || "tool" };
  }

  function mcpActivityName(name: string, input: Record<string, unknown> | null): string | null {
    const mcp = mcpParts(name, input);
    return mcp ? `${titleCaseAction(mcp.server)} · ${titleCaseAction(mcp.tool)}` : null;
  }

  function mcpActivityArg(name: string, input: Record<string, unknown> | null): string | null {
    const mcp = mcpParts(name, input);
    if (!mcp) return null;
    const resourceUri = inputString(input, ["resourceUri", "resource_uri"]);
    if (resourceUri) return resourceUri;
    const args = inputRecord(input, "arguments");
    if (!args || Object.keys(args).length === 0) return titleCaseAction(mcp.tool);
    const preferred = inputString(args, ["url", "uri", "path", "filePath", "query", "q", "pattern", "action", "ref", "key"]);
    if (preferred) return preferred;
    const entries = Object.entries(args)
      .filter(([, value]) => value !== null && value !== undefined && value !== "")
      .slice(0, 3)
      .map(([key, value]) => `${key}: ${valuePreview(value, 42)}`);
    return entries.length ? entries.join(" · ") : titleCaseAction(mcp.tool);
  }

  function browserActionArg(input: Record<string, unknown> | null): string | null {
    const action = inputString(input, ["action"]);
    if (!action) return null;
    const label = action.charAt(0).toUpperCase() + action.slice(1);
    const url = inputString(input, ["url"]);
    const ref = inputString(input, ["ref"]);
    const text = inputString(input, ["text"]);
    const key = inputString(input, ["key"]);
    if (action === "list") return "List";
    if ((action === "open" || action === "navigate") && url) return `${label} ${url}`;
    if ((action === "click" || action === "focus" || action === "close") && ref) return `${label} ${ref}`;
    if ((action === "type" || action === "fill") && text) return `${label} ${text}`;
    if (action === "press" && key) return `${label} ${key}`;
    return label;
  }

  type ToolActionDisplay = {
    name: string;
    arg: string;
  };

  type DiffLineStats = {
    added: number;
    removed: number;
  };

  function isRecordValue(value: unknown): value is Record<string, unknown> {
    return typeof value === "object" && value !== null && !Array.isArray(value);
  }

  function basename(path: string): string {
    const parts = path.split(/[\\/]+/).filter(Boolean);
    return parts.at(-1) ?? path;
  }

  function uniqueStrings(values: string[]): string[] {
    const out: string[] = [];
    for (const value of values) {
      const trimmed = value.trim();
      if (trimmed && !out.includes(trimmed)) out.push(trimmed);
    }
    return out;
  }

  function stringList(value: unknown): string[] {
    return Array.isArray(value)
      ? value.filter((entry): entry is string => typeof entry === "string" && entry.trim().length > 0)
      : [];
  }

  function changeRecords(value: unknown): Record<string, unknown>[] {
    if (Array.isArray(value)) {
      return value.filter(isRecordValue);
    }
    if (!isRecordValue(value)) return [];
    return Object.entries(value).flatMap(([path, entry]) => {
      if (isRecordValue(entry)) return [{ path, ...entry }];
      if (typeof entry === "string") return [{ path, diff: entry }];
      return [];
    });
  }

  function changePath(change: Record<string, unknown>): string | null {
    for (const key of ["path", "file_path", "filePath", "filename", "file"]) {
      const value = change[key];
      if (typeof value === "string" && value.trim()) return value;
    }
    return null;
  }

  function changeDiff(change: Record<string, unknown>): string | null {
    for (const key of ["diff", "patch", "summary", "content"]) {
      const value = change[key];
      if (typeof value === "string" && value.trim()) return value;
    }
    return null;
  }

  function changeKind(change: Record<string, unknown>): string {
    const value = change.kind;
    if (typeof value === "string") return value.toLowerCase();
    if (isRecordValue(value)) {
      const type = value.type;
      if (typeof type === "string") return type.toLowerCase();
    }
    const type = change.type;
    return typeof type === "string" ? type.toLowerCase() : "";
  }

  function contentLineCount(content: string): number {
    return content.split(/\r?\n/).filter((line) => line.length > 0).length;
  }

  function diffLineStats(diff: string): DiffLineStats {
    const stats = { added: 0, removed: 0 };
    for (const line of diff.split(/\r?\n/)) {
      if (line.startsWith("+++") || line.startsWith("---")) continue;
      if (line.startsWith("+")) stats.added += 1;
      else if (line.startsWith("-")) stats.removed += 1;
    }
    return stats;
  }

  function fileChangeLineStats(change: Record<string, unknown>): DiffLineStats | null {
    const diff = changeDiff(change);
    if (!diff) return null;
    const stats = diffLineStats(diff);
    if (stats.added > 0 || stats.removed > 0) return stats;

    const kind = changeKind(change);
    const count = contentLineCount(diff);
    if (count === 0) return stats;
    if (kind === "add" || kind === "create" || kind === "write") {
      return { added: count, removed: 0 };
    }
    if (kind === "delete" || kind === "remove" || kind === "removed") {
      return { added: 0, removed: count };
    }
    return stats;
  }

  function mergeLineStats(a: DiffLineStats, b: DiffLineStats): DiffLineStats {
    return { added: a.added + b.added, removed: a.removed + b.removed };
  }

  function fileChangeDisplay(child: ToolTimelineItem): ToolActionDisplay | null {
    const lowerName = child.toolName.toLowerCase();
    const input = parseInputObject(child);
    const output = parseOutputObject(child);
    const outputType = typeof output?.type === "string" ? output.type.toLowerCase() : "";
    const looksLikeFileChange =
      outputType === "filechange" ||
      ["edit", "edit_file", "replace_in_file", "apply_patch", "apply_diff", "file_change", "filechange"].includes(lowerName);
    if (!looksLikeFileChange) return null;

    const changes = [
      ...changeRecords(input?.changes),
      ...changeRecords(output?.changes)
    ];
    const directPath = inputString(input, ["path", "file_path", "filePath", "filename", "file"]);
    const paths = uniqueStrings([
      ...stringList(input?.files),
      ...stringList(output?.files),
      ...(directPath ? [directPath] : []),
      ...changes.flatMap((change) => {
        const path = changePath(change);
        return path ? [path] : [];
      })
    ]);

    const inlineDiff = inputString(input, ["diff", "patch"]) ?? inputString(output, ["diff", "patch"]);
    const stats = [
      ...changes.flatMap((change) => {
        const lineStats = fileChangeLineStats(change);
        return lineStats ? [lineStats] : [];
      }),
      ...(inlineDiff ? [diffLineStats(inlineDiff)] : [])
    ].reduce(mergeLineStats, { added: 0, removed: 0 });

    const name =
      paths.length === 1
        ? `Edit ${basename(paths[0])}`
        : paths.length > 1
          ? `Edit ${paths.length} files`
          : "Edit";
    const arg = stats.added > 0 || stats.removed > 0
      ? `+${stats.added} -${stats.removed}`
      : paths.length > 0
        ? paths.map(basename).join(", ")
        : "";
    return { name, arg };
  }

  function genericToolDisplay(child: ToolTimelineItem): ToolActionDisplay | null {
    const input = parseInputObject(child);
    const lowerName = child.toolName.toLowerCase();
    if (isTerminalActivity(child)) {
      const command = inputString(input, ["command"]) ?? child.summary ?? child.title ?? "";
      return { name: "Shell", arg: valuePreview(command, 120) };
    }
    if (lowerName === "read" || lowerName === "read_file") {
      const path = inputString(input, ["path", "file_path", "filePath"]);
      return { name: path ? `Read ${basename(path)}` : "Read", arg: path ?? "" };
    }
    if (lowerName === "write" || lowerName === "write_file" || lowerName === "create_file") {
      const path = inputString(input, ["path", "file_path", "filePath"]);
      return { name: path ? `Write ${basename(path)}` : "Write", arg: path ?? "" };
    }
    if (lowerName === "web_search" || lowerName === "search") {
      const query = inputString(input, ["query", "q", "pattern"]) ?? "";
      return { name: "Search", arg: valuePreview(query, 120) };
    }
    if (lowerName === "view_image") {
      const path = inputString(input, ["path"]);
      return { name: path ? `View ${basename(path)}` : "View image", arg: path ?? "" };
    }
    if (lowerName === "image_generation") {
      const prompt = inputString(input, ["prompt"]) ?? "";
      return { name: "Generate image", arg: valuePreview(prompt, 120) };
    }
    if (lowerName === "browser") {
      return { name: "Browser", arg: browserActionArg(input) ?? "Action" };
    }
    if (lowerName === "plan") return { name: "Plan", arg: "" };
    return null;
  }

  function activityActionName(child: ActivityChild): string {
    if (child.kind === "diff") return "Diff";
    if (child.kind !== "tool") return "Message";
    const input = parseInputObject(child);
    const fileChange = fileChangeDisplay(child);
    if (fileChange) return fileChange.name;
    const generic = genericToolDisplay(child);
    if (generic) return generic.name;
    const mcpName = mcpActivityName(child.toolName, input);
    if (mcpName) return mcpName;
    if (isSubagentToolName(child.toolName)) return subagentActivityName(child.toolName);
    return child.toolName && child.toolName !== "undefined" ? child.toolName : "Tool";
  }

  function arrayInputString(input: Record<string, unknown> | null, name: string): string | null {
    const value = input?.[name];
    if (!Array.isArray(value)) return null;
    const strings = value
      .map((item) => (typeof item === "string" ? item.trim() : ""))
      .filter(Boolean);
    if (strings.length === 0) return null;
    return strings.length === 1 ? strings[0] : `${strings.length} targets`;
  }

  function subagentActivityArg(name: string, input: Record<string, unknown> | null): string {
    const compact = compactToolName(name);
    if (compact === "waitagent") {
      return [
        arrayInputString(input, "targets"),
        inputString(input, ["timeoutMs", "timeout_ms"])
      ].filter(Boolean).join(" · ");
    }
    if (compact === "sendinput") {
      return [
        inputString(input, ["target", "agentId", "agent_id"]),
        valuePreview(inputString(input, ["message"]), 80)
      ].filter(Boolean).join(" · ");
    }
    if (compact === "closeagent" || compact === "resumeagent") {
      return inputString(input, ["target", "id", "agentId", "agent_id"]) ?? "";
    }
    return [
      inputString(input, ["agent_type", "agentType", "tool", "role"]),
      inputString(input, ["model"]),
      inputString(input, ["reasoningEffort", "reasoning_effort"]),
      valuePreview(inputString(input, ["message", "prompt"]), 80)
    ].filter(Boolean).join(" · ");
  }

  function activityActionArg(child: ActivityChild): string {
    if (child.kind === "diff") return child.diff.title;
    if (child.kind !== "tool") {
      const compact = child.body.replace(/\s+/g, " ").trim();
      return compact.length > 96 ? `${compact.slice(0, 95)}...` : compact || "Assistant message";
    }
    const input = parseInputObject(child);
    const fileChange = fileChangeDisplay(child);
    if (fileChange) return fileChange.arg;
    const generic = genericToolDisplay(child);
    if (generic) return generic.arg;
    const mcpArg = mcpActivityArg(child.toolName, input);
    if (mcpArg) return mcpArg;
    if (isSubagentToolName(child.toolName)) return subagentActivityArg(child.toolName, input);
    if (child.toolName.toLowerCase() === "browser") return browserActionArg(input) ?? "Action";
    const value = inputString(input, ["path", "file_path", "url", "command", "pattern", "query", "cwd"]);
    const fallback = value ?? child.summary ?? child.title ?? "";
    const compact = fallback.replace(/\s+/g, " ").trim();
    return compact.length > 96 ? `${compact.slice(0, 95)}...` : compact;
  }

  function plural(count: number, singular: string, pluralValue = `${singular}s`): string {
    return `${count} ${count === 1 ? singular : pluralValue}`;
  }

  function activitySummary(children: ActivityChild[]): ActivitySummary {
    const counts = new Map<ActivityCategory, number>();
    const failures = new Map<ActivityCategory, number>();
    for (const child of children) {
      const category = childActivityCategory(child);
      counts.set(category, (counts.get(category) ?? 0) + 1);
      if (childFailed(child)) failures.set(category, (failures.get(category) ?? 0) + 1);
    }

    const parts: string[] = [];
    const writeCount = counts.get("write") ?? 0;
    const writeFailures = failures.get("write") ?? 0;
    if (writeCount > 0) {
      parts.push(writeFailures === writeCount ? `Tried writing ${plural(writeCount, "file")}` : `Wrote ${plural(writeCount, "file")}`);
    }
    const readCount = counts.get("read") ?? 0;
    if (readCount > 0) parts.push(`Read ${plural(readCount, "file")}`);
    const browserCount = counts.get("browser") ?? 0;
    if (browserCount > 0) parts.push("Interacted with browser");
    const terminalCount = counts.get("terminal") ?? 0;
    if (terminalCount > 0) parts.push(`Ran ${plural(terminalCount, "command")}`);
    const searchCount = counts.get("search") ?? 0;
    if (searchCount > 0) parts.push(searchCount === 1 ? "Searched" : `Searched ${searchCount} times`);
    const diffCount = counts.get("diff") ?? 0;
    if (diffCount > 0) parts.push(`Updated ${plural(diffCount, "diff", "diffs")}`);
    const messageCount = counts.get("message") ?? 0;
    if (messageCount > 0) parts.push(messageCount === 1 ? "Intermediate message" : `${messageCount} intermediate messages`);
    const agentCount = counts.get("agent") ?? 0;
    if (agentCount > 0) parts.push(agentCount === 1 ? "Used subagent" : `Used ${agentCount} subagents`);
    const otherCount = counts.get("other") ?? 0;
    if (otherCount > 0) parts.push(`Used ${plural(otherCount, "tool")}`);

    const icons = activityOrder
      .filter((category) => (counts.get(category) ?? 0) > 0)
      .map(activityIcon);
    const failed = Array.from(failures.values()).reduce((sum, count) => sum + count, 0);

    return {
      icons,
      text: parts.join(", ") || `Used ${plural(children.length, "tool")}`,
      failed
    };
  }
</script>

<div class="pf-chat">
  <div class="pf-chat-thread" bind:this={threadEl}>
    <div class="pf-chat-thread-inner">
      {#if loading && rows.length === 0}
        <div class="state">Loading conversation…</div>
      {:else if rows.length === 0 && !typingLabel}
        <div class="state">No messages in this session yet. Send a prompt to get started.</div>
      {:else}
        {#each distributedRows as row, idx (row.key)}
          {#if row.kind === "user"}
            <div class="pf-msg" data-role="user">
              <div class="pf-msg-avatar">{userInitial}</div>
              <div class="pf-msg-body">
                <div class="pf-msg-meta">
                  <span class="name">{displayUserName}</span>
                  <span class="you-badge">You</span>
                  <span class="time">{formatTime((row.item as MessageTimelineItem & { createdAtMs?: number }).createdAtMs)}</span>
                </div>
                <div class="pf-msg-text">
                  <MessageBody body={row.item.body} onOpenFile={onOpenFileLink} />
                </div>
              </div>
            </div>
          {:else if row.kind === "system"}
            {@const isError = row.item.status === "error" || row.item.meta.includes("error")}
            <div class="pf-msg" data-role="system" data-error={isError}>
              <div class="pf-msg-avatar">{isError ? "err" : "sys"}</div>
              <div class="pf-msg-body">
                {#if isError}
                  <div class="pf-msg-meta">
                    <span class="name">{row.item.title || "Error"}</span>
                  </div>
                {/if}
                <div class="pf-msg-text">
                  <MessageBody body={row.item.body} onOpenFile={onOpenFileLink} />
                </div>
              </div>
            </div>
          {:else}
            <div class="pf-msg" data-role="agent">
              <div class="pf-msg-avatar"><BrandLogo size={26} /></div>
              <div class="pf-msg-body">
                <div class="pf-msg-meta">
                  <span class="name">{engineerName}</span>
                </div>
                {#if row.children.length || row.approvals.length || row.questions.length}
                  <div class="agent-tools">
                    {#if row.children.length}
                      {#if shouldCollapseActivity(row, idx)}
                        {@const activityId = activityGroupId(row, idx)}
                        {@const summary = activitySummary(row.children)}
                        <div class="activity-group" data-expanded={activityExpanded(activityId)}>
                          <button
                            type="button"
                            class="activity-head"
                            onclick={() => toggleActivity(activityId)}
                            aria-expanded={activityExpanded(activityId)}
                          >
                            <span class="activity-chevron">
                              <Icon name={activityExpanded(activityId) ? "chevD" : "chevR"} size={11} />
                            </span>
                            <span class="activity-icons" aria-hidden="true">
                              {#each summary.icons as icon, iconIdx (`${icon}-${iconIdx}`)}
                                <span class="activity-icon">
                                  <Icon name={icon} size={13} />
                                </span>
                              {/each}
                            </span>
                            <span class="activity-copy">
                              <strong>Agent activity</strong>
                              <em>{summary.text}</em>
                            </span>
                            {#if summary.failed > 0}
                              <span class="activity-failed">{summary.failed} failed</span>
                            {/if}
                            <span class="activity-count">{row.children.length}</span>
                          </button>
                          {#if activityExpanded(activityId)}
                            {@const selected = selectedActivityChild(row.children, activityId)}
                            <div class="activity-details">
                              {#each row.children as child, childIdx (child.id)}
                                {#if isThinkingActivity(child)}
                                  <div
                                    class="activity-thought"
                                    style:order={activityActionOrder(childIdx)}
                                  >
                                    <span>Thought for {thoughtDurationLabel(child, row.children, childIdx, row.item)}</span>
                                  </div>
                                {:else if child.kind === "assistant" || child.kind === "command"}
                                  <div
                                    class="activity-message pf-msg-text"
                                    style:order={activityActionOrder(childIdx)}
                                  >
                                    <MessageBody body={(child as MessageTimelineItem).body} onOpenFile={onOpenFileLink} />
                                  </div>
                                {:else}
                                  <button
                                    type="button"
                                    class="activity-action"
                                    class:selected={activityActionSelected(activityId, child)}
                                    style:order={activityActionOrder(childIdx)}
                                    onclick={() => handleActivityActionClick(activityId, child)}
                                    aria-expanded={activityChildSelected(activityId, child.id)}
                                  >
                                    <span class="activity-action-icon">
                                      <Icon name={activityIcon(childActivityCategory(child))} size={13} />
                                    </span>
                                    <span class="activity-action-name">{activityActionName(child)}</span>
                                    <span class="activity-action-arg" title={activityActionArg(child)}>
                                      {activityActionArg(child)}
                                    </span>
                                    <span class="activity-action-status" data-state={activityStatus(child)}>
                                      <span class="dot"></span>{activityStatus(child)}
                                    </span>
                                    <span class="activity-action-chevron" aria-hidden="true">
                                      <Icon
                                        name={activityChildSelected(activityId, child.id) ? "chevD" : "chevR"}
                                        size={11}
                                      />
                                    </span>
                                  </button>
                                {/if}
                              {/each}
                              {#if selected}
                                <div
                                  class="activity-panel"
                                  style:order={activityPanelOrder(selected.idx)}
                                >
                                  {#if selected.child.kind === "tool"}
                                    <ToolCard
                                      item={selected.child as ToolTimelineItem}
                                      sessionId={session?.id ?? null}
                                      defaultCollapsed={false}
                                    />
                                  {:else if selected.child.kind === "diff"}
                                    <DiffCard item={selected.child as DiffTimelineItem} defaultCollapsed={false} />
                                  {:else}
                                    <div class="activity-message pf-msg-text">
                                      <MessageBody body={(selected.child as MessageTimelineItem).body} onOpenFile={onOpenFileLink} />
                                    </div>
                                  {/if}
                                </div>
                              {/if}
                            </div>
                          {/if}
                        </div>
                      {:else}
                        {#each row.children as child (child.id)}
                          {#if child.kind === "tool"}
                            <ToolCard
                              item={child as ToolTimelineItem}
                              sessionId={session?.id ?? null}
                            />
                          {:else if child.kind === "diff"}
                            <DiffCard item={child as DiffTimelineItem} />
                          {:else}
                            <div class="activity-message pf-msg-text">
                              <MessageBody body={(child as MessageTimelineItem).body} onOpenFile={onOpenFileLink} />
                            </div>
                          {/if}
                        {/each}
                      {/if}
                    {/if}
                    {#each row.approvals as p (p.id)}
                      <Approval item={p} disabled={!turnCancelable || isPermissionResolving(p)} onResolve={onResolvePermission} />
                    {/each}
                    {#each row.questions as q (q.id)}
                      <QuestionPrompt item={q} disabled={isQuestionResolving(q)} onResolve={onResolveUserQuestion} />
                    {/each}
                  </div>
                {/if}
                {#if row.item}
                  <div class="pf-msg-text">
                    <MessageBody body={row.item.body} onOpenFile={onOpenFileLink} />
                  </div>
                {/if}
              </div>
            </div>
          {/if}
        {/each}

        {#if typingLabel}
          <div class="pf-msg" data-role="agent" style="opacity: 0.85;">
            <div class="pf-msg-avatar"><BrandLogo size={26} /></div>
            <div class="pf-msg-body">
              <div class="typing">{typingLabel}</div>
            </div>
          </div>
        {/if}
      {/if}
    </div>
  </div>

  <div class="pf-composer-wrap">
    <div class="pf-composer">
      <textarea
        value={draft}
        placeholder={session ? `Reply to ${engineerName}…` : "Select a session to continue"}
        oninput={(event) => updateDraft(event.currentTarget.value)}
        onkeydown={onKeydown}
        disabled={composerDisabled}
      ></textarea>
      <div class="pf-composer-foot">
        <ModelPicker
          snapshot={settingsSnapshot}
          currentProvider={selectedProviderId}
          currentModel={selectedModelId}
          contextKey={session?.id ?? null}
          allowProviderSwitch={allowProviderSwitch}
          disabled={modelPickerDisabled}
          onChange={pickModel}
        />
        <label class="pf-toggle-chip" class:disabled={!fastModeAvailable} title={fastModeAvailable ? "Fast mode" : "Fast mode is not available for this model"}>
          <input type="checkbox" bind:checked={fastMode} disabled={!fastModeAvailable || turnRunning} />
          <Icon name="bolt" size={11} />
          <span>Fast</span>
        </label>
        <label
          class="pf-select-chip"
          class:disabled={!thinkingAvailable}
          title={thinkingAvailable ? "Thinking level" : (thinkingLoadError ?? "Thinking level is not available for this model")}
        >
          <Icon name="cpu" size={11} />
          <select
            bind:value={selectedThinkingOptionId}
            disabled={!thinkingAvailable || turnRunning}
            aria-label="Thinking level"
          >
            <option value="">Default</option>
            {#each thinkingOptions as option (option.id)}
              <option value={option.id}>{thinkingLabel(option.id)}</option>
            {/each}
          </select>
        </label>
        <label class="pf-select-chip" title="Codex permissions">
          <Icon name="shield" size={11} />
          <select bind:value={permissionMode} disabled={turnRunning} aria-label="Codex permissions">
            <option value="read-only">Read only</option>
            <option value="workspace-write">Workspace</option>
            <option value="full-access">Full access</option>
          </select>
        </label>
        <span class="spacer"></span>
        <span class="pf-composer-hint">
          {composerBlockedReason ?? "⏎ to send · ⇧⏎ for newline"}
        </span>
        {#if turnRunning}
          <button
            type="button"
            class="pf-send-btn pf-stop-btn"
            disabled={!canCancelTurn}
            onclick={() => { if (canCancelTurn) onCancelTurn?.(); }}
            aria-label="Stop turn"
            title={canCancelTurn ? "Stop the running agent turn" : "Waiting for turn id"}
          >
            <Icon name="pause2" size={14} />
          </button>
        {:else}
          <button type="button" class="pf-send-btn" disabled={!canSubmitPrompt} onclick={submit} aria-label="Send">
            <Icon name="arrowUp" size={15} />
          </button>
        {/if}
      </div>
    </div>
  </div>
</div>

<style>
  .pf-chat {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    background: var(--background);
  }
  .pf-chat-thread {
    flex: 1;
    overflow-y: auto;
    padding: 24px 0 24px;
  }
  .pf-chat-thread-inner {
    max-width: 820px;
    margin: 0 auto;
    padding: 0 32px;
    display: flex;
    flex-direction: column;
    gap: var(--puffer-row-gap, 14px);
  }
  .pf-composer-wrap {
    border-top: 0;
    background: transparent;
    padding: 0;
    margin-bottom: 14px;
    flex-shrink: 0;
  }
  .pf-composer {
    max-width: 820px;
    margin: 0 auto;
  }
  .pf-composer-foot :global(.picker) {
    min-width: 0;
  }
  .pf-composer-foot :global(.trigger) {
    height: 28px;
    max-width: 220px;
    background: var(--background);
  }
  .pf-toggle-chip,
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
    color: var(--foreground);
    border-color: color-mix(in oklab, var(--accent-foreground) 26%, var(--border));
    background: color-mix(in oklab, var(--accent) 70%, var(--background));
  }
  .pf-toggle-chip.disabled {
    cursor: not-allowed;
    opacity: 0.55;
  }
  .pf-select-chip.disabled {
    opacity: 0.55;
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
  .you-badge {
    border: 1px solid var(--border);
    border-radius: 999px;
    padding: 1px 6px;
    color: var(--muted-foreground);
    font-size: var(--pf-chat-meta-size);
    line-height: 14px;
    font-family: var(--font-sans);
    font-weight: 600;
    background: color-mix(in oklab, var(--muted) 28%, var(--background));
  }
  .agent-tools {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin-top: 10px;
  }
  .agent-tools + .pf-msg-text {
    margin-top: 10px;
  }
  .activity-group {
    max-width: 100%;
    border: 1px solid var(--border);
    border-radius: 10px;
    overflow: hidden;
    background: var(--background);
  }
  .activity-head {
    width: 100%;
    max-width: 100%;
    min-height: 46px;
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto auto;
    align-items: center;
    gap: 10px;
    padding: 8px 12px;
    border: 0;
    border-radius: 0;
    background: color-mix(in oklab, var(--muted) 28%, var(--background));
    color: var(--foreground);
    cursor: pointer;
    font: inherit;
    text-align: left;
  }
  .activity-chevron {
    display: inline-flex;
    color: var(--muted-foreground);
  }
  .activity-icons {
    display: none;
  }
  .activity-icon {
    width: 24px;
    height: 24px;
    border: 1px solid color-mix(in oklab, var(--accent) 22%, var(--border));
    border-radius: 7px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    background: color-mix(in oklab, var(--accent) 10%, var(--background));
    color: var(--muted-foreground);
  }
  .activity-icon + .activity-icon {
    margin-left: -5px;
  }
  .activity-copy {
    min-width: 0;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 2px;
  }
  .activity-copy strong {
    flex: 0 0 auto;
    font-size: var(--pf-chat-detail-size);
    font-weight: 650;
  }
  .activity-copy em {
    display: block;
    max-width: 100%;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--muted-foreground);
    font-style: normal;
    font-size: var(--pf-chat-detail-size);
  }
  .activity-count,
  .activity-failed {
    flex: 0 0 auto;
    border-radius: 999px;
    padding: 2px 8px;
    font-size: var(--pf-chat-meta-size);
    line-height: 16px;
    font-family: var(--font-sans);
    font-weight: 600;
  }
  .activity-count {
    background: var(--background);
    color: var(--muted-foreground);
    border: 1px solid var(--border);
  }
  .activity-failed {
    background: color-mix(in oklab, var(--destructive, #dc2626) 10%, var(--background));
    color: color-mix(in oklab, var(--destructive, #dc2626) 80%, var(--foreground));
    border: 1px solid color-mix(in oklab, var(--destructive, #dc2626) 20%, var(--border));
  }
  .activity-details {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 10px;
    border-top: 1px solid var(--border);
    background: color-mix(in oklab, var(--background) 97%, var(--muted));
  }
  .activity-action {
    min-width: 0;
    min-height: 42px;
    display: grid;
    grid-template-columns: 24px minmax(180px, 0.48fr) minmax(0, 1fr) auto 18px;
    align-items: center;
    gap: 9px;
    padding: 8px 10px;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: var(--background);
    color: var(--foreground);
    cursor: pointer;
    font: inherit;
    font-family: var(--font-sans);
    font-size: var(--pf-chat-detail-size);
    text-align: left;
  }
  .activity-action:hover,
  .activity-action.selected {
    border-color: transparent;
    background: var(--pf-selected-bg-hover);
  }
  .activity-action-icon {
    width: 22px;
    height: 22px;
    border-radius: 5px;
    background: color-mix(in oklab, var(--puffer-accent) 14%, var(--background));
    color: var(--puffer-accent);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
  }
  .activity-action-name {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-weight: 600;
  }
  .activity-action-arg {
    min-width: 0;
    flex: 0 1 auto;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--muted-foreground);
  }
  .activity-action-status {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    color: var(--muted-foreground);
    font-size: var(--pf-chat-meta-size);
    font-family: var(--font-sans);
    justify-self: end;
    flex: 0 0 auto;
  }
  .activity-action-status .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: oklch(0.65 0.18 145);
  }
  .activity-action-status[data-state="failed"] .dot {
    background: oklch(0.62 0.22 25);
  }
  .activity-action-status[data-state="running"] .dot {
    background: var(--puffer-accent);
  }
  .activity-action-chevron {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 18px;
    height: 18px;
    color: var(--muted-foreground);
    flex-shrink: 0;
  }
  .activity-panel {
    min-width: 0;
  }
  .activity-panel :global(.pf-tool) {
    width: 100%;
  }
  .activity-panel :global(.pf-tool > .pf-tool-head) {
    display: none;
  }
  .activity-panel :global(.pf-tool-body) {
    max-height: 360px;
  }
  .typing {
    display: flex;
    align-items: center;
    gap: 8px;
    padding-top: 6px;
    font-size: var(--pf-chat-detail-size);
    color: var(--muted-foreground);
    font-family: var(--font-sans);
  }
  .activity-thought {
    display: flex;
    align-items: center;
    padding: 3px 12px;
    color: var(--muted-foreground);
    font-family: var(--font-sans);
    font-size: var(--pf-chat-meta-size);
    line-height: 1.4;
  }
  .activity-message {
    padding: 6px 12px;
    border: 0;
    border-radius: 0;
    background: transparent;
    font-family: var(--font-sans);
    font-size: var(--pf-chat-text-size);
    line-height: 1.55;
    text-wrap: auto;
  }
  .activity-message :global(p) {
    margin: 0;
  }
  .activity-message :global(code) {
    padding: 0 4px;
    font-size: 0.9em;
  }
  .state {
    text-align: center;
    color: var(--muted-foreground);
    padding: 40px 0;
    font-size: 14px;
  }

  @media (max-width: 720px) {
    .pf-chat-thread-inner { padding: 0 16px; }
    .pf-composer-wrap {
      padding: 0;
      margin-bottom: 10px;
    }
    .activity-head {
      grid-template-columns: auto auto minmax(0, 1fr) auto;
    }
    .activity-copy {
      display: grid;
      gap: 1px;
    }
    .activity-failed {
      display: none;
    }
  }
</style>

/**
 * Per-session chat state machine controller (Svelte 5 runes).
 *
 * This is the desktop `apps/puffer-desktop/src/App.svelte` chat state machine,
 * lifted into a standalone module. The reducer (`handleSessionEvent`) and every
 * helper it calls are ported verbatim — the streaming-event → timeline logic is
 * NOT rewritten. What's cut for momo:
 *
 *   - multi-session background caching (`cacheBackground*` /
 *     `transientConversationStates`): the reducer in desktop branched on
 *     `if (!selectedForEvent)` *before* the `switch (ev.type)` and routed
 *     hidden-session events into a side cache. Momo has one live session per
 *     controller, so that branch is gone — events go straight into the switch.
 *   - sidebar agent coloring (`setLiveSidebarAgentState` family).
 *   - session-group refresh (`refreshGroups`).
 *   - provider auth gating (`providerIsAuthenticated` / `settingsSnapshot`
 *     checks in `submitMessage`): momo lets the daemon pick the default
 *     provider (Task 1 set `default_provider`), so no providerId is forwarded.
 *   - localStorage drafts (`persistPendingSubmittedMessage`).
 *
 * Structure:
 *   - Module-level `Record<sessionId, ChatState>` (`$state`). Momo's router
 *     re-mounts the Agent component on `{#key currentRoute.path}`, so the
 *     state machine must outlive the component — it lives here, keyed by
 *     sessionId, exactly like `lib/chat.svelte.ts`.
 *   - `handleSessionEvent(sessionId, ev)` is a pure reducer: it mutates only
 *     the per-session `ChatState`, never touches the daemon. This makes it
 *     unit-testable by feeding events directly (see
 *     `tests/agent/agent-chat-reducer.spec.ts`).
 *   - `ensureSubscription` / `submitMessage` / `resolve*` / `cancel*` /
 *     `createSessionFromText` are the daemon-touching actions.
 *
 * Svelte 5 scope note: every mutation lives inside a function (event handler
 * or reducer), never in a `$derived` / module-init read scope, so the
 * `state_unsafe_mutation` guard is never tripped (same discipline as
 * `chat.svelte.ts`). Derived views (`combinedTimeline`, `pendingPermissions`,
 * …) are plain getter functions that recompute from `$state` on read.
 */

import {
  cancelTurn,
  createSession,
  loadSessionDetail,
  resolvePermission as daemonResolvePermission,
  resolveUserQuestion as daemonResolveUserQuestion,
  runAgentTurn
} from "./daemonChat";
import { subscribeSessionEvents } from "./sessionEvents";
import type { SessionStreamEvent } from "./sessionEvents";
import {
  currentDaemonClient,
  type ConnectionState
} from "../daemonClient";
import { getProjectCwd, loadProjects, DEFAULT_PROJECT_ID } from "../projectStore.svelte";
import type {
  AgentTurnOptions,
  AskUserQuestionItem,
  PermissionTimelineItem,
  SessionDetail,
  TimelineItem,
  UserQuestionTimelineItem
} from "./types";

const DISMISSED_IDS_CAP = 200;
const SETTLED_TURN_KEYS_CAP = 500;

/* ── Per-session state shape ──────────────────────────────────────── */

/**
 * Everything the desktop App.svelte tracked per *selected* session. Momo
 * keys this by sessionId at module scope so it survives component remounts.
 *
 * `$state`-reactive fields are read via the getter helpers below; the
 * non-reactive bookkeeping (`submittedMessageBaselineIds`, `settledTurnKeys`,
 * `replayTextByTurn`) was non-`$state` in desktop too — it never feeds a
 * template directly, only the reducer's dedupe math.
 */
export interface ChatState {
  sessionId: string;
  sessionDetail: SessionDetail | null;
  submittedMessages: TimelineItem[];
  liveStreamItems: TimelineItem[];
  dismissedPermissionIds: string[];
  dismissedQuestionIds: string[];
  currentTurnId: string | null;
  cancelingTurnId: string | null;
  turnStartedAtMs: number | null;
  turnThinking: boolean;
  turnStatusHint: string | null;
  turnPermissionLookup: Record<string, { turnId: string; requestId: string }>;
  turnQuestionLookup: Record<string, { turnId: string; requestId: string }>;
  // Non-reactive bookkeeping (desktop kept these as plain `let`):
  submittedMessageBaselineIds: Record<string, string[]>;
  settledTurnKeys: Set<string>;
  replayTextByTurn: Record<string, string>;
  liveErrorSeq: number;
}

/** Map of sessionId → live chat state. Module-level so it outlives remounts. */
const chatStates = $state<Record<string, ChatState>>({});

/** Active session-event unsubscribers, keyed by sessionId. */
const subscriptions = new Map<string, () => void>();
/** Per-session load generation guard for `refreshSessionAfterTurn`. */
const sessionLoadGenerations = new Map<string, number>();

function createEmptyState(sessionId: string): ChatState {
  return {
    sessionId,
    sessionDetail: null,
    submittedMessages: [],
    liveStreamItems: [],
    dismissedPermissionIds: [],
    dismissedQuestionIds: [],
    currentTurnId: null,
    cancelingTurnId: null,
    turnStartedAtMs: null,
    turnThinking: false,
    turnStatusHint: null,
    turnPermissionLookup: {},
    turnQuestionLookup: {},
    submittedMessageBaselineIds: {},
    settledTurnKeys: new Set<string>(),
    replayTextByTurn: {},
    liveErrorSeq: 0
  };
}

/**
 * Idempotently ensure a session entry exists, returning it. Safe to call
 * from an event handler or `$effect`. MUST NOT be called from a `$derived` /
 * template expression — it mutates `chatStates` and Svelte 5 forbids that in
 * tracked-read scopes (raises `state_unsafe_mutation`, fatal in WebKit/Tauri).
 */
function ensureState(sessionId: string): ChatState {
  if (!chatStates[sessionId]) {
    chatStates[sessionId] = createEmptyState(sessionId);
  }
  return chatStates[sessionId];
}

/* ── Live id generators (verbatim from desktop) ───────────────────── */

function streamingAssistantId(turnId: string): string {
  return `live-stream-assistant-${turnId}`;
}
function livePermissionId(turnId: string, requestId: string): string {
  return `live-perm-${turnId}-${requestId}`;
}
function liveQuestionId(turnId: string, requestId: string): string {
  return `live-question-${turnId}-${requestId}`;
}
function liveToolId(turnId: string, callId: string): string {
  return `live-tool-${turnId}-${callId}`;
}

function liveItemBelongsToTurn(item: TimelineItem, turnId: string): boolean {
  return (
    item.id === streamingAssistantId(turnId) ||
    item.id === `live-complete-assistant-${turnId}` ||
    item.id === `live-error-turn-error-${turnId}` ||
    item.id.startsWith(`live-tool-${turnId}-`) ||
    item.id.startsWith(`live-gate-${turnId}-`) ||
    item.id.startsWith(`live-perm-${turnId}-`) ||
    item.id.startsWith(`live-question-${turnId}-`)
  );
}

function withoutLiveItemsForTurn(items: TimelineItem[], turnId: string): TimelineItem[] {
  return items.filter((item) => !liveItemBelongsToTurn(item, turnId));
}

/* ── Turn settle bookkeeping (verbatim from desktop) ──────────────── */

function turnKey(sessionId: string, turnId: string): string {
  return `${sessionId}\u0000${turnId}`;
}

function isTurnSettled(state: ChatState, turnId: string): boolean {
  return state.settledTurnKeys.has(turnKey(state.sessionId, turnId));
}

function rememberSettledTurn(state: ChatState, turnId: string): void {
  state.settledTurnKeys.add(turnKey(state.sessionId, turnId));
  if (state.settledTurnKeys.size > SETTLED_TURN_KEYS_CAP) {
    const overflow = state.settledTurnKeys.size - SETTLED_TURN_KEYS_CAP;
    let removed = 0;
    for (const key of state.settledTurnKeys) {
      state.settledTurnKeys.delete(key);
      removed += 1;
      if (removed >= overflow) break;
    }
  }
}

function forgetSettledTurn(state: ChatState, turnId: string): void {
  state.settledTurnKeys.delete(turnKey(state.sessionId, turnId));
}

function shouldIgnoreTurnEvent(state: ChatState, turnId: string): boolean {
  if (isTurnSettled(state, turnId)) return true;
  return state.currentTurnId !== null && state.currentTurnId !== turnId;
}

function markTurnActive(state: ChatState, turnId: string): void {
  if (state.currentTurnId !== null && state.currentTurnId !== turnId) {
    state.cancelingTurnId = null;
  }
  state.currentTurnId = turnId;
  forgetSettledTurn(state, turnId);
}

function markTurnSettled(state: ChatState, turnId: string): void {
  rememberSettledTurn(state, turnId);
  const { [turnId]: _drop, ...rest } = state.replayTextByTurn;
  state.replayTextByTurn = rest;
  state.turnPermissionLookup = Object.fromEntries(
    Object.entries(state.turnPermissionLookup).filter(([, mapping]) => mapping.turnId !== turnId)
  );
  state.turnQuestionLookup = Object.fromEntries(
    Object.entries(state.turnQuestionLookup).filter(([, mapping]) => mapping.turnId !== turnId)
  );
  if (state.cancelingTurnId === turnId) {
    state.cancelingTurnId = null;
  }
  if (state.currentTurnId === turnId) {
    state.currentTurnId = null;
  }
}

/* ── Live item append / streaming (verbatim from desktop) ─────────── */

function appendLive(state: ChatState, item: TimelineItem): void {
  const existingIdx = state.liveStreamItems.findIndex((existing) => existing.id === item.id);
  if (existingIdx >= 0) {
    state.liveStreamItems = [
      ...state.liveStreamItems.slice(0, existingIdx),
      item,
      ...state.liveStreamItems.slice(existingIdx + 1)
    ];
    return;
  }
  state.liveStreamItems = [...state.liveStreamItems, item];
}

function appendAgentError(state: ChatState, title: string, body: string, code: string): void {
  const trimmed = body.trim() || "Unknown error.";
  state.liveErrorSeq += 1;
  appendLive(state, {
    id: `live-error-${code}-${Date.now()}-${state.liveErrorSeq}`,
    kind: "system",
    title,
    summary: trimmed,
    body: trimmed,
    meta: ["error", code],
    status: "error"
  });
}

function upsertStreamingAssistant(state: ChatState, turnId: string, delta: string): void {
  const id = streamingAssistantId(turnId);
  const existingIdx = state.liveStreamItems.findIndex(
    (item) => item.id === id && item.kind === "assistant"
  );
  if (existingIdx >= 0) {
    const existing = state.liveStreamItems[existingIdx];
    const updated = { ...existing, body: existing.body + delta, summary: existing.body + delta };
    state.liveStreamItems = [
      ...state.liveStreamItems.slice(0, existingIdx),
      updated,
      ...state.liveStreamItems.slice(existingIdx + 1)
    ];
  } else {
    appendLive(state, {
      id,
      kind: "assistant",
      title: "Assistant",
      summary: delta,
      body: delta,
      meta: []
    });
  }
}

/**
 * Replay-dedupe for assistant text. On reconnect the daemon re-streams a
 * turn's deltas with `replay: true`; this collapses what's already in the
 * streaming bubble so text isn't doubled. Verbatim from desktop.
 */
function replaySafeDelta(state: ChatState, turnId: string, delta: string): string {
  const replayText = `${state.replayTextByTurn[turnId] ?? ""}${delta}`;
  state.replayTextByTurn = { ...state.replayTextByTurn, [turnId]: replayText };
  const currentItem = state.liveStreamItems.find(
    (item) => item.id === streamingAssistantId(turnId)
  );
  if (!currentItem || currentItem.kind !== "assistant") {
    return delta;
  }
  const current = currentItem.body;
  if (current.startsWith(replayText)) return "";
  if (replayText.startsWith(current)) return replayText.slice(current.length);
  return delta;
}

/* ── Dedupe family (verbatim from desktop) ────────────────────────── */

function timelineItemBody(item: TimelineItem): string {
  return "body" in item && typeof item.body === "string" ? item.body : "";
}

function timelineHasBody(items: TimelineItem[], kind: TimelineItem["kind"], body: string): boolean {
  const trimmed = body.trim();
  if (!trimmed) return true;
  return items.some((item) => item.kind === kind && timelineItemBody(item).includes(trimmed));
}

function transientMessageSignature(item: TimelineItem): string | null {
  if (item.kind !== "user" && item.kind !== "assistant") return null;
  const body = timelineItemBody(item).trim();
  if (!body) return null;
  return `${item.kind}:${body}`;
}

function stableJsonText(value: unknown): string {
  if (Array.isArray(value)) {
    return `[${value.map(stableJsonText).join(",")}]`;
  }
  if (value && typeof value === "object") {
    const record = value as Record<string, unknown>;
    return `{${Object.keys(record)
      .sort()
      .map((key) => `${JSON.stringify(key)}:${stableJsonText(record[key])}`)
      .join(",")}}`;
  }
  return JSON.stringify(value) ?? "undefined";
}

function normalizedToolInput(item: TimelineItem): string | null {
  if (item.kind !== "tool") return null;
  const raw = item.input?.trim();
  if (raw) {
    try {
      return stableJsonText(JSON.parse(raw) as unknown);
    } catch {
      return raw;
    }
  }
  return item.inputJson ? stableJsonText(item.inputJson) : null;
}

function transientToolSignature(item: TimelineItem): string | null {
  if (item.kind !== "tool") return null;
  const input = normalizedToolInput(item);
  return input ? `${item.toolName}:${input}` : null;
}

function normalizedGateKey(value: string | null | undefined): string {
  return (value ?? "").toLowerCase().replace(/[^a-z0-9]/g, "");
}

function gateBodyField(item: TimelineItem, name: string): string | null {
  if (item.kind !== "system") return null;
  const target = normalizedGateKey(name);
  for (const line of item.body.split("\n")) {
    const idx = line.indexOf(":");
    if (idx < 0) continue;
    if (normalizedGateKey(line.slice(0, idx)) !== target) continue;
    const value = line.slice(idx + 1).trim();
    return value || null;
  }
  return null;
}

function transientGateSignature(item: TimelineItem): string | null {
  if (item.kind !== "system") return null;
  const isGate =
    item.title === "Verified Skill Gate" ||
    item.meta.includes("verified skill") ||
    item.body.trim().startsWith("Verified Skill Gate");
  if (!isGate) return null;
  const event =
    gateBodyField(item, "event") ??
    item.meta.find((value) => normalizedGateKey(value) !== "verifiedskill") ??
    null;
  const hostTool = gateBodyField(item, "host_tool") ?? gateBodyField(item, "hosttool");
  const concreteTool = gateBodyField(item, "concrete_tool") ?? gateBodyField(item, "concretetool");
  const reason = gateBodyField(item, "reason");
  if (!event && !hostTool && !concreteTool && !reason) return null;
  return stableJsonText({
    event: event ? normalizedGateKey(event) : null,
    hostTool,
    concreteTool,
    reason
  });
}

function wasPersistedBeforeSubmit(
  state: ChatState,
  pendingId: string,
  persistedId: string
): boolean {
  return state.submittedMessageBaselineIds[pendingId]?.includes(persistedId) ?? false;
}

function reuseTransientMessageIds(
  state: ChatState,
  persisted: TimelineItem[],
  transient: TimelineItem[]
): TimelineItem[] {
  const transientIds = new Map<string, string[]>();
  for (let index = transient.length - 1; index >= 0; index -= 1) {
    const item = transient[index];
    const signature = transientMessageSignature(item);
    if (!signature) continue;
    transientIds.set(signature, [...(transientIds.get(signature) ?? []), item.id]);
  }
  const keyed = [...persisted];
  for (let index = keyed.length - 1; index >= 0; index -= 1) {
    const item = keyed[index];
    const signature = transientMessageSignature(item);
    const candidates = signature ? transientIds.get(signature) : null;
    const candidateIndex =
      candidates?.findIndex((candidate) => !wasPersistedBeforeSubmit(state, candidate, item.id)) ?? -1;
    const replacement =
      candidates && candidateIndex >= 0 ? candidates.splice(candidateIndex, 1)[0] : null;
    if (replacement) keyed[index] = { ...item, id: replacement };
  }
  return keyed;
}

function timelineItemCreatedAtMs(item: TimelineItem): number | null {
  return typeof item.createdAtMs === "number" && Number.isFinite(item.createdAtMs)
    ? item.createdAtMs
    : null;
}

function transientTimestampsMatch(persisted: TimelineItem, pending: TimelineItem): boolean {
  const persistedAt = timelineItemCreatedAtMs(persisted);
  const pendingAt = timelineItemCreatedAtMs(pending);
  if (persistedAt === null || pendingAt === null) return true;
  return Math.abs(persistedAt - pendingAt) <= 5 * 60 * 1000;
}

function timelineHasTransientMatch(
  state: ChatState,
  items: TimelineItem[],
  pending: TimelineItem
): boolean {
  const gateSignature = transientGateSignature(pending);
  if (gateSignature) {
    return items.some(
      (item) =>
        !wasPersistedBeforeSubmit(state, pending.id, item.id) &&
        transientGateSignature(item) === gateSignature
    );
  }
  const body = timelineItemBody(pending).trim();
  if (!body) {
    const toolSignature = transientToolSignature(pending);
    if (toolSignature) {
      return items.some(
        (item) =>
          !wasPersistedBeforeSubmit(state, pending.id, item.id) &&
          transientToolSignature(item) === toolSignature
      );
    }
    return items.some(
      (item) =>
        !wasPersistedBeforeSubmit(state, pending.id, item.id) &&
        item.kind === pending.kind &&
        item.id === pending.id
    );
  }
  return items.some(
    (item) =>
      !wasPersistedBeforeSubmit(state, pending.id, item.id) &&
      item.kind === pending.kind &&
      ((item.id && item.id === pending.id) ||
        (timelineItemBody(item).trim() === body && transientTimestampsMatch(item, pending)))
  );
}

function stillMissingFromPersisted(
  state: ChatState,
  items: TimelineItem[],
  pending: TimelineItem[]
): TimelineItem[] {
  return pending.filter((item) => !timelineHasTransientMatch(state, items, item));
}

function submittedStillMissingFromPersisted(
  state: ChatState,
  items: TimelineItem[],
  pending: TimelineItem[]
): TimelineItem[] {
  const missing = stillMissingFromPersisted(state, items, pending);
  const missingIds = new Set(missing.map((item) => item.id));
  let nextBaselineIds = state.submittedMessageBaselineIds;
  for (const item of pending) {
    if (missingIds.has(item.id)) continue;
    if (nextBaselineIds === state.submittedMessageBaselineIds) {
      nextBaselineIds = { ...state.submittedMessageBaselineIds };
    }
    delete nextBaselineIds[item.id];
  }
  if (nextBaselineIds !== state.submittedMessageBaselineIds) {
    state.submittedMessageBaselineIds = nextBaselineIds;
  }
  return missing;
}

function withCompletionAssistantFallback(
  items: TimelineItem[],
  text: string,
  turnId: string
): TimelineItem[] {
  const trimmed = text.trim();
  if (!trimmed) return items;
  const streamingId = streamingAssistantId(turnId);
  const streamingIndex = items.findIndex(
    (item) => item.kind === "assistant" && item.id === streamingId
  );
  if (streamingIndex >= 0) {
    const existing = items[streamingIndex];
    if (existing.kind !== "assistant" || existing.body.trim() === trimmed) return items;
    return [
      ...items.slice(0, streamingIndex),
      { ...existing, summary: trimmed, body: trimmed },
      ...items.slice(streamingIndex + 1)
    ];
  }
  if (timelineHasBody(items, "assistant", trimmed)) return items;
  return [
    ...items,
    {
      id: `live-complete-assistant-${turnId}`,
      kind: "assistant",
      title: "Assistant",
      summary: trimmed,
      body: trimmed,
      meta: []
    }
  ];
}

/* ── lambda-gate rendering helpers (verbatim from desktop) ────────── */

function lambdaGateSummary(ev: Extract<SessionStreamEvent, { type: "lambda-gate" }>): string {
  if (ev.gateEvent === "host_call_admitted") {
    return `Gate admitted ${ev.hostTool ?? "host call"} for ${ev.concreteTool ?? ev.toolId}`;
  }
  if (ev.gateEvent === "host_call_committed") {
    return `Gate committed ${ev.hostTool ?? "host call"} through ${ev.concreteTool ?? ev.toolId}`;
  }
  if (ev.gateEvent === "gate_rejected") {
    return `Gate rejected ${ev.toolId}: ${ev.reason ?? "call did not satisfy the Verified Skill gate"}`;
  }
  return `Gate event: ${ev.gateEvent}`;
}

function gateJson(value: unknown): string | null {
  if (value === null || value === undefined) return null;
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}

function lambdaGateBody(ev: Extract<SessionStreamEvent, { type: "lambda-gate" }>): string {
  const hostTool = ev.hostTool ?? "host call";
  const concreteTool = ev.concreteTool ?? ev.toolId;
  const hostArgs = gateJson(ev.hostArgs);
  const concreteInput = gateJson(ev.concreteInput);
  const registeredFacts = gateJson(ev.registeredFacts);
  const lines = ["Verified Skill Gate", `event: ${ev.gateEvent}`];
  if (ev.gateEvent === "gate_rejected") {
    lines.push("check: Compared the attempted tool call against the active LambdaHostCall gate.");
    if (ev.reason) lines.push(`reason: ${ev.reason}`);
    if (ev.retryTool) lines.push(`retry_tool: ${ev.retryTool}`);
    lines.push(
      "confirmation: Puffer rejected this call before committing the Lambda gate. Retry by opening LambdaHostCall with the formal host tool, host args, concrete tool, and exact concrete input."
    );
    return lines.join("\n");
  }
  if (ev.gateEvent === "host_call_committed") {
    lines.push(
      `check: Confirmed the concrete ${concreteTool} call matched the pending LambdaHostCall bridge for formal host tool ${hostTool}.`
    );
  } else {
    lines.push(
      `check: Verified LambdaHostCall may bind formal host tool ${hostTool} to concrete tool ${concreteTool}, and recorded the exact concrete input that must run next.`
    );
  }
  lines.push(`host_tool: ${hostTool}`);
  if (hostArgs) lines.push(`host_args: ${hostArgs}`);
  lines.push(`concrete_tool: ${concreteTool}`);
  if (concreteInput) lines.push(`concrete_input: ${concreteInput}`);
  if (registeredFacts) lines.push(`registered_facts: ${registeredFacts}`);
  lines.push(
    ev.gateEvent === "host_call_committed"
      ? "confirmation: Puffer observed the declared concrete tool succeed, then committed the Lambda gate and any registered facts."
      : "confirmation: Compare concrete_tool with the next activity row's tool name and concrete_input with that tool's input. Puffer only allows the next concrete call when both match exactly."
  );
  return lines.join("\n");
}

/* ── parsing helpers (verbatim from desktop) ──────────────────────── */

function safeParseJson(text: string): Record<string, unknown> | null {
  try {
    const v = JSON.parse(text);
    return typeof v === "object" && v !== null ? (v as Record<string, unknown>) : null;
  } catch {
    return null;
  }
}

function normalizeUserQuestions(raw: unknown[]): AskUserQuestionItem[] {
  return raw
    .map((item) => (typeof item === "object" && item !== null ? (item as Record<string, unknown>) : null))
    .filter((item): item is Record<string, unknown> => item !== null)
    .map((item) => ({
      question: typeof item.question === "string" ? item.question : "Question",
      header: typeof item.header === "string" ? item.header : "Question",
      type: item.type === "input" ? ("input" as const) : ("choice" as const),
      multiSelect: item.multiSelect === true,
      searchable: item.searchable === true,
      options: Array.isArray(item.options)
        ? item.options
            .map((option) =>
              typeof option === "object" && option !== null
                ? (option as Record<string, unknown>)
                : null
            )
            .filter((option): option is Record<string, unknown> => option !== null)
            .map((option) => ({
              label: typeof option.label === "string" ? option.label : "Option",
              description: typeof option.description === "string" ? option.description : "",
              preview: typeof option.preview === "string" ? option.preview : null
            }))
        : []
    }));
}

function mapPermissionAction(
  choice: string
): "allow_once" | "allow_session" | "allow_all_session" | "deny" {
  const n = choice.toLowerCase();
  if (n.includes("always")) return "allow_session";
  if (n.includes("session")) return "allow_session";
  if (n.includes("deny") || n.includes("never")) return "deny";
  return "allow_once";
}

/* ── error formatting ─────────────────────────────────────────────── */

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/* ── dismiss bookkeeping (verbatim from desktop, sans session scoping) ─ */

function dismissPermissionId(state: ChatState, permissionId: string): void {
  if (state.dismissedPermissionIds.includes(permissionId)) return;
  state.dismissedPermissionIds = [...state.dismissedPermissionIds, permissionId].slice(
    -DISMISSED_IDS_CAP
  );
}

function dismissQuestionId(state: ChatState, questionId: string): void {
  if (state.dismissedQuestionIds.includes(questionId)) return;
  state.dismissedQuestionIds = [...state.dismissedQuestionIds, questionId].slice(-DISMISSED_IDS_CAP);
}

/* ── after-turn transcript refresh (verbatim from desktop) ────────── */

async function refreshSessionAfterTurn(
  state: ChatState,
  completedTurnId: string,
  liveItemsAtCompletion: TimelineItem[],
  submittedAtCompletion: TimelineItem[],
  preservedErrorItems: TimelineItem[],
  turnEndedWithError: boolean
): Promise<void> {
  const loadGeneration = (sessionLoadGenerations.get(state.sessionId) ?? 0) + 1;
  sessionLoadGenerations.set(state.sessionId, loadGeneration);
  try {
    const detail = await loadSessionDetail(state.sessionId);
    if (loadGeneration !== sessionLoadGenerations.get(state.sessionId)) return;
    if (state.currentTurnId !== null && state.currentTurnId !== completedTurnId) return;
    const persistedTimeline = reuseTransientMessageIds(state, detail.timeline, [
      ...submittedAtCompletion,
      ...liveItemsAtCompletion
    ]);
    state.sessionDetail = { ...detail, timeline: persistedTimeline };
    if (turnEndedWithError) {
      state.liveStreamItems = stillMissingFromPersisted(state, persistedTimeline, preservedErrorItems);
      state.submittedMessages = submittedStillMissingFromPersisted(
        state,
        persistedTimeline,
        submittedAtCompletion
      );
      return;
    }
    state.liveStreamItems = stillMissingFromPersisted(state, persistedTimeline, liveItemsAtCompletion);
    state.submittedMessages = submittedStillMissingFromPersisted(
      state,
      persistedTimeline,
      submittedAtCompletion
    );
  } catch (error) {
    if (loadGeneration !== sessionLoadGenerations.get(state.sessionId)) return;
    appendAgentError(state, "Conversation load failed", errorText(error), "load-session");
  }
}

/* ── reset (verbatim from desktop) ────────────────────────────────── */

function resetLiveTurnState(state: ChatState): void {
  state.submittedMessages = [];
  state.submittedMessageBaselineIds = {};
  state.liveStreamItems = [];
  state.replayTextByTurn = {};
  state.turnPermissionLookup = {};
  state.turnQuestionLookup = {};
  state.currentTurnId = null;
  state.cancelingTurnId = null;
  state.turnStartedAtMs = null;
  state.turnThinking = false;
  state.turnStatusHint = null;
}

/* ── The reducer ──────────────────────────────────────────────────── */

/**
 * Pure state reducer for one session's stream events. Mutates only the
 * per-session `ChatState`; never calls the daemon. This is the `switch`
 * body lifted from desktop `handleSessionEvent` — the desktop pre-switch
 * `if (!selectedForEvent)` cacheBackground routing is removed because momo
 * has one live session per controller.
 *
 * Still honors the desktop "ignore stale turn" guards: a settled turn's
 * late events are dropped, and a second concurrent turn's events are ignored
 * while another is in flight (mirrors `shouldIgnoreTurnEvent`).
 */
export function handleSessionEvent(sessionId: string, ev: SessionStreamEvent): void {
  const state = ensureState(sessionId);
  if (isTurnSettled(state, ev.turnId)) return;
  if (shouldIgnoreTurnEvent(state, ev.turnId)) {
    if (ev.type === "turn-complete" || ev.type === "turn-error") {
      markTurnSettled(state, ev.turnId);
    }
    return;
  }
  switch (ev.type) {
    case "turn-start":
      markTurnActive(state, ev.turnId);
      state.turnStartedAtMs = Date.now();
      state.turnThinking = true;
      state.turnStatusHint = "Thinking";
      if (!ev.replay) {
        const { [ev.turnId]: _drop, ...rest } = state.replayTextByTurn;
        state.replayTextByTurn = rest;
      }
      break;
    case "thinking-delta":
      markTurnActive(state, ev.turnId);
      state.turnThinking = true;
      state.turnStatusHint = "Thinking";
      break;
    case "text-delta":
      markTurnActive(state, ev.turnId);
      state.turnThinking = false;
      state.turnStatusHint = null;
      {
        const delta = ev.replay ? replaySafeDelta(state, ev.turnId, ev.delta) : ev.delta;
        if (delta) upsertStreamingAssistant(state, ev.turnId, delta);
      }
      break;
    case "tool-calls-requested":
      markTurnActive(state, ev.turnId);
      state.turnThinking = false;
      state.turnStatusHint = "Running tools";
      for (const req of ev.requests) {
        const id = liveToolId(ev.turnId, req.callId);
        if (state.liveStreamItems.some((x) => x.id === id)) continue;
        appendLive(state, {
          id,
          kind: "tool",
          title: req.toolId,
          summary: `${req.toolId} · running`,
          body: "",
          meta: [],
          toolName: req.toolId,
          status: "running",
          input: req.input,
          output: "",
          inputJson: safeParseJson(req.input)
        });
      }
      break;
    case "tool-invocations":
      markTurnActive(state, ev.turnId);
      state.turnThinking = false;
      state.turnStatusHint = null;
      for (const inv of ev.invocations) {
        const id = liveToolId(ev.turnId, inv.callId);
        const existingIdx = state.liveStreamItems.findIndex((x) => x.id === id);
        const payload: TimelineItem = {
          id,
          kind: "tool",
          title: inv.toolId,
          summary: `${inv.toolId} · ${inv.success ? "success" : "error"}`,
          body: inv.output,
          meta: [],
          toolName: inv.toolId,
          status: inv.success ? "success" : "error",
          input: inv.input,
          output: inv.output,
          inputJson: safeParseJson(inv.input),
          metadata: inv.metadata
        };
        if (existingIdx >= 0) {
          state.liveStreamItems = [
            ...state.liveStreamItems.slice(0, existingIdx),
            payload,
            ...state.liveStreamItems.slice(existingIdx + 1)
          ];
        } else {
          appendLive(state, payload);
        }
      }
      break;
    case "lambda-gate":
      markTurnActive(state, ev.turnId);
      state.turnThinking = false;
      state.turnStatusHint = ev.gateEvent === "gate_rejected" ? "Gate rejected" : "Gate checked";
      {
        const id = `live-gate-${ev.turnId}-${ev.callId}-${ev.gateEvent}`;
        const payload: TimelineItem = {
          id,
          kind: "system",
          title: "Verified Skill Gate",
          summary: lambdaGateSummary(ev),
          body: lambdaGateBody(ev),
          meta: ["verified skill", ev.gateEvent],
          status: ev.gateEvent === "gate_rejected" ? "error" : "success",
          actor: ev.actor ?? null
        };
        const existingIdx = state.liveStreamItems.findIndex((item) => item.id === id);
        if (existingIdx >= 0) {
          state.liveStreamItems = [
            ...state.liveStreamItems.slice(0, existingIdx),
            payload,
            ...state.liveStreamItems.slice(existingIdx + 1)
          ];
        } else {
          appendLive(state, payload);
        }
      }
      break;
    case "plan-updated":
      state.currentTurnId = ev.turnId;
      state.turnThinking = false;
      state.turnStatusHint = "Planning";
      break;
    case "plan-completed":
      state.currentTurnId = ev.turnId;
      state.turnThinking = false;
      state.turnStatusHint = "Plan ready";
      break;
    case "reflection-checkpoint":
      markTurnActive(state, ev.turnId);
      state.turnThinking = true;
      state.turnStatusHint = "Thinking";
      break;
    case "retry-attempt":
      markTurnActive(state, ev.turnId);
      state.turnThinking = true;
      state.turnStatusHint = `Retrying ${ev.attempt}/${ev.maxAttempts}`;
      break;
    case "usage":
      markTurnActive(state, ev.turnId);
      break;
    case "permission-request": {
      markTurnActive(state, ev.turnId);
      state.turnThinking = false;
      state.turnStatusHint = "Awaiting approval";
      const id = livePermissionId(ev.turnId, ev.requestId);
      const choices = ev.browser
        ? ["Approve once", "Always allow browser context", "Deny"]
        : ["Approve once", "Always allow", "Deny"];
      appendLive(state, {
        id,
        kind: "permission",
        title: `Permission · ${ev.toolId}`,
        summary: ev.summary,
        body: ev.reason ?? ev.summary,
        meta: [],
        toolName: ev.toolId,
        status: "pending",
        permissionDialog: {
          state: "pending",
          reason: ev.reason ?? ev.summary,
          summary: ev.summary,
          inputText: null,
          toolName: ev.toolId,
          choices
        },
        scopeLabel: null,
        choices
      });
      state.turnPermissionLookup = {
        ...state.turnPermissionLookup,
        [id]: { turnId: ev.turnId, requestId: ev.requestId }
      };
      break;
    }
    case "user-question-request": {
      markTurnActive(state, ev.turnId);
      state.turnThinking = false;
      state.turnStatusHint = "Waiting for answer";
      const id = liveQuestionId(ev.turnId, ev.requestId);
      const questions = normalizeUserQuestions(ev.questions);
      appendLive(state, {
        id,
        kind: "question",
        title: "Question",
        summary: questions.map((q) => q.question).join("\n"),
        body: "",
        meta: [],
        status: "pending",
        questions
      });
      state.turnQuestionLookup = {
        ...state.turnQuestionLookup,
        [id]: { turnId: ev.turnId, requestId: ev.requestId }
      };
      break;
    }
    case "turn-complete":
    case "turn-error": {
      const wasCancelingTurn = state.cancelingTurnId === ev.turnId;
      markTurnSettled(state, ev.turnId);
      state.turnStartedAtMs = null;
      state.turnThinking = false;
      state.turnStatusHint = null;
      if (ev.type === "turn-error") {
        const detail = ev.error?.trim() || "Unknown agent error.";
        appendAgentError(state, "Agent error", detail, "turn-error");
      }
      const completionText = ev.type === "turn-complete" ? ev.assistantText : "";
      const liveItemsAtCompletion = wasCancelingTurn
        ? withoutLiveItemsForTurn(state.liveStreamItems, ev.turnId)
        : withCompletionAssistantFallback(state.liveStreamItems, completionText, ev.turnId);
      const submittedAtCompletion = state.submittedMessages;
      state.liveStreamItems = stillMissingFromPersisted(
        state,
        [...(state.sessionDetail?.timeline ?? []), ...submittedAtCompletion],
        liveItemsAtCompletion
      );
      const turnEndedWithError = ev.type === "turn-error";
      const preservedErrorItems = liveItemsAtCompletion.filter(
        (item) => item.kind === "system" && item.meta.includes("error")
      );
      void refreshSessionAfterTurn(
        state,
        ev.turnId,
        liveItemsAtCompletion,
        submittedAtCompletion,
        preservedErrorItems,
        turnEndedWithError
      );
      break;
    }
  }
}

/* ── Derived view getters ─────────────────────────────────────────── */

function renderedSubmittedMessages(state: ChatState): TimelineItem[] {
  return stillMissingFromPersisted(
    state,
    state.sessionDetail?.timeline ?? [],
    state.submittedMessages
  );
}

function renderedLiveStreamItems(state: ChatState, submitted: TimelineItem[]): TimelineItem[] {
  return stillMissingFromPersisted(
    state,
    [...(state.sessionDetail?.timeline ?? []), ...submitted],
    state.liveStreamItems
  );
}

function isPendingPermission(item: PermissionTimelineItem): boolean {
  const status = item.status?.toLowerCase() ?? "";
  const dialogState = item.permissionDialog.state?.toLowerCase() ?? "";
  return status === "pending" || dialogState === "pending";
}

/* ── Connection state (derived from the shared daemon client) ─────── */

/**
 * Live daemon connection state. Desktop tracked this in `connectionState`
 * (used as a `submitMessage` pre-send guard); momo derives it from the shared
 * `DaemonClient`. Returns "idle" before a client exists. The controller
 * subscribes via `onConnectionChange` in `ensureSubscription` so the value
 * stays current.
 */
const connectionStateBox = $state<{ value: ConnectionState }>({ value: "idle" });
let connectionUnsub: (() => void) | null = null;

function trackConnectionState(): void {
  if (connectionUnsub) return;
  const client = currentDaemonClient();
  if (!client) return;
  connectionUnsub = client.onConnectionChange((s) => {
    connectionStateBox.value = s;
  });
}

/* ── Subscription ─────────────────────────────────────────────────── */

/**
 * Subscribe this session's daemon events into the reducer. Idempotent per
 * sessionId. The handler is the pure reducer, so the daemon plumbing and the
 * state logic stay decoupled (the reducer is unit-tested without any daemon).
 */
export function ensureSubscription(sessionId: string): void {
  ensureState(sessionId);
  trackConnectionState();
  if (subscriptions.has(sessionId)) return;
  // Placeholder until the async subscribe resolves; prevents a double-subscribe
  // race if ensureSubscription is called twice before the await lands.
  subscriptions.set(sessionId, () => {});
  void subscribeSessionEvents(sessionId, (ev) => handleSessionEvent(sessionId, ev)).then(
    (unsub) => {
      // If the entry was torn down meanwhile, immediately dispose.
      if (subscriptions.get(sessionId)) {
        subscriptions.set(sessionId, unsub);
      } else {
        unsub();
      }
    }
  );
}

/* ── Controller factory ───────────────────────────────────────────── */

export interface ChatController {
  readonly sessionId: string;
  /** Reactive read of this session's state (creates the entry on first read). */
  state(): ChatState;
  combinedTimeline(): TimelineItem[];
  pendingPermissions(): PermissionTimelineItem[];
  pendingQuestions(): UserQuestionTimelineItem[];
  turnRunning(): boolean;
  connectionState(): ConnectionState;
  ensureSubscription(): void;
  /** Load (or reload) the persisted transcript for this session. */
  loadDetail(): Promise<void>;
  submitMessage(message: string, options?: AgentTurnOptions): Promise<boolean>;
  appendUserMessage(text: string): Promise<boolean>;
  resolvePermission(permissionId: string, choice: string): Promise<void>;
  resolveUserQuestion(
    questionId: string,
    answers: Record<string, string | string[]>,
    annotations?: Record<string, Record<string, string>>
  ): Promise<void>;
  cancelCurrentTurn(): Promise<void>;
  reset(): void;
  /** Feed an event directly into the reducer (for tests / replay). */
  handleSessionEvent(ev: SessionStreamEvent): void;
}

/**
 * Build a controller bound to one sessionId. The underlying state lives in
 * the module-level `chatStates` record, so a controller is a thin, cheap
 * façade — recreating it (e.g. after a route remount) reattaches to the same
 * state. All reads go through getters that recompute from `$state`.
 */
export function createController(sessionId: string): ChatController {
  ensureState(sessionId);

  function get(): ChatState {
    return ensureState(sessionId);
  }

  return {
    sessionId,

    state(): ChatState {
      return get();
    },

    combinedTimeline(): TimelineItem[] {
      const s = get();
      const submitted = renderedSubmittedMessages(s);
      const live = renderedLiveStreamItems(s, submitted);
      return [...(s.sessionDetail?.timeline ?? []), ...submitted, ...live];
    },

    pendingPermissions(): PermissionTimelineItem[] {
      const s = get();
      const submitted = renderedSubmittedMessages(s);
      const live = renderedLiveStreamItems(s, submitted);
      const combined = [...(s.sessionDetail?.timeline ?? []), ...submitted, ...live];
      return combined.filter(
        (t): t is PermissionTimelineItem =>
          t.kind === "permission" &&
          isPendingPermission(t) &&
          !s.dismissedPermissionIds.includes(t.id)
      );
    },

    pendingQuestions(): UserQuestionTimelineItem[] {
      const s = get();
      const submitted = renderedSubmittedMessages(s);
      const live = renderedLiveStreamItems(s, submitted);
      const combined = [...(s.sessionDetail?.timeline ?? []), ...submitted, ...live];
      return combined.filter(
        (t): t is UserQuestionTimelineItem =>
          t.kind === "question" &&
          t.status === "pending" &&
          !s.dismissedQuestionIds.includes(t.id)
      );
    },

    turnRunning(): boolean {
      const s = get();
      return s.currentTurnId !== null || s.turnStartedAtMs !== null;
    },

    connectionState(): ConnectionState {
      return connectionStateBox.value;
    },

    ensureSubscription(): void {
      ensureSubscription(sessionId);
    },

    async loadDetail(): Promise<void> {
      const s = get();
      try {
        const detail = await loadSessionDetail(sessionId);
        // Re-key persisted ids against any in-flight optimistic items so the
        // reload doesn't duplicate a just-sent user message.
        const persistedTimeline = reuseTransientMessageIds(s, detail.timeline, [
          ...s.submittedMessages,
          ...s.liveStreamItems
        ]);
        s.sessionDetail = { ...detail, timeline: persistedTimeline };
        s.submittedMessages = submittedStillMissingFromPersisted(
          s,
          persistedTimeline,
          s.submittedMessages
        );
        s.liveStreamItems = stillMissingFromPersisted(s, persistedTimeline, s.liveStreamItems);
      } catch (error) {
        appendAgentError(s, "Conversation load failed", errorText(error), "load-session");
      }
    },

    async submitMessage(message: string, options: AgentTurnOptions = {}): Promise<boolean> {
      return submitMessageImpl(sessionId, message, options);
    },

    async appendUserMessage(text: string): Promise<boolean> {
      const trimmed = text.trim();
      if (!trimmed) return false;
      return submitMessageImpl(sessionId, trimmed);
    },

    async resolvePermission(permissionId: string, choice: string): Promise<void> {
      const s = get();
      const mapping = s.turnPermissionLookup[permissionId];
      if (!mapping) {
        dismissPermissionId(s, permissionId);
        return;
      }
      try {
        await daemonResolvePermission(mapping.turnId, mapping.requestId, mapPermissionAction(choice));
        dismissPermissionId(s, permissionId);
        if (s.currentTurnId === mapping.turnId) {
          s.turnThinking = false;
          s.turnStatusHint = "Running";
        }
        const { [permissionId]: _drop, ...rest } = s.turnPermissionLookup;
        s.turnPermissionLookup = rest;
      } catch (error) {
        appendAgentError(s, "Permission response failed", errorText(error), "permission-error");
      }
    },

    async resolveUserQuestion(
      questionId: string,
      answers: Record<string, string | string[]>,
      annotations: Record<string, Record<string, string>> = {}
    ): Promise<void> {
      const s = get();
      const mapping = s.turnQuestionLookup[questionId];
      if (!mapping) {
        dismissQuestionId(s, questionId);
        return;
      }
      try {
        await daemonResolveUserQuestion(mapping.turnId, mapping.requestId, answers, annotations);
        dismissQuestionId(s, questionId);
        if (s.currentTurnId === mapping.turnId) {
          s.turnThinking = false;
          s.turnStatusHint = "Running";
        }
        const { [questionId]: _drop, ...rest } = s.turnQuestionLookup;
        s.turnQuestionLookup = rest;
      } catch (error) {
        appendAgentError(s, "Question response failed", errorText(error), "question-error");
      }
    },

    async cancelCurrentTurn(): Promise<void> {
      const s = get();
      const turnId = s.currentTurnId;
      if (!turnId || s.cancelingTurnId === turnId) return;
      s.cancelingTurnId = turnId;
      s.turnStatusHint = "Cancel requested";
      try {
        await cancelTurn(turnId);
      } catch (error) {
        if (s.currentTurnId !== turnId) return;
        s.cancelingTurnId = null;
        s.turnStatusHint = "Running";
        appendAgentError(s, "Cancel failed", errorText(error), "cancel-error");
      }
    },

    reset(): void {
      resetLiveTurnState(get());
    },

    handleSessionEvent(ev: SessionStreamEvent): void {
      handleSessionEvent(sessionId, ev);
    }
  };
}

/**
 * Optimistically push the user message, fire `run_agent_turn`, then bind the
 * returned turnId. Ported from desktop `submitMessage` with the provider-auth
 * gate and sidebar/localStorage side effects removed. The only send-guard
 * kept is the desktop in-flight / running-turn guard plus a connection check
 * derived from the shared daemon client.
 */
async function submitMessageImpl(
  sessionId: string,
  message: string,
  options: AgentTurnOptions = {}
): Promise<boolean> {
  const state = ensureState(sessionId);
  ensureSubscription(sessionId);
  const conn = connectionStateBox.value;
  if (
    (conn !== "open" && conn !== "idle") ||
    state.turnStartedAtMs !== null ||
    state.currentTurnId !== null
  ) {
    return false;
  }
  const now = Date.now();
  const localUserId = `local-user-${now}`;
  state.submittedMessageBaselineIds = {
    ...state.submittedMessageBaselineIds,
    [localUserId]: (state.sessionDetail?.timeline ?? [])
      .map((item) => item.id)
      .filter((id) => id.length > 0)
  };
  const submittedMessage: TimelineItem = {
    id: localUserId,
    kind: "user",
    createdAtMs: now,
    title: "User",
    summary: message,
    body: message,
    meta: []
  };
  state.submittedMessages = [...state.submittedMessages, submittedMessage];
  state.turnStartedAtMs = now;
  state.turnThinking = true;
  state.turnStatusHint = "Thinking";
  try {
    // No providerId forwarded: the daemon routes via its configured default
    // provider (Task 1 set default_provider=openai). permissionMode falls back
    // to the daemonChat helper's default ("workspace-write").
    const turnId = await runAgentTurn(sessionId, message, options);
    if (isTurnSettled(state, turnId)) {
      // The turn already settled before run_agent_turn returned (rare; the
      // event arrived first). Clear the live turn flags.
      state.currentTurnId = null;
      state.cancelingTurnId = null;
      state.turnStartedAtMs = null;
      state.turnThinking = false;
      state.turnStatusHint = null;
      return true;
    }
    state.currentTurnId = turnId;
    if (state.cancelingTurnId !== turnId) {
      state.cancelingTurnId = null;
    }
    forgetSettledTurn(state, turnId);
    return true;
  } catch (error) {
    const detail = errorText(error);
    // Roll back the optimistic message + reset turn flags (desktop did the
    // same in its non-recoverable error branch).
    state.submittedMessages = state.submittedMessages.filter((item) => item.id !== localUserId);
    const { [localUserId]: _drop, ...restBaselineIds } = state.submittedMessageBaselineIds;
    state.submittedMessageBaselineIds = restBaselineIds;
    state.currentTurnId = null;
    state.cancelingTurnId = null;
    state.turnStartedAtMs = null;
    state.turnThinking = false;
    state.turnStatusHint = null;
    appendAgentError(state, "Agent start failed", detail, "turn-start-error");
    return false;
  }
}

/**
 * Create a brand-new session from the home composer's first message: resolve
 * the default project cwd, `create_session`, then submit. Returns the new
 * sessionId so the caller can navigate to `/agent/<id>`. Ported from desktop's
 * new-session + submit flow, collapsed to momo's single default project.
 */
export async function createSessionFromText(text: string): Promise<string> {
  const trimmed = text.trim();
  // Resolve the default project cwd before creating the session. `getProjectCwd`
  // is a sync read that returns `undefined` until `loadProjects()` has resolved,
  // so we await it first (idempotent / memoized) — otherwise a not-yet-loaded
  // store would create the session under an empty cwd, silently landing it in
  // the wrong group with no error surfaced.
  await loadProjects();
  const cwd = getProjectCwd(DEFAULT_PROJECT_ID);
  if (!cwd) {
    throw new Error("Default project cwd is unavailable; cannot start a session.");
  }
  const sessionId = await createSession(cwd);
  ensureState(sessionId);
  ensureSubscription(sessionId);
  if (trimmed) {
    await submitMessageImpl(sessionId, trimmed);
  }
  return sessionId;
}

/* ── DEV test bridge ──────────────────────────────────────────────── */

if (import.meta.env.DEV) {
  (window as unknown as { __agentChat?: { createController: typeof createController } }).__agentChat =
    {
      createController
    };
}

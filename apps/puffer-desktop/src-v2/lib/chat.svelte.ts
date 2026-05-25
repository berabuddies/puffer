/**
 * Chat session store (Svelte 5 runes).
 *
 * Owns every dynamic Momo conversation surfaced under `/agent/<sessionId>`.
 * Every session is puffer-backed — the scripted-demo branch was removed
 * once the user opted out of the calendar / restaurant timelines. The
 * sessionId is whatever `create_session` returns (a UUID today); real
 * LLM output streams in via WebSocket events from the Tauri backend (see
 * `lib/agentClient.ts` and `lib/wsClient.ts`).
 *
 * Consumers:
 *   - `Composer.svelte` awaits `createSessionFromText` / `appendUserMessage`.
 *   - `Agent.svelte` calls `ensureSession` from a `$effect` to seed the
 *     entry, then reads `chatSessions[id]` from a `$derived` — splitting
 *     read from write avoids Svelte 5's `state_unsafe_mutation` guard.
 */

import * as agent from "./agentClient";
import type { SessionEventPayload } from "./agentClient";
import { pushToast } from "./toast.svelte";

export interface ChatMessage {
  id: string;
  role: "user" | "assistant";
  text: string;
  /** True while the assistant bubble is rendering its "typing…" indicator. */
  pending?: boolean;
  createdAt: number;
  /** Set when the assistant bubble represents an error (e.g. turn-error). */
  error?: boolean;
}

/** Map of sessionId → ordered message list. */
export const chatSessions = $state<Record<string, ChatMessage[]>>({});

/**
 * Visible hydration phase for `/agent/<sessionId>`:
 *   - "idle":    session was never opened in this webview lifetime
 *   - "loading": `load_session_detail` is in flight
 *   - "loaded":  timeline merged successfully
 *   - "error":   `load_session_detail` rejected; retryHydration can re-run it
 *
 * Backed by a Svelte 5 `$state` Record so component reads via
 * `getHydrationState()` re-evaluate when entries are assigned.
 */
export type HydrationState = "idle" | "loading" | "loaded" | "error";

/* ── ID generators ────────────────────────────────────────────── */

let messageCounter = 0;
function nextMessageId(): string {
  messageCounter += 1;
  return `msg-${Date.now().toString(36)}-${messageCounter}`;
}

/* ── Subscription bookkeeping ─────────────────────────────────── */

/** Active session-event unsubscribers, keyed by sessionId. */
const subscriptions = new Map<string, () => void>();
/** Per-session id of the assistant bubble currently filling from deltas. */
const pendingByTurn = new Map<string, { sessionId: string; messageId: string }>();
/**
 * Per-session hydration state for replaying a persisted timeline.
 * Backed by `$state` so component reads via `getHydrationState()` are
 * reactive without exposing the raw store. Sessions not present in the
 * record are treated as "idle".
 */
const hydrationState = $state<Record<string, HydrationState>>({});

function ensureSubscription(sessionId: string): void {
  if (subscriptions.has(sessionId)) return;
  const unsub = agent.subscribeSessionEvents(sessionId, (payload) => {
    handleSessionEvent(sessionId, payload);
  });
  subscriptions.set(sessionId, unsub);
}

function handleSessionEvent(sessionId: string, payload: SessionEventPayload): void {
  const list = chatSessions[sessionId];
  if (!list) return;
  switch (payload.type) {
    case "turn-start": {
      // Normal path: fireTurn's .then already registered the bubble for this
      // turnId before this event fired (Tauri WS poll sends the run_agent_turn
      // response inline at the end of an iteration, then drains broadcast
      // events at the top of the next iteration — see
      // apps/puffer-desktop/src-tauri/src/websocket.rs:46-55). So if we
      // already have a binding for this turnId there's nothing to do.
      const turnId = (payload as { turnId: string }).turnId;
      if (pendingByTurn.has(turnId)) break;
      // Defensive; the .then in fireTurn registers normally. Fall back to a
      // pending bubble not yet claimed by another in-flight turn (NOT just
      // the first pending one — concurrent submits would all collide on #1).
      const claimed = new Set(
        Array.from(pendingByTurn.values())
          .filter((r) => r.sessionId === sessionId)
          .map((r) => r.messageId)
      );
      let bubble = list.find(
        (m) => m.pending && m.role === "assistant" && !claimed.has(m.id)
      );
      if (!bubble) {
        bubble = {
          id: nextMessageId(),
          role: "assistant",
          text: "",
          pending: true,
          createdAt: Date.now(),
        };
        list.push(bubble);
      }
      pendingByTurn.set(turnId, { sessionId, messageId: bubble.id });
      break;
    }
    case "text-delta": {
      const turnId = (payload as { turnId: string }).turnId;
      const delta = (payload as { delta: string }).delta ?? "";
      const ref = pendingByTurn.get(turnId);
      if (!ref) return;
      const target = list.find((m) => m.id === ref.messageId);
      if (!target) return;
      target.text = (target.text ?? "") + delta;
      break;
    }
    case "turn-complete": {
      const turnId = (payload as { turnId: string }).turnId;
      const assistantText = (payload as { assistantText?: string }).assistantText;
      const ref = pendingByTurn.get(turnId);
      if (!ref) return;
      const target = list.find((m) => m.id === ref.messageId);
      if (target) {
        if (typeof assistantText === "string" && assistantText.length > 0) {
          target.text = assistantText;
        }
        target.pending = false;
      }
      pendingByTurn.delete(turnId);
      break;
    }
    case "turn-error": {
      const turnId = (payload as { turnId: string }).turnId;
      const error = (payload as { error: string }).error ?? "Turn failed";
      const ref = pendingByTurn.get(turnId);
      if (ref) {
        const target = list.find((m) => m.id === ref.messageId);
        if (target) {
          target.text = `Error: ${error}`;
          target.pending = false;
          target.error = true;
        }
        pendingByTurn.delete(turnId);
      }
      pushToast(error, "error");
      break;
    }
    // thinking-delta, tool-calls-requested, tool-invocations,
    // permission-request, user-question-request, plan-*, usage,
    // reflection-checkpoint, retry-attempt — intentionally no-op for the
    // first cut. Will be surfaced once the V2 UI has primitives for them.
    default:
      break;
  }
}

/* ── Public helpers ───────────────────────────────────────────── */

/**
 * Returns the session's message list, creating an empty one on demand.
 *
 * NOTE: This mutates `chatSessions` via `ensureSession`. DO NOT call
 * from a `$derived(...)` or template expression — Svelte 5 will raise
 * `state_unsafe_mutation` and the surrounding render will tear down
 * (fatal in WebKit/Tauri). Call from a `$effect` or an event handler
 * instead, and read `chatSessions[id]` directly in your derived.
 */
export function getSession(sessionId: string): ChatMessage[] {
  ensureSession(sessionId);
  return chatSessions[sessionId];
}

/**
 * Create a brand-new puffer-backed chat from the user's first composer
 * message: requests a fresh sessionId, seeds the user + pending bubble,
 * subscribes to session events, kicks off the turn, and returns the
 * sessionId so the caller can navigate to `/agent/<id>`.
 *
 * Errors during session creation surface as a toast and propagate so the
 * caller can decide whether to leave the composer alone.
 */
export async function createSessionFromText(text: string): Promise<string> {
  const trimmed = text.trim();
  let result: agent.CreateSessionResult;
  try {
    result = await agent.createSession({ providerId: "puffer" });
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err);
    pushToast(`Could not start session: ${msg}`, "error");
    throw err;
  }
  const sessionId = result.sessionId;
  chatSessions[sessionId] = [];
  if (trimmed) {
    pushUser(sessionId, trimmed);
    const bubbleId = pushPendingAssistant(sessionId);
    ensureSubscription(sessionId);
    fireTurn(sessionId, bubbleId, trimmed);
  }
  return sessionId;
}

/**
 * Append a user turn to an existing session and kick off a real
 * `run_agent_turn` against the puffer backend.
 */
export async function appendUserMessage(sessionId: string, text: string): Promise<void> {
  const trimmed = text.trim();
  if (!trimmed) return;
  ensureSession(sessionId);
  pushUser(sessionId, trimmed);
  const bubbleId = pushPendingAssistant(sessionId);
  ensureSubscription(sessionId);
  fireTurn(sessionId, bubbleId, trimmed);
}

/* ── Internals ────────────────────────────────────────────────── */

/**
 * Idempotently ensure a session entry exists. Safe to call from an
 * event handler or a Svelte `$effect`, but MUST NOT be called from a
 * `$derived` / template expression — it mutates `chatSessions` and
 * Svelte 5 forbids that in tracked-read scopes (raises
 * `state_unsafe_mutation`, which is fatal in WebKit/Tauri).
 */
export function ensureSession(sessionId: string): void {
  if (!chatSessions[sessionId]) {
    // Seed an empty list first so subscribers / $derived reads downstream
    // never see `undefined`. Hydration below merges past timeline asynchronously.
    chatSessions[sessionId] = [];
  }
  // Subscribe so any in-flight turn for this session still streams in. Safe to
  // call repeatedly — ensureSubscription dedupes by sessionId.
  ensureSubscription(sessionId);
  // Hydrate persisted history exactly once per session. Without this, a
  // webview reload leaves the existing-session view blank because the chat
  // store only carries live deltas (see git history: this was deferred when
  // V2 first wired up, then bit when sessions survived a desktop restart).
  if (hydrationState[sessionId] === undefined) {
    hydrationState[sessionId] = "loading";
    hydrateSession(sessionId);
  }
}

export function getHydrationState(sessionId: string): HydrationState {
  return hydrationState[sessionId] ?? "idle";
}

/**
 * Re-run `load_session_detail` for a session that previously errored.
 * No-op unless the current state is "error" so a stray click on a stale
 * Retry button can't double-fire.
 */
export function retryHydration(sessionId: string): void {
  if (hydrationState[sessionId] !== "error") return;
  hydrationState[sessionId] = "loading";
  hydrateSession(sessionId);
}

function hydrateSession(sessionId: string): void {
  agent
    .loadSessionDetail(sessionId)
    .then((detail) => {
      const historical: ChatMessage[] = [];
      for (const item of detail.timeline ?? []) {
        if (item.kind === "user_message" || item.kind === "assistant_message") {
          historical.push({
            // Stable id namespaced by sessionId so it never collides with live
            // `msg-…` ids generated by nextMessageId().
            id: `hist-${sessionId}-${item.id}`,
            role: item.kind === "user_message" ? "user" : "assistant",
            text: typeof item.text === "string" ? item.text : "",
            // DTO has no timestamp on these timeline items; 0 sorts to the
            // top, which matches the prepend below.
            createdAt: 0,
          });
        }
        // Other kinds (system_message, command, tool_call, permission_dialog,
        // diff_snapshot) are intentionally skipped — V2 has no UI primitives
        // for them yet.
      }
      const current = chatSessions[sessionId];
      if (current) {
        // PREPEND in place so any live turn the user kicked off while
        // hydration was in flight stays at the bottom in submission order.
        // Reassigning would orphan a `list.push(...)` from a handler that
        // captured the old array reference (e.g. the defensive turn-start
        // fallback in handleSessionEvent above).
        current.splice(0, 0, ...historical);
      } else {
        chatSessions[sessionId] = historical;
      }
      hydrationState[sessionId] = "loaded";
    })
    .catch((err) => {
      const msg = err instanceof Error ? err.message : String(err);
      pushToast(`Could not load session history: ${msg}`, "error");
      // Park in "error" so Agent.svelte can render a retry affordance.
      // `ensureSession` guards against accidental re-fire on $effect re-runs;
      // only `retryHydration` can re-enter `hydrateSession` from here.
      hydrationState[sessionId] = "error";
    });
}

function pushUser(sessionId: string, text: string): void {
  chatSessions[sessionId].push({
    id: nextMessageId(),
    role: "user",
    text,
    createdAt: Date.now()
  });
}

function pushPendingAssistant(sessionId: string): string {
  const id = nextMessageId();
  chatSessions[sessionId].push({
    id,
    role: "assistant",
    text: "",
    pending: true,
    createdAt: Date.now(),
  });
  return id;
}

function fireTurn(sessionId: string, bubbleId: string, message: string): void {
  agent
    .runAgentTurn(sessionId, message)
    .then((res) => {
      // Bind the bubble to the turn at the submission site so concurrent
      // in-flight turns can't collide on the same pending bubble. The Tauri
      // WS poll loop sends the run_agent_turn response before broadcasting
      // turn-start (websocket.rs:46-55), so this .then runs ahead of any
      // event for this turn.
      if (res.turnId) {
        pendingByTurn.set(res.turnId, { sessionId, messageId: bubbleId });
      }
    })
    .catch((err) => {
      const msg = err instanceof Error ? err.message : String(err);
      const list = chatSessions[sessionId];
      if (list) {
        const target = list.find((m) => m.id === bubbleId);
        if (target) {
          target.text = `Error: ${msg}`;
          target.pending = false;
          target.error = true;
        }
      }
      pushToast(`Turn failed: ${msg}`, "error");
    });
}

/**
 * Chat session store (Svelte 5 runes).
 *
 * Owns every dynamic Momo conversation surfaced under `/agent/<sessionId>`.
 * Two flavours of session live in here:
 *
 *   1. Dynamic puffer-backed sessions created from the global Composer.
 *      The sessionId is whatever `create_session` returns (a UUID today).
 *      Real LLM output streams in via WebSocket events from the Tauri
 *      backend; see `lib/agentClient.ts` and `lib/wsClient.ts`.
 *   2. The two static "scripted" tasks (calendar, restaurant) which are
 *      seeded from `data/conversations.ts` on first read so the user can
 *      keep typing into the same thread. Those are still mock-only —
 *      additional turns appended to scripted sessions just sit in the
 *      transcript without firing an agent run.
 *
 * Discriminator: if `data/conversations.ts` has an entry for the
 * sessionId, treat it as scripted; otherwise treat it as dynamic and
 * route through `agentClient`.
 *
 * Consumers:
 *   - `Composer.svelte` awaits `createSessionFromText` / `appendUserMessage`.
 *   - `Agent.svelte` calls `ensureSession` from a `$effect` to seed the
 *     entry, then reads `chatSessions[id]` from a `$derived` — splitting
 *     read from write avoids Svelte 5's `state_unsafe_mutation` guard.
 */

import { conversations } from "../data/conversations";
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

/* ── Scripted detection ───────────────────────────────────────── */

function isScripted(sessionId: string): boolean {
  return Object.prototype.hasOwnProperty.call(conversations, sessionId);
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
 * Append a user turn to an existing session. Scripted sessions
 * (calendar / restaurant) stay mock-only — we just push the user message
 * and stop, since there's no real agent on the other end. Dynamic
 * sessions go through `run_agent_turn` like the create path.
 */
export async function appendUserMessage(sessionId: string, text: string): Promise<void> {
  const trimmed = text.trim();
  if (!trimmed) return;
  ensureSession(sessionId);
  pushUser(sessionId, trimmed);
  if (isScripted(sessionId)) return;
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
  if (chatSessions[sessionId]) return;
  // Seed the two scripted conversations so the user can continue typing
  // into the same thread without losing the original timeline. We only
  // pull the `user`/`agent` turns — tool/options/result blocks are owned
  // by Agent.svelte's static renderer and stay outside the chat stream.
  const scripted = conversations[sessionId];
  if (scripted) {
    chatSessions[sessionId] = scripted.steps
      .filter((s): s is { kind: "user"; text: string } | { kind: "agent"; text: string } =>
        s.kind === "user" || s.kind === "agent"
      )
      .map((s, idx) => ({
        id: `seed-${sessionId}-${idx}`,
        role: s.kind === "user" ? "user" : "assistant",
        text: s.text,
        createdAt: 0
      }));
    return;
  }
  chatSessions[sessionId] = [];
  // Dynamic session existed before this page load (e.g. user hit refresh
  // mid-conversation). Subscribe so any in-flight turn still streams in.
  // Note: we don't reload past transcript here — the V2 UI doesn't
  // surface a "load history" affordance yet; turn-* events for new turns
  // will continue to land correctly.
  if (!isScripted(sessionId)) {
    ensureSubscription(sessionId);
  }
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

/**
 * Chat session store (Svelte 5 runes).
 *
 * Owns every dynamic Momo conversation surfaced under `/agent/<sessionId>`.
 * Two flavours of session live in here:
 *
 *   1. Ad-hoc sessions created from the global Composer, identified by a
 *      `chat-<timestamp>-<rand>` id. These start with the user's first
 *      message and are filled in as the conversation goes.
 *   2. The two static "scripted" tasks (calendar, restaurant) which are
 *      seeded from src-v2/data/conversations.ts on first read so the user
 *      can keep typing into the same thread.
 *
 * Mock-only — no real LLM. The mock assistant pushes a `pending: true`
 * placeholder bubble immediately and resolves it ~700ms later with one of
 * the proactive reply lines below (occasionally followed by a short
 * second beat to mimic multi-turn pacing).
 *
 * Consumers:
 *   - `Composer.svelte` calls `createSessionFromText` / `appendUserMessage`.
 *   - `Agent.svelte` calls `ensureSession` from a `$effect` to seed/create
 *     the entry, then reads `chatSessions[id]` from a `$derived` — splitting
 *     read from write avoids Svelte 5's `state_unsafe_mutation` guard.
 */

import { conversations } from "../data/conversations";

export interface ChatMessage {
  id: string;
  role: "user" | "assistant";
  text: string;
  /** True while the assistant bubble is rendering its "typing…" indicator. */
  pending?: boolean;
  createdAt: number;
}

/** Map of sessionId → ordered message list. */
export const chatSessions = $state<Record<string, ChatMessage[]>>({});

/* ── ID generators ────────────────────────────────────────────── */

let messageCounter = 0;
function nextMessageId(): string {
  messageCounter += 1;
  return `msg-${Date.now().toString(36)}-${messageCounter}`;
}

function nextSessionId(): string {
  const rand = Math.random().toString(36).slice(2, 8);
  return `chat-${Date.now().toString(36)}-${rand}`;
}

/* ── Mock assistant reply pool ────────────────────────────────── */

/**
 * Proactive, friendly, Momo-flavoured one-liners. Each tries to either
 * acknowledge + commit to action, ask a clarifying question, or surface
 * a related signal so the assistant feels like it's leaning forward.
 */
const MOCK_PRIMARY_REPLIES: readonly string[] = [
  "Got it. Drafting the reply for you — give me a sec.",
  "Looks like this involves your calendar. Want me to connect Google Calendar?",
  "Done. I've put it on your Friday at 7:30 PM. Sent confirmation to Hanzhi.",
  "Before I act, can you confirm: is this for the team dinner or the client meeting?",
  "I noticed two of your meetings overlap tomorrow. Want me to reschedule the 2 PM?",
  "Heads up — the restaurant doesn't take same-day bookings online. I'll call them at 11 AM.",
  "Top up Wallet first? It needs about $50 to cover this.",
  "On it. I'll loop in Mei and Daniel and share the agenda once it's locked.",
  "Quick check — should I keep this private, or share with the team channel too?",
  "I can handle this end-to-end. Want me to just do it and ping you when it's done?"
];

/** Occasional second beat to suggest a multi-turn cadence. */
const MOCK_FOLLOWUP_REPLIES: readonly string[] = [
  "Anything else?",
  "Let me know if you'd like me to proceed.",
  "I'll keep watching for updates in the meantime."
];

function pickReply(): string {
  const idx = Math.floor(Math.random() * MOCK_PRIMARY_REPLIES.length);
  return MOCK_PRIMARY_REPLIES[idx];
}

function pickFollowup(): string {
  const idx = Math.floor(Math.random() * MOCK_FOLLOWUP_REPLIES.length);
  return MOCK_FOLLOWUP_REPLIES[idx];
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
 * Create a brand-new ad-hoc chat from the user's first composer message,
 * write it into the store, kick off a mock assistant reply, and return the
 * fresh sessionId so the caller can navigate to `/agent/<id>`.
 */
export function createSessionFromText(text: string): string {
  const sessionId = nextSessionId();
  const trimmed = text.trim();
  chatSessions[sessionId] = [];
  if (trimmed) {
    pushUser(sessionId, trimmed);
    scheduleAssistantReply(sessionId);
  }
  return sessionId;
}

/**
 * Append a user turn to an existing session (any of the chat-* ids, or
 * the seeded "calendar" / "restaurant" sessions) and trigger the mock
 * assistant reply.
 */
export function appendUserMessage(sessionId: string, text: string): void {
  const trimmed = text.trim();
  if (!trimmed) return;
  ensureSession(sessionId);
  pushUser(sessionId, trimmed);
  scheduleAssistantReply(sessionId);
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
}

function pushUser(sessionId: string, text: string): void {
  chatSessions[sessionId].push({
    id: nextMessageId(),
    role: "user",
    text,
    createdAt: Date.now()
  });
}

/**
 * Push a pending assistant bubble immediately so the UI can render a
 * "typing…" indicator, then resolve it ~700ms later with mock copy.
 * Roughly 30% of the time, queue a short follow-up beat.
 */
function scheduleAssistantReply(sessionId: string): void {
  if (typeof window === "undefined") return;
  const pendingMsg: ChatMessage = {
    id: nextMessageId(),
    role: "assistant",
    text: "",
    pending: true,
    createdAt: Date.now()
  };
  chatSessions[sessionId].push(pendingMsg);

  window.setTimeout(() => {
    const list = chatSessions[sessionId];
    if (!list) return;
    const target = list.find((m) => m.id === pendingMsg.id);
    if (!target) return;
    target.text = pickReply();
    target.pending = false;

    if (Math.random() < 0.3) {
      const followup: ChatMessage = {
        id: nextMessageId(),
        role: "assistant",
        text: "",
        pending: true,
        createdAt: Date.now()
      };
      chatSessions[sessionId].push(followup);
      window.setTimeout(() => {
        const live = chatSessions[sessionId];
        if (!live) return;
        const t = live.find((m) => m.id === followup.id);
        if (!t) return;
        t.text = pickFollowup();
        t.pending = false;
      }, 600);
    }
  }, 700);
}

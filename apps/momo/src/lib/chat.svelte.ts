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
import type { AskUserQuestionItem, SessionEventPayload } from "./agentClient";
import { pushToast } from "./toast.svelte";

/**
 * Discriminated union over chat bubble kinds. New roles get added here as
 * the chat UI grows primitives for them — keep the union flat so each
 * variant carries exactly the fields it needs:
 *
 *   - "user"      — composer-submitted prompt
 *   - "assistant" — streaming/resolved model output (also error variant)
 *   - "thinking"  — streamed `thinking-delta` content; collapsed to a
 *                   fixed pill in prod, shown verbatim in dev
 */
export type ChatMessage =
  | {
      id: string;
      role: "user";
      text: string;
      /** True while the assistant bubble is rendering its "typing…" indicator. */
      pending?: boolean;
      createdAt: number;
      /** Set when the assistant bubble represents an error (e.g. turn-error). */
      error?: boolean;
    }
  | {
      id: string;
      role: "assistant";
      text: string;
      /** True while the assistant bubble is rendering its "typing…" indicator. */
      pending?: boolean;
      createdAt: number;
      /** Set when the assistant bubble represents an error (e.g. turn-error). */
      error?: boolean;
      /**
       * Bound in fireTurn.then() once the daemon returns a turnId. Lets the
       * renderer match an assistant bubble to its sibling thinking bubble so
       * we can suppress redundant typing-dot indicators while the thinking
       * block already conveys "agent is working".
       */
      turnId?: string;
    }
  | {
      id: string;
      role: "thinking";
      text: string;
      /** True until turn-complete / turn-error lands for this turn. */
      pending: boolean;
      createdAt: number;
      /** The turn this thinking bubble belongs to. Used to flip pending on completion. */
      turnId: string;
    }
  | {
      id: string;
      role: "tool";
      toolId: string;
      callId: string;
      status: "running" | "success" | "failed";
      /** Optional input (JSON-ish) for dev-mode inspection. */
      input?: unknown;
      /** Optional output payload from tool-invocations. */
      output?: unknown;
      createdAt: number;
      /** The turn this tool call belongs to. Used for ordering + cleanup. */
      turnId: string;
    }
  | {
      id: string;
      role: "question";
      /** Backend-minted id for this prompt; routed through resolve_user_question. */
      requestId: string;
      /** The turn this question belongs to — used for ordering before the assistant bubble. */
      turnId: string;
      /** Originally an array (multi-question batch). MVP renders only the first; rest deferred. */
      questions: AskUserQuestionItem[];
      /** Flipped to true on optimistic submit; rolled back if the RPC rejects. */
      answered: boolean;
      /** Picked answers keyed by question index (stringified). */
      answers?: Record<string, string | string[]>;
      createdAt: number;
    };

/** Map of sessionId → ordered message list. */
export const chatSessions = $state<Record<string, ChatMessage[]>>({});

/**
 * Per-session id of the in-flight turn (if any). Set when `run_agent_turn`
 * resolves with a turnId; cleared on `turn-complete` / `turn-error`. The
 * Composer reads this to decide whether to render its Stop button.
 */
export const runningTurnBySessionId = $state<Record<string, string>>({});

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
        (m) => m.role === "assistant" && m.pending && !claimed.has(m.id)
      );
      if (!bubble) {
        bubble = {
          id: nextMessageId(),
          role: "assistant",
          text: "",
          pending: true,
          createdAt: Date.now(),
          turnId
        };
        list.push(bubble);
      } else if (bubble.role === "assistant") {
        // Claim path — stamp the turnId so the renderer can match it
        // against thinking siblings for typing-dot suppression.
        bubble.turnId = turnId;
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
      // pendingByTurn only ever binds to assistant bubbles, but narrow
      // anyway so the tool variant (which has no `text`) typechecks.
      if (target.role !== "assistant") return;
      target.text = (target.text ?? "") + delta;
      break;
    }
    case "turn-complete": {
      const turnId = (payload as { turnId: string }).turnId;
      const assistantText = (payload as { assistantText?: string }).assistantText;
      const ref = pendingByTurn.get(turnId);
      if (ref) {
        const target = list.find((m) => m.id === ref.messageId);
        if (target && target.role === "assistant") {
          if (typeof assistantText === "string" && assistantText.length > 0) {
            target.text = assistantText;
          }
          target.pending = false;
        }
        pendingByTurn.delete(turnId);
      }
      // Also flip any thinking bubble for this turn out of pending. Loop
      // unconditionally (not gated on `ref`) because a thinking-only turn
      // never registers in `pendingByTurn` — there was no assistant bubble
      // to bind.
      for (const m of list) {
        if (m.role === "thinking" && m.turnId === turnId) m.pending = false;
      }
      if (runningTurnBySessionId[sessionId] === turnId) {
        delete runningTurnBySessionId[sessionId];
      }
      break;
    }
    case "turn-error": {
      const turnId = (payload as { turnId: string }).turnId;
      const error = (payload as { error: string }).error ?? "Turn failed";
      const ref = pendingByTurn.get(turnId);
      if (ref) {
        const target = list.find((m) => m.id === ref.messageId);
        if (target && target.role === "assistant") {
          target.text = `Error: ${error}`;
          target.pending = false;
          target.error = true;
        }
        pendingByTurn.delete(turnId);
      }
      // Same as turn-complete: also flip any thinking bubble for this turn.
      for (const m of list) {
        if (m.role === "thinking" && m.turnId === turnId) m.pending = false;
      }
      if (runningTurnBySessionId[sessionId] === turnId) {
        delete runningTurnBySessionId[sessionId];
      }
      pushToast(error, "error");
      break;
    }
    case "tool-calls-requested": {
      const turnId = (payload as { turnId: string }).turnId;
      const requests = ((payload as { requests?: unknown }).requests ?? []) as Array<{
        callId: string;
        toolId: string;
        input?: unknown;
      }>;
      for (const r of requests) {
        // De-dupe (callId, turnId) — a re-emit within the same turn
        // shouldn't double-render a pill (real backends may resend on
        // reconnect). But callId is only unique per-turn (see v1
        // chat-session-ui.spec.ts:1112): the same id appearing in a
        // later turn must produce a fresh pill so the user can see
        // both calls in the timeline.
        if (list.some((m) => m.role === "tool" && m.callId === r.callId && m.turnId === turnId)) continue;
        const newPill: Extract<ChatMessage, { role: "tool" }> = {
          id: nextMessageId(),
          role: "tool",
          toolId: r.toolId,
          callId: r.callId,
          status: "running",
          input: r.input,
          createdAt: Date.now(),
          turnId
        };
        // Insert BEFORE the pending assistant bubble for this turn (if any),
        // mirroring the thinking-delta pattern — tools should read before the
        // final answer they fed, not after.
        const ref = pendingByTurn.get(turnId);
        const assistantIdx = ref
          ? list.findIndex((m) => m.id === ref.messageId)
          : -1;
        if (assistantIdx >= 0) {
          list.splice(assistantIdx, 0, newPill);
        } else {
          list.push(newPill);
        }
      }
      break;
    }
    case "tool-invocations": {
      const turnId = (payload as { turnId: string }).turnId;
      const invocations = ((payload as { invocations?: unknown }).invocations ?? []) as Array<{
        callId: string;
        toolId: string;
        input?: unknown;
        output?: unknown;
        success: boolean;
      }>;
      for (const i of invocations) {
        // Look up by (callId, turnId) — callId is only unique per-turn.
        // A later turn that reuses the same callId must not retroactively
        // update an old turn's pill (see v1 chat-session-ui.spec.ts:1112).
        const target = list.find(
          (m): m is Extract<ChatMessage, { role: "tool" }> =>
            m.role === "tool" && m.callId === i.callId && m.turnId === turnId
        );
        if (target) {
          target.status = i.success ? "success" : "failed";
          target.output = i.output;
        } else {
          // Edge: some backends batch the result without firing
          // tool-calls-requested first. Synthesize the pill at terminal
          // status, still slotted before the assistant bubble for this turn.
          const synth: Extract<ChatMessage, { role: "tool" }> = {
            id: nextMessageId(),
            role: "tool",
            toolId: i.toolId,
            callId: i.callId,
            status: i.success ? "success" : "failed",
            input: i.input,
            output: i.output,
            createdAt: Date.now(),
            turnId
          };
          const ref = pendingByTurn.get(turnId);
          const assistantIdx = ref
            ? list.findIndex((m) => m.id === ref.messageId)
            : -1;
          if (assistantIdx >= 0) {
            list.splice(assistantIdx, 0, synth);
          } else {
            list.push(synth);
          }
        }
      }
      break;
    }
    case "user-question-request": {
      const turnId = (payload as { turnId: string }).turnId;
      const requestId = (payload as { requestId: string }).requestId;
      const questions = ((payload as { questions?: unknown }).questions ?? []) as AskUserQuestionItem[];
      // De-dupe by requestId — the backend may re-emit on reconnect and we
      // mustn't double-render the form.
      if (list.some((m) => m.role === "question" && m.requestId === requestId)) break;
      const newQuestion: Extract<ChatMessage, { role: "question" }> = {
        id: nextMessageId(),
        role: "question",
        requestId,
        turnId,
        questions,
        answered: false,
        createdAt: Date.now()
      };
      // Slot BEFORE the pending assistant bubble for this turn — same
      // pattern as thinking-delta and tool-calls-requested. The agent
      // composes its final answer AFTER the user picks, so the form
      // belongs ahead of the assistant bubble in reading order.
      const ref = pendingByTurn.get(turnId);
      const assistantIdx = ref ? list.findIndex((m) => m.id === ref.messageId) : -1;
      if (assistantIdx >= 0) {
        list.splice(assistantIdx, 0, newQuestion);
      } else {
        list.push(newQuestion);
      }
      break;
    }
    case "thinking-delta": {
      const turnId = (payload as { turnId: string }).turnId;
      const delta = (payload as { delta: string }).delta ?? "";
      // Find or create the thinking bubble for this turn. Attach by turnId
      // (not callId) because thinking-delta events have no callId — one
      // bubble per turn accumulates every delta in submission order.
      let bubble = list.find(
        (m): m is Extract<ChatMessage, { role: "thinking" }> =>
          m.role === "thinking" && m.turnId === turnId
      );
      if (!bubble) {
        bubble = {
          id: nextMessageId(),
          role: "thinking",
          text: "",
          pending: true,
          createdAt: Date.now(),
          turnId
        };
        // Insert BEFORE the assistant bubble for this turn (if any) so the
        // chronological reading order matches the cognitive order
        // (think → answer). If there's no assistant bubble yet, append.
        const ref = pendingByTurn.get(turnId);
        const assistantIdx = ref
          ? list.findIndex((m) => m.id === ref.messageId)
          : -1;
        if (assistantIdx >= 0) {
          list.splice(assistantIdx, 0, bubble);
        } else {
          list.push(bubble);
        }
      }
      bubble.text = (bubble.text ?? "") + delta;
      break;
    }
    // permission-request, user-question-request, plan-*, usage,
    // reflection-checkpoint, retry-attempt — intentionally no-op for
    // the first cut. Will be surfaced once the V2 UI has primitives
    // for them.
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
        // Composer reads this to swap Send → Stop. Cleared on
        // turn-complete / turn-error above (or by the .catch below if the
        // RPC itself fails before any turn id was assigned).
        runningTurnBySessionId[sessionId] = res.turnId;
        // Tag the optimistic bubble with its turnId so the renderer can
        // match it against thinking siblings.
        const list = chatSessions[sessionId];
        if (list) {
          const target = list.find((m) => m.id === bubbleId);
          if (target && target.role === "assistant") {
            target.turnId = res.turnId;
          }
        }
      }
    })
    .catch((err) => {
      const msg = err instanceof Error ? err.message : String(err);
      const list = chatSessions[sessionId];
      if (list) {
        const target = list.find((m) => m.id === bubbleId);
        // Only assistant bubbles carry the `error` field — the pending
        // bubble created in pushPendingAssistant is always the assistant
        // variant, so this narrow is exact.
        if (target && target.role === "assistant") {
          target.text = `Error: ${msg}`;
          target.pending = false;
          target.error = true;
        }
      }
      // No turnId is available here — the RPC rejected before the daemon
      // assigned one. Defensive cleanup in case a prior turn left state.
      if (runningTurnBySessionId[sessionId]) {
        delete runningTurnBySessionId[sessionId];
      }
      pushToast(`Turn failed: ${msg}`, "error");
    });
}

/**
 * Submit the user's answer for an inline question form. Optimistically
 * flips `answered=true` so the form locks immediately; on RPC rejection
 * rolls `answered` back so the user can retry. The daemon-side worker
 * thread is blocked on `rx.recv()` until this round-trip lands (see
 * `apps/momo/src-tauri/src/backend.rs::resolve_user_question`).
 *
 * No-op if the requestId doesn't resolve to a known question message
 * or it's already answered — protects against double-submits when the
 * user double-clicks the submit button before the RPC settles.
 */
export async function answerQuestion(
  sessionId: string,
  requestId: string,
  answers: Record<string, string | string[]>,
): Promise<void> {
  const list = chatSessions[sessionId];
  if (!list) return;
  const target = list.find(
    (m): m is Extract<ChatMessage, { role: "question" }> =>
      m.role === "question" && m.requestId === requestId
  );
  if (!target || target.answered) return;
  // Optimistic lock — form re-renders disabled, double-clicks are ignored
  // upstream because answerQuestion returns early on `target.answered`.
  target.answered = true;
  target.answers = answers;
  try {
    await agent.resolveUserQuestion(target.turnId, requestId, answers);
  } catch (err) {
    // Roll back so the user can retry. We don't roll back `target.answers`
    // because the selection (which the form reads via the `answered` flag)
    // is meaningful next-attempt context — but the form keys disabled state
    // off `answered`, so reverting that alone is enough.
    target.answered = false;
    const msg = err instanceof Error ? err.message : String(err);
    pushToast(`Could not send answer: ${msg}`, "error");
  }
}

/**
 * Cancel the in-flight turn for `sessionId` if any. Surfaces a toast on
 * RPC failure but does NOT roll back local state — `turn-complete` /
 * `turn-error` from the daemon is the authoritative cleanup signal.
 * Returns once the cancel RPC settles.
 */
export async function cancelRunningTurn(sessionId: string): Promise<void> {
  const turnId = runningTurnBySessionId[sessionId];
  if (!turnId) return;
  try {
    await agent.cancelTurn(turnId);
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err);
    pushToast(`Cancel failed: ${msg}`, "error");
  }
}

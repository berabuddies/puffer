/**
 * Domain wrappers around `wsClient` for the puffer agent runtime.
 *
 * Everything here speaks the JSON shapes documented in
 * `src-tauri/src/backend.rs` (router) and `src-tauri/src/turn.rs`
 * (`EmittedEvent` enum). Types are redeclared locally so V2 has zero
 * compile-time coupling to v1 (`src/lib/api/*`).
 */

import * as ws from "./wsClient";

export interface CreateSessionResult {
  sessionId: string;
  cwd: string;
  createdAtMs: number;
  providerId?: string;
  modelId?: string | null;
}

export interface RunAgentTurnResult {
  turnId: string;
}

export interface CreateSessionOptions {
  cwd?: string;
  providerId?: string;
  modelId?: string;
}

export interface RunAgentTurnOptions {
  thinkingOptionId?: string;
  fastMode?: boolean;
  permissionMode?: string;
}

/**
 * Subset of payload shapes emitted on `session:<id>:event`. Mirrors the
 * `EmittedEvent` enum in `src-tauri/src/turn.rs`. Only the fields we use
 * for the first chat cut are typed; the rest are accepted as opaque so
 * we don't crash on `tool-*`, `permission-request`, `usage`, etc.
 */
export type SessionEventPayload =
  | { type: "turn-start"; turnId: string }
  | { type: "text-delta"; turnId: string; delta: string }
  | { type: "thinking-delta"; turnId: string; delta: string }
  | { type: "turn-complete"; turnId: string; assistantText?: string }
  | { type: "turn-error"; turnId: string; error: string }
  | { type: string; [key: string]: unknown };

export async function createSession(
  opts: CreateSessionOptions = {},
): Promise<CreateSessionResult> {
  const params: Record<string, unknown> = {
    providerId: opts.providerId ?? "puffer",
  };
  if (opts.cwd) params.cwd = opts.cwd;
  if (opts.modelId) params.modelId = opts.modelId;
  return ws.request<CreateSessionResult>("create_session", params);
}

export async function runAgentTurn(
  sessionId: string,
  message: string,
  opts: RunAgentTurnOptions = {},
): Promise<RunAgentTurnResult> {
  const params: Record<string, unknown> = { sessionId, message };
  if (opts.thinkingOptionId) params.thinkingOptionId = opts.thinkingOptionId;
  if (opts.fastMode !== undefined) params.fastMode = opts.fastMode;
  if (opts.permissionMode) params.permissionMode = opts.permissionMode;
  return ws.request<RunAgentTurnResult>("run_agent_turn", params);
}

export async function cancelTurn(turnId: string): Promise<void> {
  await ws.request<unknown>("cancel_turn", { turnId });
}

/**
 * Subscribe to `session:<sessionId>:event` and forward unwrapped payloads.
 * Returns an unsubscribe handle. Safe to call multiple times — each
 * registration is independent.
 */
export function subscribeSessionEvents(
  sessionId: string,
  handler: (payload: SessionEventPayload) => void,
): () => void {
  const channel = `session:${sessionId}:event`;
  return ws.subscribe(channel, (raw) => {
    if (!raw || typeof raw !== "object") return;
    handler(raw as SessionEventPayload);
  });
}

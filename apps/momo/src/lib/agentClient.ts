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
  | {
      type: "tool-calls-requested";
      turnId: string;
      requests: Array<{ callId: string; toolId: string; input?: unknown }>;
    }
  | {
      type: "tool-invocations";
      turnId: string;
      invocations: Array<{
        callId: string;
        toolId: string;
        input?: unknown;
        output?: unknown;
        success: boolean;
      }>;
    }
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

/* ── Session list / rename ───────────────────────────────────────
 * Wraps `list_grouped_sessions` and `rename_session` for the V2
 * sidebar. Shapes mirror `SessionListItemDto` / `FolderGroupDto`
 * in `src-tauri/src/dtos.rs` (camelCase via serde rename_all).
 */

export interface SessionListItem {
  sessionId: string;
  displayName: string | null;
  generatedTitle: string | null;
  title: string;
  cwd: string;
  folderPath: string;
  updatedAtMs: number;
  createdAtMs: number;
  eventCount: number;
  activityStatus: string;
  slug: string | null;
  tags: string[];
  note: string | null;
  parentSessionId: string | null;
  providerId: string;
  modelId: string | null;
}

export interface FolderGroup {
  folderId: string;
  folderLabel: string;
  folderPath: string;
  sessionCount: number;
  sessions: SessionListItem[];
}

export async function listGroupedSessions(): Promise<FolderGroup[]> {
  const result = await ws.request<FolderGroup[]>("list_grouped_sessions", {});
  return Array.isArray(result) ? result : [];
}

/**
 * Server returns a full `SessionDetailDto` from `rename_session`, but for
 * the sidebar we only care about the fields that overlap with
 * `SessionListItem` (displayName / title / updatedAtMs). Caller patches
 * the row in place rather than swapping it wholesale.
 */
export interface RenameSessionResult {
  sessionId: string;
  displayName: string | null;
  title: string;
  updatedAtMs: number;
}

export async function renameSession(
  sessionId: string,
  title: string,
): Promise<RenameSessionResult> {
  return ws.request<RenameSessionResult>("rename_session", { sessionId, title });
}

/* ── Session hydration ────────────────────────────────────────────
 * Wraps `load_session_detail` so the V2 chat surface can replay the
 * persisted timeline when a user opens an existing session after a
 * webview reload. Shape mirrors `SessionDetailDto` in
 * `src-tauri/src/dtos.rs:156-182` — only the fields chat hydration
 * reads are typed; the rest are passed through as opaque so future
 * consumers (diff panel, repo status, etc.) can pick them up without
 * a type churn here.
 */

export interface SessionTimelineItem {
  kind: string; // "user_message" | "assistant_message" | (others we ignore)
  id: string;
  text?: string;
  // Other fields exist on tool_call / permission_dialog / etc. — leave
  // them un-typed since chat hydration ignores them.
  [key: string]: unknown;
}

export interface SessionDetail {
  sessionId: string;
  title: string;
  timeline: SessionTimelineItem[];
  // Other DTO fields (cwd, providerId, repoStatus, agentDiff, …) are
  // returned by the server but the chat hydration path doesn't need them.
  [key: string]: unknown;
}

export async function loadSessionDetail(sessionId: string): Promise<SessionDetail> {
  return ws.request<SessionDetail>("load_session_detail", { sessionId });
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

/**
 * Thin daemon chat RPC layer.
 *
 * One helper per chat-relevant puffer *daemon* RPC. Each acquires the shared
 * `daemonClient` (handshake via momo's 1431 backend, then a direct ws to the
 * puffer daemon) and sends a single request. Deliberately a thin wrapper:
 * no Tauri-invoke fallback, no mock data, no browser-preview branch (those
 * live in desktop's `api/desktop.ts` and would mask daemon errors). A failed
 * daemon request rejects so the caller surfaces it.
 */
import { ensureDaemonClient } from "../daemonClient";
import type { AgentTurnOptions, SessionDetail } from "./types";
import { normalizeSessionDetail } from "./normalize";

/**
 * Daemon `create_session` response (camelCase, see
 * `crates/puffer-cli/src/daemon.rs::handle_create_session`). The sidebar
 * needs the cwd / timestamps / routing to synthesise an optimistic row, so
 * we surface the whole shape rather than just the id.
 */
export interface CreateSessionResult {
  sessionId: string;
  displayName: string | null;
  generatedTitle: string | null;
  cwd: string;
  createdAtMs: number;
  updatedAtMs: number;
  slug: string | null;
  providerId: string | null;
  modelId: string | null;
}

/**
 * Daemon `rename_session` echoes the full `SessionDetailDto`; the sidebar
 * only patches `displayName` / `title` / `updatedAtMs` in place, so we type
 * just those (the rest pass through as opaque).
 */
export interface RenameSessionResult {
  sessionId: string;
  displayName: string | null;
  title: string;
  updatedAtMs: number;
  [key: string]: unknown;
}

export async function createSession(
  cwd: string,
  providerId?: string
): Promise<CreateSessionResult> {
  const c = await ensureDaemonClient();
  const params: Record<string, unknown> = { cwd };
  if (providerId) params.providerId = providerId;
  return c.request<CreateSessionResult>("create_session", params);
}

export async function renameSession(
  sessionId: string,
  title: string
): Promise<RenameSessionResult> {
  const c = await ensureDaemonClient();
  return c.request<RenameSessionResult>("rename_session", { sessionId, title });
}

export async function runAgentTurn(
  sessionId: string,
  message: string,
  options: AgentTurnOptions = {}
): Promise<string> {
  const c = await ensureDaemonClient();
  const r = await c.request<{ turnId: string }>("run_agent_turn", {
    sessionId,
    message,
    permissionMode: options.permissionMode ?? "workspace-write",
    ...(options.mode ? { mode: options.mode } : {})
  });
  return r.turnId;
}

export async function cancelTurn(turnId: string): Promise<void> {
  const c = await ensureDaemonClient();
  await c.request("cancel_turn", { turnId });
}

export async function resolvePermission(
  turnId: string,
  requestId: string,
  action: string
): Promise<void> {
  const c = await ensureDaemonClient();
  await c.request("resolve_permission", { turnId, requestId, action });
}

export async function resolveUserQuestion(
  turnId: string,
  requestId: string,
  answers: Record<string, string | string[]>,
  annotations: Record<string, Record<string, string>> = {}
): Promise<void> {
  const c = await ensureDaemonClient();
  await c.request("resolve_user_question", { turnId, requestId, answers, annotations });
}

export async function loadSessionDetail(sessionId: string): Promise<SessionDetail> {
  const c = await ensureDaemonClient();
  const raw = await c.request("load_session_detail", { sessionId });
  return normalizeSessionDetail(raw as Parameters<typeof normalizeSessionDetail>[0]);
}

export async function listGroupedSessions(): Promise<unknown[]> {
  const c = await ensureDaemonClient();
  return c.request("list_grouped_sessions", {});
}

/**
 * Subscribe to the daemon's workspace-level session-change broadcast
 * (`workspace:sessions:changed`). The daemon fires this when it mutates a
 * session out-of-band — most visibly when it generates a session title after
 * the first turn completes (reason `"generated_title"`), but also on routing
 * changes, renames, etc. The handler receives no payload; callers refetch the
 * list. Returns a disposer; a no-op disposer is returned when the daemon isn't
 * reachable so callers don't need to branch.
 */
export async function subscribeSessionsChanged(
  handler: () => void
): Promise<() => void> {
  try {
    const c = await ensureDaemonClient();
    return c.on("workspace:sessions:changed", () => handler());
  } catch {
    return () => {};
  }
}

// Test bridge: lets Playwright drive the daemon chat RPCs without a wired-up
// chat UI. DEV-only so it never ships in a production bundle. Temporary — a
// later task removes this once the chat controller consumes these directly.
if (import.meta.env.DEV) {
  (
    window as unknown as {
      __daemonChat?: {
        createSession: typeof createSession;
        renameSession: typeof renameSession;
        runAgentTurn: typeof runAgentTurn;
        cancelTurn: typeof cancelTurn;
        resolvePermission: typeof resolvePermission;
        resolveUserQuestion: typeof resolveUserQuestion;
        loadSessionDetail: typeof loadSessionDetail;
        listGroupedSessions: typeof listGroupedSessions;
      };
    }
  ).__daemonChat = {
    createSession,
    renameSession,
    runAgentTurn,
    cancelTurn,
    resolvePermission,
    resolveUserQuestion,
    loadSessionDetail,
    listGroupedSessions
  };
}

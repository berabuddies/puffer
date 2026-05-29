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

export async function createSession(cwd: string): Promise<string> {
  const c = await ensureDaemonClient();
  const r = await c.request<{ sessionId: string }>("create_session", { cwd });
  return r.sessionId;
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

// Test bridge: lets Playwright drive the daemon chat RPCs without a wired-up
// chat UI. DEV-only so it never ships in a production bundle. Temporary — a
// later task removes this once the chat controller consumes these directly.
if (import.meta.env.DEV) {
  (
    window as unknown as {
      __daemonChat?: {
        createSession: typeof createSession;
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
    runAgentTurn,
    cancelTurn,
    resolvePermission,
    resolveUserQuestion,
    loadSessionDetail,
    listGroupedSessions
  };
}

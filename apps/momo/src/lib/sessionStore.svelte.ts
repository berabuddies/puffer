/**
 * Sidebar session store (Svelte 5 runes).
 *
 * Owns the flat list of puffer-backed sessions surfaced in the rail.
 * Membership is derived from `session.cwd` matching the single default
 * project's cwd returned by `list_projects` (see `projectStore.svelte.ts`).
 * Sessions whose cwd doesn't match (e.g. legacy `projects/work|life` chats)
 * are hidden from the rail.
 *
 * Surface:
 *   - `sessionList` ($state) — flat list of the default project's sessions
 *     (already cwd-filtered in `loadSessions()`), sorted by `updatedAtMs` desc.
 *   - `projectSessions()` — the rail list (returns `sessionList` as-is).
 *   - `loadSessions()` — refetch from the daemon.
 *   - `createNewSession()` — mint a new session via the puffer daemon under
 *     the default project's fixed cwd.
 *   - `renameSession(id, title)` — server rename + local mutation so
 *     the row updates without a refetch.
 *
 * Wishlist for puffer backend (out of scope here):
 *   - `delete_session(sessionId)` RPC so the Trash icon can actually
 *     do something (today it's disabled with a tooltip).
 */

import type { SessionListItem } from "./agentClient";
import {
  createSession as daemonCreateSession,
  listGroupedSessions,
  renameSession as daemonRenameSession,
  subscribeSessionsChanged,
} from "./agent/daemonChat";
import { NEW_SESSION_TITLE } from "./sessionTitle";
import {
  getProjectCwd,
  loadProjects,
  DEFAULT_PROJECT_ID,
} from "./projectStore.svelte";
import { pushToast } from "./toast.svelte";

/**
 * Minimal view of a daemon `list_grouped_sessions` group we actually read.
 * The daemon returns camelCase DTOs (`FolderGroupDto` / `SessionListItemDto`,
 * same shape desktop's daemon path consumes); `listGroupedSessions()` types
 * them as `unknown[]`, so we narrow here. We keep this loose (only the fields
 * the sidebar + cwd filter touch are required) so a richer daemon payload
 * passes through untouched.
 */
interface DaemonSessionGroup {
  folderPath?: string | null;
  cwd?: string | null;
  sessions?: unknown[];
}

function asGroup(value: unknown): DaemonSessionGroup | null {
  if (!value || typeof value !== "object") return null;
  return value as DaemonSessionGroup;
}

function asSession(value: unknown): SessionListItem | null {
  if (!value || typeof value !== "object") return null;
  const s = value as Partial<SessionListItem>;
  return typeof s.sessionId === "string" ? (value as SessionListItem) : null;
}

/** Flat, reactive list of every session known to the sidebar. */
export const sessionList = $state<SessionListItem[]>([]);

/**
 * Svelte 5 forbids `export const x = $derived(...)` from `.svelte.ts`
 * modules (svelte/derived_invalid_export) — derivations have to live in
 * component instances or be exposed as functions. We export a thin filter
 * function; consumers call `projectSessions()` inside a reactive context
 * (template each-blocks, $derived, etc.) and get the normal Svelte 5
 * tracking they would get from `$derived`.
 *
 * The rail shows every session in `sessionList`. Membership is already
 * narrowed to the default project's cwd in `loadSessions()` (it keeps only
 * the matching daemon group), and `createNewSession()` only ever stubs rows
 * under that same cwd, so no second cwd filter is needed here.
 */
export function projectSessions(): SessionListItem[] {
  return sessionList;
}

function sortByUpdatedDesc(list: SessionListItem[]): SessionListItem[] {
  return [...list].sort((a, b) => b.updatedAtMs - a.updatedAtMs);
}

function replaceList(next: SessionListItem[]): void {
  // Preserve local-only entries (e.g. an optimistic stub from
  // createNewSession whose create_session response landed before this
  // listGroupedSessions snapshot) so we don't drop them on reconcile.
  const nextIds = new Set(next.map((s) => s.sessionId));
  const localOnly = sessionList.filter((s) => !nextIds.has(s.sessionId));
  const merged = sortByUpdatedDesc([...next, ...localOnly]);
  sessionList.splice(0, sessionList.length, ...merged);
}

/**
 * Refetch the sidebar session list from the puffer **daemon**.
 *
 * `list_grouped_sessions` on the daemon is *global* — it returns every cwd
 * group the daemon knows about, including the task-monitor sessions that
 * live under unrelated cwds. Momo only surfaces sessions under its single
 * default project, so we resolve that project's fixed cwd
 * (`getProjectCwd("default")`, the same cwd `create_session` is handed) and
 * keep only the matching group's sessions. `loadProjects()` is idempotent,
 * so awaiting it here just guarantees the cwd is resolved before we filter
 * (it usually already is — Shell kicks it off in parallel on mount).
 */
export async function loadSessions(): Promise<void> {
  let raw: unknown[];
  try {
    await loadProjects();
    raw = await listGroupedSessions();
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err);
    pushToast(`Could not load sessions: ${msg}`, "error");
    return;
  }

  const defaultCwd = getProjectCwd(DEFAULT_PROJECT_ID);
  // Until the default project's cwd is known we can't tell momo's own
  // sessions from monitor sessions, so surface nothing rather than leak
  // unrelated groups into the rail.
  if (!defaultCwd) {
    replaceList([]);
    return;
  }

  const flat: SessionListItem[] = [];
  const seen = new Set<string>();
  for (const value of raw) {
    const group = asGroup(value);
    if (!group) continue;
    const groupPath = group.folderPath ?? group.cwd;
    if (groupPath !== defaultCwd) continue;
    for (const sessionValue of group.sessions ?? []) {
      const s = asSession(sessionValue);
      if (!s || seen.has(s.sessionId)) continue;
      seen.add(s.sessionId);
      flat.push(s);
    }
  }
  replaceList(flat);
}

/** Live disposer + in-flight guard for the workspace session-change
 *  subscription, so `subscribeSessionChanges()` only ever wires one listener
 *  even across Shell remounts / concurrent calls. */
let sessionsChangedUnsub: (() => void) | null = null;
let sessionsChangedSubscribing = false;

/**
 * Wire a one-time listener for the daemon's `workspace:sessions:changed`
 * broadcast and refetch the rail whenever it fires. Without this the rail only
 * loaded once on Shell mount, so a title the daemon generates after the first
 * turn (or any out-of-band rename) wouldn't appear until the app restarted —
 * the user saw the sidebar title "change on restart". Idempotent.
 */
export async function subscribeSessionChanges(): Promise<void> {
  if (sessionsChangedUnsub || sessionsChangedSubscribing) return;
  sessionsChangedSubscribing = true;
  try {
    sessionsChangedUnsub = await subscribeSessionsChanged(() => {
      void loadSessions();
    });
  } finally {
    sessionsChangedSubscribing = false;
  }
}

/**
 * Mint a fresh empty session via the puffer **daemon** under the default
 * project's fixed cwd. Returns the new sessionId so the caller can navigate
 * to `/agent/<id>`. Goes through the same daemon RPC (`create_session`) that
 * `loadSessions()` reads back via `list_grouped_sessions`, so a new chat
 * shows up in the rail on the next refetch — no data-source split between
 * the daemon (~/.puffer) and the legacy 1431 store.
 *
 * The new row is pushed onto the local list immediately so the sidebar
 * updates without waiting for a reload — the next `loadSessions()`
 * reconciles any drift.
 */
export async function createNewSession(): Promise<string> {
  const cwd = getProjectCwd(DEFAULT_PROJECT_ID);
  if (!cwd) {
    pushToast("Project not ready yet — try again in a moment.", "error");
    throw new Error("default project cwd not loaded");
  }
  let result: Awaited<ReturnType<typeof daemonCreateSession>>;
  try {
    // Don't pin a providerId: the real daemon's create_session routing
    // rejects an unknown provider (e.g. "unknown provider `puffer`"), and the
    // canonical providers are openai / anthropic — not "puffer". Omitting it
    // lets the daemon fall back to default routing (Task 1 set the daemon's
    // default_provider), exactly like the composer's createSessionFromText.
    result = await daemonCreateSession(cwd);
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err);
    pushToast(`Could not start session: ${msg}`, "error");
    throw err;
  }
  const id = result.sessionId;
  // Synthesize a SessionListItem so the sidebar can render the row before
  // the next loadSessions() returns. The fields not supplied by
  // create_session default to safe placeholders; loadSessions() will
  // overwrite them with authoritative values on next refetch.
  const stub: SessionListItem = {
    sessionId: id,
    displayName: result.displayName,
    generatedTitle: result.generatedTitle,
    title: result.displayName ?? result.generatedTitle ?? NEW_SESSION_TITLE,
    cwd: result.cwd,
    folderPath: result.cwd,
    updatedAtMs: result.updatedAtMs ?? result.createdAtMs,
    createdAtMs: result.createdAtMs,
    eventCount: 0,
    activityStatus: "idle",
    slug: result.slug,
    tags: [],
    note: null,
    parentSessionId: null,
    providerId: result.providerId ?? null,
    modelId: result.modelId ?? null,
  };
  // Drop any stale copy (defensive — shouldn't happen with fresh ids).
  const existing = sessionList.findIndex((s) => s.sessionId === id);
  if (existing >= 0) sessionList.splice(existing, 1);
  sessionList.unshift(stub);
  return id;
}

export async function renameSession(id: string, title: string): Promise<void> {
  const trimmed = title.trim();
  if (!trimmed) return;
  try {
    // Goes through the daemon's `rename_session` (writes ~/.puffer), the same
    // store `loadSessions()` reads, so a rename in the rail survives a refetch.
    const detail = await daemonRenameSession(id, trimmed);
    const target = sessionList.find((s) => s.sessionId === id);
    if (target) {
      target.displayName = detail.displayName;
      target.title = detail.title || trimmed;
      target.updatedAtMs = detail.updatedAtMs;
    }
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err);
    pushToast(`Rename failed: ${msg}`, "error");
    throw err;
  }
}

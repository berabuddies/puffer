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
 *   - `sessionList` ($state) — flat list of all sessions, sorted by
 *     `updatedAtMs` desc.
 *   - `projectSessions()` — the default project's sessions (rail list).
 *   - `loadSessions()` — refetch from the backend.
 *   - `createNewSession()` — mint a new session via puffer under the
 *     default project's fixed cwd.
 *   - `renameSession(id, title)` — server rename + local mutation so
 *     the row updates without a refetch.
 *
 * Wishlist for puffer backend (out of scope here):
 *   - `delete_session(sessionId)` RPC so the Trash icon can actually
 *     do something (today it's disabled with a tooltip).
 */

import * as agent from "./agentClient";
import type { SessionListItem } from "./agentClient";
import {
  getProjectCwd,
  projectIdForCwd,
  DEFAULT_PROJECT_ID,
} from "./projectStore.svelte";
import { pushToast } from "./toast.svelte";

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
 * The rail shows every session under the default project. Legacy work/life
 * sessions fall out because their cwd no longer matches any known project.
 */
export function projectSessions(): SessionListItem[] {
  return sessionList.filter((s) => projectIdForCwd(s.cwd) === DEFAULT_PROJECT_ID);
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

export async function loadSessions(): Promise<void> {
  let groups: agent.FolderGroup[];
  try {
    groups = await agent.listGroupedSessions();
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err);
    pushToast(`Could not load sessions: ${msg}`, "error");
    return;
  }
  const flat: SessionListItem[] = [];
  const seen = new Set<string>();
  for (const g of groups) {
    for (const s of g.sessions) {
      if (seen.has(s.sessionId)) continue;
      seen.add(s.sessionId);
      flat.push(s);
    }
  }
  replaceList(flat);
}

/**
 * Mint a fresh empty session via puffer under the default project's fixed
 * cwd. Returns the new sessionId so the caller can navigate to
 * `/agent/<id>`. The new row is pushed onto the local list immediately
 * so the sidebar updates without waiting for a reload — the next
 * `loadSessions()` reconciles any drift.
 */
export async function createNewSession(): Promise<string> {
  const cwd = getProjectCwd(DEFAULT_PROJECT_ID);
  if (!cwd) {
    pushToast("Project not ready yet — try again in a moment.", "error");
    throw new Error("default project cwd not loaded");
  }
  let result: agent.CreateSessionResult;
  try {
    result = await agent.createSession({ cwd, providerId: "puffer" });
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
    displayName: null,
    generatedTitle: null,
    title: "New chat",
    cwd: result.cwd,
    folderPath: result.cwd,
    updatedAtMs: result.createdAtMs,
    createdAtMs: result.createdAtMs,
    eventCount: 0,
    activityStatus: "idle",
    slug: null,
    tags: [],
    note: null,
    parentSessionId: null,
    providerId: result.providerId ?? "puffer",
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
    const detail = await agent.renameSession(id, trimmed);
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

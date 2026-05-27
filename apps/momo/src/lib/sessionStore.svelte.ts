/**
 * Sidebar session store (Svelte 5 runes).
 *
 * Owns the flat list of puffer-backed sessions surfaced under the
 * Work / Life projects in the rail. Project membership is derived from
 * `session.cwd` matching one of the two fixed project cwds returned by
 * `list_projects` (see `projectStore.svelte.ts`). Sessions whose cwd
 * doesn't match either project are hidden from the rail.
 *
 * Surface:
 *   - `sessionList` ($state) — flat list of all sessions, sorted by
 *     `updatedAtMs` desc.
 *   - `workSessions` / `lifeSessions` — filtered slices.
 *   - `loadSessions()` — refetch from the backend.
 *   - `createSessionForProject(projectId)` — mint a new session via puffer
 *     with the project's fixed cwd.
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
  type ProjectId,
} from "./projectStore.svelte";
import { pushToast } from "./toast.svelte";

/** Flat, reactive list of every session known to the sidebar. */
export const sessionList = $state<SessionListItem[]>([]);

/**
 * Svelte 5 forbids `export const x = $derived(...)` from `.svelte.ts`
 * modules (svelte/derived_invalid_export) — derivations have to live in
 * component instances or be exposed as functions. We export thin filter
 * functions; consumers call `workSessions()` / `lifeSessions()` inside
 * a reactive context (template each-blocks, $derived, etc.) and get the
 * normal Svelte 5 tracking they would get from `$derived`.
 */
export function workSessions(): SessionListItem[] {
  return sessionsForProject("work");
}

export function lifeSessions(): SessionListItem[] {
  return sessionsForProject("life");
}

export function sessionsForProject(projectId: ProjectId): SessionListItem[] {
  return sessionList.filter((s) => projectIdForCwd(s.cwd) === projectId);
}

function sortByUpdatedDesc(list: SessionListItem[]): SessionListItem[] {
  return [...list].sort((a, b) => b.updatedAtMs - a.updatedAtMs);
}

function replaceList(next: SessionListItem[]): void {
  // Preserve local-only entries (e.g. an optimistic stub from
  // createSessionForProject whose create_session response landed before this
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
 * Mint a fresh empty session via puffer under the given project's fixed
 * cwd. Returns the new sessionId so the caller can navigate to
 * `/agent/<id>`. The new row is pushed onto the local list immediately
 * so the sidebar updates without waiting for a reload — the next
 * `loadSessions()` reconciles any drift.
 */
export async function createSessionForProject(projectId: ProjectId): Promise<string> {
  const cwd = getProjectCwd(projectId);
  if (!cwd) {
    pushToast("Project not ready yet — try again in a moment.", "error");
    throw new Error(`project ${projectId} cwd not loaded`);
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

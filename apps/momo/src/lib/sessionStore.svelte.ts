/**
 * Sidebar session store (Svelte 5 runes).
 *
 * Owns the flat list of puffer-backed sessions surfaced under the
 * Work / Life groups in the rail. Group membership is a frontend-only
 * concept persisted in localStorage — Corbina's session record has no
 * "group" field yet (see the wishlist note below).
 *
 * Surface:
 *   - `sessionList` ($state) — flat list of all sessions, sorted by
 *     `updatedAtMs` desc.
 *   - `workSessions` / `lifeSessions` ($derived) — filtered slices.
 *   - `loadSessions()` — refetch from the backend.
 *   - `createSessionInGroup(group)` — mint a new session via puffer and
 *     remember which group it belongs to.
 *   - `renameSession(id, title)` — server rename + local mutation so
 *     the row updates without a refetch.
 *
 * Wishlist for puffer backend (out of scope here, see Report):
 *   - First-class "group" / "tag" on `SessionMetadata` so we don't have
 *     to mirror it in localStorage and lose it across machines.
 *   - `delete_session(sessionId)` RPC so the Trash icon can actually
 *     do something (today it's disabled with a tooltip).
 */

import * as agent from "./agentClient";
import type { SessionListItem } from "./agentClient";
import { pushToast } from "./toast.svelte";

export type SessionGroup = "work" | "life";

const STORAGE_KEY = "puffer.sessionGroups";

/** Flat, reactive list of every session known to the sidebar. */
export const sessionList = $state<SessionListItem[]>([]);

/**
 * Group membership map. We mirror it into a `$state` so derived filters
 * re-run whenever it changes; the localStorage write is a side effect.
 */
const groupMap = $state<Record<string, SessionGroup>>(readGroupMap());

function readGroupMap(): Record<string, SessionGroup> {
  if (typeof localStorage === "undefined") return {};
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw) as Record<string, unknown>;
    const out: Record<string, SessionGroup> = {};
    for (const [id, value] of Object.entries(parsed)) {
      if (value === "work" || value === "life") out[id] = value;
    }
    return out;
  } catch {
    return {};
  }
}

function writeGroupMap(): void {
  if (typeof localStorage === "undefined") return;
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(groupMap));
  } catch {
    // Quota / serialization failure is non-fatal — group membership just
    // won't persist across reloads.
  }
}

/**
 * Sessions not yet tracked in `puffer.sessionGroups` (created before
 * this code shipped, or via the puffer CLI) default to "work" as a
 * graceful fallback so the user can see them.
 */
function groupFor(sessionId: string): SessionGroup {
  return groupMap[sessionId] ?? "work";
}

/**
 * Svelte 5 forbids `export const x = $derived(...)` from `.svelte.ts`
 * modules (svelte/derived_invalid_export) — derivations have to live in
 * component instances or be exposed as functions. We export thin filter
 * functions; consumers call `workSessions()` / `lifeSessions()` inside
 * a reactive context (template each-blocks, $derived, etc.) and get the
 * normal Svelte 5 tracking they would get from `$derived`.
 */
export function workSessions(): SessionListItem[] {
  return sessionList.filter((s) => groupFor(s.sessionId) === "work");
}

export function lifeSessions(): SessionListItem[] {
  return sessionList.filter((s) => groupFor(s.sessionId) === "life");
}

function sortByUpdatedDesc(list: SessionListItem[]): SessionListItem[] {
  return [...list].sort((a, b) => b.updatedAtMs - a.updatedAtMs);
}

function replaceList(next: SessionListItem[]): void {
  // Preserve local-only entries (e.g. an optimistic stub from
  // createSessionInGroup whose create_session response landed before this
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
 * Mint a fresh empty session via puffer and remember which group it
 * belongs to. Returns the new sessionId so the caller can navigate to
 * `/agent/<id>`. The new row is pushed onto the local list immediately
 * so the sidebar updates without waiting for a reload — the next
 * `loadSessions()` reconciles any drift.
 */
export async function createSessionInGroup(group: SessionGroup): Promise<string> {
  let result: agent.CreateSessionResult;
  try {
    result = await agent.createSession({ providerId: "puffer" });
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err);
    pushToast(`Could not start session: ${msg}`, "error");
    throw err;
  }
  const id = result.sessionId;
  groupMap[id] = group;
  writeGroupMap();
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

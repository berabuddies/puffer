/**
 * Shared session-title derivation for the rail (`Sidebar.svelte`) and the chat
 * header (`Agent.svelte`) — the single source of truth so the two surfaces
 * never disagree.
 *
 * The daemon's own `title` field falls back to the session *slug* (and then
 * the cwd name) when neither a user-set display name nor a generated title
 * exists, which leaks raw `session-<uuid>` strings into the UI. Momo never
 * wants to show those, so we derive the title from the meaningful fields only
 * and let the caller supply a friendly placeholder.
 *
 * Both `displayName` (a user rename) and `generatedTitle` (the daemon's
 * post-first-turn summary, which itself falls back to a truncated first user
 * message) are persisted in the daemon's `~/.puffer` store, so whatever we
 * show here survives an app restart.
 */

/** Friendly placeholder for a session with no meaningful title yet. */
export const NEW_SESSION_TITLE = "New task";

/** Minimal shape both `SessionListItem` variants (agentClient / agent/types)
 *  satisfy — we only ever read these two fields. */
interface TitledSession {
  displayName: string | null;
  generatedTitle: string | null;
}

/**
 * The meaningful title for a session, or `null` when none exists yet. Prefers
 * the user's display name over the daemon's generated title; deliberately
 * ignores slug / cwd so a raw `session-…` id can never surface. Callers append
 * their own fallback (e.g. `?? NEW_SESSION_TITLE`, or the first user message in
 * the chat header).
 */
export function sessionTitle(session: TitledSession | null | undefined): string | null {
  return session?.displayName?.trim() || session?.generatedTitle?.trim() || null;
}

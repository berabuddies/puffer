/**
 * Monitor task store (Svelte 5 runes).
 *
 * Owns the list of Telegram monitor tasks surfaced on the Tasks page. The
 * list comes from the puffer daemon's `workflow_list` snapshot
 * (`monitor_tasks[]`), reached through `taskApi.ts` → `daemonClient.ts`.
 *
 * Surface (mirrors `sessionStore.svelte.ts` conventions — module-level
 * `$state`, plain async loaders with try/catch; we do NOT
 * `export const x = $derived(...)`, which Svelte 5 forbids from a
 * `.svelte.ts` module):
 *   - `monitorTasks` ($state) — the current list, mutated in place.
 *   - `taskState` ($state) — { loading, ready, error } for the page's
 *     three render states.
 *   - `loadTasks()` — (re)fetch the snapshot.
 *   - `ignoreTask(taskId)` — ignore one task, then refetch.
 */

import {
  loadWorkflowSnapshot,
  ignoreMonitorTask,
  type WorkflowMonitorTask,
} from "./taskApi";
import { resetDaemonClient } from "./daemonClient";
import { pushToast } from "./toast.svelte";

/** Reactive list of the current monitor tasks; the page iterates this directly. */
export const monitorTasks = $state<WorkflowMonitorTask[]>([]);

/** Page load state, surfaced as the Tasks page's three render branches. */
export const taskState = $state<{ loading: boolean; ready: boolean; error: string | null }>({
  loading: false,
  ready: false,
  error: null,
});

export async function loadTasks(): Promise<void> {
  taskState.loading = true;
  taskState.error = null;
  try {
    const snapshot = await loadWorkflowSnapshot();
    // The daemon keeps ignored/handled tasks in the snapshot — `task_monitor_ignore`
    // sets `ignored: true` (+ status=completed) rather than dropping the task, so
    // `workflow_list` still returns it. The actionable list shown to the user is
    // the non-ignored subset; mirror puffer-desktop's Tasks screen, which filters
    // `task.ignored === true` out of the default view. Without this, clicking
    // Ignore appears to do nothing — the refetched snapshot still contains the
    // task, so the row never disappears.
    const next = (snapshot.monitor_tasks ?? []).filter((task) => task.ignored !== true);
    monitorTasks.splice(0, monitorTasks.length, ...next);
    taskState.ready = true;
  } catch (error) {
    taskState.error = error instanceof Error ? error.message : String(error);
    // The daemon may have died/restarted — drop the dead client so the next
    // loadTasks() re-fetches the handshake and dials a fresh socket.
    resetDaemonClient();
  } finally {
    taskState.loading = false;
  }
}

export async function ignoreTask(taskId: string): Promise<void> {
  try {
    await ignoreMonitorTask(taskId);
    await loadTasks();
  } catch {
    pushToast("Failed to ignore task", "error");
  }
}

/** Background poll interval for the monitor task feed. */
const POLL_INTERVAL_MS = 15_000;

let pollHandle: ReturnType<typeof setInterval> | null = null;
let focusHandler: (() => void) | null = null;

/**
 * Start polling the monitor task feed so new Telegram tasks surface on Home
 * without the user navigating away and back.
 *
 * The puffer daemon does NOT push a workflow/task-changed event — the triage
 * agent that creates a monitor task writes `monitor_tasks.json` from a
 * background cron thread with no access to the daemon's event bus, so there is
 * nothing to subscribe to (the upstream puffer-desktop Tasks screen only
 * refreshes on mount + a manual Refresh button for the same reason). Polling
 * `workflow_list` is the only way the feed can self-update.
 *
 * Also refetches on window focus so returning to the app shows fresh tasks
 * immediately (mirrors `creditStore`'s poll + focus pattern). Idempotent:
 * calling twice without an intervening `stopTaskPolling()` is a no-op, so Home
 * remounts don't stack intervals/listeners.
 */
export function startTaskPolling(): void {
  if (pollHandle !== null) return;
  void loadTasks();
  pollHandle = setInterval(() => void loadTasks(), POLL_INTERVAL_MS);
  focusHandler = () => void loadTasks();
  window.addEventListener("focus", focusHandler);
}

export function stopTaskPolling(): void {
  if (pollHandle !== null) {
    clearInterval(pollHandle);
    pollHandle = null;
  }
  if (focusHandler !== null) {
    window.removeEventListener("focus", focusHandler);
    focusHandler = null;
  }
}

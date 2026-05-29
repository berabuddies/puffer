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
    const next = snapshot.monitor_tasks ?? [];
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

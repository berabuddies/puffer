/**
 * Thin task API over the puffer daemon client.
 *
 * Builds on `daemonClient.ts` (Task 3): everything here round-trips through
 * `ensureDaemonClient().request(...)` and speaks the puffer daemon's wire
 * contract directly. Method names and param keys are puffer's contract and
 * are intentionally snake_case — do NOT camelCase them.
 *
 * Following the momo frontend convention (see `agentClient.ts`), types are
 * redeclared locally so we stay decoupled from puffer's Rust DTOs. Only
 * `monitor_tasks` is typed precisely; the rest of the workflow snapshot is
 * passed through opaquely so future consumers can pick up other fields
 * (`workflows`, `runs`, `tasks`, `connectors`, `connections`, …) without a
 * type churn here.
 */

import { ensureDaemonClient } from "./daemonClient";

export interface WorkflowMonitorTaskAction {
  name: string;
  prompt: string;
}

export interface WorkflowMonitorTask {
  task_id: string;
  subject: string;
  description: string;
  status: string;
  monitor_connection?: string | null;
  monitor_connector?: string | null;
  monitor_memory_path?: string | null;
  ignored?: boolean;
  actions?: WorkflowMonitorTaskAction[];
  possible_ignore_reasons?: string[];
  started_at_ms?: number | null;
  updated_at_ms?: number | null;
}

/** Subset of puffer's workflow snapshot — only monitor_tasks is typed precisely. */
export interface WorkflowSnapshot {
  monitor_tasks?: WorkflowMonitorTask[];
  monitor_task_error?: string | null;
  [key: string]: unknown;
}

export async function loadWorkflowSnapshot(): Promise<WorkflowSnapshot> {
  const client = await ensureDaemonClient();
  return client.request<WorkflowSnapshot>("workflow_list");
}

export async function createMonitor(connectionSlug: string): Promise<WorkflowSnapshot> {
  const client = await ensureDaemonClient();
  return client.request<WorkflowSnapshot>("task_monitor_create", { connection_slug: connectionSlug });
}

export async function ignoreMonitorTask(taskId: string, reason?: string): Promise<WorkflowSnapshot> {
  const client = await ensureDaemonClient();
  return client.request<WorkflowSnapshot>("task_monitor_ignore", {
    task_id: taskId,
    reason: reason?.trim() || undefined,
  });
}

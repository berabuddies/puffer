<!--
  Tasks — the Telegram monitor task feed.

  Surfaces the puffer daemon's `monitor_tasks[]` (see taskStore.svelte.ts →
  taskApi.ts → daemonClient.ts). Each row shows the task subject, its
  description, a meta line (connection · status · updated time), and an
  "Ignore" action that calls `task_monitor_ignore` and refetches.

  Three render states:
    - error  → "Daemon not ready — {error}" with a Refresh affordance.
    - empty  → friendly "all clear" hint.
    - list   → one row per monitor task.

  Out of scope this milestone (intentionally not rendered): the per-task
  `actions[]` buttons and an "Open" affordance.
-->
<script lang="ts">
  import { onMount } from "svelte";
  import { RefreshCw } from "lucide-svelte";

  import PageHeader from "../components/shell/PageHeader.svelte";
  import Button from "../components/common/Button.svelte";
  import {
    monitorTasks,
    taskState,
    loadTasks,
    ignoreTask,
  } from "../lib/taskStore.svelte";
  import type { WorkflowMonitorTask } from "../lib/taskApi";

  onMount(() => {
    void loadTasks();
  });

  function refresh(): void {
    void loadTasks();
  }

  function ignore(task: WorkflowMonitorTask): void {
    void ignoreTask(task.task_id);
  }

  /** Compact "12:34 PM" / "May 29" style stamp for the meta line. */
  function formatUpdated(ms: number | null | undefined): string | null {
    if (typeof ms !== "number" || !Number.isFinite(ms)) return null;
    const date = new Date(ms);
    if (Number.isNaN(date.getTime())) return null;
    return date.toLocaleString(undefined, {
      month: "short",
      day: "numeric",
      hour: "numeric",
      minute: "2-digit",
    });
  }

  function metaParts(task: WorkflowMonitorTask): string[] {
    const parts: string[] = [task.monitor_connection ?? "telegram", task.status];
    const updated = formatUpdated(task.updated_at_ms);
    if (updated) parts.push(updated);
    return parts;
  }
</script>

<PageHeader title="Tasks" subtitle="Telegram monitor tasks">
  {#snippet accessory()}
    <Button
      variant="secondary"
      size="sm"
      label="Refresh"
      icon={RefreshCw}
      onclick={refresh}
      disabled={taskState.loading}
    />
  {/snippet}
</PageHeader>

<section class="tasks">
  {#if taskState.error}
    <div class="tasks__notice" role="alert">
      <p class="text-task-title tasks__notice-title">Daemon not ready</p>
      <p class="tasks__notice-body">{taskState.error}</p>
    </div>
  {:else if monitorTasks.length === 0}
    <div class="tasks__empty">
      <h2 class="tasks__empty-title text-display">No monitor tasks</h2>
      <p class="tasks__empty-body">
        Momo will surface Telegram tasks here once your monitor connection
        finds something that needs your attention.
      </p>
    </div>
  {:else}
    <ul class="tasks__list">
      {#each monitorTasks as task (task.task_id)}
        <li class="task-row">
          <div class="task-row__text">
            <h3 class="text-task-title task-row__title">{task.subject}</h3>
            {#if task.description}
              <p class="text-body-compact task-row__description">{task.description}</p>
            {/if}
            <p class="text-eyebrow task-row__meta">
              {#each metaParts(task) as part, index (index)}
                {#if index > 0}<span class="task-row__meta-sep" aria-hidden="true">·</span>{/if}<span>{part}</span>
              {/each}
            </p>
          </div>
          <div class="task-row__actions">
            <Button
              variant="secondary"
              size="sm"
              label="Ignore"
              onclick={() => ignore(task)}
            />
          </div>
        </li>
      {/each}
    </ul>
  {/if}
</section>

<style>
  .tasks {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    padding-bottom: var(--space-5);
  }

  /* Error notice — calm, not alarming; matches card vocabulary. */
  .tasks__notice {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    padding: var(--space-4);
    border-radius: var(--radius-card);
    background: var(--color-surface-app);
    border: 1px solid var(--color-card-border);
  }
  .tasks__notice-body {
    font-family: var(--font-system);
    font-size: var(--font-size-body);
    line-height: var(--line-height-body);
    color: var(--color-text-secondary);
  }

  /* Empty state — same hero vocabulary as Home's "all caught up". */
  .tasks__empty {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: var(--space-3);
    text-align: center;
    padding: var(--space-7) var(--space-4);
  }
  .tasks__empty-title {
    margin: 0;
  }
  .tasks__empty-body {
    max-width: 360px;
    font-family: var(--font-system);
    font-size: 14px;
    line-height: 20px;
    color: var(--color-text-secondary);
  }

  .tasks__list {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
  }

  /* Task row — mirrors TaskCard's card surface, but with a stacked text
     block (title + description + meta) and a trailing action cluster. */
  .task-row {
    display: flex;
    align-items: flex-start;
    gap: var(--space-4);
    padding: var(--space-4);
    border-radius: var(--radius-card);
    background: var(--color-surface-app);
    border: 1px solid var(--color-card-border);
    width: 100%;
  }

  .task-row__text {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    flex: 1;
    min-width: 0;
  }
  .task-row__title {
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .task-row__description {
    color: var(--color-text-secondary);
  }
  .task-row__meta {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-1);
    text-transform: none;
    letter-spacing: 0;
    color: var(--color-text-muted);
  }
  .task-row__meta-sep {
    color: var(--color-text-muted);
  }

  .task-row__actions {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    flex-shrink: 0;
  }
</style>

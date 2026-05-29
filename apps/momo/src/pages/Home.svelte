<!--
  Home — the Telegram monitor task feed (the standalone /tasks page was
  folded in here). Renders the puffer daemon's `monitor_tasks[]` (see
  taskStore.svelte.ts → taskApi.ts → daemonClient.ts) as a flat list of
  TaskCards, with the Composer pinned to the bottom of the page column.

  Three render states mirror the old Tasks page: error (daemon not ready),
  empty ("all caught up"), and the list.

  Per-task interactions (the actions[] / Open affordances were deferred
  until chat moved onto the daemon — that migration is done, so they land
  here now). TaskCard is owned by another agent and not modified; we wrap
  each card in a clickable layer and delegate clicks off the rendered
  button classes:
    - .task-card__source (Bot icon)  → Open  → `/tasks show <id>`
    - .btn--primary (cream)          → the task's first action prompt
                                        (falls back to Open when the task
                                        carries no actions)
    - .btn--secondary (neutral)      → Ignore → task_monitor_ignore
    - bare card click                → Open
  Open and action both spin up a brand-new agent session via
  `createSessionFromText` and navigate into it — every task action gets its
  own thread (momo's router model; mirrors desktop's no-selected-session
  fallback). A single in-flight guard (`runningTaskId`) prevents double-fire
  while the session is being minted.
-->
<script lang="ts">
  import { onMount } from "svelte";

  import PageHeader from "../components/shell/PageHeader.svelte";
  import Composer from "../components/shell/Composer.svelte";
  import Mascot from "../components/common/Mascot.svelte";
  import TaskCard from "../components/home/TaskCard.svelte";

  import type { Task } from "../data/types";
  import { currentUser } from "../data/user";
  import { navigate } from "../router.svelte";
  import { pushToast } from "../lib/toast.svelte";
  import { authState } from "../lib/auth.svelte";
  import { createSessionFromText } from "../lib/agent/agentChat.svelte";
  import {
    monitorTasks,
    taskState,
    loadTasks,
    ignoreTask,
  } from "../lib/taskStore.svelte";
  import type {
    WorkflowMonitorTask,
    WorkflowMonitorTaskAction,
  } from "../lib/taskApi";

  onMount(() => {
    void loadTasks();
  });

  /** id of the task whose action/Open is currently minting a session. */
  let runningTaskId = $state<string | null>(null);

  let attentionSubtitle = $derived(
    monitorTasks.length === 0
      ? "You're all caught up"
      : `${monitorTasks.length} ${monitorTasks.length === 1 ? "thing needs" : "things need"} your attention today`
  );

  /** Time-of-day greeting keyed off the local hour. */
  function greetingForHour(hour: number): string {
    if (hour >= 5 && hour < 12) return "Good morning";
    if (hour >= 12 && hour < 18) return "Good afternoon";
    return "Good evening";
  }

  /** First name from the signed-in identity, capitalised. */
  function firstNameOf(displayName: string): string {
    const first = displayName.trim().split(/\s+/)[0] ?? "";
    if (!first) return "";
    return first.charAt(0).toUpperCase() + first.slice(1);
  }

  /**
   * "Good morning, Shun." — the greeting matches the local time of day and
   * the signed-in user. Identity falls back the same way the Sidebar does
   * (JWT name → email local-part → mock user) so the header stays populated
   * before auth resolves. Computed at mount; it doesn't tick with the clock.
   */
  let greetingTitle = $derived.by(() => {
    const displayName =
      authState.user?.name ||
      authState.user?.email?.split("@")[0] ||
      currentUser.name;
    const first = firstNameOf(displayName);
    const greeting = greetingForHour(new Date().getHours());
    return first ? `${greeting}, ${first}.` : `${greeting}.`;
  });

  /** Compact "May 29, 12:34 PM" style stamp for the card meta line. */
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

  /** Meta line under the title: prefer the description, else connection · status · time. */
  function cardMeta(mt: WorkflowMonitorTask): string {
    if (mt.description?.trim()) return mt.description.trim();
    const parts = [mt.monitor_connection ?? "telegram", mt.status];
    const updated = formatUpdated(mt.updated_at_ms);
    if (updated) parts.push(updated);
    return parts.join(" · ");
  }

  /**
   * Project a monitor task onto momo's `Task` shape so the existing TaskCard
   * renders it. The first action becomes the cream primary (or "Open" when
   * the task has no actions); Ignore is the neutral secondary; the Bot
   * source-icon button is the Open affordance. Additional actions beyond the
   * first aren't shown — the card only has one primary slot (accepted
   * trade-off for keeping the TaskCard visual).
   */
  function monitorTaskToCard(mt: WorkflowMonitorTask): Task {
    const firstAction = mt.actions?.[0];
    return {
      id: mt.task_id,
      icon: "message-circle",
      title: mt.subject,
      meta: cardMeta(mt),
      primaryAction: { label: firstAction ? firstAction.name : "Open", tone: "cream" },
      secondaryAction: { label: "Ignore", tone: "neutral" },
    };
  }

  /* ── Command builders (ported verbatim from puffer-desktop Tasks.svelte) ── */

  /** The prompt an action button hands the agent to act on this task. */
  function taskActionPrompt(mt: WorkflowMonitorTask, action: WorkflowMonitorTaskAction): string {
    return [
      `Act on monitored task ${mt.task_id}: ${mt.subject}`,
      "",
      "Task description:",
      mt.description,
      "",
      `Selected action: ${action.name}`,
      "",
      action.prompt,
      "",
      `When the action is fully handled, update task ${mt.task_id} with TaskUpdate status=completed. If you need more context, inspect the connector or ask the user.`,
    ].join("\n");
  }

  /** The slash command "Open" submits so the agent expands this task. */
  function taskShowCommand(mt: WorkflowMonitorTask): string {
    return `/tasks show ${mt.task_id}`;
  }

  /**
   * Mint a fresh agent session seeded with `command`, then navigate into it.
   * Guarded so a task's action/Open can't double-fire while the session is
   * being created. Errors surface as a toast (createSessionFromText hits the
   * daemon over WS).
   */
  async function runTaskCommand(mt: WorkflowMonitorTask, command: string): Promise<void> {
    if (runningTaskId !== null || !command.trim()) return;
    runningTaskId = mt.task_id;
    try {
      const sessionId = await createSessionFromText(command);
      navigate(`/agent/${sessionId}`);
    } catch {
      pushToast("Couldn't start a session for this task", "error");
    } finally {
      runningTaskId = null;
    }
  }

  /** Open = run `/tasks show <id>` in a new session. */
  function openTask(mt: WorkflowMonitorTask): void {
    void runTaskCommand(mt, taskShowCommand(mt));
  }

  /** Primary = run the first action's prompt (or Open when there are none). */
  function runPrimary(mt: WorkflowMonitorTask): void {
    const action = mt.actions?.[0];
    void runTaskCommand(mt, action ? taskActionPrompt(mt, action) : taskShowCommand(mt));
  }

  /**
   * Click delegation off the rendered button classes (TaskCard isn't
   * modified). Order matters: the Bot source button has no `.btn--*` class,
   * so it's matched first; the cream/neutral Buttons render as
   * `.btn--primary` / `.btn--secondary`.
   */
  function onCardClick(event: MouseEvent, mt: WorkflowMonitorTask): void {
    const target = event.target as HTMLElement | null;
    if (target?.closest(".task-card__source")) {
      event.stopPropagation();
      openTask(mt);
      return;
    }
    const btn = target?.closest("button");
    if (btn) {
      if (btn.classList.contains("btn--secondary")) {
        event.stopPropagation();
        void ignoreTask(mt.task_id);
        return;
      }
      if (btn.classList.contains("btn--primary")) {
        event.stopPropagation();
        runPrimary(mt);
        return;
      }
    }
    // Bare card click → Open.
    openTask(mt);
  }

  function onCardKey(event: KeyboardEvent, mt: WorkflowMonitorTask): void {
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      openTask(mt);
    }
  }

  function onMascotClick(): void {
    pushToast("Hi there!", "info");
  }
</script>

<PageHeader title={greetingTitle} subtitle={attentionSubtitle}>
  {#snippet accessory()}
    <button
      type="button"
      class="mascot-btn"
      aria-label="Say hi to Momo"
      onclick={onMascotClick}
    >
      <Mascot size="md" />
    </button>
  {/snippet}
</PageHeader>

<section class="feed">
  {#if taskState.error}
    <div class="feed__notice" role="alert">
      <p class="text-task-title feed__notice-title">Daemon not ready</p>
      <p class="feed__notice-body">{taskState.error}</p>
    </div>
  {:else if monitorTasks.length === 0}
    <div class="feed__empty">
      <Mascot size="lg" />
      <h2 class="feed__empty-title text-display">You're all caught up</h2>
      <p class="feed__empty-body">
        Momo will surface Telegram tasks here when they need your attention.
      </p>
    </div>
  {:else}
    <ul class="feed__list">
      {#each monitorTasks as mt (mt.task_id)}
        {@const card = monitorTaskToCard(mt)}
        <li>
          <div
            class="task-wrap"
            role="button"
            tabindex="0"
            aria-label={`Open ${mt.subject}`}
            onclick={(e) => onCardClick(e, mt)}
            onkeydown={(e) => onCardKey(e, mt)}
          >
            <TaskCard task={card} />
          </div>
        </li>
      {/each}
    </ul>
  {/if}
</section>

<Composer placeholder="Hi, Tomo. How's my luck today?" />

<style>
  .feed {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: var(--space-5);
    padding-bottom: var(--space-5);
  }

  /* Error notice — calm, not alarming; matches the card surface vocabulary
     (mirrors the old Tasks page's daemon-not-ready notice). */
  .feed__notice {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    padding: var(--space-4);
    border-radius: var(--radius-card);
    background: var(--color-surface-app);
    border: 1px solid var(--color-card-border);
  }
  .feed__notice-body {
    font-family: var(--font-system);
    font-size: var(--font-size-body);
    line-height: var(--line-height-body);
    color: var(--color-text-secondary);
  }

  /* Empty state — calm "all caught up" placeholder, same hero vocabulary as
     the other empty-state pages (ContactEmpty / HomeEmpty). Fills the feed
     column and centers; the bottom Composer stays as the action entry. */
  .feed__empty {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: var(--space-3);
    text-align: center;
    padding: var(--space-7) var(--space-4);
  }
  .feed__empty-title {
    margin: 0;
  }
  .feed__empty-body {
    max-width: 360px;
    font-family: var(--font-system);
    font-size: 14px;
    line-height: 20px;
    color: var(--color-text-secondary);
  }

  .feed__list {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
  }

  .task-wrap {
    display: block;
    cursor: pointer;
    border-radius: var(--radius-card);
  }
  .task-wrap:focus-visible {
    outline: 2px solid var(--color-action-cream-border);
    outline-offset: 2px;
  }

  .mascot-btn {
    appearance: none;
    background: transparent;
    border: 0;
    padding: 0;
    cursor: pointer;
    border-radius: 50%;
    transition: filter 120ms ease;
  }
  .mascot-btn:hover {
    filter: brightness(1.05);
  }
  .mascot-btn:focus-visible {
    outline: 2px solid var(--color-action-cream-border);
    outline-offset: 2px;
  }
</style>

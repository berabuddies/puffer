<!--
  Home — the task feed. Two serif-titled sections ("Work", "Life") with a
  neutral count chip beside the label, followed by the matching TaskCards.
  Composer is pinned to the bottom of the page column.

  Page-level interaction wiring (TaskCard component is owned by another
  agent, so we wrap each card in a clickable layer and use event delegation
  to intercept the inner "Ignore" button without modifying the component).
-->
<script lang="ts">
  import PageHeader from "../components/shell/PageHeader.svelte";
  import Composer from "../components/shell/Composer.svelte";
  import Mascot from "../components/common/Mascot.svelte";
  import SectionHeading from "../components/home/SectionHeading.svelte";
  import TaskCard from "../components/home/TaskCard.svelte";

  import type { Task, TaskGroup } from "../data/types";
  import { navigate } from "../router.svelte";
  import { pushToast } from "../lib/toast.svelte";
  import { resetOnboarding } from "../lib/auth.svelte";

  // Mock task feed was removed alongside the scripted demos. Home now
  // renders with empty Work/Life groups until the backend exposes a real
  // task source — the structure is preserved so the page lays out the
  // same way and existing tests/snapshots stay valid.
  let groups = $state<TaskGroup[]>([
    { label: "Work", count: 0, tasks: [] },
    { label: "Life", count: 0, tasks: [] }
  ]);

  // Collapsed group labels (toggle via clicking the count chip header).
  let collapsed = $state<Record<string, boolean>>({});

  function toggleGroup(label: string): void {
    collapsed[label] = !collapsed[label];
  }

  function removeTask(id: string): void {
    for (const g of groups) {
      const idx = g.tasks.findIndex((t) => t.id === id);
      if (idx >= 0) {
        g.tasks.splice(idx, 1);
        g.count = g.tasks.length;
        return;
      }
    }
  }

  /**
   * Click delegation: TaskCard renders a Bot icon button, an optional
   * "Ignore" Button, and a primary Button. We can't modify the component,
   * so we sniff `event.target` on the wrapper and decide:
   *   - Ignore button → toast + remove task (stop propagation)
   *   - Bot/source button → snooze toast (stop propagation)
   *   - Primary button → let the component handle nav/log itself; we still
   *     fire a toast as feedback when there's no navigateTo set.
   *   - Anywhere else on the card → navigate to /agent/:taskId
   */
  function onCardClick(event: MouseEvent, task: Task): void {
    const target = event.target as HTMLElement | null;
    const btn = target?.closest("button");
    if (btn) {
      const label = (btn.textContent ?? "").trim();
      if (label === "Ignore") {
        event.stopPropagation();
        pushToast(`Ignored "${task.title}"`, "info");
        removeTask(task.id);
        return;
      }
      if (btn.getAttribute("aria-label") === "Open in source app") {
        // Bot icon — use as the "snooze/share" icon per spec.
        event.stopPropagation();
        pushToast("Snoozed", "info");
        return;
      }
      // Primary action button. Component already handles navigate when a
      // navigateTo exists; otherwise we still want feedback + navigate.
      if (!task.primaryAction.navigateTo) {
        pushToast(`Action: ${task.primaryAction.label}`, "success");
      }
      // Always navigate so the user lands on the conversation page.
      navigate(`/agent/${task.id}`);
      return;
    }
    // Bare card click.
    navigate(`/agent/${task.id}`);
  }

  function onCardKey(event: KeyboardEvent, task: Task): void {
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      navigate(`/agent/${task.id}`);
    }
  }

  function onMascotClick(): void {
    pushToast("Hi there!", "info");
  }

  // Dev-only state toggles (lives in the header accessory).
  function showEmpty(): void {
    navigate("/home/empty");
  }
  function showFull(): void {
    navigate("/home");
  }
  function reonboard(): void {
    resetOnboarding();
    navigate("/onboarding/where");
  }
</script>

<PageHeader title="Good morning, Yuna." subtitle="5 things need your attention today">
  {#snippet accessory()}
    <div class="header-accessory">
      <div class="dev-toggles" aria-label="Dev state toggles">
        <button type="button" class="dev-link" onclick={showEmpty}>Show empty</button>
        <button type="button" class="dev-link" onclick={reonboard}>Reset onboarding</button>
        <button type="button" class="dev-link" onclick={showFull}>Show full</button>
      </div>
      <button
        type="button"
        class="mascot-btn"
        aria-label="Say hi to Momo"
        onclick={onMascotClick}
      >
        <Mascot size="md" />
      </button>
    </div>
  {/snippet}
</PageHeader>

<section class="feed">
  {#each groups as group (group.label)}
    <div class="feed__group">
      <button
        type="button"
        class="feed__heading-btn"
        aria-expanded={!collapsed[group.label]}
        onclick={() => toggleGroup(group.label)}
      >
        <SectionHeading label={group.label} count={group.count} />
      </button>
      {#if !collapsed[group.label]}
        <ul class="feed__list">
          {#each group.tasks as task (task.id)}
            <li>
              <div
                class="task-wrap"
                role="button"
                tabindex="0"
                aria-label={`Open ${task.title}`}
                onclick={(e) => onCardClick(e, task)}
                onkeydown={(e) => onCardKey(e, task)}
              >
                <TaskCard {task} />
              </div>
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  {/each}
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

  .feed__group {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
  }

  .feed__heading-btn {
    appearance: none;
    background: transparent;
    border: 0;
    padding: 0;
    margin: 0;
    text-align: left;
    cursor: pointer;
    align-self: flex-start;
    border-radius: var(--radius-control);
  }
  .feed__heading-btn:focus-visible {
    outline: 2px solid var(--color-action-cream-border);
    outline-offset: 2px;
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

  .header-accessory {
    display: flex;
    align-items: center;
    gap: var(--space-3);
  }

  .dev-toggles {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }

  .dev-link {
    appearance: none;
    background: transparent;
    border: 0;
    padding: 2px 6px;
    color: var(--color-text-muted);
    font-family: var(--font-system);
    font-size: 11px;
    line-height: 14px;
    font-weight: var(--font-weight-medium);
    cursor: pointer;
    border-radius: var(--radius-control);
    transition: background-color 120ms ease, color 120ms ease;
  }
  .dev-link:hover {
    background: var(--color-surface-rail);
    color: var(--color-text-primary);
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

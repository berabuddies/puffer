<!--
  TaskCard — single task row on Home.

  Anatomy (DESIGN_SYSTEM.md → Task card):
    [IconBlock 40×40 neutral, line icon resolved from task.icon]
    [Stack: title (15/22 medium) + meta (13/18 regular)]
    [Action cluster pinned right: 32×32 source-app icon button (Bot)
       + secondary "Ignore" (when present) + primary cream action]

  All button clicks log via console.log — wiring comes later.
-->
<script lang="ts">
  import {
    Bot,
    Cake,
    Calendar,
    CreditCard,
    Mail,
    MessageCircle,
    Phone,
    ShoppingBag,
    User,
    Users,
    Wallet,
    Briefcase
  } from "lucide-svelte";

  import IconBlock from "../common/IconBlock.svelte";
  import Button from "../common/Button.svelte";
  import { navigate } from "../../router.svelte";
  import type { IconName, Task } from "../../data/types";

  interface Props {
    task: Task;
  }

  let { task }: Props = $props();

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  type IconComponent = any;

  const iconMap: Record<IconName, IconComponent> = {
    calendar: Calendar,
    "message-circle": MessageCircle,
    cake: Cake,
    "shopping-bag": ShoppingBag,
    mail: Mail,
    phone: Phone,
    wallet: Wallet,
    "credit-card": CreditCard,
    user: User,
    users: Users,
    briefcase: Briefcase
  };

  let leadingIcon = $derived(iconMap[task.icon] ?? Calendar);

  function handlePrimary(): void {
    if (task.primaryAction.navigateTo) {
      navigate(task.primaryAction.navigateTo);
      return;
    }
    console.log("[TaskCard] primary action", task.id, task.primaryAction.label);
  }

  function handleSecondary(): void {
    console.log("[TaskCard] secondary action", task.id, task.secondaryAction?.label);
  }

  function handleSourceApp(): void {
    console.log("[TaskCard] source-app", task.id);
  }
</script>

<article class="task-card">
  <IconBlock icon={leadingIcon} />

  <div class="task-card__body">
    <div class="task-card__text">
      <h3 class="text-task-title task-card__title">{task.title}</h3>
      <p class="text-body-compact task-card__meta">{task.meta}</p>
    </div>

    <div class="task-card__actions">
      <button
        class="task-card__source"
        type="button"
        aria-label="Open in source app"
        onclick={handleSourceApp}
      >
        <Bot size={16} strokeWidth={1.75} aria-hidden="true" />
      </button>

      {#if task.secondaryAction}
        <Button
          variant="secondary"
          size="sm"
          label={task.secondaryAction.label}
          onclick={handleSecondary}
        />
      {/if}

      <Button
        variant="primary"
        size="sm"
        label={task.primaryAction.label}
        onclick={handlePrimary}
      />
    </div>
  </div>
</article>

<style>
  .task-card {
    display: flex;
    align-items: flex-start;
    gap: var(--space-4);
    padding: var(--space-4);
    border-radius: var(--radius-card);
    background: var(--color-surface-app);
    border: 1px solid var(--color-card-border);
    width: 100%;
  }

  .task-card__body {
    display: flex;
    align-items: center;
    gap: var(--space-4);
    flex: 1;
    min-width: 0;
  }

  .task-card__text {
    display: flex;
    flex-direction: column;
    gap: 2px;
    flex: 1;
    min-width: 0;
  }

  .task-card__title {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .task-card__meta {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .task-card__actions {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    flex-shrink: 0;
  }

  .task-card__source {
    width: var(--height-button-card);
    height: var(--height-button-card);
    border-radius: var(--radius-pill);
    background: var(--color-surface-rail);
    color: var(--color-text-primary);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    transition: background-color 120ms ease;
  }
  .task-card__source:hover {
    background: var(--color-card-border);
  }
</style>

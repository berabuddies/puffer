<!--
  ActivityRow — one row in the Wallet "Recent activity" list.

  Anatomy (matches DESIGN_SYSTEM Task card geometry):
    [IconBlock 40×40 neutral, directional Lucide icon — in / out]
    [Stack: title (15/22 medium) + meta (13/18 regular)]
    [Right stack pinned top-right: amount (15/22 medium) + time (13/18 muted)]
-->
<script lang="ts">
  import { ArrowDown, ArrowUpRight } from "lucide-svelte";

  import IconBlock from "../common/IconBlock.svelte";
  import type { WalletActivity } from "../../data/types";

  interface Props {
    activity: WalletActivity;
  }

  let { activity }: Props = $props();

  let icon = $derived(activity.direction === "in" ? ArrowDown : ArrowUpRight);
  let meta = $derived(activity.description ?? activity.category);
  let stamp = $derived(activity.time ?? activity.date);
</script>

<article class="activity">
  <IconBlock {icon} />

  <div class="activity__body">
    <div class="activity__text">
      <h3 class="text-task-title">{activity.merchant}</h3>
      <p class="text-body-compact">{meta}</p>
    </div>

    <div class="activity__amount">
      <span class="text-task-title">{activity.amount}</span>
      <span class="activity__stamp">{stamp}</span>
    </div>
  </div>
</article>

<style>
  .activity {
    display: flex;
    align-items: flex-start;
    gap: var(--space-4);
    padding: var(--space-4);
    border-radius: var(--radius-card);
    background: var(--color-surface-app);
    border: 1px solid var(--color-card-border);
    width: 100%;
  }

  .activity__body {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: var(--space-4);
    flex: 1;
    min-width: 0;
  }

  .activity__text {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
    flex: 1;
  }

  .activity__amount {
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    gap: 2px;
    flex-shrink: 0;
  }

  .activity__stamp {
    font-family: var(--font-system);
    font-size: var(--font-size-body);
    line-height: var(--line-height-body);
    color: var(--color-text-muted);
  }
</style>

<script lang="ts">
  import Icon from "../../design/Icon.svelte";
  import type { PermissionTimelineItem } from "../../types";

  type Props = {
    item: PermissionTimelineItem;
    disabled?: boolean;
    onResolve: (permissionId: string, choice: string) => void;
  };

  let { item, disabled = false, onResolve }: Props = $props();

  function variantFor(choice: string): "default" | "outline" | "ghost" {
    const n = choice.toLowerCase();
    if (n.includes("deny")) return "ghost";
    if (n.includes("always") || n.includes("session")) return "outline";
    return "default";
  }
</script>

<div class="pf-approval">
  <div style="display: flex; align-items: center; gap: 8px; font-size: 12.5px; font-weight: 500;">
    <Icon name="bolt" size={14} color="var(--puffer-accent)" />
    Approval needed
    {#if item.toolName}
      <span style="color: var(--muted-foreground); font-weight: 400; font-family: var(--font-mono); font-size: 12px;">
        · {item.toolName}
      </span>
    {/if}
  </div>
  <div class="pf-approval-row">
    <span class="what">
      {item.permissionDialog.reason || item.summary || item.title}
    </span>
  </div>
  <div class="pf-approval-actions">
    {#each item.choices as choice (choice)}
      <button
        type="button"
        class="sc-btn"
        data-variant={variantFor(choice)}
        data-size="sm"
        disabled={disabled}
        onclick={() => onResolve(item.id, choice)}
      >{choice}</button>
    {/each}
  </div>
</div>

<style>
  .pf-approval {
    border: 1px solid color-mix(in oklab, var(--puffer-accent) 40%, var(--border));
    background: color-mix(in oklab, var(--puffer-accent) 6%, var(--background));
    border-radius: 10px;
    padding: 12px 14px;
    display: flex;
    flex-direction: column;
    gap: 10px;
    font-size: 13px;
  }
  .pf-approval-row {
    display: flex;
    align-items: center;
    gap: 10px;
    font-family: var(--font-mono);
    font-size: 12px;
  }
  .pf-approval-row .what { flex: 1; color: var(--foreground); }
  .pf-approval-actions { display: flex; gap: 6px; align-items: center; flex-wrap: wrap; }
</style>

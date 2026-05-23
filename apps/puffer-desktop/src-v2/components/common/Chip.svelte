<!--
  Chip — small pill used for counts, statuses, cream call-to-actions, and
  multi-select onboarding chips. Variants:
    - count            gray pill, muted text  (Work / Life counts)
    - status-connected gray pill, secondary text  ("Connected")
    - cream            cream pill, cream text  ("Connect", "Top up crypto")
    - select           hollow / filled pill for onboarding multi-select
-->
<script lang="ts">
  type Variant = "count" | "status-connected" | "cream" | "select";

  interface Props {
    label: string;
    variant?: Variant;
    selected?: boolean;
    onclick?: (event: MouseEvent) => void;
    title?: string;
  }

  let { label, variant = "count", selected = false, onclick, title }: Props = $props();

  let isButton = $derived(typeof onclick === "function");
</script>

{#if isButton}
  <button
    class="chip chip--{variant}"
    class:chip--selected={selected}
    type="button"
    {title}
    onclick={onclick}
  >
    {label}
  </button>
{:else}
  <span class="chip chip--{variant}" class:chip--selected={selected} {title}>{label}</span>
{/if}

<style>
  .chip {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    padding: 6px 14px;
    border-radius: var(--radius-pill);
    border: 1px solid transparent;
    font-family: var(--font-sans);
    font-size: var(--font-size-button);
    line-height: var(--line-height-button);
    font-weight: var(--font-weight-medium);
    white-space: nowrap;
    cursor: default;
  }

  button.chip {
    cursor: pointer;
    transition: background-color 120ms ease, color 120ms ease, border-color 120ms ease;
  }

  .chip--count {
    background: var(--color-surface-rail);
    color: var(--color-text-muted);
    padding: 2px 10px; /* counts are tighter than other chips */
  }

  .chip--status-connected {
    background: var(--color-surface-rail);
    color: var(--color-text-secondary);
  }

  .chip--cream {
    background: var(--color-action-cream);
    color: var(--color-action-cream-text);
  }
  button.chip--cream:hover {
    filter: brightness(0.97);
  }

  .chip--select {
    background: var(--color-surface-app);
    color: var(--color-text-primary);
    border-color: var(--color-input-border);
  }
  button.chip--select:hover {
    background: var(--color-surface-rail);
  }
  .chip--select.chip--selected {
    background: var(--color-text-primary);
    color: var(--color-surface-app);
    border-color: var(--color-text-primary);
  }
</style>

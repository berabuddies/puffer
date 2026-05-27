<!--
  KpiCard — compact stat card used in the Wallet header strip.
  Anatomy: eyebrow label (12 caps muted) + value (15 medium primary).
  Optional trailing snippet for inline accessories (e.g. KYC verified check).
-->
<script lang="ts">
  import type { Snippet } from "svelte";

  interface Props {
    label: string;
    value: string;
    trailing?: Snippet;
  }

  let { label, value, trailing }: Props = $props();
</script>

<article class="kpi">
  <span class="kpi__label">{label}</span>
  <div class="kpi__value-row">
    <span class="text-task-title kpi__value">{value}</span>
    {#if trailing}
      <span class="kpi__trailing">{@render trailing()}</span>
    {/if}
  </div>
</article>

<style>
  .kpi {
    display: flex;
    flex-direction: column;
    flex: 1 1 0;
    min-width: 0;
    gap: var(--space-1);
    padding: var(--space-4);
    border-radius: var(--radius-card);
    background: var(--color-surface-app);
    border: 1px solid var(--color-card-border);
    height: 80px;
    justify-content: center;
  }

  /* Title-case eyebrow (matches Paper 1K3-0). 12/16 semibold muted, no caps
     transform — the labels read "Monthly spend", not "MONTHLY SPEND". */
  .kpi__label {
    font-family: var(--font-system);
    font-size: var(--font-size-button);
    line-height: var(--line-height-button);
    font-weight: var(--font-weight-semibold);
    color: var(--color-text-muted);
  }

  .kpi__value-row {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    min-width: 0;
  }

  .kpi__value {
    color: var(--color-text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .kpi__trailing {
    display: inline-flex;
    align-items: center;
    flex-shrink: 0;
    color: #16a34a;
  }
</style>

<!--
  ResultCard — left-aligned white card confirming the outcome of a flow.

  Anatomy (Paper artboard 1Y2-0 / 1Y3-0 / 1YA-0 / 1YI-0):
    - Outer: 540 max width, padding 13/15, gap 9, white bg, 1px
      var(--color-input-border) border, asymmetric radius 4/16/16/16 (same
      "agent side" radius as OptionsCard).
    - Header row: 32×32 check icon block (radius 9, bg #E5EFE0 — pale green)
      with a green Check stroke + stacked title (14/20 medium #161616) and
      meta (12/16 #6F6F6F, --color-text-muted). gap 10px between block and
      text stack.
    - Detail panel: bg --color-surface-rail, radius 12, padding 10/13, gap 6
      column. Title row 14px medium; facts row 13px gap 14 secondary text;
      notes 13/19 secondary.
    - Action row: 32px tall, right-aligned, gap 8. Primary cream + secondary
      neutral. Both pill, 32 high, padding-inline 16, label 12/16 Inter
      medium per the existing common Button component.

  Soft green is intentionally inline (not a token): the design system reserves
  cream as the only warm accent, so we don't promote green to a token —
  this is a confirmation-only badge color.
-->
<script lang="ts">
  import { Check } from "lucide-svelte";

  import Button from "../common/Button.svelte";
  import type { ResultStep } from "../../data/types";

  interface Props {
    step: ResultStep;
  }

  let { step }: Props = $props();

  function handleAction(label: string): void {
    console.log("[ResultCard] action", label);
  }
</script>

<article class="result-card">
  <header class="result-card__head">
    <span class="result-card__check" aria-hidden="true">
      <Check size={18} strokeWidth={2} color="#3f8a3a" />
    </span>
    <div class="result-card__head-text">
      <h3 class="result-card__title">{step.title}</h3>
      <p class="result-card__subtitle">{step.subtitle}</p>
    </div>
  </header>

  <div class="result-card__detail">
    <p class="result-card__detail-title">{step.detail.title}</p>
    {#if step.detail.facts.length > 0}
      <div class="result-card__detail-facts">
        {#each step.detail.facts as fact}
          <span>{fact}</span>
        {/each}
      </div>
    {/if}
    {#if step.detail.notes}
      <p class="result-card__detail-notes">{step.detail.notes}</p>
    {/if}
  </div>

  {#if step.actions.length > 0}
    <div class="result-card__actions">
      {#each step.actions as action}
        <Button
          variant={action.tone === "cream" ? "primary" : "secondary"}
          size="sm"
          label={action.label}
          onclick={() => handleAction(action.label)}
        />
      {/each}
    </div>
  {/if}
</article>

<style>
  .result-card {
    display: flex;
    flex-direction: column;
    width: 100%;
    max-width: 540px;
    background: var(--color-surface-app);
    border: 1px solid var(--color-input-border);
    border-radius: 4px 16px 16px 16px;
    padding: 13px 15px;
    gap: 9px;
  }

  .result-card__head {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .result-card__check {
    width: 32px;
    height: 32px;
    border-radius: 9px;
    background: #e5efe0;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
  }

  .result-card__head-text {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }

  .result-card__title {
    font-family: var(--font-system);
    font-size: 14px;
    line-height: 20px;
    font-weight: var(--font-weight-medium);
    color: var(--color-text-primary);
    margin: 0;
  }

  .result-card__subtitle {
    font-family: var(--font-system);
    font-size: var(--font-size-button);
    line-height: var(--line-height-button);
    font-weight: var(--font-weight-regular);
    color: var(--color-text-muted);
    margin: 0;
  }

  .result-card__detail {
    display: flex;
    flex-direction: column;
    gap: 6px;
    background: var(--color-surface-rail);
    border-radius: 12px;
    padding: 10px 13px;
  }

  .result-card__detail-title {
    font-family: var(--font-system);
    font-size: 14px;
    line-height: 18px;
    font-weight: var(--font-weight-medium);
    color: var(--color-text-primary);
    margin: 0;
  }

  .result-card__detail-facts {
    display: flex;
    flex-wrap: wrap;
    gap: 14px;
    font-family: var(--font-system);
    font-size: var(--font-size-body);
    line-height: var(--line-height-button);
    font-weight: var(--font-weight-regular);
    color: var(--color-text-primary);
  }

  .result-card__detail-notes {
    font-family: var(--font-system);
    font-size: var(--font-size-body);
    line-height: 19px;
    font-weight: var(--font-weight-regular);
    color: var(--color-text-secondary);
    margin: 0;
  }

  .result-card__actions {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: var(--space-2);
    margin-top: var(--space-1);
  }
</style>

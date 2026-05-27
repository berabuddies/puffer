<!--
  StepCard — one of the three numbered cards on the Wallet KYC empty state.
  Anatomy: eyebrow (12 caps muted, with leading number) + body (13/18 secondary).
-->
<script lang="ts">
  interface Props {
    eyebrow: string;
    body: string;
  }

  let { eyebrow, body }: Props = $props();
</script>

<article class="step">
  <span class="step__eyebrow">{eyebrow}</span>
  <p class="text-body-compact">{body}</p>
</article>

<style>
  .step {
    display: flex;
    flex-direction: column;
    flex: 1 1 0;
    min-width: 0;
    gap: var(--space-2);
    padding: var(--space-4);
    border-radius: var(--radius-card);
    background: var(--color-surface-app);
    border: 1px solid var(--color-card-border);
    height: 116px;
  }

  /* Title-case eyebrow (matches Paper 1QA-0). Uses the 12/16 semibold rhythm
     of .text-eyebrow but skips the uppercase + tracking transform so the
     "1 · Verify identity" reads naturally. */
  .step__eyebrow {
    font-family: var(--font-system);
    font-size: var(--font-size-button);
    line-height: var(--line-height-button);
    font-weight: var(--font-weight-semibold);
    color: var(--color-text-muted);
  }
</style>

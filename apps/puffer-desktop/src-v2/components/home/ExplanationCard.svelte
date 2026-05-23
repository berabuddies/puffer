<!--
  ExplanationCard — small "Why need apps" card used on HomeEmpty.
  Width is fluid (3 cards share a row with 8px gap), 12px padding,
  10px radius, soft gray bg, hairline border.
-->
<script lang="ts">
  interface Props {
    title: string;
    body: string;
  }

  let { title, body }: Props = $props();
</script>

<article class="explanation-card">
  <h3 class="explanation-card__title">{title}</h3>
  <p class="explanation-card__body">{body}</p>
</article>

<style>
  .explanation-card {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    flex: 1;
    min-width: 0;
    padding: var(--space-3);
    border-radius: 10px;
    background: #f8f8f8;
    border: 1px solid var(--color-card-border);
  }

  .explanation-card__title {
    font-family: var(--font-sans);
    font-size: var(--font-size-button);
    line-height: var(--line-height-button);
    font-weight: var(--font-weight-semibold);
    color: var(--color-text-primary);
  }

  .explanation-card__body {
    font-family: var(--font-sans);
    font-size: var(--font-size-button);
    line-height: 17px;
    font-weight: var(--font-weight-regular);
    color: var(--color-text-muted);
  }
</style>

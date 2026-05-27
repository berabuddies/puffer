<!--
  OptionsCard — left-aligned white card with a heading, a stack of option
  rows, and an optional footnote.

  Anatomy (Paper artboard 1XL-0 / 1XN-0):
    - Outer: 540 max width, padding 13/15, gap 9, white bg, 1px
      var(--color-input-border) border, asymmetric radius 4/16/16/16 (matches
      the user-bubble inversion — agent-side cards have the top-LEFT corner
      sharpened, suggesting the speech tail on the left).
    - Heading: 14/20 system medium #161616.
    - Row stack: column, gap 7px.
    - Row: 34px tall, radius 10, padding 8/11, justify space-between, flex
      align-center. Two variants:
        · highlighted ("Best fit"): bg #FFF7E8, border #F2DCA7 — the warm
          cream-tint variant of the row. Trailing text shows the badge in
          cream-700 (#8A650E), weight 600.
        · plain: bg white, border --color-card-border, trailing text in
          --color-text-secondary, weight regular.
    - Footnote: 13/17 #525252, regular.
-->
<script lang="ts">
  import type { ConversationOption } from "../../data/types";

  interface Props {
    intro: string;
    options: ConversationOption[];
    footnote?: string;
  }

  let { intro, options, footnote }: Props = $props();
</script>

<article class="options-card">
  <p class="options-card__intro">{intro}</p>

  <div class="options-card__rows">
    {#each options as option}
      <div class="row" class:row--highlight={option.highlighted}>
        <span class="row__primary">{option.primary}</span>
        <span class="row__trailing" class:row__trailing--badge={option.highlighted}>
          {option.highlighted ? (option.badge ?? option.trailing) : option.trailing}
        </span>
      </div>
    {/each}
  </div>

  {#if footnote}
    <p class="options-card__footnote">{footnote}</p>
  {/if}
</article>

<style>
  .options-card {
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

  .options-card__intro {
    font-family: var(--font-system);
    font-size: 14px;
    line-height: 20px;
    font-weight: var(--font-weight-medium);
    color: var(--color-text-primary);
    margin: 0;
  }

  .options-card__rows {
    display: flex;
    flex-direction: column;
    gap: 7px;
  }

  .row {
    height: 34px;
    display: flex;
    align-items: center;
    justify-content: space-between;
    border-radius: 10px;
    padding: 8px 11px;
    background: var(--color-surface-app);
    border: 1px solid var(--color-card-border);
  }

  .row--highlight {
    background: #fff7e8;
    border-color: #f2dca7;
  }

  .row__primary {
    font-family: var(--font-system);
    font-size: var(--font-size-body);
    line-height: var(--line-height-button);
    font-weight: var(--font-weight-regular);
    color: var(--color-text-primary);
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .row__trailing {
    font-family: var(--font-system);
    font-size: var(--font-size-body);
    line-height: var(--line-height-button);
    font-weight: var(--font-weight-regular);
    color: var(--color-text-secondary);
    flex-shrink: 0;
    margin-left: var(--space-3);
  }

  .row__trailing--badge {
    font-weight: var(--font-weight-semibold);
    color: #8a650e;
  }

  .options-card__footnote {
    font-family: var(--font-system);
    font-size: var(--font-size-body);
    line-height: 17px;
    font-weight: var(--font-weight-regular);
    color: var(--color-text-secondary);
    margin: 0;
  }
</style>

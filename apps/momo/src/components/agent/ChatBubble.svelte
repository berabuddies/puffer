<!--
  ChatBubble — right-aligned user message bubble.

  Anatomy (Paper artboard 1X7-0 / 1X6-0):
    - Outer row: full width, flex, justify-content: end so the bubble hugs
      the right edge of the 760 content column.
    - Bubble: max-width 500px, background var(--color-surface-rail), asymmetric
      radius 16/4/16/16 (top-right corner sharper to suggest the speech tail
      on the user side), padding 11px / 15px.
    - Text: 14px / 20px system, weight 400, color --color-text-primary.

  Only the user variant exists today — the agent's plain text is rendered by
  AgentText.svelte, not as a bubble, per the design.
-->
<script lang="ts">
  interface Props {
    text: string;
  }

  let { text }: Props = $props();
</script>

<div class="bubble-row">
  <div class="bubble">
    <p class="bubble__text">{text}</p>
  </div>
</div>

<style>
  .bubble-row {
    display: flex;
    justify-content: flex-end;
    width: 100%;
  }

  .bubble {
    max-width: 500px;
    background: var(--color-surface-rail);
    border-radius: 16px 4px 16px 16px;
    padding: 11px 15px;
  }

  .bubble__text {
    font-family: var(--font-system);
    font-size: 14px;
    line-height: 20px;
    font-weight: var(--font-weight-regular);
    color: var(--color-text-primary);
    margin: 0;
    white-space: pre-wrap;
    word-wrap: break-word;
  }
</style>

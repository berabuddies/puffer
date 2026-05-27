<!--
  ChatBubble — right-aligned user message bubble.

  Anatomy (Paper artboard 1X7-0 / 1X6-0):
    - Outer row: full width, flex, justify-content: end so the bubble hugs
      the right edge of the 760 content column.
    - Bubble: max-width 500px, background var(--color-surface-rail), asymmetric
      radius 16/4/16/16 (top-right corner sharper to suggest the speech tail
      on the user side), padding 11px / 15px.
    - Text: rendered through MessageBody (Markdown-aware). Plain prose stays
      visually identical to the old `<p>` because MessageBody emits a
      single paragraph for non-Markdown input.

  Optional `createdAt` (ms epoch) renders an HH:MM meta row underneath
  the body. Hidden when unset / 0 / undefined (e.g. historical bubbles
  from `load_session_detail` where the DTO has no timestamp).
-->
<script lang="ts">
  import MessageBody from "../common/MessageBody.svelte";
  import { formatTime } from "../../lib/timeFormat";

  interface Props {
    text: string;
    createdAt?: number;
  }

  let { text, createdAt }: Props = $props();
</script>

<div class="bubble-row">
  <div class="bubble">
    <MessageBody body={text} />
    {#if createdAt}
      <span class="chat-bubble__time">{formatTime(createdAt)}</span>
    {/if}
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
    display: flex;
    flex-direction: column;
  }

  .chat-bubble__time {
    display: block;
    margin-top: 4px;
    font-family: var(--font-system);
    font-size: 11px;
    line-height: 14px;
    color: var(--color-text-secondary);
    text-align: right;
  }
</style>

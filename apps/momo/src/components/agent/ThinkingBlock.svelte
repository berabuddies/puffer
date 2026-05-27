<!--
  ThinkingBlock — streaming "thinking" surface.

  Dev builds (vite dev / Playwright): show the trimmed thinking text in a
  soft-grey italic block so engineers can see what the model is deliberating
  about. Prod builds (vite build): collapse to a fixed `"I'm working on it
  now..."` pill so consumers don't read raw chain-of-thought.

  Toggle lives in `lib/debugFlags.ts` (`SHOW_RAW_AGENT_ACTIVITY`). Vite
  tree-shakes the unused branch at build time so prod ships zero overhead.
-->
<script lang="ts">
  import ToolBlock from "./ToolBlock.svelte";
  import { SHOW_RAW_AGENT_ACTIVITY } from "../../lib/debugFlags";

  interface Props {
    text: string;
    pending: boolean;
  }
  let { text, pending }: Props = $props();

  let trimmed = $derived(text.trim());
</script>

{#if SHOW_RAW_AGENT_ACTIVITY && trimmed.length > 0}
  <div class="thinking-block" data-pending={pending} data-testid="thinking-block">
    <span class="thinking-block__label">Thinking</span>
    <p class="thinking-block__text">{trimmed}</p>
  </div>
{:else}
  <div data-testid="thinking-pill">
    <ToolBlock icon="bot" label="I'm working on it now..." />
  </div>
{/if}

<style>
  .thinking-block {
    max-width: 600px;
    padding: 8px 12px;
    background: var(--color-surface-rail);
    border-radius: 10px;
    color: var(--color-text-muted);
    font-family: var(--font-system);
    font-size: 13px;
    line-height: 18px;
    font-style: italic;
  }
  .thinking-block__label {
    display: block;
    font-size: 11px;
    font-style: normal;
    font-weight: var(--font-weight-medium);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    margin-bottom: 4px;
  }
  .thinking-block__text {
    margin: 0;
    white-space: pre-wrap;
  }
</style>

<!--
  AgentText — left-aligned plain text block for the agent's spoken lines.

  Per the design (Paper artboard 1XC-0), the agent does not use a bubble — it
  is the page voice. Text is 14px / 20px system, --color-text-primary, left-
  aligned, full content width.
-->
<script lang="ts">
  interface Props {
    text: string;
  }

  let { text }: Props = $props();
</script>

<p class="agent-text">{text}</p>

<style>
  .agent-text {
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

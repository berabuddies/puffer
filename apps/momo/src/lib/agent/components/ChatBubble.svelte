<script lang="ts">
  import MessageBody from "./MessageBody.svelte";
  import type { MessageTimelineItem } from "../types";
  let { item }: { item: MessageTimelineItem } = $props();
  let role = $derived(item.kind); // user | assistant | system | command
  let isError = $derived(item.status === "error");
</script>

<div class="mb-row" data-role={role} data-error={isError}>
  {#if role === "command"}
    <div class="mb-cmd">{item.body || item.title}</div>
  {:else if role === "system"}
    <div class="mb-system">{item.body || item.summary || item.title}</div>
  {:else}
    <div class="mb-bubble">
      <div class="pf-msg-text"><MessageBody body={item.body} /></div>
    </div>
  {/if}
</div>

<script lang="ts">
  import { AGENT_STATE_LABELS, type MockAgent } from "../../data/mockProjects";

  type Props = { a: MockAgent; onOpen?: () => void };
  let { a, onOpen }: Props = $props();

  let title = $derived(a.title || a.name || "New Session");
  let statusLabel = $derived(AGENT_STATE_LABELS[a.status] ?? a.status);
</script>

<button
  type="button"
  class="pf-pw-agent"
  data-status={a.status}
  onclick={onOpen}
  title={`${title} - ${statusLabel} - ${a.elapsed}`}
>
  <span class="title">{title}</span>
  <span class="status-pill" data-status={a.status}>{statusLabel}</span>
  <span class="activity">{a.elapsed}</span>
</button>

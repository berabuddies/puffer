<script lang="ts">
  import type { ToolTimelineItem } from "../../types";
  import ToolCard from "./ToolCard.svelte";

  type Props = {
    item: ToolTimelineItem;
    sessionId?: string | null;
    defaultCollapsed?: boolean;
    prompt?: string;
  };

  let {
    item,
    sessionId = null,
    defaultCollapsed = true
  }: Props = $props();
</script>

<ToolCard {item} {sessionId} {defaultCollapsed} />

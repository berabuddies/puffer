<!--
  ToolCallPill — visual surface for a single backend tool call.

  Wraps the existing `ToolBlock.svelte` primitive (a white 36px pill) and
  layers on a `data-status` state machine so the pill can read "running",
  "success", or "failed" at a glance. Friendly copy comes from
  `toolLabels.ts`; unmapped tool ids fall back to debug-flag-controlled
  text (raw `Calling: <toolId>` in dev, fixed "I'm working on it now..."
  in prod).

  Test selectors:
    - `[data-testid="tool-pill"]`         — generic addressing
    - `[data-status="running|success|failed"]` — state assertions
    - `[data-tool-id="<id>"]`             — disambiguate when multiple
    - `[data-call-id="<id>"]`             — identity across status flips
-->
<script lang="ts">
  import ToolBlock from "./ToolBlock.svelte";
  import { lookupToolLabel } from "../../lib/toolLabels";
  import { SHOW_RAW_AGENT_ACTIVITY } from "../../lib/debugFlags";

  interface Props {
    toolId: string;
    callId: string;
    status: "running" | "success" | "failed";
  }
  let { toolId, callId, status }: Props = $props();

  let mapped = $derived(lookupToolLabel(toolId));
  let label = $derived(
    mapped?.label
      ?? (SHOW_RAW_AGENT_ACTIVITY ? `Calling: ${toolId}` : "I'm working on it now...")
  );
  let icon = $derived(mapped?.icon ?? "bot");
</script>

<div
  class="tool-call-pill"
  data-testid="tool-pill"
  data-status={status}
  data-tool-id={toolId}
  data-call-id={callId}
>
  <ToolBlock {icon} {label} />
</div>

<style>
  .tool-call-pill {
    width: fit-content;
    max-width: 100%;
  }
  .tool-call-pill[data-status="running"] {
    opacity: 0.85;
  }
  .tool-call-pill[data-status="failed"] :global(.tool-block) {
    border-color: var(--color-danger, #c0392b);
    color: var(--color-danger, #c0392b);
  }
</style>

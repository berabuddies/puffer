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
  import type { IconName } from "../../data/types";

  interface Props {
    toolId: string;
    callId: string;
    status: "running" | "success" | "failed";
  }
  let { toolId, callId, status }: Props = $props();

  let mapped = $derived(lookupToolLabel(toolId));

  // Per-status icon. Mapped tools keep their domain icon while running and
  // swap to status icons (✓ / ⚠) on resolve so the state is obvious at a
  // glance, even when the friendly label stays the same.
  let icon = $derived.by<IconName>(() => {
    if (status === "success") return "check";
    if (status === "failed") return "circle-alert";
    return mapped?.icon ?? "bot";
  });

  // Per-status label. Keep mapped friendly labels stable (status conveyed
  // via icon + color); for unmapped tools, vary the tense in DEV so the
  // raw toolId remains visible, and use end-user-friendly copy in PROD.
  let label = $derived.by(() => {
    if (mapped) return mapped.label;
    if (SHOW_RAW_AGENT_ACTIVITY) {
      if (status === "success") return `Called: ${toolId}`;
      if (status === "failed") return `Failed: ${toolId}`;
      return `Calling: ${toolId}`;
    }
    if (status === "success") return "Done.";
    if (status === "failed") return "Sorry, that didn't work.";
    return "I'm working on it now...";
  });
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
  /* Success: subtle green tint on the icon + a hairline green border so
   * a row of resolved pills doesn't blend into the surrounding chrome.
   * Label color stays neutral — we don't want green text shouting at the
   * user, just enough visual cue that the step is done. */
  .tool-call-pill[data-status="success"] :global(.tool-block) {
    border-color: var(--color-success, #16a34a);
  }
  .tool-call-pill[data-status="success"] :global(.tool-block svg) {
    color: var(--color-success, #16a34a);
  }
  .tool-call-pill[data-status="failed"] :global(.tool-block) {
    border-color: var(--color-danger, #c0392b);
    color: var(--color-danger, #c0392b);
  }
  .tool-call-pill[data-status="failed"] :global(.tool-block svg) {
    color: var(--color-danger, #c0392b);
  }
</style>

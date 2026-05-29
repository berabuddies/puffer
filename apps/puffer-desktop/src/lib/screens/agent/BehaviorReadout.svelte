<script lang="ts">
  import { onMount } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import { isDesktopMac } from "../../shell/platform";
  import {
    minicpm5BehaviorStart,
    minicpm5BehaviorStop,
    type Minicpm5Behavior
  } from "../../api/desktop";

  // Compact, ambient readout of the on-device model's rolling analysis of what
  // the user is doing. Starts/stops the watcher as the active session changes;
  // silent until the local model is installed + producing (start errors are
  // swallowed — the model simply may not be installed).
  let { sessionId }: { sessionId: string | null } = $props();

  let latest = $state<Minicpm5Behavior | null>(null);
  let active = $state(false);

  onMount(() => {
    if (!isDesktopMac()) return;
    let disposed = false;
    let unlisten: (() => void) | null = null;
    void listen<{ sessionId: string; behavior: Minicpm5Behavior }>(
      "minicpm5://behavior",
      (e) => {
        // Ignore events from a watcher for a different (now inactive) session.
        if (!e.payload || e.payload.sessionId !== sessionId) return;
        const b = e.payload.behavior;
        if (b && !b.error) latest = b;
      }
    ).then((u) => {
      if (disposed) u();
      else unlisten = u;
    });
    return () => {
      disposed = true;
      unlisten?.();
      void minicpm5BehaviorStop().catch(() => {});
    };
  });

  // Restart the watcher whenever the active session changes.
  $effect(() => {
    if (!isDesktopMac()) return;
    const id = sessionId;
    if (!id) {
      active = false;
      latest = null;
      void minicpm5BehaviorStop().catch(() => {});
      return;
    }
    latest = null;
    void minicpm5BehaviorStart(id)
      .then(() => (active = true))
      .catch(() => (active = false));
  });

  const STATE_DOT: Record<string, string> = {
    exploring: "#6aa9ff",
    implementing: "#46c08d",
    debugging: "#e0a52e",
    stuck: "#e0552e",
    reviewing: "#9a7bff",
    idle: "#7a7a82"
  };
</script>

{#if active && latest}
  <div class="pf-beh" title={latest.activity ?? ""}>
    <span class="pf-beh-dot" style:background={STATE_DOT[latest.state ?? "idle"] ?? "#7a7a82"}
    ></span>
    <span class="pf-beh-state">{latest.state ?? "idle"}</span>
    {#if latest.activity}<span class="pf-beh-activity">{latest.activity}</span>{/if}
    {#if latest.suggestion}<span class="pf-beh-sug">💡 {latest.suggestion}</span>{/if}
  </div>
{/if}

<style>
  .pf-beh {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    max-width: 100%;
    padding: 4px 10px;
    border: 1px solid var(--border);
    border-radius: 999px;
    background: var(--card, var(--background));
    font-size: 11.5px;
    color: var(--muted-foreground);
    overflow: hidden;
    white-space: nowrap;
  }
  .pf-beh-dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    flex: none;
  }
  .pf-beh-state {
    font-weight: 600;
    color: var(--foreground);
    text-transform: capitalize;
  }
  .pf-beh-activity {
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .pf-beh-sug {
    color: var(--puffer-accent);
    overflow: hidden;
    text-overflow: ellipsis;
  }
</style>

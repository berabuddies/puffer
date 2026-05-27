<!--
  Connected Apps — the catalogue of integrations Tomo can act through.
  Layout mirrors Paper artboard 1B6-0:
    - "Connected Apps" serif section title (no subtitle).
    - Two-column grid of 10 app cards (12px row/column gap).
    - Composer pinned to the bottom of the page column.

  Connection state lives in $state so clicking a card flips Connect ↔
  Connected. No real API.
-->
<script lang="ts">
  import Composer from "../components/shell/Composer.svelte";
  import AppCard from "../components/apps/AppCard.svelte";
  import { apps as seedApps } from "../data/apps";
  import type { AppEntry } from "../data/types";
  import { pushToast } from "../lib/toast.svelte";

  // Local mutable copy so we can flip status without mutating the seed export.
  let apps = $state<AppEntry[]>(seedApps.map((a) => ({ ...a })));

  function toggle(id: string): void {
    const entry = apps.find((a) => a.id === id);
    if (!entry) return;
    const nowConnected = entry.status !== "connected";
    entry.status = nowConnected ? "connected" : "not_connected";
    pushToast(
      `${entry.name} ${nowConnected ? "connected" : "disconnected"}`,
      nowConnected ? "success" : "info"
    );
  }

</script>

<header class="apps-header">
  <h1 class="text-section">Connected Apps</h1>
</header>

<section class="apps-grid" aria-label="Connected apps">
  {#each apps as app (app.id)}
    <AppCard {app} onToggle={toggle} />
  {/each}
</section>

<div class="apps-spacer"></div>

<Composer placeholder="Send a message to chaofan" />

<style>
  .apps-header {
    padding-bottom: var(--space-5);
  }

  .apps-grid {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    column-gap: var(--space-4);
    row-gap: var(--space-4);
  }

  .apps-spacer {
    flex: 1;
    min-height: var(--space-5);
  }
</style>

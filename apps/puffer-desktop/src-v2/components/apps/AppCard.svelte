<!--
  AppCard — one row in the Connected Apps grid. White card, 1px border,
  16px radius. Left: 32×32 brand logo in a clean square slot + app name.
  Right (pinned): a status chip — cream "Connect" or gray "Connected".

  The whole card is clickable; clicking flips the app's connection state
  in the parent's $state (via onToggle).

  Anatomy mirrors the Paper artboard "App ..." frames (350×74, padding 16,
  gap 14 between logo group and status, gap 6 between logo and name).
-->
<script lang="ts">
  import Chip from "../common/Chip.svelte";
  import AppLogo from "./AppLogo.svelte";
  import type { AppEntry } from "../../data/types";

  type LogoName =
    | "google-calendar"
    | "gmail"
    | "google-meet"
    | "lark"
    | "twitter"
    | "facebook"
    | "instagram"
    | "github"
    | "notion"
    | "imessage";

  interface Props {
    app: AppEntry;
    onToggle: (id: string) => void;
  }

  let { app, onToggle }: Props = $props();

  let logoName = $derived(app.id as LogoName);

  let connected = $derived(app.status === "connected");

  function handleClick(): void {
    onToggle(app.id);
  }

  function handleKey(event: KeyboardEvent): void {
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      onToggle(app.id);
    }
  }
</script>

<div
  class="app-card"
  role="button"
  tabindex="0"
  aria-pressed={connected}
  aria-label={connected ? `Disconnect ${app.name}` : `Connect ${app.name}`}
  onclick={handleClick}
  onkeydown={handleKey}
>
  <div class="app-card__lead">
    <div class="app-card__logo">
      <AppLogo name={logoName} size={24} />
    </div>
    <span class="app-card__name text-task-title">{app.name}</span>
  </div>

  <div class="app-card__status">
    <Chip
      label={connected ? "Connected" : "Connect"}
      variant={connected ? "status-connected" : "cream"}
    />
  </div>
</div>

<style>
  .app-card {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
    width: 100%;
    padding: var(--space-4);
    background: var(--color-surface-app);
    border: var(--border-hairline);
    border-radius: var(--radius-card);
    cursor: pointer;
    transition: background-color 120ms ease, border-color 120ms ease;
    text-align: left;
  }

  .app-card:hover {
    background: var(--color-surface-input);
    border-color: #d9d9d9;
  }

  .app-card:focus-visible {
    outline: 2px solid var(--color-action-cream-border);
    outline-offset: 2px;
  }

  .app-card__lead {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    flex: 1;
  }

  .app-card__logo {
    width: 32px;
    height: 32px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    background: var(--color-surface-app);
  }

  .app-card__name {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .app-card__status {
    flex-shrink: 0;
  }
</style>

<!--
  AppConnectRow — single row of the onboarding "connect your apps" list.
  Geometry from artboard 2A1-0:
    width: 100% (capped at 520 by the parent), height 70, padding 14 16,
    pill-radius, white background, 1px #E0E0E0 border, gap 14.
    Logo well: 40×40 pill, 1px #ECECEC border, hosts the AppLogo glyph.
    Title is Source Serif 4 15.5/20 medium #161616.
    Body is system-ui 13/16 #6F6F6F, 2px above-line spacing.
    Right chip: cream "Connect" → swaps to neutral "Connected" on click.
-->
<script lang="ts">
  import Chip from "../common/Chip.svelte";
  import AppLogo from "./AppLogo.svelte";

  type LogoName = "gmail" | "google-calendar" | "imessage";

  interface Props {
    logo: LogoName;
    name: string;
    description: string;
  }

  let { logo, name, description }: Props = $props();

  let connected = $state<boolean>(false);

  function onConnect(): void {
    if (connected) return;
    connected = true;
    // eslint-disable-next-line no-console
    console.log(`[onboarding/apps] connect: ${logo}`);
  }
</script>

<div class="row">
  <div class="logo-well">
    <AppLogo name={logo} size={24} />
  </div>
  <div class="meta">
    <div class="meta__title">{name}</div>
    <div class="meta__body">{description}</div>
  </div>
  <div class="action">
    {#if connected}
      <Chip variant="status-connected" label="Connected" />
    {:else}
      <Chip variant="cream" label="Connect" onclick={onConnect} />
    {/if}
  </div>
</div>

<style>
  .row {
    display: flex;
    align-items: center;
    width: 100%;
    height: 70px;
    padding: 14px 16px;
    gap: 14px;
    border-radius: var(--radius-pill);
    background: var(--color-surface-app);
    border: 1px solid var(--color-input-border);
    flex-shrink: 0;
  }

  .logo-well {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 40px;
    height: 40px;
    border-radius: var(--radius-pill);
    background: var(--color-surface-app);
    border: 1px solid var(--color-card-border);
    overflow: hidden;
    flex-shrink: 0;
  }

  .meta {
    flex: 1 1 0;
    min-width: 0;
  }

  .meta__title {
    font-family: var(--font-serif);
    font-size: 15.5px;
    line-height: 20px;
    font-weight: var(--font-weight-medium);
    color: var(--color-text-primary);
  }

  .meta__body {
    margin-top: 2px;
    font-family: var(--font-system);
    font-size: var(--font-size-body);
    line-height: var(--line-height-button); /* 16px to match design */
    color: var(--color-text-muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .action {
    flex-shrink: 0;
  }
</style>

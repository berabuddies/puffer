<!--
  PageHeader — display + optional subtitle on the left, optional accessory
  (e.g. mascot on Home) on the right.
-->
<script lang="ts">
  import type { Snippet } from "svelte";

  interface Props {
    title: string;
    subtitle?: string;
    accessory?: Snippet;
  }

  let { title, subtitle, accessory }: Props = $props();
</script>

<header class="page-header">
  <div class="page-header__text">
    <h1 class="text-display">{title}</h1>
    {#if subtitle}
      <p class="text-subtitle">{subtitle}</p>
    {/if}
  </div>
  {#if accessory}
    <div class="page-header__accessory">{@render accessory()}</div>
  {/if}
</header>

<style>
  .page-header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: var(--space-5);
    padding-bottom: var(--space-5);
  }

  .page-header__text {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    min-width: 0;
  }

  .page-header__accessory {
    flex-shrink: 0;
  }
</style>

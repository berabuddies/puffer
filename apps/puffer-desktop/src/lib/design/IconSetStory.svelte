<script lang="ts">
  import Icon, { type IconName } from "./Icon.svelte";

  export let icons: IconName[] = [];
</script>

<div class="icon-grid">
  {#each icons as icon}
    <div class="icon-card">
      <Icon name={icon} size={18} />
      <span>{icon}</span>
    </div>
  {/each}
</div>

<style>
  .icon-grid {
    display: grid;
    grid-template-columns: repeat(5, 120px);
    gap: 16px;
    color: var(--foreground);
  }

  .icon-card {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--background);
  }

  span {
    font-size: 12px;
  }
</style>

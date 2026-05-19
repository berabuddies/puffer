<script lang="ts">
  import Icon from "../design/Icon.svelte";

  type Props = {
    onSearch?: () => void;
  };

  let { onSearch }: Props = $props();
</script>

<div class="pf-titlebar" data-tauri-drag-region>
  <div class="pf-titlebar-drag-fill" data-tauri-drag-region></div>
  {#if onSearch}
    <button
      type="button"
      class="sc-btn"
      data-variant="ghost"
      data-size="icon-sm"
      style="height: 24px; width: 24px;"
      onclick={onSearch}
      aria-label="Search"
    >
      <Icon name="search" size={13} />
    </button>
  {/if}
</div>

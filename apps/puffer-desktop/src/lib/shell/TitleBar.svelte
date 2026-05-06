<script lang="ts">
  import BrandLogo from "../design/BrandLogo.svelte";
  import Icon from "../design/Icon.svelte";

  type Props = {
    onSearch?: () => void;
  };

  let { onSearch }: Props = $props();
</script>

<div class="pf-titlebar" data-tauri-drag-region>
  <div class="pf-titlebar-brand" data-tauri-drag-region>
    <BrandLogo size={20} />
  </div>
  <div style="flex: 1;" data-tauri-drag-region></div>
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

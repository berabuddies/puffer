<script lang="ts">
  export let className = "";
  export let style = "";
</script>

<div class={className} {style}>
  <slot />
</div>

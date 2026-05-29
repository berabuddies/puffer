<script lang="ts">
  type Props = {
    size?: number;
    alt?: string;
    decorative?: boolean;
  };

  let { size = 24, alt = "Puffer logo", decorative = true }: Props = $props();
</script>

<img
  class="pf-brand-logo"
  src="/brand-logo.svg"
  {alt}
  loading="eager"
  draggable="false"
  aria-hidden={decorative ? "true" : undefined}
  style={`width:${size}px;height:${size}px;`}
/>

<style>
  .pf-brand-logo {
    display: block;
    object-fit: contain;
    user-select: none;
  }
</style>

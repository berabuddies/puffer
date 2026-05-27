<!--
  Mascot — Momo's inline face artwork. Sourced from the design (node SA-0,
  artboard PK-0) and exported to /public/v2/mascot.png. Use `size` to switch
  between header (sm/md) and onboarding-hero (lg) placements.
-->
<script lang="ts">
  type Size = "sm" | "md" | "lg";

  interface Props {
    size?: Size;
    alt?: string;
  }

  let { size = "md", alt = "Momo" }: Props = $props();

  const dimensions: Record<Size, number> = {
    sm: 32,
    md: 56,
    lg: 96
  };

  let px = $derived(dimensions[size]);
</script>

<img class="mascot" src="/v2/mascot.png" {alt} width={px} height={px} draggable="false" />

<style>
  .mascot {
    object-fit: contain;
    user-select: none;
    -webkit-user-drag: none;
  }
</style>

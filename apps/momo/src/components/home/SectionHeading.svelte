<!--
  SectionHeading — serif section title + neutral count chip.
  Used on Home to label "Work 3" / "Life 2".
-->
<script lang="ts">
  import Chip from "../common/Chip.svelte";

  interface Props {
    label: string;
    count: number;
  }

  let { label, count }: Props = $props();
</script>

<header class="section-heading">
  <h2 class="text-section">{label}</h2>
  <Chip label={String(count)} variant="count" />
</header>

<style>
  .section-heading {
    display: flex;
    align-items: center;
    gap: var(--space-3);
  }
</style>

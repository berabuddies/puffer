<!--
  ContactAvatar — circular surface-rail badge with a single serif letter.
  Two sizes:
    - sm: 40×40 (list rows) with an 18px serif letter
    - lg: 120×120 (detail hero) with a 48px serif letter
  Both mirror the artboards (16O-0 row avatars, 1QI-0 detail avatar).
-->
<script lang="ts">
  type Size = "sm" | "lg";

  interface Props {
    label: string;
    size?: Size;
    title?: string;
  }

  let { label, size = "sm", title }: Props = $props();
</script>

<span class="avatar avatar--{size}" {title}>
  <span class="avatar__letter">{label}</span>
</span>

<style>
  .avatar {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    border-radius: var(--radius-pill);
    background: var(--color-surface-rail);
    color: var(--color-text-primary);
    border: 1px solid var(--color-card-border);
  }

  .avatar--sm {
    width: var(--height-icon-block);
    height: var(--height-icon-block);
  }
  .avatar--lg {
    width: 120px;
    height: 120px;
  }

  .avatar__letter {
    font-family: var(--font-serif);
    font-weight: var(--font-weight-regular);
    line-height: 1;
    color: var(--color-text-primary);
  }

  .avatar--sm .avatar__letter {
    font-size: 18px;
  }
  .avatar--lg .avatar__letter {
    font-size: 48px;
  }
</style>

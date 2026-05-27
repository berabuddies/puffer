<!--
  IconBlock — the 40×40 circular neutral icon block used at the leading edge
  of task cards (DESIGN_SYSTEM.md → Task card). Pass a Lucide icon component.
-->
<script lang="ts">
  // Lucide-svelte v1 still ships SvelteComponentTyped-style typings even
  // though the runtime is Svelte 5 fns. Accept either shape via `any` so
  // downstream pages don't have to cast every icon they pass in.
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  type IconComponent = any;

  type Tone = "neutral" | "cream";

  interface Props {
    icon: IconComponent;
    tone?: Tone;
    /** Optional accessible label for icon-only contexts. */
    title?: string;
  }

  let { icon: Icon, tone = "neutral", title }: Props = $props();
</script>

<span class="icon-block icon-block--{tone}" {title}>
  <Icon size={18} strokeWidth={1.75} aria-hidden="true" />
</span>

<style>
  .icon-block {
    width: var(--height-icon-block);
    height: var(--height-icon-block);
    flex-shrink: 0;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border-radius: var(--radius-pill);
  }

  .icon-block--neutral {
    background: var(--color-surface-rail);
    color: var(--color-text-primary);
  }

  .icon-block--cream {
    background: var(--color-action-cream);
    color: var(--color-action-cream-text);
  }
</style>

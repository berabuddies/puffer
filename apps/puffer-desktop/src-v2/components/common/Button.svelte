<!--
  Button — primary (cream), secondary (neutral), or icon-only.
  Sizes: sm 32 / md 36 / lg 48.
-->
<script lang="ts">
  import type { Snippet } from "svelte";

  // See IconBlock.svelte — lucide-svelte v1 ships legacy class typings.
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  type IconComponent = any;

  type Variant = "primary" | "secondary" | "icon";
  type Size = "sm" | "md" | "lg";

  interface Props {
    variant?: Variant;
    size?: Size;
    label?: string;
    /** Optional Lucide icon component (rendered left of label, or alone in icon variant). */
    icon?: IconComponent;
    /** Alternative to icon prop — render arbitrary content (e.g. emoji, glyph). */
    children?: Snippet;
    onclick?: (event: MouseEvent) => void;
    disabled?: boolean;
    type?: "button" | "submit";
    title?: string;
    ariaLabel?: string;
  }

  let {
    variant = "primary",
    size = "sm",
    label,
    icon: Icon,
    children,
    onclick,
    disabled = false,
    type = "button",
    title,
    ariaLabel
  }: Props = $props();

  // Pixel icon sizes per button size — keeps 16 / 18 / 20 rhythm.
  const iconPx: Record<Size, number> = { sm: 16, md: 18, lg: 20 };
</script>

<button
  class="btn btn--{variant} btn--{size}"
  {type}
  {title}
  {disabled}
  aria-label={ariaLabel ?? (variant === "icon" ? title : undefined)}
  onclick={onclick}
>
  {#if Icon}
    <Icon size={iconPx[size]} strokeWidth={1.75} aria-hidden="true" />
  {/if}
  {#if children}
    {@render children()}
  {/if}
  {#if label && variant !== "icon"}
    <span class="btn__label">{label}</span>
  {/if}
</button>

<style>
  .btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: var(--space-2);
    border-radius: var(--radius-pill);
    border: 0;
    cursor: pointer;
    transition: filter 120ms ease, background-color 120ms ease;
    font-family: var(--font-sans);
    font-size: var(--font-size-button);
    line-height: var(--line-height-button);
    font-weight: var(--font-weight-medium);
    white-space: nowrap;
  }

  .btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .btn__label {
    /* Inherits font from .btn — explicit class lets pages style label only. */
    color: inherit;
  }

  /* Sizes — height + horizontal padding for text variants. */
  .btn--sm {
    height: var(--height-button-card);
    padding: 0 var(--space-4);
  }
  .btn--md {
    height: var(--height-button-composer);
    padding: 0 var(--space-4);
  }
  .btn--lg {
    height: var(--height-button-utility);
    padding: 0 var(--space-5);
  }

  /* Icon-only variants are perfect squares of the given height. */
  .btn--icon.btn--sm {
    width: var(--height-button-card);
    padding: 0;
  }
  .btn--icon.btn--md {
    width: var(--height-button-composer);
    padding: 0;
  }
  .btn--icon.btn--lg {
    width: var(--height-button-utility);
    padding: 0;
  }

  /* Variants. */
  .btn--primary {
    background: var(--color-action-cream);
    color: var(--color-action-cream-text);
  }
  .btn--primary:hover:not(:disabled) {
    filter: brightness(0.97);
  }

  .btn--secondary {
    background: var(--color-surface-rail);
    color: var(--color-text-primary);
  }
  .btn--secondary:hover:not(:disabled) {
    background: var(--color-card-border);
  }

  .btn--icon {
    background: var(--color-surface-rail);
    color: var(--color-text-primary);
  }
  .btn--icon:hover:not(:disabled) {
    background: var(--color-card-border);
  }
</style>

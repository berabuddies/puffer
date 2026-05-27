<!--
  Modal — backdrop + centered panel.

  Reusable primitive used by the Wallet "Top up" flow (and any future
  in-app dialogs). Behaviours:
    - `open` controls visibility; mounting is conditional, so the slot
      content remounts each time the modal opens (fresh state per open).
    - ESC and backdrop click both call `onClose`.
    - Body scroll is locked while the modal is open — the modal handles
      its own internal overflow.
    - The close button sits inline in the header when `title` is given.

  Slots: default — the modal body. Render headers/footers inside the slot
  if you need finer control (the Top-up modal does this).
-->
<script lang="ts">
  import type { Snippet } from "svelte";
  import { onDestroy } from "svelte";
  import { X } from "lucide-svelte";

  interface Props {
    open: boolean;
    onClose: () => void;
    title?: string;
    maxWidth?: string;
    children?: Snippet;
  }

  let { open, onClose, title, maxWidth = "480px", children }: Props = $props();

  let previousBodyOverflow: string | null = null;

  // Lock body scroll while open; restore the original value on close /
  // unmount. Avoids double-locking when the modal toggles rapidly.
  $effect(() => {
    if (typeof document === "undefined") return;
    if (open) {
      if (previousBodyOverflow === null) {
        previousBodyOverflow = document.body.style.overflow;
      }
      document.body.style.overflow = "hidden";
    } else if (previousBodyOverflow !== null) {
      document.body.style.overflow = previousBodyOverflow;
      previousBodyOverflow = null;
    }
  });

  function handleKey(event: KeyboardEvent): void {
    if (!open) return;
    if (event.key === "Escape") {
      event.preventDefault();
      onClose();
    }
  }

  $effect(() => {
    if (typeof window === "undefined") return;
    if (!open) return;
    window.addEventListener("keydown", handleKey);
    return () => window.removeEventListener("keydown", handleKey);
  });

  onDestroy(() => {
    if (typeof document !== "undefined" && previousBodyOverflow !== null) {
      document.body.style.overflow = previousBodyOverflow;
      previousBodyOverflow = null;
    }
  });

  function handleBackdrop(event: MouseEvent): void {
    // Only close when the click landed on the backdrop itself, not on a
    // descendant. (Panel clicks bubble up to here.)
    if (event.target === event.currentTarget) onClose();
  }
</script>

{#if open}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="modal-backdrop" onclick={handleBackdrop} role="presentation">
    <div
      class="modal-panel"
      style="max-width: {maxWidth};"
      role="dialog"
      aria-modal="true"
      aria-label={title}
    >
      {#if title}
        <header class="modal-header">
          <h2 class="modal-title">{title}</h2>
          <button
            type="button"
            class="modal-close"
            aria-label="Close"
            onclick={onClose}
          >
            <X size={20} strokeWidth={1.75} aria-hidden="true" />
          </button>
        </header>
      {/if}
      {#if children}{@render children()}{/if}
    </div>
  </div>
{/if}

<style>
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(22, 22, 22, 0.18);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
    padding: var(--space-4);
  }

  .modal-panel {
    width: 100%;
    background: var(--color-surface-app);
    border-radius: var(--radius-card);
    box-shadow: 0 18px 48px rgba(22, 22, 22, 0.16);
    overflow: hidden;
    display: flex;
    flex-direction: column;
    max-height: calc(100vh - var(--space-7));
  }

  .modal-header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: var(--space-4);
    padding: 20px 20px 0 20px;
  }

  .modal-title {
    font-family: var(--font-serif);
    font-size: 20px;
    line-height: 24px;
    font-weight: var(--font-weight-regular);
    color: var(--color-text-primary);
  }

  .modal-close {
    appearance: none;
    background: transparent;
    border: 0;
    padding: 0;
    width: 28px;
    height: 28px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border-radius: var(--radius-pill);
    color: var(--color-text-secondary);
    cursor: pointer;
    transition: background-color 120ms ease;
  }
  .modal-close:hover {
    background: var(--color-surface-rail);
  }
</style>

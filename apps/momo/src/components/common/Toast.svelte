<!--
  Toast — global notification stack rendered once near the root of App.svelte.

  Pinned to the bottom-center of the viewport. Each toast is a rounded
  capsule using the design-system surfaces (font-system, --color-card-border,
  --color-surface-app) with a soft 0 6px 24px shadow. Toasts fade in/out via
  the svelte `fade` transition; auto-dismiss is owned by lib/toast.svelte.ts.

  Kind tints:
    info     → neutral primary text on white
    success  → green-700 accent border + text
    error    → red-700 accent border + text
-->
<script lang="ts">
  import { fade } from "svelte/transition";

  import { toasts, dismissToast } from "../../lib/toast.svelte";
</script>

<div class="toast-stack" aria-live="polite" aria-atomic="false">
  {#each toasts as toast (toast.id)}
    <button
      type="button"
      class="toast"
      class:toast--info={toast.kind === "info"}
      class:toast--success={toast.kind === "success"}
      class:toast--error={toast.kind === "error"}
      aria-label={`${toast.kind} notification: ${toast.message}. Click to dismiss.`}
      onclick={() => dismissToast(toast.id)}
      in:fade={{ duration: 160 }}
      out:fade={{ duration: 160 }}
    >
      {toast.message}
    </button>
  {/each}
</div>

<style>
  .toast-stack {
    position: fixed;
    left: 50%;
    bottom: 32px;
    transform: translateX(-50%);
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    z-index: 1000;
    pointer-events: none;
  }

  .toast {
    pointer-events: auto;
    max-width: 420px;
    padding: 10px 16px;
    border-radius: var(--radius-pill);
    background: var(--color-surface-app);
    border: 1px solid var(--color-card-border);
    box-shadow: 0 6px 24px rgba(0, 0, 0, 0.08);
    font-family: var(--font-system);
    font-size: var(--font-size-body);
    line-height: var(--line-height-body);
    font-weight: var(--font-weight-medium);
    color: var(--color-text-primary);
    text-align: center;
    transition: filter 120ms ease;
  }
  .toast:hover {
    filter: brightness(0.98);
  }

  /* Success/error use a slightly stronger hairline so they're scannable
   * without abandoning the calm system palette. */
  .toast--success {
    border-color: #cfe5d1;
    color: #1f5132;
  }
  .toast--error {
    border-color: #f3c8c8;
    color: #7a1b1b;
  }
</style>

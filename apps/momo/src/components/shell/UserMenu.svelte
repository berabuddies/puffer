<!--
  UserMenu — small floating dropdown anchored to the sidebar user row.

  Single item for now: "Sign out". The trigger is the user row that the
  Sidebar passes via the default snippet — we wrap it in a relatively
  positioned container so the floating panel can anchor above the row
  (the user row sits at the bottom of the rail, so the menu opens
  upward).

  Open/close behavior:
    - Clicking the trigger toggles `open`.
    - Clicking outside the menu (capture-phase document listener) closes it.
    - Pressing Escape closes it.

  Styling matches the rest of v2: white surface, --color-card-border
  hairline, --radius-control corners. No shadow per the design system —
  the border carries the elevation.
-->
<script lang="ts">
  import type { Snippet } from "svelte";

  import { LogOut } from "lucide-svelte";

  import { signOut } from "../../lib/auth.svelte";

  interface Props {
    /** The trigger element; clicking it toggles the menu open. */
    trigger: Snippet;
  }

  let { trigger }: Props = $props();

  let open = $state(false);
  let rootEl: HTMLDivElement | null = null;

  function toggle(event: MouseEvent): void {
    // Sidebar may stop propagation in some rows; we just need the click.
    event.stopPropagation();
    open = !open;
  }

  function close(): void {
    open = false;
  }

  function onDocPointerDown(event: PointerEvent): void {
    if (!open) return;
    const target = event.target;
    if (target instanceof Node && rootEl && rootEl.contains(target)) return;
    close();
  }

  function onKeyDown(event: KeyboardEvent): void {
    if (event.key === "Escape" && open) {
      event.preventDefault();
      close();
    }
  }

  function onSignOut(event: MouseEvent): void {
    event.stopPropagation();
    close();
    signOut();
  }

  // Document listeners wired only while the menu is open so we're not
  // paying for them on every page render.
  $effect(() => {
    if (!open || typeof window === "undefined") return;
    document.addEventListener("pointerdown", onDocPointerDown, true);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("pointerdown", onDocPointerDown, true);
      document.removeEventListener("keydown", onKeyDown);
    };
  });
</script>

<div class="user-menu" bind:this={rootEl}>
  <button
    class="user-menu__trigger"
    type="button"
    aria-haspopup="menu"
    aria-expanded={open}
    onclick={toggle}
  >
    {@render trigger()}
  </button>

  {#if open}
    <div class="user-menu__panel" role="menu">
      <button
        class="user-menu__item"
        type="button"
        role="menuitem"
        onclick={onSignOut}
      >
        <LogOut size={14} strokeWidth={1.75} aria-hidden="true" />
        <span>Sign out</span>
      </button>
    </div>
  {/if}
</div>

<style>
  .user-menu {
    position: relative;
    width: 100%;
  }

  /* The trigger is a transparent button so the user row inside it keeps
     its own visual style. We just need the click handler + a11y semantics. */
  .user-menu__trigger {
    display: block;
    width: 100%;
    background: transparent;
    border: 0;
    padding: 0;
    margin: 0;
    text-align: left;
    cursor: pointer;
  }

  /* Anchored above the trigger (user row sits at the bottom of the rail,
     so the menu opens upward). Width matches the rail content area. */
  .user-menu__panel {
    position: absolute;
    bottom: calc(100% + 6px);
    left: 0;
    right: 0;
    z-index: 30;
    background: var(--color-surface-app);
    border: 1px solid var(--color-card-border);
    border-radius: var(--radius-control);
    padding: 4px;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }

  .user-menu__item {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    height: 32px;
    padding: 0 10px;
    background: transparent;
    border: 0;
    border-radius: 6px;
    color: var(--color-text-primary);
    font-family: var(--font-system);
    font-size: var(--font-size-body);
    line-height: var(--line-height-body);
    text-align: left;
    cursor: pointer;
    transition: background-color 120ms ease;
  }
  .user-menu__item:hover {
    background: var(--color-selected-fill);
  }
</style>

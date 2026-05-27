<!--
  Shell — chrome that wraps every "has shell" page.
  Layout: sidebar (248 expanded / 56 collapsed) on the left, page column
  (flex) on the right. Inside the page column, content centers in a
  760-wide max column with 24px side padding.

  Layout invariants (verified at 1200×800, 1440×900, 1920×1080):
    - .shell is 100% × 100% of the Tauri window (the parent #app is locked
      to 100vw/100vh in base.css), no fixed pixel size.
    - Sidebar owns its own internal scrolling (.rail-scroll).
    - .page owns its own internal scrolling (overflow-y: auto on the page,
      min-width/min-height: 0 to let flex children shrink). Pages may also
      opt into `fullbleed` (see below), in which case .page does NOT
      scroll and the page is expected to manage its own internal scroll
      region.
    - No surface above .page / Sidebar ever scrolls; the outer chrome stays
      pinned to the window.

  The page (children snippet) renders its own composer. Shell does not own
  the composer because each page has its own placeholder text.

  fullbleed mode:
    Pages like Agent want a sticky-header / scrolling-thread / sticky-
    composer layout. For that to work, .page__column must be locked to the
    viewport height (not `min-height: 100%`, which lets it grow). When the
    parent passes `fullbleed`, we:
      • drop the column's top padding (the page draws its own header
        beneath the traffic-light strip),
      • lock the column to height: 100% with overflow: hidden,
      • leave .page non-scrolling so children control all scroll regions.
    Regular pages (Home, Wallet, Contact, etc.) remain unchanged — .page
    still scrolls the column when content overflows.

  The Sidebar's traffic-light spacer (data-tauri-drag-region) doubles as the
  macOS window drag handle, so users can grab the top of the rail to move
  the window.
-->
<script lang="ts">
  import { onMount } from "svelte";
  import type { Snippet } from "svelte";

  import Sidebar from "./Sidebar.svelte";
  import { loadSessions } from "../../lib/sessionStore.svelte";

  interface Props {
    children: Snippet;
    /** See header comment — when true, .page__column is locked to viewport
     *  height and the page owns its own internal scrolling. */
    fullbleed?: boolean;
  }

  let { children, fullbleed = false }: Props = $props();

  let collapsed = $state(false);

  // Kick the session list once when the shell mounts — onboarding routes
  // render bare (no Shell), so by the time we get here the user is on a
  // real product page and the sidebar needs its data. Errors surface as
  // toast inside the store.
  onMount(() => {
    void loadSessions();
  });
</script>

<div class="shell">
  <Sidebar {collapsed} onToggle={() => (collapsed = !collapsed)} />

  <main class="page" class:page--fullbleed={fullbleed}>
    <div class="page__column" class:page__column--fullbleed={fullbleed}>
      {@render children()}
    </div>
  </main>
</div>

<style>
  .shell {
    display: flex;
    width: 100%;
    height: 100%;
    background: var(--color-surface-app);
    overflow: hidden;
  }

  .page {
    flex: 1;
    min-width: 0;
    min-height: 0;
    position: relative;
    padding: 0 var(--shell-page-padding);
    overflow-y: auto;
    overflow-x: hidden;
  }
  /* In fullbleed mode the page is the locked-height frame for the column
   * — no scrolling here; the page renders its own internal scroller. */
  .page--fullbleed {
    overflow: hidden;
    display: flex;
    flex-direction: column;
  }

  .page__column {
    width: 100%;
    max-width: var(--shell-page-max);
    margin: 0 auto;
    min-height: 100%;
    display: flex;
    flex-direction: column;
    /* Top padding clears the native traffic-light strip on macOS so page
     * content (greetings, page headers) doesn't slide under it. */
    padding: calc(var(--shell-traffic-spacer) + var(--space-2)) 0 0;
  }
  /* Fullbleed column: take exactly the parent's height and clip overflow.
   * The page (e.g. Agent.svelte) is responsible for its own header padding
   * and internal scroll region. */
  .page__column--fullbleed {
    height: 100%;
    min-height: 0;
    max-height: 100%;
    padding-top: 0;
    overflow: hidden;
    /* `flex: 1` so the column fills the flex parent (.page--fullbleed)
     * exactly, regardless of child intrinsic sizes. */
    flex: 1;
  }
</style>

<!--
  Sidebar — left rail. Full implementation per DESIGN_SYSTEM.md.

  Sections top → bottom:
    1. Rail top      — drag/traffic-light spacer + sidebar toggle
    2. Rail search   — search input pill (icon-only in collapsed mode)
    3. Rail scroll   — nav rows + Tasks (flat session list)
    4. Rail upgrade  — Credits pill (icon badge in collapsed mode)
    5. Rail user     — avatar + name + chevron (avatar-only in collapsed mode)

  macOS chrome:
    The window is configured with titleBarStyle="Overlay" + hiddenTitle=true,
    so the OS draws the native traffic-light buttons in the top-left at
    ~(12, 14). We do NOT render our own dots; instead the .rail-top div is a
    ~44px tall transparent spacer marked with data-tauri-drag-region so the
    user can drag the window from that strip without colliding with the
    native buttons. The collapse toggle is anchored to the right edge of
    that strip so it never overlaps the traffic lights.

  Collapsed mode:
    `collapsed` (passed from Shell) toggles a CSS class. The rail width
    smoothly animates from 248px → 56px (CSS transition). In collapsed
    mode nav rows become 32px icon-only buttons (centered), the search
    input is replaced by a search icon button, the credits pill becomes
    a small badge, the user row collapses to just the avatar, and the
    Spaces section is hidden (those entries are still reachable via the
    Home → tasks list in product nav). Each collapsed icon shows a
    native browser tooltip on hover (title attribute) — lightweight and
    matches macOS expectations without bringing in a portal library.

  Selection is derived from currentRoute.path. The toggle button calls
  `onToggle()` so the parent (Shell) controls collapsed state.
-->
<script lang="ts">
  import { tick } from "svelte";
  import {
    Home,
    Wallet,
    LayoutGrid,
    Search,
    PanelLeftClose,
    PanelLeftOpen,
    Plus,
    ChevronDown,
    Sparkles,
    Pencil,
    Trash2
  } from "lucide-svelte";

  import { currentRoute, navigate } from "../../router.svelte";
  import { currentUser } from "../../data/user";
  import { apps } from "../../data/apps";
  import { authState } from "../../lib/auth.svelte";
  import {
    creditState,
    startCreditPolling,
    stopCreditPolling
  } from "../../lib/creditStore.svelte";
  import { formatCredits } from "../../lib/format";
  import UserMenu from "./UserMenu.svelte";
  import ConfirmDeleteSessionModal from "./ConfirmDeleteSessionModal.svelte";
  import {
    projectSessions,
    renameSession,
    deleteSession,
    optimisticTitle
  } from "../../lib/sessionStore.svelte";
  import { sessionTitle, NEW_SESSION_TITLE } from "../../lib/sessionTitle";
  import type { SessionListItem } from "../../lib/agentClient";

  // See IconBlock.svelte — lucide-svelte v1 ships legacy class typings; use
  // a permissive alias so the nav table can mix any lucide icon.
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  type IconComponent = any;

  interface Props {
    collapsed?: boolean;
    onToggle?: () => void;
  }

  let { collapsed = false, onToggle }: Props = $props();

  interface NavEntry {
    label: string;
    icon: IconComponent;
    href: string;
    /** Path prefixes that count as 'active' for this nav row. */
    activePrefixes: string[];
  }

  const navEntries: NavEntry[] = [
    { label: "Home", icon: Home, href: "/home", activePrefixes: ["/home"] },
    { label: "Wallet", icon: Wallet, href: "/wallet", activePrefixes: ["/wallet"] },
    { label: "Connected Apps", icon: LayoutGrid, href: "/apps", activePrefixes: ["/apps"] }
  ];

  // Session rows in the rail are driven by sessionStore; a session shows
  // here when its cwd matches the single default project path returned by
  // `list_projects`.

  /** Id of the row currently being inline-renamed, or null. */
  let editingId = $state<string | null>(null);
  /** Live value of the rename input — bound to the visible <input>. */
  let editingValue = $state("");
  /** Element ref so we can autoselect on focus enter. */
  let editingInputEl = $state<HTMLInputElement | null>(null);

  /** Session awaiting delete-confirmation, or null when the dialog is closed. */
  let deletingSession = $state<SessionListItem | null>(null);
  /** True while the delete_session RPC is in flight. */
  let deleteBusy = $state(false);

  function beginRename(session: SessionListItem, event: MouseEvent): void {
    event.stopPropagation();
    editingId = session.sessionId;
    // Prefill with the meaningful title (display name / generated title); never
    // the slug fallback, and leave it empty for an untitled session so the user
    // types from scratch rather than editing a "session-…" string.
    editingValue = sessionTitle(session) ?? "";
    void tick().then(() => {
      editingInputEl?.focus();
      editingInputEl?.select();
    });
  }

  function commitRename(session: SessionListItem): void {
    // Guard against Escape → onblur races: once editingId is cleared the
    // input is being torn down and the onblur callback should no-op.
    if (editingId !== session.sessionId) return;
    const next = editingValue.trim();
    const before = (sessionTitle(session) ?? "").trim();
    if (!next || next === before) {
      cancelRename();
      return;
    }
    const id = session.sessionId;
    editingId = null;
    editingValue = "";
    void renameSession(id, next);
  }

  function cancelRename(): void {
    editingId = null;
    editingValue = "";
  }

  function onRenameKey(event: KeyboardEvent, session: SessionListItem): void {
    if (event.key === "Enter") {
      event.preventDefault();
      commitRename(session);
    } else if (event.key === "Escape") {
      event.preventDefault();
      cancelRename();
    }
  }

  function beginDelete(session: SessionListItem, event: MouseEvent): void {
    event.stopPropagation();
    deletingSession = session;
  }

  function cancelDelete(): void {
    // Ignore close attempts (ESC / backdrop / Cancel) while the RPC is in
    // flight so the dialog can't be dropped mid-delete.
    if (deleteBusy) return;
    deletingSession = null;
  }

  async function confirmDelete(): Promise<void> {
    const session = deletingSession;
    if (!session || deleteBusy) return;
    deleteBusy = true;
    try {
      await deleteSession(session.sessionId);
    } catch {
      // The store already surfaced a toast; keep the dialog open so the user
      // can retry or back out.
      deleteBusy = false;
      return;
    }
    deleteBusy = false;
    deletingSession = null;
    // If we just deleted the session currently open in the agent view, its
    // /agent/<id> route is now dead — bounce Home.
    if (isAgentActive(session.sessionId)) {
      navigate("/home");
    }
  }

  function sessionLabel(session: SessionListItem): string {
    return sessionTitle(session) ?? optimisticTitle(session.sessionId) ?? NEW_SESSION_TITLE;
  }

  /* ───── Search dropdown ─────
   * Substring search across connected apps. Contacts are hidden, and
   * sessions live in the rail directly, so neither appears in this
   * dropdown.
   */
  const MAX_RESULTS = 5;
  const FOCUS_LOSS_DELAY_MS = 800;

  type ResultKind = "App";
  type ResultIcon = "layoutGrid";

  interface SearchResult {
    id: string;
    label: string;
    kind: ResultKind;
    icon: ResultIcon;
    href: string;
  }

  let searchQuery = $state("");
  let searchOpen = $state(false);
  let searchInputEl = $state<HTMLInputElement | null>(null);
  let blurTimer: ReturnType<typeof setTimeout> | null = null;
  /** Set by the collapsed-mode Search button so we re-focus once expanded. */
  let pendingSearchFocus = $state(false);

  function buildResults(query: string): SearchResult[] {
    const q = query.trim().toLowerCase();
    if (!q) return [];
    const out: SearchResult[] = [];

    for (const a of apps) {
      if (a.name.toLowerCase().includes(q)) {
        out.push({
          id: `app-${a.id}`,
          label: a.name,
          kind: "App",
          icon: "layoutGrid",
          href: "/apps"
        });
      }
    }

    return out.slice(0, MAX_RESULTS);
  }

  let searchResults = $derived(buildResults(searchQuery));
  let dropdownVisible = $derived(searchOpen && searchResults.length > 0);

  function onSearchInput(event: Event): void {
    const target = event.target as HTMLInputElement;
    searchQuery = target.value;
    searchOpen = searchQuery.trim().length > 0;
  }

  function onSearchKeydown(event: KeyboardEvent): void {
    if (event.key === "Escape") {
      event.preventDefault();
      closeSearch();
      searchInputEl?.blur();
    }
  }

  function onSearchFocus(): void {
    if (blurTimer) {
      clearTimeout(blurTimer);
      blurTimer = null;
    }
    if (searchQuery.trim().length > 0) {
      searchOpen = true;
    }
  }

  function onSearchBlur(): void {
    if (blurTimer) clearTimeout(blurTimer);
    blurTimer = setTimeout(() => {
      searchOpen = false;
      blurTimer = null;
    }, FOCUS_LOSS_DELAY_MS);
  }

  function closeSearch(): void {
    searchOpen = false;
    searchQuery = "";
    if (blurTimer) {
      clearTimeout(blurTimer);
      blurTimer = null;
    }
  }

  function pickResult(r: SearchResult): void {
    navigate(r.href);
    closeSearch();
  }

  function onCollapsedSearchClick(): void {
    // We can only request a toggle (Shell owns collapsed state) — that
    // unconditionally expands here because the icon button only renders
    // while collapsed === true. After the prop flips back to false the
    // $effect below moves focus into the now-visible <input>.
    pendingSearchFocus = true;
    onToggle?.();
  }

  $effect(() => {
    if (!collapsed && pendingSearchFocus) {
      pendingSearchFocus = false;
      tick().then(() => {
        searchInputEl?.focus();
      });
    }
  });

  function isNavActive(entry: NavEntry): boolean {
    return entry.activePrefixes.some((prefix) => {
      if (currentRoute.path === prefix) return true;
      return currentRoute.path.startsWith(`${prefix}/`);
    });
  }

  function isAgentActive(sessionId: string): boolean {
    return currentRoute.path === `/agent/${sessionId}`;
  }

  function go(path: string): void {
    navigate(path);
  }

  /**
   * Open the bare "/new" page. We deliberately do NOT create a session here —
   * the session is minted only when the user sends their first message on that
   * page (via the composer's createSessionFromText). This avoids stray empty
   * "New task" rows from clicks that never lead to a message.
   */
  function startNewChat(): void {
    navigate("/new");
  }

  /* ───── Credits ─────
   * The pill shows the user's WorldRouter balance, loaded + polled by
   * creditStore while signed in. While loading / on error it renders "—".
   * Clicking opens the external top-up page in the system browser (Tauri)
   * or a new tab (web / playwright). */
  const WR_DASHBOARD_URL =
    (import.meta.env.VITE_WR_DASHBOARD_URL as string | undefined) ??
    "https://www.worldrouter.ai";

  // Render the last known balance whenever we have one — even while a
  // background refresh is in flight. `loadCredits` flips status to "loading"
  // on every 30s poll / window-focus refetch but never clears `creditsUsd`,
  // so keying the label off `creditsUsd` (not `status`) keeps the pill from
  // flashing number → — → number on each refresh. "—" shows only when there
  // is genuinely no balance yet (cold first load, or a load that never
  // succeeded). The load path itself is untouched.
  let creditsLabel = $derived(
    creditState.creditsUsd !== null ? formatCredits(creditState.creditsUsd) : "—"
  );

  function openTopUp(): void {
    const url = `${WR_DASHBOARD_URL.replace(/\/$/, "")}/dashboard/credits`;
    if (typeof window !== "undefined" && "__TAURI_INTERNALS__" in window) {
      void import("@tauri-apps/plugin-opener").then(({ openUrl }) =>
        openUrl(url)
      );
    } else if (typeof window !== "undefined") {
      window.open(url, "_blank");
    }
  }

  // Poll credits while signed in; tear down on sign-out / unmount. The
  // Sidebar only mounts when signedIn, so this also covers the first load.
  $effect(() => {
    if (authState.status === "signedIn") {
      startCreditPolling();
      return () => stopCreditPolling();
    }
  });
</script>

<aside
  class="sidebar"
  class:sidebar--collapsed={collapsed}
  aria-label="Primary navigation"
>
  <!-- 1. Rail top: native traffic-light spacer + drag region + toggle.
       The spacer carries data-tauri-drag-region so dragging this strip
       moves the window (Tauri 2 honors this attribute on any element).

       Two layouts:
       - Expanded (248px wide): one row; the spacer fills the strip and
         the toggle floats at the right edge — well clear of the OS
         traffic lights on the left.
       - Collapsed (56px wide): the rail is narrower than the OS traffic
         light cluster (~78px). If we centered the toggle on a single
         row, it would land directly on the red/yellow/green dots and
         intercept their clicks. Instead we render the rail-top as two
         stacked rows — first a pure drag spacer that lets the traffic
         lights overlay it cleanly, then a second row below the OS
         buttons that hosts the centered toggle. -->
  <div class="rail-top" class:rail-top--stacked={collapsed}>
    <div class="rail-top__drag" data-tauri-drag-region></div>
    <div class="rail-top__row">
      <button
        class="rail-top__toggle"
        type="button"
        aria-label={collapsed ? "Show sidebar" : "Hide sidebar"}
        title={collapsed ? "Show sidebar" : "Hide sidebar"}
        onclick={() => onToggle?.()}
      >
        {#if collapsed}
          <PanelLeftOpen size={18} strokeWidth={1.6} aria-hidden="true" />
        {:else}
          <PanelLeftClose size={18} strokeWidth={1.6} aria-hidden="true" />
        {/if}
      </button>
    </div>
  </div>

  <!-- 2. Rail search -->
  <div class="rail-search">
    {#if collapsed}
      <button
        class="rail-icon-btn"
        type="button"
        aria-label="Search"
        title="Search"
        onclick={onCollapsedSearchClick}
      >
        <Search size={16} strokeWidth={1.75} aria-hidden="true" />
      </button>
    {:else}
      <div class="search-pill">
        <Search size={14} strokeWidth={1.75} aria-hidden="true" />
        <input
          bind:this={searchInputEl}
          class="search-pill__input"
          type="search"
          placeholder="Search"
          aria-label="Search"
          value={searchQuery}
          oninput={onSearchInput}
          onkeydown={onSearchKeydown}
          onfocus={onSearchFocus}
          onblur={onSearchBlur}
        />
      </div>

      {#if dropdownVisible}
        <div class="search-dropdown" role="listbox" aria-label="Search results">
          {#each searchResults as r (r.id)}
            <button
              type="button"
              class="search-dropdown__row"
              role="option"
              aria-selected="false"
              onmousedown={(e) => e.preventDefault()}
              onclick={() => pickResult(r)}
            >
              <span class="search-dropdown__icon" aria-hidden="true">
                <LayoutGrid size={14} strokeWidth={1.75} />
              </span>
              <span class="search-dropdown__label">{r.label}</span>
              <span class="search-dropdown__badge">{r.kind}</span>
            </button>
          {/each}
        </div>
      {/if}
    {/if}
  </div>

  <!-- 3a. Rail nav: pinned top block holding the primary nav rows. -->
  <div class="rail-nav">
    <nav class="nav-stack" aria-label="App sections">
      {#each navEntries as entry (entry.href)}
        {@const active = isNavActive(entry)}
        <button
          class="nav-row"
          class:nav-row--active={active}
          class:nav-row--icon={collapsed}
          type="button"
          onclick={() => go(entry.href)}
          aria-current={active ? "page" : undefined}
          aria-label={collapsed ? entry.label : undefined}
          title={collapsed ? entry.label : undefined}
        >
          <entry.icon size={16} strokeWidth={1.75} aria-hidden="true" />
          {#if !collapsed}
            <span class="nav-row__label">{entry.label}</span>
          {/if}
        </button>
      {/each}
    </nav>
  </div>

  <!-- 3b. Rail scroll: only the Spaces list scrolls; nav + bottom rows stay pinned. -->
  <div class="rail-scroll">
    {#if !collapsed}
      <div class="spaces">
        <!-- Header row: "Tasks" eyebrow + a trailing "+" that mints a fresh
             puffer session under the default project and navigates straight
             to /agent/<sessionId>. -->
        <div class="spaces__header">
          <p class="text-eyebrow spaces__eyebrow">Tasks</p>
          <button
            class="spaces__add"
            type="button"
            aria-label="New chat"
            title="New chat"
            onclick={startNewChat}
          >
            <Plus size={14} strokeWidth={1.75} aria-hidden="true" />
          </button>
        </div>

        {#snippet sessionRow(session: SessionListItem)}
          {@const active = isAgentActive(session.sessionId)}
          {@const editing = editingId === session.sessionId}
          <div class="session-row" class:session-row--active={active}>
            {#if editing}
              <input
                bind:this={editingInputEl}
                bind:value={editingValue}
                class="session-row__input"
                type="text"
                aria-label="Rename session"
                onkeydown={(e) => onRenameKey(e, session)}
                onblur={() => commitRename(session)}
              />
            {:else}
              <button
                class="session-row__label-btn"
                type="button"
                onclick={() => go(`/agent/${session.sessionId}`)}
                aria-current={active ? "page" : undefined}
                title={sessionLabel(session)}
              >
                <span class="session-row__label">{sessionLabel(session)}</span>
              </button>
              <div class="session-row__actions" aria-hidden={!active && undefined}>
                <button
                  class="session-row__icon-btn"
                  type="button"
                  aria-label="Rename session"
                  title="Rename"
                  onclick={(e) => beginRename(session, e)}
                >
                  <Pencil size={12} strokeWidth={1.75} aria-hidden="true" />
                </button>
                <button
                  class="session-row__icon-btn"
                  type="button"
                  aria-label="Delete session"
                  title="Delete"
                  onclick={(e) => beginDelete(session, e)}
                >
                  <Trash2 size={12} strokeWidth={1.75} aria-hidden="true" />
                </button>
              </div>
            {/if}
          </div>
        {/snippet}

        <div class="space-items">
          {#each projectSessions() as session (session.sessionId)}
            {@render sessionRow(session)}
          {:else}
            <p class="space-items__add">No sessions yet</p>
          {/each}
        </div>
      </div>
    {/if}
  </div>

  <!-- 4. Rail upgrade: credits -->
  <div class="rail-upgrade">
    {#if collapsed}
      <button
        class="credits-badge"
        type="button"
        aria-label={`${creditsLabel} — open top-up`}
        title={creditsLabel}
        onclick={openTopUp}
      >
        <Sparkles size={14} strokeWidth={1.75} aria-hidden="true" />
      </button>
    {:else}
      <button
        class="credits-pill"
        type="button"
        aria-label="Available credits — open top-up"
        onclick={openTopUp}
      >
        <span class="credits-pill__label">Credits</span>
        <span class="credits-pill__value">{creditsLabel}</span>
      </button>
    {/if}
  </div>

  <!-- 5. Rail user
       Wrapped in <UserMenu> so clicking the row pops a small dropdown
       anchored above (the row sits at the bottom of the rail). The menu
       carries a single "Sign out" item that clears local auth state and
       bounces to Auth Station's /logout endpoint.

       Display name + avatar pull from `authState.user` when signed in.
       During the brief "unknown" window on mount (before
       loadAuthFromStorage runs) we fall back to the mock currentUser
       so the rail never flashes empty. The {@const}s sit inside the
       UserMenu's trigger snippet because Svelte 5 requires
       {@const} to be the immediate child of a snippet / block /
       component — they can't float at the template's top level. -->
  <div class="rail-user">
    <UserMenu>
      {#snippet trigger()}
        {@const displayName =
          authState.user?.name || authState.user?.email || currentUser.name}
        {@const avatarUrl = authState.user?.picture ?? null}
        {@const avatarInitial = (displayName?.trim()?.[0] || currentUser.avatarLabel).toUpperCase()}
        <div
          class="user-row"
          class:user-row--icon={collapsed}
          aria-label={`Account: ${displayName}`}
          title={collapsed ? displayName : undefined}
        >
          <span class="user-row__avatar" aria-hidden="true">
            {#if avatarUrl}
              <img class="user-row__avatar-img" src={avatarUrl} alt="" />
            {:else}
              <span class="user-row__initial">{avatarInitial}</span>
            {/if}
          </span>
          {#if !collapsed}
            <span class="user-row__name">{displayName}</span>
            <ChevronDown size={18} strokeWidth={1.75} aria-hidden="true" />
          {/if}
        </div>
      {/snippet}
    </UserMenu>
  </div>
</aside>

<ConfirmDeleteSessionModal
  open={deletingSession !== null}
  name={deletingSession ? sessionLabel(deletingSession) : ""}
  busy={deleteBusy}
  onConfirm={confirmDelete}
  onClose={cancelDelete}
/>

<style>
  .sidebar {
    width: var(--shell-rail-width);
    height: 100%;
    flex-shrink: 0;
    background: var(--color-surface-rail);
    display: flex;
    flex-direction: column;
    color: var(--color-text-primary);
    transition: width 180ms ease;
    overflow: hidden;
  }
  .sidebar--collapsed {
    width: var(--shell-rail-collapsed);
  }

  /* ───── 1. Rail top ─────
   * Two layouts (see template comment):
   *
   *   Expanded:   single row, --shell-traffic-spacer tall, toggle pinned
   *               to the right edge so the OS traffic lights (top-left)
   *               never collide with it.
   *
   *   Collapsed:  stacked — a 28px drag spacer on top (lets the native
   *               traffic lights overlay it without intercepting clicks),
   *               then a row below where the toggle sits centered. The
   *               wrapper is taller in this mode so the toggle clears
   *               the OS controls.
   *
   * `data-tauri-drag-region` is now on the inner `.rail-top__drag` so
   * the spacer continues to drag the window, but the row hosting the
   * toggle button is non-drag (otherwise drag intercepts the click).
   */
  .rail-top {
    height: var(--shell-traffic-spacer);
    padding: 0 8px 0 0;
    display: flex;
    align-items: center;
    justify-content: flex-end;
    flex-shrink: 0;
    position: relative;
  }
  .rail-top__drag {
    /* Default (expanded): the drag region fills the row behind the toggle
     * so users can grab the strip to move the window. */
    position: absolute;
    inset: 0;
  }
  .rail-top__row {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    width: 100%;
    /* Stack above the drag layer so the toggle stays clickable. */
    position: relative;
    z-index: 1;
  }

  /* ── Collapsed: stacked layout ── */
  .rail-top--stacked {
    /* Tall enough for: 28px drag spacer (OS traffic lights are ~28px tall,
     * anchored at y=14 → bottom edge ~42) + a 40px toggle row. */
    height: 68px;
    padding: 0;
    display: flex;
    flex-direction: column;
    align-items: stretch;
    justify-content: flex-start;
  }
  .rail-top--stacked .rail-top__drag {
    /* In stacked mode the drag spacer is its own row, not an overlay. */
    position: static;
    inset: auto;
    height: 28px;
    flex-shrink: 0;
  }
  .rail-top--stacked .rail-top__row {
    flex: 1;
    justify-content: center;
  }
  .sidebar--collapsed .rail-top {
    /* Collapsed rail (56px) is narrower than the OS traffic-light cluster
     * (~78px). The drag spacer absorbs the overlap so the lights can
     * paint over it; the toggle lives one row down, well clear. */
    padding-right: 0;
  }

  .rail-top__toggle {
    width: 26px;
    height: 26px;
    border-radius: var(--radius-control);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    color: var(--color-text-secondary);
    transition: background-color 120ms ease;
    /* Toggle is outside the drag spacer in both layouts, but keep the
     * no-drag hint for safety (Tauri respects -webkit-app-region). */
    -webkit-app-region: no-drag;
  }
  .rail-top__toggle:hover {
    background: rgba(0, 0, 0, 0.04);
  }

  /* ───── 2. Rail search ───── */
  .rail-search {
    height: 52px;
    padding: 0 10px;
    display: flex;
    align-items: center;
    flex-shrink: 0;
    /* Anchor for the absolutely-positioned search dropdown. */
    position: relative;
  }
  .sidebar--collapsed .rail-search {
    padding: 0;
    justify-content: center;
  }
  .search-pill {
    width: 100%;
    height: 32px;
    background: var(--color-surface-input);
    border: 1px solid var(--color-input-border);
    border-radius: var(--radius-control);
    padding: 0 10px;
    display: inline-flex;
    align-items: center;
    gap: 8px;
    color: var(--color-text-muted);
  }
  .search-pill__input {
    flex: 1;
    height: 100%;
    border: 0;
    outline: 0;
    background: transparent;
    font-family: var(--font-system);
    font-size: var(--font-size-body);
    color: var(--color-text-primary);
  }
  .search-pill__input::placeholder {
    color: var(--color-text-muted);
  }
  /* Hide native clear/search button so the pill stays clean. */
  .search-pill__input::-webkit-search-cancel-button {
    display: none;
  }

  /* Generic 32×32 icon-only button used by collapsed-mode controls
   * (search shortcut, etc.). Keeps the visual rhythm of nav rows. */
  .rail-icon-btn {
    width: 32px;
    height: 32px;
    border-radius: var(--radius-control);
    background: transparent;
    color: var(--color-text-secondary);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    transition: background-color 120ms ease;
  }
  .rail-icon-btn:hover {
    background: rgba(0, 0, 0, 0.04);
  }

  /* ───── 3a. Rail nav (pinned) ─────
   * Holds the primary nav rows. Stays fixed at the top of the rail so the
   * Home / Contact / Wallet / Apps row never scrolls away with the session
   * list below. */
  .rail-nav {
    flex-shrink: 0;
    padding: 0 12px;
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
  }
  .sidebar--collapsed .rail-nav {
    align-items: center;
  }

  /* ───── 3b. Rail scroll (Spaces only) ─────
   * Confines vertical scrolling to the Spaces section so the bottom user /
   * credits rows stay pinned. In collapsed mode the Spaces tree is hidden
   * entirely, so this block degrades to an empty flex spacer. */
  .rail-scroll {
    flex: 1 1 0;
    min-height: 0;
    padding: var(--space-4) 12px 0;
    overflow-y: auto;
    overflow-x: hidden;
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
  }
  .sidebar--collapsed .rail-scroll {
    padding: var(--space-4) 12px 0;
    align-items: center;
  }

  .nav-stack {
    display: flex;
    flex-direction: column;
    gap: 2px;
    width: 100%;
  }
  .sidebar--collapsed .nav-stack {
    align-items: center;
  }

  .nav-row {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    height: var(--height-nav-row);
    padding: 7px 10px;
    border-radius: var(--radius-control);
    background: transparent;
    color: var(--color-text-primary);
    font-family: var(--font-system);
    font-size: var(--font-size-body);
    line-height: var(--line-height-body);
    font-weight: var(--font-weight-medium);
    text-align: left;
    transition: background-color 120ms ease;
  }
  .nav-row:hover {
    background: rgba(0, 0, 0, 0.03);
  }
  .nav-row--active {
    background: var(--color-selected-fill);
  }
  /* Icon-only mode: square button, centered, no label. */
  .nav-row--icon {
    width: 32px;
    height: 32px;
    padding: 0;
    justify-content: center;
    gap: 0;
  }
  .nav-row__label {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* ───── Spaces ───── */
  .spaces {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }
  .spaces__header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0 6px 0 10px;
    margin-bottom: 2px;
  }
  .spaces__eyebrow {
    text-transform: none; /* design shows "Tasks" mixed-case */
    letter-spacing: 0;
  }

  /* Trailing "+" in the Tasks header — mints a fresh session. Faint by
   * default, solid on hover, so the affordance stays quiet until reached. */
  .spaces__add {
    display: inline-flex;
    width: 22px;
    height: 22px;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    background: transparent;
    border: 0;
    padding: 0;
    color: var(--color-text-muted);
    border-radius: 4px;
    opacity: 0.65;
    cursor: pointer;
    transition: background-color 120ms ease, color 120ms ease, opacity 120ms ease;
  }
  .spaces__add:hover {
    background: rgba(0, 0, 0, 0.06);
    color: var(--color-text-primary);
    opacity: 1;
  }

  .space-items {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .space-items__add {
    padding: 7px 10px;
    color: var(--color-text-muted);
    font-size: var(--font-size-body);
  }

  /* ───── Session rows ─────
   * Each row holds a flex-1 label button and a trailing icon group that
   * stays hidden until the row is hovered (or active). Layout mirrors
   * `.nav-row` so vertical rhythm matches the rest of the rail.
   */
  .session-row {
    display: flex;
    align-items: center;
    width: 100%;
    min-height: var(--height-nav-row);
    padding: 0 6px 0 10px;
    border-radius: var(--radius-control);
    background: transparent;
    color: var(--color-text-primary);
    transition: background-color 120ms ease;
    gap: 2px;
  }
  .session-row:hover {
    background: rgba(0, 0, 0, 0.03);
  }
  .session-row--active {
    background: var(--color-selected-fill);
  }

  .session-row__label-btn {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    height: var(--height-nav-row);
    padding: 0 4px 0 0;
    background: transparent;
    border: 0;
    color: inherit;
    font-family: var(--font-system);
    font-size: var(--font-size-body);
    line-height: var(--line-height-body);
    font-weight: var(--font-weight-medium);
    text-align: left;
    cursor: pointer;
  }
  .session-row__label {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .session-row__input {
    flex: 1;
    min-width: 0;
    height: 24px;
    margin: 0 4px;
    padding: 0 6px;
    border: 1px solid var(--color-input-border);
    border-radius: 4px;
    background: var(--color-surface-app);
    color: var(--color-text-primary);
    font-family: var(--font-system);
    font-size: var(--font-size-body);
    line-height: var(--line-height-body);
    outline: 0;
  }
  .session-row__input:focus {
    border-color: var(--color-action-cream-border);
  }

  .session-row__actions {
    display: inline-flex;
    align-items: center;
    gap: 2px;
    flex-shrink: 0;
    opacity: 0;
    transition: opacity 120ms ease;
  }
  .session-row:hover .session-row__actions,
  .session-row:focus-within .session-row__actions {
    opacity: 1;
  }

  .session-row__icon-btn {
    width: 22px;
    height: 22px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    background: transparent;
    border: 0;
    border-radius: 4px;
    color: var(--color-text-muted);
    cursor: pointer;
    transition: background-color 120ms ease, color 120ms ease;
  }
  .session-row__icon-btn:hover:not(:disabled) {
    background: rgba(0, 0, 0, 0.06);
    color: var(--color-text-primary);
  }
  .session-row__icon-btn:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }

  /* ───── 4. Rail upgrade ───── */
  .rail-upgrade {
    height: 42px;
    padding: 0 12px 6px;
    display: flex;
    align-items: center;
    flex-shrink: 0;
  }
  .sidebar--collapsed .rail-upgrade {
    padding: 0 0 6px;
    justify-content: center;
  }
  .credits-pill {
    display: flex;
    align-items: center;
    justify-content: space-between;
    width: 100%;
    height: 28px;
    padding: 6px 12px;
    border-radius: var(--radius-pill);
    background: var(--color-surface-app);
    border: 1px solid var(--color-input-border);
    color: var(--color-text-primary);
    font-family: var(--font-system);
    font-size: var(--font-size-button);
    line-height: var(--line-height-button);
    text-align: left;
    cursor: pointer;
    transition: background-color 120ms ease;
  }
  .credits-pill:hover {
    background: var(--color-selected-fill);
  }
  .credits-pill__label {
    color: var(--color-text-muted);
    font-weight: var(--font-weight-medium);
  }
  .credits-pill__value {
    color: var(--color-text-primary);
    font-weight: var(--font-weight-medium);
  }
  /* Compact badge form used when the rail is collapsed. Uses the cream
   * accent so the credits affordance stays discoverable. */
  .credits-badge {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    border-radius: var(--radius-pill);
    background: var(--color-action-cream);
    color: var(--color-action-cream-text);
    border: 1px solid var(--color-action-cream-border);
    cursor: pointer;
  }

  /* ───── 5. Rail user ───── */
  .rail-user {
    height: 52px;
    padding: 0 12px;
    display: flex;
    align-items: center;
    flex-shrink: 0;
  }
  .sidebar--collapsed .rail-user {
    padding: 0;
    justify-content: center;
  }
  .user-row {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    height: 40px;
    padding: 4px 8px;
    border-radius: var(--radius-control);
    background: transparent;
    color: var(--color-text-primary);
    transition: background-color 120ms ease;
  }
  .user-row:hover {
    background: var(--color-selected-fill);
  }
  .user-row--icon {
    width: 40px;
    padding: 0;
    justify-content: center;
    gap: 0;
  }
  .user-row__avatar {
    width: 28px;
    height: 28px;
    border-radius: var(--radius-pill);
    background: var(--color-surface-app);
    border: 1px solid var(--color-input-border);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
  }
  .user-row__initial {
    font-family: var(--font-system);
    font-size: var(--font-size-button);
    font-weight: var(--font-weight-semibold);
    color: var(--color-text-secondary);
  }
  /* When the Auth Station JWT carries a `picture` URL we render the
     image inside the same 28×28 avatar pill, replacing the initial. */
  .user-row__avatar-img {
    width: 100%;
    height: 100%;
    border-radius: var(--radius-pill);
    object-fit: cover;
    display: block;
  }
  .user-row__name {
    flex: 1;
    font-family: var(--font-system);
    font-size: var(--font-size-body);
    font-weight: var(--font-weight-medium);
    text-align: left;
  }

  /* ───── Search dropdown ─────
   * Anchored just below the search pill (52px rail-search row, 10px side
   * padding, 32px pill ⇒ ~46px top inside the row). Inset matches the pill
   * so the dropdown reads as a panel attached to the input. */
  .search-dropdown {
    position: absolute;
    top: 44px;
    left: 10px;
    right: 10px;
    z-index: 20;
    background: var(--color-surface-app);
    border: 1px solid var(--color-card-border);
    border-radius: var(--radius-control);
    box-shadow: 0 6px 24px rgba(0, 0, 0, 0.08);
    padding: 4px;
    display: flex;
    flex-direction: column;
    gap: 1px;
    max-height: 260px;
    overflow-y: auto;
  }

  .search-dropdown__row {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    height: 32px;
    padding: 0 8px;
    border-radius: 6px;
    background: transparent;
    color: var(--color-text-primary);
    font-family: var(--font-system);
    font-size: var(--font-size-body);
    line-height: var(--line-height-body);
    text-align: left;
    transition: background-color 120ms ease;
  }
  .search-dropdown__row:hover {
    background: var(--color-selected-fill);
  }

  .search-dropdown__icon {
    width: 18px;
    height: 18px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    color: var(--color-text-secondary);
    flex-shrink: 0;
  }

  .search-dropdown__label {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .search-dropdown__badge {
    flex-shrink: 0;
    font-family: var(--font-system);
    font-size: 11px;
    line-height: 14px;
    font-weight: var(--font-weight-medium);
    color: var(--color-text-muted);
    padding: 2px 6px;
    border-radius: var(--radius-pill);
    background: var(--color-surface-rail);
    border: 1px solid var(--color-card-border);
  }
</style>

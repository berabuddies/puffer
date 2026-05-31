<script lang="ts">
  import { onMount, tick } from "svelte";
  import Icon, { type IconName } from "../../design/Icon.svelte";
  import AgentDetailContent from "./AgentDetailContent.svelte";
  import {
    AGENT_STATE_LABELS,
    agentPufferState,
    type AgentStatus
  } from "../../data/mockProjects";
  import { sessionDisplayName, sessionDisplayTitle } from "../../sessionDisplay";
  import type {
    BrowserRenderer,
    PermissionTimelineItem,
    SessionDetail,
    SessionListItem,
    SettingsSnapshot,
    TimelineItem,
    UserQuestionTimelineItem
  } from "../../types";
  import type { AgentState } from "../../shell/tweaks";
  import type { AgentTurnOptions } from "../../api/desktop";
  type SubmitMessageResult = boolean | void | Promise<boolean | void>;

  type Props = {
    // Live session data from the backend.
    session: SessionListItem | null;
    sessionDetail: SessionDetail | null;
    timeline: TimelineItem[];
    pendingPermissions: PermissionTimelineItem[];
    pendingQuestions: UserQuestionTimelineItem[];
    resolvingPermissionIds?: string[];
    resolvingQuestionIds?: string[];
    loading: boolean;
    turnRunning?: boolean;
    turnCancelable?: boolean;
    turnStartedAtMs?: number | null;
    turnThinking?: boolean;
    turnStatusHint?: string | null;
    settingsSnapshot?: SettingsSnapshot | null;
    backendConnected?: boolean;
    browserRenderer?: BrowserRenderer;
    userDisplayName?: string;
    onBack: () => void;
    onSubmitMessage: (message: string, options?: AgentTurnOptions) => SubmitMessageResult;
    onResolvePermission: (permissionId: string, choice: string) => void;
    onResolveUserQuestion: (
      questionId: string,
      answers: Record<string, string | string[]>,
      annotations?: Record<string, Record<string, string>>
    ) => void;
    onCancelTurn?: () => void;
    onDraftChange?: (hasDraft: boolean) => void;
    onRenameTitle?: (title: string) => void | Promise<void>;
  };

  let {
    session,
    sessionDetail,
    timeline,
    pendingPermissions,
    pendingQuestions,
    resolvingPermissionIds = [],
    resolvingQuestionIds = [],
    loading,
    turnRunning = false,
    turnCancelable = true,
    turnStartedAtMs = null,
    turnThinking = false,
    turnStatusHint = null,
    settingsSnapshot = null,
    backendConnected = true,
    browserRenderer = "cef",
    userDisplayName = "Otter",
    onBack,
    onSubmitMessage,
    onResolvePermission,
    onResolveUserQuestion,
    onCancelTurn,
    onDraftChange,
    onRenameTitle
  }: Props = $props();

  type Tab = "chat" | "diff" | "terminal" | "files" | "browser";
  type FileOpenTarget = { path: string; line: number | null; requestId: number };
  let tab = $state<Tab>("chat");
  let sideTab = $state<Tab | null>(null);
  let sideWidth = $state(420);
  let sideDragStart: { pointerId: number; startX: number; startWidth: number } | null = null;
  let fileToOpen = $state<FileOpenTarget | null>(null);
  let fileOpenRequestId = 0;
  let fileToOpenSessionId: string | null = null;
  let rootEl = $state<HTMLElement | undefined>(undefined);
  let searchInputEl = $state<HTMLInputElement | undefined>(undefined);
  let searchOpen = $state(false);
  let searchQuery = $state("");
  let searchMatchCount = $state(0);
  let searchIndex = $state(0);
  let searchMarks: HTMLElement[] = [];

  // Header identity comes straight from the live session record. No
  // local board persona — the daemon is the source of truth.
  let displayName = $derived(sessionDisplayName(session));
  let displayTitle = $derived(sessionDisplayTitle(session));
  let displayBranch = $derived(sessionDetail?.repoStatus?.branch ?? "");
  let projectCwd = $derived(sessionDetail?.repoStatus?.cwd ?? session?.cwd ?? "");
  let displayWorktree = $derived("");
  let status = $derived<AgentStatus>(
    pendingPermissions.length > 0 || pendingQuestions.length > 0
      ? "awaiting"
      : inferStatusFromSession(sessionDetail)
  );
  let editingTitle = $state(false);
  let titleDraft = $state("");
  let titleSaving = $state(false);
  let titleEditSessionId: string | null = null;
  let titleEditGeneration = 0;
  let titleEditing = $derived(Boolean(editingTitle && titleEditSessionId === (session?.id ?? null)));
  let detailSessionId: string | null = null;

  $effect(() => {
    if (!titleEditing) titleDraft = displayName;
  });

  $effect(() => {
    const nextSessionId = session?.id ?? null;
    if (nextSessionId === titleEditSessionId) return;
    titleEditGeneration += 1;
    titleEditSessionId = nextSessionId;
    editingTitle = false;
    titleSaving = false;
    titleDraft = displayName;
  });

  $effect(() => {
    const nextSessionId = session?.id ?? null;
    if (nextSessionId === fileToOpenSessionId) return;
    fileToOpenSessionId = nextSessionId;
    fileToOpen = null;
  });

  $effect(() => {
    const nextSessionId = session?.id ?? null;
    if (nextSessionId === detailSessionId) return;
    detailSessionId = nextSessionId;
    closeSearch();
  });

  function inferStatusFromSession(d: SessionDetail | null): AgentStatus {
    if (
      session?.activityStatus === "running" ||
      session?.activityStatus === "awaiting" ||
      session?.activityStatus === "review"
    ) {
      return session.activityStatus;
    }
    if (!d) return "idle";
    if (d.repoStatus?.pullRequest) return "review";
    return "idle";
  }

  let pufferState = $derived<AgentState>(
    pendingPermissions.length > 0 || pendingQuestions.length > 0
      ? "awaiting"
      : turnRunning
        ? turnThinking
          ? "thinking"
          : "running"
        : agentPufferState(status)
  );
  let statusLabel = $derived(
    turnRunning
      ? turnThinking
        ? "thinking"
        : "running"
      : AGENT_STATE_LABELS[status] ?? status
  );
  let diffCount = $derived(timeline.filter((t) => t.kind === "diff").length);
  function startTitleEdit() {
    if (!session || !onRenameTitle) return;
    titleEditGeneration += 1;
    titleEditSessionId = session.id;
    titleDraft = displayName;
    editingTitle = true;
  }

  function cancelTitleEdit() {
    titleEditGeneration += 1;
    titleDraft = displayName;
    editingTitle = false;
    titleEditSessionId = null;
  }

  async function saveTitleEdit() {
    if (!session || !onRenameTitle || titleSaving || !titleEditing) return;
    const saveGeneration = titleEditGeneration;
    const saveSessionId = session.id;
    titleSaving = true;
    let saved = false;
    try {
      await onRenameTitle(titleDraft);
      saved = true;
    } finally {
      if (titleEditGeneration === saveGeneration && titleEditSessionId === saveSessionId) {
        titleSaving = false;
        if (saved) {
          editingTitle = false;
          titleEditSessionId = null;
        }
      }
    }
  }

  function handleTitleKeydown(event: KeyboardEvent) {
    if (event.key === "Enter") {
      event.preventDefault();
      void saveTitleEdit();
    } else if (event.key === "Escape") {
      event.preventDefault();
      cancelTitleEdit();
    }
  }

  function beginSideResize(event: PointerEvent) {
    sideDragStart = {
      pointerId: event.pointerId,
      startX: event.clientX,
      startWidth: sideWidth
    };
    (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
    event.preventDefault();
  }

  function moveSideResize(event: PointerEvent) {
    if (!sideDragStart || event.pointerId !== sideDragStart.pointerId) return;
    const next = sideDragStart.startWidth + sideDragStart.startX - event.clientX;
    sideWidth = Math.max(300, Math.min(760, Math.round(next)));
  }

  function endSideResize(event: PointerEvent) {
    if (!sideDragStart || event.pointerId !== sideDragStart.pointerId) return;
    try {
      (event.currentTarget as HTMLElement).releasePointerCapture(event.pointerId);
    } catch {
      /* ignore */
    }
    sideDragStart = null;
  }

  function handleTabClick(event: MouseEvent, nextTab: Tab) {
    if (event.metaKey || event.ctrlKey) {
      event.preventDefault();
      if (!sidePanelTabAllowed(nextTab) || nextTab === tab) return;
      sideTab = nextTab;
      if (searchOpen) void refreshSearch(false);
      return;
    }
    tab = nextTab;
    if (sideTab === nextTab) sideTab = null;
    if (searchOpen) void refreshSearch(false);
  }

  function sidePanelTabAllowed(value: Tab): boolean {
    return value === "diff";
  }

  function openLinkedFile(path: string, line: number | null = null) {
    fileToOpen = { path, line, requestId: ++fileOpenRequestId };
    tab = "files";
  }

  function tabLabel(value: Tab): string {
    switch (value) {
      case "chat":
        return "Chat";
      case "diff":
        return "Diff";
      case "terminal":
        return "Terminal";
      case "files":
        return "Files";
      case "browser":
        return "Browser";
    }
  }

  function tabIcon(value: Tab): IconName {
    switch (value) {
      case "chat":
        return "sparkles";
      case "diff":
        return "git";
      case "terminal":
        return "terminal";
      case "files":
        return "folder";
      case "browser":
        return "globe";
    }
  }

  function isEditableTarget(target: EventTarget | null): boolean {
    const element = target instanceof HTMLElement ? target : null;
    if (!element) return false;
    if (element.closest(".pf-agent-find")) return false;
    return Boolean(
      element.closest("input, textarea, select, [contenteditable='true'], .pf-browser-canvas")
    );
  }

  function handleGlobalKeydown(event: KeyboardEvent) {
    const key = event.key.toLowerCase();
    if (isEditableTarget(event.target)) return;
    if ((event.metaKey || event.ctrlKey) && key === "f") {
      event.preventDefault();
      openSearch();
      return;
    }
    if (!searchOpen) return;
    if (event.key === "Escape") {
      event.preventDefault();
      closeSearch();
      return;
    }
    if ((event.metaKey || event.ctrlKey) && key === "g") {
      event.preventDefault();
      jumpSearch(event.shiftKey ? -1 : 1);
    }
  }

  onMount(() => {
    window.addEventListener("keydown", handleGlobalKeydown, true);
    return () => {
      window.removeEventListener("keydown", handleGlobalKeydown, true);
      clearSearchMarks();
    };
  });

  function openSearch() {
    searchOpen = true;
    void tick().then(() => {
      searchInputEl?.focus();
      searchInputEl?.select();
      void refreshSearch(false);
    });
  }

  function closeSearch() {
    searchOpen = false;
    searchQuery = "";
    searchMatchCount = 0;
    searchIndex = 0;
    clearSearchMarks();
  }

  function clearSearchMarks() {
    for (const mark of searchMarks) {
      const parent = mark.parentNode;
      if (!parent) continue;
      parent.replaceChild(document.createTextNode(mark.textContent ?? ""), mark);
      parent.normalize();
    }
    searchMarks = [];
  }

  function searchableScopes(): HTMLElement[] {
    if (!rootEl) return [];
    return Array.from(rootEl.querySelectorAll<HTMLElement>(".pf-agent-detail-content"));
  }

  function textNodeSearchable(node: Text): boolean {
    const parent = node.parentElement;
    if (!parent) return false;
    if (!node.nodeValue?.trim()) return false;
    if (parent.closest(".pf-agent-find, .pf-composer-wrap, input, textarea, select, script, style")) return false;
    return true;
  }

  function collectTextNodes(scopes: HTMLElement[]): Text[] {
    const nodes: Text[] = [];
    for (const scope of scopes) {
      const walker = document.createTreeWalker(scope, NodeFilter.SHOW_TEXT);
      let current = walker.nextNode();
      while (current) {
        if (current instanceof Text && textNodeSearchable(current)) nodes.push(current);
        current = walker.nextNode();
      }
    }
    return nodes;
  }

  function markTextNode(node: Text, query: string) {
    const source = node.nodeValue ?? "";
    const lower = source.toLowerCase();
    const needle = query.toLowerCase();
    let cursor = 0;
    let found = lower.indexOf(needle, cursor);
    if (found === -1) return;

    const fragment = document.createDocumentFragment();
    while (found !== -1) {
      if (found > cursor) fragment.append(document.createTextNode(source.slice(cursor, found)));
      const mark = document.createElement("mark");
      mark.className = "pf-search-mark";
      mark.textContent = source.slice(found, found + query.length);
      mark.dataset.searchIndex = String(searchMarks.length);
      searchMarks.push(mark);
      fragment.append(mark);
      cursor = found + query.length;
      found = lower.indexOf(needle, cursor);
    }
    if (cursor < source.length) fragment.append(document.createTextNode(source.slice(cursor)));
    node.parentNode?.replaceChild(fragment, node);
  }

  async function refreshSearch(resetIndex: boolean) {
    await tick();
    clearSearchMarks();
    const query = searchQuery.trim();
    if (!query) {
      searchMatchCount = 0;
      searchIndex = 0;
      return;
    }
    for (const node of collectTextNodes(searchableScopes())) {
      markTextNode(node, query);
    }
    searchMatchCount = searchMarks.length;
    if (searchMatchCount === 0) {
      searchIndex = 0;
      return;
    }
    searchIndex = resetIndex ? 0 : Math.min(searchIndex, searchMatchCount - 1);
    activateSearchMatch();
  }

  function activateSearchMatch() {
    searchMarks.forEach((mark, index) => {
      mark.classList.toggle("active", index === searchIndex);
      mark.classList.remove("pulse");
    });
    const active = searchMarks[searchIndex];
    if (!active) return;
    active.scrollIntoView({ block: "center", inline: "nearest", behavior: "smooth" });
    window.requestAnimationFrame(() => active.classList.add("pulse"));
  }

  function jumpSearch(direction: 1 | -1) {
    if (searchMatchCount === 0) return;
    searchIndex = (searchIndex + direction + searchMatchCount) % searchMatchCount;
    activateSearchMatch();
  }

  function handleSearchKeydown(event: KeyboardEvent) {
    if (event.key === "Enter") {
      event.preventDefault();
      jumpSearch(event.shiftKey ? -1 : 1);
    } else if (event.key === "Escape") {
      event.preventDefault();
      closeSearch();
    }
  }
</script>

<div class="pf-agent-detail" bind:this={rootEl}>
  <div class="pf-agent-detail-head">
    <button type="button" class="pf-agent-back" onclick={onBack} title="Back to workspace" aria-label="Back">
      <Icon name="chevL" size={13} />
    </button>
    <div class="pf-agent-identity">
      <div class="name" class:editing={titleEditing}>
        {#if titleEditing}
          <input
            class="title-input"
            bind:value={titleDraft}
            onkeydown={handleTitleKeydown}
            disabled={titleSaving}
            aria-label="Session title"
          />
          <button
            type="button"
            class="title-icon-btn"
            onclick={() => void saveTitleEdit()}
            disabled={titleSaving}
            title="Save title"
            aria-label="Save title"
          >
            <Icon name="check" size={12} />
          </button>
          <button
            type="button"
            class="title-icon-btn"
            onclick={cancelTitleEdit}
            disabled={titleSaving}
            title="Cancel"
            aria-label="Cancel title edit"
          >
            <Icon name="x" size={12} />
          </button>
        {:else}
          <span class="primary-title" title={displayName}>{displayName}</span>
          {#if displayTitle}
            <span class="sep">·</span>
            <span class="title" title={displayTitle}>{displayTitle}</span>
          {/if}
          {#if onRenameTitle}
            <button
              type="button"
              class="title-icon-btn"
              onclick={startTitleEdit}
              title="Edit title"
              aria-label="Edit session title"
            >
              <Icon name="edit" size={12} />
            </button>
          {/if}
        {/if}
      </div>
      {#if displayBranch || displayWorktree}
        <div class="meta">
          {#if displayBranch}
            <span class="branch mono"><Icon name="branch" size={10} />{displayBranch}</span>
            {#if displayWorktree}
              <span class="sep">·</span>
            {/if}
          {/if}
          {#if displayWorktree}
            <span class="mono">{displayWorktree}</span>
          {/if}
        </div>
      {/if}
    </div>
    <span class="pf-agent-status-pill" data-status={status}>
      {#if pufferState === "running"}
        <span class="pip"></span>
      {/if}
      {statusLabel}
    </span>
    <div class="pf-agent-tabs" role="group" aria-label="Agent detail panes">
      <button
        type="button"
        class="pf-agent-tab"
        class:on={tab === "chat"}
        aria-pressed={tab === "chat"}
        onclick={(event) => handleTabClick(event, "chat")}
      >
        <Icon name="sparkles" size={12} />Chat
      </button>
      <button
        type="button"
        class="pf-agent-tab"
        class:on={tab === "diff"}
        aria-pressed={tab === "diff"}
        onclick={(event) => handleTabClick(event, "diff")}
      >
        <Icon name="git" size={12} />Diff
        {#if diffCount > 0}
          <span class="pf-agent-tab-badge">{diffCount}</span>
        {/if}
      </button>
      <button
        type="button"
        class="pf-agent-tab"
        class:on={tab === "terminal"}
        aria-pressed={tab === "terminal"}
        onclick={(event) => handleTabClick(event, "terminal")}
      >
        <Icon name="terminal" size={12} />Terminal
      </button>
      <button
        type="button"
        class="pf-agent-tab"
        class:on={tab === "files"}
        aria-pressed={tab === "files"}
        onclick={(event) => handleTabClick(event, "files")}
      >
        <Icon name="folder" size={12} />Files
      </button>
      <button
        type="button"
        class="pf-agent-tab"
        class:on={tab === "browser"}
        aria-pressed={tab === "browser"}
        onclick={(event) => handleTabClick(event, "browser")}
      >
        <Icon name="globe" size={12} />Browser
      </button>
    </div>
    <button type="button" class="pf-agent-close" onclick={onBack} title="Close session" aria-label="Close session">
      <Icon name="x" size={13} />
    </button>
  </div>

  <div class="pf-agent-detail-shell" class:withSubpage={sideTab !== null}>
    <div class="pf-agent-detail-body">
      <AgentDetailContent
        {tab}
        {session}
        {sessionDetail}
        {timeline}
        {pendingPermissions}
        {pendingQuestions}
        {resolvingPermissionIds}
        {resolvingQuestionIds}
        {loading}
        {displayName}
        {pufferState}
        {projectCwd}
        {turnRunning}
        {turnCancelable}
        {turnStartedAtMs}
        {turnThinking}
        {turnStatusHint}
        {settingsSnapshot}
        {backendConnected}
        {browserRenderer}
        {userDisplayName}
        {onSubmitMessage}
        {onResolvePermission}
        {onResolveUserQuestion}
        {onCancelTurn}
        {onDraftChange}
        onOpenFileLink={openLinkedFile}
        {fileToOpen}
      />
    </div>
    {#if sideTab}
      <div class="pf-side-panel" style:width={`${sideWidth}px`}>
        <button
          class="pf-side-resize"
          type="button"
          aria-label="Resize side page"
          onpointerdown={beginSideResize}
          onpointermove={moveSideResize}
          onpointerup={endSideResize}
          onpointercancel={endSideResize}
        ></button>
        <div class="pf-side-head">
          <span><Icon name={tabIcon(sideTab)} size={12} />{tabLabel(sideTab)}</span>
          <button
            type="button"
            class="pf-side-close"
            aria-label="Close side page"
            onclick={() => (sideTab = null)}
          >
            <Icon name="x" size={12} />
          </button>
        </div>
        <AgentDetailContent
          tab={sideTab}
          {session}
          {sessionDetail}
          {timeline}
          {pendingPermissions}
          {pendingQuestions}
          {resolvingPermissionIds}
          {resolvingQuestionIds}
          {loading}
          {displayName}
          {pufferState}
          {projectCwd}
          {turnRunning}
          {turnCancelable}
          {turnStartedAtMs}
          {turnThinking}
          {turnStatusHint}
          {settingsSnapshot}
          {backendConnected}
          {browserRenderer}
          {userDisplayName}
          {onSubmitMessage}
          {onResolvePermission}
          {onResolveUserQuestion}
          {onCancelTurn}
          {onDraftChange}
          onOpenFileLink={openLinkedFile}
          {fileToOpen}
        />
      </div>
    {/if}
  </div>

  {#if searchOpen}
    <div class="pf-agent-find" role="search" aria-label="Find in agent view">
      <div class="find-glow" aria-hidden="true"></div>
      <Icon name="search" size={15} />
      <input
        bind:this={searchInputEl}
        bind:value={searchQuery}
        placeholder={tab === "diff" ? "Search diff…" : "Search chat…"}
        oninput={() => void refreshSearch(true)}
        onkeydown={handleSearchKeydown}
      />
      <span class="find-count" data-empty={searchQuery.trim() && searchMatchCount === 0}>
        {#if searchQuery.trim()}
          {searchMatchCount === 0 ? "0 results" : `${searchIndex + 1} / ${searchMatchCount}`}
        {:else}
          Chat + Diff
        {/if}
      </span>
      <button type="button" class="find-btn" onclick={() => jumpSearch(-1)} disabled={searchMatchCount === 0} aria-label="Previous match">
        <Icon name="arrowUp" size={13} />
      </button>
      <button type="button" class="find-btn" onclick={() => jumpSearch(1)} disabled={searchMatchCount === 0} aria-label="Next match">
        <Icon name="chevD" size={14} />
      </button>
      <button type="button" class="find-btn" onclick={closeSearch} aria-label="Close find">
        <Icon name="x" size={13} />
      </button>
    </div>
  {/if}
</div>

<style>
  .pf-agent-detail {
    position: relative;
    flex: 1;
    display: flex;
    flex-direction: column;
    min-height: 0;
    background: var(--background);
  }
  .pf-agent-detail-head {
    flex-shrink: 0;
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 14px;
    background: color-mix(in oklab, var(--background) 96%, var(--muted));
    border-bottom: 1px solid var(--border);
    min-height: 52px;
  }
  .pf-agent-back {
    width: 28px;
    height: 28px;
    border-radius: 6px;
    border: 1px solid var(--border);
    background: var(--background);
    color: var(--muted-foreground);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
    flex-shrink: 0;
    transition: background 120ms, color 120ms;
  }
  .pf-agent-back:hover { background: var(--accent); color: var(--foreground); }
  .pf-agent-close {
    width: 28px;
    height: 28px;
    border-radius: 6px;
    border: 1px solid transparent;
    background: transparent;
    color: var(--muted-foreground);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
    flex-shrink: 0;
    transition: background 120ms, color 120ms, border-color 120ms;
  }
  .pf-agent-close:hover {
    background: var(--accent);
    color: var(--foreground);
    border-color: var(--border);
  }
  .pf-agent-identity {
    display: flex;
    flex-direction: column;
    gap: 1px;
    min-width: 0;
    flex: 1 1 auto;
  }
  .pf-agent-identity .name {
    font-size: 14px;
    font-weight: 600;
    letter-spacing: 0;
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
  }
  .pf-agent-identity .name.editing {
    align-items: center;
  }
  .pf-agent-identity .name .primary-title {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .pf-agent-identity .name .sep { color: var(--muted-foreground); opacity: 0.5; }
  .pf-agent-identity .name .title {
    font-weight: 500;
    color: var(--foreground);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .title-input {
    width: min(320px, 34vw);
    height: 26px;
    min-width: 140px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--background);
    color: var(--foreground);
    font: inherit;
    padding: 0 8px;
    outline: none;
  }
  .title-input:focus {
    border-color: color-mix(in oklab, var(--accent-foreground) 35%, var(--border));
    box-shadow: 0 0 0 2px color-mix(in oklab, var(--accent) 70%, transparent);
  }
  .title-icon-btn {
    width: 24px;
    height: 24px;
    border-radius: 5px;
    border: 1px solid transparent;
    background: transparent;
    color: var(--muted-foreground);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
    flex-shrink: 0;
  }
  .title-icon-btn:hover:not(:disabled) {
    color: var(--foreground);
    background: var(--accent);
    border-color: var(--border);
  }
  .title-icon-btn:disabled {
    cursor: wait;
    opacity: 0.55;
  }
  .pf-agent-identity .meta {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 11px;
    color: var(--muted-foreground);
  }
  .pf-agent-identity .meta .mono { font-family: var(--font-mono); }
  .pf-agent-identity .meta .sep { opacity: 0.4; }
  .pf-agent-identity .meta .branch {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 1px 6px;
    border-radius: 4px;
    background: var(--muted);
  }

  .pf-agent-status-pill {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    font-size: 10.5px;
    font-weight: 600;
    font-family: var(--font-mono);
    padding: 3px 8px;
    border-radius: 999px;
    background: var(--muted);
    color: var(--muted-foreground);
    text-transform: lowercase;
    flex-shrink: 0;
    margin-left: auto;
  }
  .pf-agent-status-pill[data-status="running"]  { background: color-mix(in oklab, oklch(0.7 0.17 70) 15%, var(--background)); color: oklch(0.55 0.17 70); }
  .pf-agent-status-pill[data-status="awaiting"] { background: color-mix(in oklab, oklch(0.72 0.18 30) 16%, var(--background)); color: oklch(0.55 0.2 30); }
  .pf-agent-status-pill[data-status="review"]   { background: color-mix(in oklab, oklch(0.7 0.16 40) 15%, var(--background));  color: oklch(0.55 0.17 40); }
  .pf-agent-status-pill .pip {
    width: 6px; height: 6px; border-radius: 50%;
    background: oklch(0.7 0.17 70);
    animation: pf-pulse-dot 1.6s infinite;
  }
  @keyframes pf-pulse-dot {
    0%, 100% { opacity: 1; }
    50% { opacity: 0.4; }
  }

  .pf-agent-tabs {
    display: flex;
    gap: 1px;
    background: var(--muted);
    padding: 3px;
    border-radius: 8px;
    flex-shrink: 0;
  }
  .pf-agent-tab {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 5px 10px;
    font-size: 12px;
    font-weight: 500;
    color: var(--muted-foreground);
    border: 0;
    background: transparent;
    border-radius: 5px;
    cursor: pointer;
    transition: background 120ms, color 120ms;
    font: inherit;
  }
  .pf-agent-tab:hover { color: var(--foreground); }
  .pf-agent-tab.on {
    background: var(--background);
    color: var(--foreground);
    box-shadow: 0 1px 2px rgb(0 0 0 / 0.06);
  }
  .pf-agent-tab-badge {
    font-size: 9px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    padding: 1px 5px;
    border-radius: 3px;
    background: oklch(0.7 0.16 40);
    color: white;
    margin-left: 2px;
  }

  .pf-agent-detail-shell {
    flex: 1;
    min-height: 0;
    display: flex;
    overflow: hidden;
  }

  .pf-agent-detail-body {
    flex: 1 1 auto;
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .pf-side-panel {
    flex: 0 0 auto;
    min-width: 300px;
    max-width: 760px;
    min-height: 0;
    position: relative;
    display: flex;
    flex-direction: column;
    border-left: 1px solid var(--border);
    box-shadow: -8px 0 20px rgb(0 0 0 / 0.04);
    background: var(--background);
  }

  .pf-side-resize {
    position: absolute;
    z-index: 5;
    top: 0;
    bottom: 0;
    left: -4px;
    width: 8px;
    padding: 0;
    border: 0;
    background: transparent;
    cursor: col-resize;
    touch-action: none;
  }

  .pf-side-resize::before {
    content: "";
    position: absolute;
    top: 0;
    bottom: 0;
    left: 3px;
    width: 2px;
    background: transparent;
  }

  .pf-side-resize:hover::before {
    background: color-mix(in oklab, var(--accent-foreground) 35%, var(--border));
  }

  .pf-side-head {
    height: 36px;
    flex: 0 0 auto;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    padding: 0 10px 0 12px;
    border-bottom: 1px solid var(--border);
    background: color-mix(in oklab, var(--background) 96%, var(--muted));
    color: var(--foreground);
    font-size: 12px;
    font-weight: 600;
  }

  .pf-side-head span {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
  }

  .pf-side-close {
    width: 24px;
    height: 24px;
    border: 1px solid transparent;
    border-radius: 5px;
    background: transparent;
    color: var(--muted-foreground);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
  }

  .pf-side-close:hover {
    color: var(--foreground);
    background: var(--accent);
    border-color: var(--border);
  }

  .pf-side-panel :global(.pf-agent-detail-content) {
    flex: 1;
    min-height: 0;
  }

  .pf-agent-find {
    position: absolute;
    z-index: 40;
    left: 50%;
    top: 76px;
    transform: translateX(-50%);
    width: min(560px, calc(100% - 44px));
    min-height: 48px;
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto auto auto auto;
    align-items: center;
    gap: 8px;
    padding: 8px 10px 8px 12px;
    border: 1px solid color-mix(in oklab, white 36%, var(--border));
    border-radius: 18px;
    background:
      linear-gradient(
        135deg,
        color-mix(in oklab, var(--background) 72%, white) 0%,
        color-mix(in oklab, var(--background) 50%, transparent) 100%
      );
    color: var(--foreground);
    box-shadow:
      0 18px 50px rgb(0 0 0 / 0.18),
      inset 0 1px 0 rgb(255 255 255 / 0.36),
      inset 0 -1px 0 rgb(255 255 255 / 0.14);
    backdrop-filter: blur(22px) saturate(170%);
  }

  .find-glow {
    position: absolute;
    inset: -2px;
    z-index: -1;
    border-radius: 20px;
    background:
      radial-gradient(circle at 18% 0%, color-mix(in oklab, var(--puffer-accent) 34%, transparent), transparent 32%),
      radial-gradient(circle at 86% 12%, color-mix(in oklab, oklch(0.78 0.16 210) 28%, transparent), transparent 36%);
    filter: blur(8px);
    opacity: 0.88;
    pointer-events: none;
  }

  .pf-agent-find input {
    min-width: 0;
    height: 30px;
    border: 0;
    outline: none;
    background: transparent;
    color: var(--foreground);
    font: inherit;
    font-size: 13px;
  }

  .pf-agent-find input::placeholder {
    color: var(--muted-foreground);
  }

  .find-count {
    min-width: 72px;
    text-align: right;
    color: var(--muted-foreground);
    font-family: var(--font-mono);
    font-size: 11px;
    white-space: nowrap;
  }

  .find-count[data-empty="true"] {
    color: var(--destructive);
  }

  .find-btn {
    width: 30px;
    height: 30px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border: 1px solid color-mix(in oklab, var(--border) 76%, transparent);
    border-radius: 999px;
    background: color-mix(in oklab, var(--background) 58%, transparent);
    color: var(--muted-foreground);
    cursor: pointer;
  }

  .find-btn:hover:not(:disabled) {
    color: var(--foreground);
    background: color-mix(in oklab, var(--accent) 70%, transparent);
  }

  .find-btn:disabled {
    opacity: 0.42;
    cursor: default;
  }

  :global(.pf-search-mark) {
    border-radius: 4px;
    padding: 0 2px;
    color: inherit;
    background: color-mix(in oklab, oklch(0.86 0.15 90) 62%, transparent);
    box-shadow: 0 0 0 1px color-mix(in oklab, oklch(0.78 0.16 90) 45%, transparent);
  }

  :global(.pf-search-mark.active) {
    background: color-mix(in oklab, var(--puffer-accent) 42%, white);
    box-shadow:
      0 0 0 2px color-mix(in oklab, var(--puffer-accent) 54%, transparent),
      0 0 18px color-mix(in oklab, var(--puffer-accent) 28%, transparent);
  }

  :global(.pf-search-mark.active.pulse) {
    animation: pf-search-pulse 900ms ease-out;
  }

  @keyframes pf-search-pulse {
    0% {
      box-shadow:
        0 0 0 2px color-mix(in oklab, var(--puffer-accent) 64%, transparent),
        0 0 0 0 color-mix(in oklab, var(--puffer-accent) 34%, transparent);
    }
    100% {
      box-shadow:
        0 0 0 2px color-mix(in oklab, var(--puffer-accent) 54%, transparent),
        0 0 0 12px transparent;
    }
  }

  @media (max-width: 720px) {
    .pf-agent-detail-head { flex-wrap: wrap; row-gap: 6px; padding: 8px 10px; }
    .pf-agent-tabs { order: 3; width: 100%; overflow-x: auto; }
    .pf-agent-status-pill { order: 2; margin-left: 0; }
    .pf-agent-find {
      top: 92px;
      grid-template-columns: auto minmax(0, 1fr) auto auto auto;
      width: calc(100% - 20px);
    }
    .find-count {
      display: none;
    }
  }
</style>

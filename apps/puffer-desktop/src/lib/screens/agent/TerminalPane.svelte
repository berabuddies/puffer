<script lang="ts">
  import { onDestroy, tick, untrack } from "svelte";
  import { Terminal } from "@xterm/xterm";
  import { FitAddon } from "@xterm/addon-fit";
  import "@xterm/xterm/css/xterm.css";
  import { ensureLocalDaemonClient } from "../../api/daemonClient";
  import {
    closePty,
    focusPty,
    isDaemonReachable,
    listPtys,
    openPty,
    replayPty,
    resizePty,
    writePty,
    type PtyTabInfo
  } from "../../api/desktop";
  import Icon from "../../design/Icon.svelte";

  type Props = {
    /** Filesystem root the shell starts in. Sessions pass their cwd here. */
    cwd: string;
    /** Agent session id used to keep daemon-owned PTYs grouped correctly. */
    sessionId?: string;
  };
  let { cwd, sessionId = "preview" }: Props = $props();

  type PtyDataEvent = {
    data: string;
    seq?: number;
  };

  let container: HTMLDivElement | null = $state(null);
  let term: Terminal | null = null;
  let fit: FitAddon | null = null;
  let activePtyId = $state<string | null>(null);
  let ptyTabs = $state<PtyTabInfo[]>([]);
  let loading = $state(false);
  let creatingPty = $state(false);
  let closingPtyIds = $state<string[]>([]);
  let error = $state<string | null>(null);
  let closeError = $state<string | null>(null);
  let disposed = false;
  let attachGeneration = 0;
  let dataDisposer: (() => void) | null = null;
  let exitDisposer: (() => void) | null = null;
  let inputDisposer: { dispose: () => void } | null = null;
  let resizeObserver: ResizeObserver | null = null;
  let seenSeqByPty = new Map<string, number>();
  let restoreGeneration = 0;
  const previewMode = !isDaemonReachable();

  $effect(() => {
    const targetSessionId = sessionId;
    const targetCwd = cwd;
    if (previewMode || targetSessionId === "preview") return;
    untrack(() => {
      const generation = ++restoreGeneration;
      activePtyId = null;
      ptyTabs = [];
      creatingPty = false;
      closingPtyIds = [];
      closeError = null;
      seenSeqByPty = new Map();
      cleanupTerminalAttach();
      void restoreOrCreateTerminal(targetSessionId, targetCwd, generation);
    });
  });

  onDestroy(() => {
    disposed = true;
    cleanupTerminalAttach();
  });

  async function restoreOrCreateTerminal(targetSessionId: string, targetCwd: string, generation: number) {
    if (targetSessionId === "preview") return;
    loading = true;
    error = null;
    closeError = null;
    try {
      const info = await listPtys(targetSessionId);
      if (disposed || generation !== restoreGeneration || targetSessionId !== sessionId || targetCwd !== cwd) return;
      ptyTabs = info.tabs;
      const active = info.tabs.find((tab) => tab.active) ?? info.tabs[0];
      if (active) {
        await activatePty(active.ptyId, targetSessionId, generation);
      } else if (!info.initialized) {
        await createTerminalTab(targetSessionId, targetCwd, generation);
      }
    } catch (err) {
      if (generation === restoreGeneration) error = err instanceof Error ? err.message : String(err);
    } finally {
      if (generation === restoreGeneration) loading = false;
    }
  }

  async function createTerminalTab(targetSessionId = sessionId, targetCwd = cwd, generation = restoreGeneration) {
    if (previewMode || targetSessionId === "preview" || creatingPty) return;
    creatingPty = true;
    loading = true;
    error = null;
    closeError = null;
    try {
      const title = nextTerminalTitle();
      const { ptyId } = await openPty({
        sessionId: targetSessionId,
        cwd: targetCwd,
        cols: term?.cols ?? 80,
        rows: term?.rows ?? 24,
        title
      });
      const info = await listPtys(targetSessionId);
      if (disposed || generation !== restoreGeneration || targetSessionId !== sessionId || targetCwd !== cwd) return;
      ptyTabs = info.tabs;
      await activatePty(ptyId, targetSessionId, generation);
    } catch (err) {
      if (generation === restoreGeneration) error = err instanceof Error ? err.message : String(err);
    } finally {
      if (generation === restoreGeneration) {
        loading = false;
        creatingPty = false;
      }
    }
  }

  function terminalContextCurrent(targetSessionId: string, generation: number): boolean {
    return !disposed && generation === restoreGeneration && targetSessionId === sessionId;
  }

  async function activatePty(
    ptyId: string,
    targetSessionId = sessionId,
    generation = restoreGeneration
  ) {
    if (!terminalContextCurrent(targetSessionId, generation)) return;
    if (ptyId === activePtyId && term && dataDisposer) return;
    activePtyId = ptyId;
    ptyTabs = ptyTabs.map((tab) => ({ ...tab, active: tab.ptyId === ptyId }));
    await focusPty(ptyId).catch(() => {});
    if (!terminalContextCurrent(targetSessionId, generation)) return;
    await attachTerminal(ptyId, targetSessionId, generation);
  }

  async function attachTerminal(
    ptyId: string,
    targetSessionId = sessionId,
    restoreContext = restoreGeneration
  ) {
    const generation = ++attachGeneration;
    cleanupTerminalAttach();
    await tick();
    if (
      !terminalContextCurrent(targetSessionId, restoreContext) ||
      generation !== attachGeneration ||
      !container
    ) {
      return;
    }

    const t = new Terminal({
      cursorBlink: true,
      fontFamily: '"JetBrains Mono", "JetBrainsMono Nerd Font", "SF Mono", Menlo, Consolas, monospace',
      fontSize: 13,
      letterSpacing: 0,
      scrollback: 3000,
      theme: {
        background: "#ffffff",
        foreground: "#171717",
        cursor: "#171717",
        selectionBackground: "#d4d4d4",
        black: "#171717",
        red: "#b91c1c",
        green: "#15803d",
        yellow: "#a16207",
        blue: "#1d4ed8",
        magenta: "#9333ea",
        cyan: "#0e7490",
        white: "#f5f5f5",
        brightBlack: "#737373",
        brightRed: "#dc2626",
        brightGreen: "#16a34a",
        brightYellow: "#ca8a04",
        brightBlue: "#2563eb",
        brightMagenta: "#a855f7",
        brightCyan: "#0891b2",
        brightWhite: "#ffffff"
      }
    });
    const fa = new FitAddon();
    t.loadAddon(fa);
    t.open(container);
    term = t;
    fit = fa;
    fitTerminal(ptyId, generation);

    const client = await ensureLocalDaemonClient();
    if (
      !terminalContextCurrent(targetSessionId, restoreContext) ||
      generation !== attachGeneration
    ) {
      return;
    }

    seenSeqByPty.set(ptyId, 0);
    let replaying = true;
    const queued: PtyDataEvent[] = [];

    dataDisposer = client.on<PtyDataEvent>(`pty:${ptyId}:data`, (event) => {
      if (replaying) {
        queued.push(event);
      } else {
        writePtyEvent(ptyId, event);
      }
    });
    exitDisposer = client.on<{ exitCode: number }>(`pty:${ptyId}:exit`, ({ exitCode }) => {
      t.writeln(`\r\n\x1b[90m[exit ${exitCode}]\x1b[0m`);
      ptyTabs = ptyTabs.map((tab) =>
        tab.ptyId === ptyId && !tab.title.endsWith(" (exit)")
          ? { ...tab, title: `${tab.title} (exit)` }
          : tab
      );
    });

    try {
      const chunks = await replayPty(ptyId);
      if (
        !terminalContextCurrent(targetSessionId, restoreContext) ||
        generation !== attachGeneration
      ) {
        return;
      }
      for (const chunk of chunks) writePtyEvent(ptyId, chunk);
    } catch (err) {
      if (
        terminalContextCurrent(targetSessionId, restoreContext) &&
        generation === attachGeneration
      ) {
        t.writeln(`\r\n\x1b[31mterminal replay: ${String(err)}\x1b[0m`);
      }
    } finally {
      replaying = false;
      if (
        terminalContextCurrent(targetSessionId, restoreContext) &&
        generation === attachGeneration
      ) {
        for (const event of queued) writePtyEvent(ptyId, event);
      }
    }

    if (
      !terminalContextCurrent(targetSessionId, restoreContext) ||
      generation !== attachGeneration
    ) {
      return;
    }

    inputDisposer = t.onData((str) => {
      if (closingPtyIds.includes(ptyId)) return;
      const bytes = new TextEncoder().encode(str);
      let bin = "";
      for (const b of bytes) bin += String.fromCharCode(b);
      void writePty(ptyId, btoa(bin)).catch(() => {});
    });

    resizeObserver = new ResizeObserver(() => fitTerminal(ptyId, generation));
    resizeObserver.observe(container);
  }

  function writePtyEvent(ptyId: string, event: PtyDataEvent) {
    if (!term) return;
    if (typeof event.seq === "number") {
      const seen = seenSeqByPty.get(ptyId) ?? 0;
      if (event.seq <= seen) return;
      seenSeqByPty.set(ptyId, event.seq);
    }
    try {
      const raw = atob(event.data);
      const bytes = new Uint8Array(raw.length);
      for (let index = 0; index < raw.length; index += 1) {
        bytes[index] = raw.charCodeAt(index);
      }
      term.write(bytes);
    } catch {
      /* malformed frame - skip */
    }
  }

  function fitTerminal(ptyId: string, generation = attachGeneration) {
    if (disposed || generation !== attachGeneration || activePtyId !== ptyId) return;
    if (!term || !fit) return;
    try {
      fit.fit();
    } catch {
      return;
    }
    void resizePty(ptyId, term.cols, term.rows).catch(() => {});
  }

  async function closeTerminalTab(event: Event, ptyId: string) {
    event.stopPropagation();
    if (closingPtyIds.includes(ptyId)) return;
    const targetSessionId = sessionId;
    const generation = restoreGeneration;
    closingPtyIds = [...closingPtyIds, ptyId];
    try {
      const closingIndex = ptyTabs.findIndex((tab) => tab.ptyId === ptyId);
      try {
        await closePty(ptyId);
      } catch (err) {
        if (disposed || generation !== restoreGeneration || targetSessionId !== sessionId) return;
        const message = err instanceof Error ? err.message : String(err);
        if (activePtyId === ptyId || ptyTabs.length <= 1) {
          error = message;
          closeError = null;
        } else {
          closeError = message;
        }
        return;
      }
      if (disposed || generation !== restoreGeneration || targetSessionId !== sessionId) return;
      error = null;
      closeError = null;
      const nextTabs = ptyTabs.filter((tab) => tab.ptyId !== ptyId);
      ptyTabs = nextTabs;
      seenSeqByPty.delete(ptyId);
      if (activePtyId !== ptyId) return;
      const next = nextTabs[Math.min(closingIndex, nextTabs.length - 1)] ?? nextTabs[nextTabs.length - 1];
      if (next) {
        await activatePty(next.ptyId);
      } else {
        activePtyId = null;
        cleanupTerminalAttach();
      }
    } finally {
      if (!disposed && generation === restoreGeneration && targetSessionId === sessionId) {
        closingPtyIds = closingPtyIds.filter((id) => id !== ptyId);
      }
    }
  }

  function cleanupTerminalAttach() {
    dataDisposer?.();
    exitDisposer?.();
    dataDisposer = null;
    exitDisposer = null;
    inputDisposer?.dispose();
    inputDisposer = null;
    resizeObserver?.disconnect();
    resizeObserver = null;
    if (term) {
      term.dispose();
      term = null;
    }
    fit = null;
  }

  function nextTerminalTitle(): string {
    const used = new Set(ptyTabs.map((tab) => tab.title));
    for (let index = 1; index < 100; index += 1) {
      const candidate = `Terminal ${index}`;
      if (!used.has(candidate)) return candidate;
    }
    return "Terminal";
  }
</script>

<div class="pf-terminal-pane">
  {#if previewMode}
    <div class="terminal-empty">
      <Icon name="terminal" size={20} color="var(--muted-foreground)" />
      <div class="title">Terminal is available in the Puffer desktop app</div>
      <div class="sub">Launch Puffer locally to get a live shell in this session's cwd.</div>
    </div>
  {:else}
    <div class="terminal-tabs" role="tablist" aria-label="Terminal sessions">
      {#each ptyTabs as tab (tab.ptyId)}
        <div class="terminal-tab" class:active={activePtyId === tab.ptyId}>
          <button
            type="button"
            role="tab"
            aria-selected={activePtyId === tab.ptyId}
            class="terminal-tab-main"
            title={tab.cwd}
            onclick={() => void activatePty(tab.ptyId)}
          >
            <Icon name="terminal" size={11} color="var(--muted-foreground)" />
            <span>{tab.title}</span>
          </button>
          <button
            type="button"
            class="terminal-tab-close"
            title={`Close ${tab.title}`}
            aria-label="Close {tab.title}"
            disabled={closingPtyIds.includes(tab.ptyId)}
            onclick={(event) => void closeTerminalTab(event, tab.ptyId)}
          >
            <Icon name="x" size={11} />
          </button>
        </div>
      {/each}
      <button
        type="button"
        class="terminal-new"
        title="New terminal"
        aria-label="New terminal"
        onclick={() => void createTerminalTab()}
        disabled={loading}
      >
        <Icon name="plus" size={13} />
      </button>
    </div>

    {#if closeError}
      <div class="terminal-inline-error" role="alert">{closeError}</div>
    {/if}

    {#if error}
      <div class="terminal-empty error">
        <Icon name="terminal" size={20} color="var(--muted-foreground)" />
        <div class="title">Terminal failed</div>
        <div class="sub mono">{error}</div>
      </div>
    {:else if loading && ptyTabs.length === 0}
      <div class="terminal-empty">
        <Icon name="terminal" size={20} color="var(--muted-foreground)" />
        <div class="title">Starting terminal...</div>
      </div>
    {:else if activePtyId}
      <div class="pf-terminal-host" bind:this={container}></div>
    {:else}
      <div class="terminal-empty">
        <Icon name="terminal" size={20} color="var(--muted-foreground)" />
        <div class="title">No terminal open</div>
        <button type="button" class="empty-action" onclick={() => void createTerminalTab()}>
          New terminal
        </button>
      </div>
    {/if}
  {/if}
</div>

<style>
  .pf-terminal-pane {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    background: var(--background);
  }
  .terminal-tabs {
    flex-shrink: 0;
    min-height: 32px;
    display: flex;
    align-items: stretch;
    overflow-x: auto;
    overflow-y: hidden;
    border-bottom: 1px solid var(--border);
    background: color-mix(in oklab, var(--background) 95%, var(--muted));
  }
  .terminal-tab {
    min-width: 128px;
    max-width: 220px;
    display: inline-flex;
    align-items: center;
    border-right: 1px solid var(--border);
    border-bottom: 2px solid transparent;
    color: var(--muted-foreground);
  }
  .terminal-tab.active {
    background: var(--background);
    border-bottom-color: var(--puffer-accent, var(--foreground));
    color: var(--foreground);
  }
  .terminal-tab-main {
    flex: 1;
    min-width: 0;
    height: 30px;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 0 4px 0 10px;
    border: 0;
    background: transparent;
    color: inherit;
    font: inherit;
    font-family: var(--font-mono);
    font-size: 12px;
    cursor: pointer;
  }
  .terminal-tab-main span {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .terminal-tab-main:hover,
  .terminal-tab-close:hover:not(:disabled),
  .terminal-new:hover:not(:disabled) {
    background: color-mix(in oklab, var(--accent) 55%, transparent);
    color: var(--foreground);
  }
  .terminal-tab-close,
  .terminal-new {
    width: 28px;
    height: 30px;
    flex-shrink: 0;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border: 0;
    background: transparent;
    color: var(--muted-foreground);
    cursor: pointer;
  }
  .terminal-tab-close:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }
  .terminal-new {
    border-right: 1px solid var(--border);
  }
  .terminal-new:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .terminal-inline-error {
    flex-shrink: 0;
    padding: 7px 10px;
    border-bottom: 1px solid color-mix(in oklab, oklch(0.7 0.18 25) 28%, var(--border));
    background: color-mix(in oklab, oklch(0.7 0.18 25) 10%, var(--background));
    color: oklch(0.5 0.2 25);
    font-family: var(--font-mono);
    font-size: 12px;
    line-height: 1.45;
  }
  .pf-terminal-host {
    flex: 1;
    min-height: 0;
    padding: 10px;
    background: var(--background);
  }
  .terminal-empty {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 8px;
    padding: 40px;
    color: var(--muted-foreground);
    text-align: center;
  }
  .terminal-empty .title {
    font-size: 14px;
    font-weight: 600;
    color: var(--foreground);
  }
  .terminal-empty .sub {
    max-width: 380px;
    font-size: 12.5px;
    line-height: 1.55;
  }
  .terminal-empty .mono {
    font-family: var(--font-mono);
    white-space: pre-wrap;
    word-break: break-word;
  }
  .terminal-empty.error .sub {
    color: oklch(0.55 0.2 30);
  }
  .empty-action {
    height: 28px;
    border: 1px solid var(--border);
    border-radius: 5px;
    padding: 0 10px;
    background: var(--background);
    color: var(--foreground);
    font-size: 12px;
    font-weight: 500;
    cursor: pointer;
  }
  .empty-action:hover {
    background: var(--accent);
  }
  /* xterm.js sets its own inline sizing; we just need the host to fill. */
  .pf-terminal-host :global(.xterm),
  .pf-terminal-host :global(.xterm-viewport),
  .pf-terminal-host :global(.xterm-screen) {
    height: 100%;
  }
  .pf-terminal-host :global(.xterm) {
    padding: 8px;
    border: 0;
    border-radius: 0;
    background: var(--background);
    letter-spacing: 0;
  }
  .pf-terminal-host :global(.xterm-viewport) {
    background: var(--background) !important;
  }
</style>

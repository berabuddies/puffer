<script lang="ts">
  import { onDestroy, onMount, tick } from "svelte";
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
  let error = $state<string | null>(null);
  let disposed = false;
  let attachGeneration = 0;
  let dataDisposer: (() => void) | null = null;
  let exitDisposer: (() => void) | null = null;
  let inputDisposer: { dispose: () => void } | null = null;
  let resizeObserver: ResizeObserver | null = null;
  let seenSeqByPty = new Map<string, number>();
  const previewMode = !isDaemonReachable();

  onMount(() => {
    if (previewMode) return;
    void restoreOrCreateTerminal();
  });

  onDestroy(() => {
    disposed = true;
    cleanupTerminalAttach();
  });

  async function restoreOrCreateTerminal() {
    if (sessionId === "preview") return;
    loading = true;
    error = null;
    try {
      const info = await listPtys(sessionId);
      if (disposed) return;
      ptyTabs = info.tabs;
      const active = info.tabs.find((tab) => tab.active) ?? info.tabs[0];
      if (active) {
        await activatePty(active.ptyId);
      } else if (!info.initialized) {
        await createTerminalTab();
      }
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    } finally {
      loading = false;
    }
  }

  async function createTerminalTab() {
    if (previewMode || sessionId === "preview") return;
    loading = true;
    error = null;
    try {
      const title = nextTerminalTitle();
      const { ptyId } = await openPty({
        sessionId,
        cwd,
        cols: term?.cols ?? 80,
        rows: term?.rows ?? 24,
        title
      });
      const info = await listPtys(sessionId);
      if (disposed) return;
      ptyTabs = info.tabs;
      await activatePty(ptyId);
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    } finally {
      loading = false;
    }
  }

  async function activatePty(ptyId: string) {
    if (disposed) return;
    activePtyId = ptyId;
    ptyTabs = ptyTabs.map((tab) => ({ ...tab, active: tab.ptyId === ptyId }));
    await focusPty(ptyId).catch(() => {});
    await attachTerminal(ptyId);
  }

  async function attachTerminal(ptyId: string) {
    const generation = ++attachGeneration;
    cleanupTerminalAttach();
    await tick();
    if (disposed || generation !== attachGeneration || !container) return;

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
    fitTerminal(ptyId);

    const client = await ensureLocalDaemonClient();
    if (disposed || generation !== attachGeneration) return;

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
      if (disposed || generation !== attachGeneration) return;
      for (const chunk of chunks) writePtyEvent(ptyId, chunk);
    } catch (err) {
      if (generation === attachGeneration) {
        t.writeln(`\r\n\x1b[31mterminal replay: ${String(err)}\x1b[0m`);
      }
    } finally {
      replaying = false;
      if (generation === attachGeneration) {
        for (const event of queued) writePtyEvent(ptyId, event);
      }
    }

    inputDisposer = t.onData((str) => {
      const bytes = new TextEncoder().encode(str);
      let bin = "";
      for (const b of bytes) bin += String.fromCharCode(b);
      void writePty(ptyId, btoa(bin)).catch(() => {});
    });

    resizeObserver = new ResizeObserver(() => fitTerminal(ptyId));
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
      term.write(atob(event.data));
    } catch {
      /* malformed frame - skip */
    }
  }

  function fitTerminal(ptyId: string) {
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
    const closingIndex = ptyTabs.findIndex((tab) => tab.ptyId === ptyId);
    await closePty(ptyId).catch(() => {});
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
      <div class="title">Terminal is available in the Corbina desktop app</div>
      <div class="sub">Launch Corbina locally to get a live shell in this session's cwd.</div>
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
            title="Close terminal"
            aria-label="Close {tab.title}"
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
  .terminal-tab-close:hover,
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
  .terminal-new {
    border-right: 1px solid var(--border);
  }
  .terminal-new:disabled {
    opacity: 0.5;
    cursor: not-allowed;
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

<script lang="ts">
  // Workspace picker — a focused modal that lets the user switch which
  // daemon the app is connected to without leaving the workspace board.
  //
  // The daemon's cwd is baked in at spawn time (sessions live under
  // `<cwd>/.puffer/`), so "switch workspace" means: tear down the current
  // local subprocess and spawn a new one with a different cwd. Remote
  // workspaces reuse the existing SSH flow, just surfaced here instead of
  // only from inside the Connect-project modal.

  import Icon from "../design/Icon.svelte";
  import { connectSshDaemon, restartLocalDaemon } from "../api/desktop";
  import { canInvokeTauri, currentDaemonClient, switchDaemonClient } from "../api/daemonClient";
  import { focusTrap } from "../focusTrap";

  type Props = {
    onClose: () => void;
    /** Fired after the daemon swap succeeds so the parent can refresh
     *  groups, reset selected session, etc. */
    onSwitched?: (handshake: {
      url: string;
      workspaceRoot: string;
    }) => void | Promise<void>;
  };

  let { onClose, onSwitched }: Props = $props();

  type Mode = "current" | "local" | "remote";
  let mode = $state<Mode>("current");

  let localCwd = $state("");
  let sshTarget = $state("");
  let remoteWorkspace = $state("");
  let remoteBinary = $state("");

  let busy = $state(false);
  let error = $state<string | null>(null);
  let status = $state<string | null>(null);

  // Current daemon handshake — populated once the app is connected. Shown
  // in the "Current" panel so users know where they are before switching.
  const current = currentDaemonClient()?.handshake ?? null;

  function selectMode(nextMode: Mode) {
    if (busy || mode === nextMode) return;
    error = null;
    status = null;
    mode = nextMode;
  }

  async function pickDirectory(): Promise<string | null> {
    if (!canInvokeTauri()) {
      return null;
    }
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const picked = await open({ directory: true, multiple: false });
      if (typeof picked === "string" && picked.length > 0) return picked;
      return null;
    } catch (e) {
      console.warn("dialog open failed", e);
      return null;
    }
  }

  $effect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !busy) onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  async function submitLocal() {
    if (busy || !localCwd.trim()) return;
    busy = true;
    error = null;
    status = `Restarting daemon in ${localCwd}…`;
    try {
      const hs = await restartLocalDaemon(localCwd.trim());
      status = `Connected to ${hs.workspaceRoot}.`;
      await onSwitched?.({ url: hs.url, workspaceRoot: hs.workspaceRoot });
      onClose();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }

  async function submitRemote() {
    if (busy || !sshTarget.trim()) return;
    const previousHandshake = currentDaemonClient()?.handshake ?? null;
    busy = true;
    error = null;
    status = `Connecting to ${sshTarget}…`;
    try {
      const hs = await connectSshDaemon(sshTarget.trim(), {
        remoteWorkspace: remoteWorkspace.trim() || undefined,
        remoteBinary: remoteBinary.trim() || undefined
      });
      status = `Connected to ${hs.workspaceRoot} on ${sshTarget}.`;
      await onSwitched?.({ url: hs.url, workspaceRoot: hs.workspaceRoot });
      onClose();
    } catch (e) {
      if (previousHandshake) {
        await switchDaemonClient(previousHandshake).catch(() => undefined);
      }
      error = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }
</script>

<div
  class="pf-modal-scrim"
  onclick={() => { if (!busy) onClose(); }}
  role="presentation"
  onkeydown={() => {}}
>
  <div
    class="pf-modal pf-workspace-modal"
    onclick={(e) => e.stopPropagation()}
    role="dialog"
    aria-label="Switch workspace"
    aria-modal="true"
    tabindex="-1"
    use:focusTrap
    onkeydown={() => {}}
  >
    <div class="pf-modal-head">
      <div class="pf-modal-title-group">
        <div class="pf-modal-eyebrow">Workspace</div>
        <div class="pf-modal-title">Switch workspace</div>
      </div>
      <button type="button" class="pf-modal-close" onclick={onClose} aria-label="Close" disabled={busy}>
        <Icon name="x" size={14} />
      </button>
    </div>

    <div class="pf-modal-seg" role="tablist">
      <button
        type="button"
        role="tab"
        aria-selected={mode === "current"}
        class="pf-modal-seg-btn"
        data-active={mode === "current"}
        onclick={() => selectMode("current")}
        disabled={busy}
      >
        <Icon name="check" size={13} />
        <div class="pf-modal-seg-body">
          <span class="pf-modal-seg-title">Current</span>
          <span class="pf-modal-seg-sub">Where you are now</span>
        </div>
      </button>
      <button
        type="button"
        role="tab"
        aria-selected={mode === "local"}
        class="pf-modal-seg-btn"
        data-active={mode === "local"}
        onclick={() => selectMode("local")}
        disabled={busy}
      >
        <Icon name="folder" size={13} />
        <div class="pf-modal-seg-body">
          <span class="pf-modal-seg-title">Local</span>
          <span class="pf-modal-seg-sub">Switch local workspace</span>
        </div>
      </button>
      <button
        type="button"
        role="tab"
        aria-selected={mode === "remote"}
        class="pf-modal-seg-btn"
        data-active={mode === "remote"}
        onclick={() => selectMode("remote")}
        disabled={busy}
      >
        <Icon name="globe" size={13} />
        <div class="pf-modal-seg-body">
          <span class="pf-modal-seg-title">Remote</span>
          <span class="pf-modal-seg-sub">Connect via SSH</span>
        </div>
      </button>
    </div>

    <div class="pf-modal-body">
      {#if mode === "current"}
        {#if current}
          <div class="pf-current">
            <div class="pf-current-row">
              <span class="pf-current-label">Workspace root</span>
              <code class="pf-current-value">{current.workspaceRoot || "(unset)"}</code>
            </div>
            <div class="pf-current-row">
              <span class="pf-current-label">Daemon URL</span>
              <code class="pf-current-value">{current.url}</code>
            </div>
            <div class="pf-current-hint">
              Switch to a local or remote workspace using the tabs above.
            </div>
          </div>
        {:else}
          <div class="pf-modal-status">Not connected to a daemon yet.</div>
        {/if}
      {:else if mode === "local"}
        <div class="pf-field">
          <label class="pf-field-label" for="pf-wp-cwd">Workspace directory</label>
          <div class="pf-field-row">
            <div class="pf-field-input pf-field-input-path">
              <Icon name="folder" size={12} />
              <input
                id="pf-wp-cwd"
                bind:value={localCwd}
                placeholder="/Users/me/src"
                spellcheck="false"
                disabled={busy}
              />
            </div>
            <button
              type="button"
              class="sc-btn"
              data-variant="outline"
              data-size="sm"
              disabled={busy}
              onclick={async () => {
                const picked = await pickDirectory();
                if (picked) localCwd = picked;
              }}
            >Browse…</button>
          </div>
          <div class="pf-field-hint">
            Sessions will live under <span class="pf-mono">{localCwd || "<cwd>"}/.puffer/</span>. The current daemon is torn down and replaced.
          </div>
        </div>
      {:else}
        <div class="pf-field">
          <label class="pf-field-label" for="pf-wp-ssh">SSH target</label>
          <div class="pf-field-input pf-field-input-path">
            <Icon name="globe" size={12} />
            <input
              id="pf-wp-ssh"
              bind:value={sshTarget}
              placeholder="you@build-01.internal"
              spellcheck="false"
              disabled={busy}
            />
          </div>
        </div>
        <div class="pf-field">
          <label class="pf-field-label" for="pf-wp-remote-ws">
            Remote workspace <span class="pf-field-label-opt">optional</span>
          </label>
          <div class="pf-field-input pf-field-input-path">
            <Icon name="folder" size={12} />
            <input
              id="pf-wp-remote-ws"
              bind:value={remoteWorkspace}
              placeholder="~"
              spellcheck="false"
              disabled={busy}
            />
          </div>
        </div>
        <div class="pf-field">
          <label class="pf-field-label" for="pf-wp-remote-bin">
            Remote binary <span class="pf-field-label-opt">advanced</span>
          </label>
          <div class="pf-field-input">
            <Icon name="terminal" size={12} />
            <input
              id="pf-wp-remote-bin"
              bind:value={remoteBinary}
              placeholder="puffer"
              spellcheck="false"
              disabled={busy}
            />
          </div>
          <div class="pf-field-hint">
            Override if <span class="pf-mono">puffer</span> isn't on the remote's $PATH.
          </div>
        </div>
      {/if}

      {#if error}
        <div class="pf-modal-status" data-error="true">{error}</div>
      {:else if status}
        <div class="pf-modal-status">{status}</div>
      {/if}
    </div>

    <div class="pf-modal-foot">
      <div class="pf-modal-foot-hint">
        {#if mode === "local"}
          Replaces the local daemon subprocess.
        {:else if mode === "remote"}
          Spawns <span class="pf-mono">puffer daemon</span> over SSH.
        {:else}
          Switch to Local or Remote to change workspace.
        {/if}
      </div>
      <div class="pf-modal-foot-btns">
        <button type="button" class="sc-btn" data-variant="ghost" onclick={onClose} disabled={busy}>
          Close
        </button>
        {#if mode === "local"}
          <button
            type="button"
            class="sc-btn"
            data-variant="default"
            onclick={submitLocal}
            disabled={busy || !localCwd.trim()}
          >
            {#if busy}
              <Icon name="refresh" size={13} />{status ?? "Working…"}
            {:else}
              Switch local workspace
            {/if}
          </button>
        {:else if mode === "remote"}
          <button
            type="button"
            class="sc-btn"
            data-variant="default"
            onclick={submitRemote}
            disabled={busy || !sshTarget.trim()}
          >
            {#if busy}
              <Icon name="refresh" size={13} />{status ?? "Working…"}
            {:else}
              Connect remote
            {/if}
          </button>
        {/if}
      </div>
    </div>
  </div>
</div>

<style>
  .pf-current {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .pf-current-row {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .pf-current-label {
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--muted-foreground);
  }
  .pf-current-value {
    font-family: var(--font-mono);
    font-size: 12px;
    padding: 6px 8px;
    background: var(--muted);
    border-radius: 6px;
    color: var(--foreground);
    word-break: break-all;
  }
  .pf-current-hint {
    font-size: 12px;
    color: var(--muted-foreground);
    padding-top: 4px;
  }
  .pf-modal-status {
    font-size: 12px;
    padding: 8px 10px;
    border-radius: 8px;
    background: color-mix(in oklab, var(--muted) 60%, var(--background));
    color: var(--muted-foreground);
    font-family: var(--font-mono);
  }
  .pf-modal-status[data-error="true"] {
    background: color-mix(in oklab, oklch(0.7 0.18 25) 12%, var(--background));
    color: oklch(0.5 0.2 25);
    border: 1px solid color-mix(in oklab, oklch(0.7 0.18 25) 30%, var(--border));
  }
</style>

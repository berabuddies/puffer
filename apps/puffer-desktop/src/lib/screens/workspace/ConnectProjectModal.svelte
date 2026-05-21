<script lang="ts">
  import Icon from "../../design/Icon.svelte";
  import { focusTrap } from "../../focusTrap";
  import {
    cloneRepo,
    connectSshDaemon,
    createSession,
    loadSettingsSnapshot,
    listDir,
    type DirEntry
  } from "../../api/desktop";
  import { canInvokeTauri, currentDaemonClient, switchDaemonClient } from "../../api/daemonClient";
  import {
    canonicalDaemonProviderId,
    providerCanRunAgent,
    providerIsAvailableForAgent,
    providerIdsEquivalent
  } from "../../providerIds";
  import { providerCatalogForSetup } from "../../providerFallbacks";
  import type { ProviderSummary, SettingsSnapshot } from "../../types";

  type CreatedSessionResult = Awaited<ReturnType<typeof createSession>>;

  /** Native directory chooser — only works in Tauri. Silently becomes a
   *  no-op in the web preview so the modal can still be used for testing. */
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

  type Props = {
    onClose: () => void;
    /** Fired when the modal finishes (clone + create_session) with the payload
     *  of the new session so the caller can open AgentDetail. */
    onConnected?: (created: CreatedSessionResult) => void | Promise<void>;
    /** Shown as the placeholder for the local path field. */
    defaultLocalPath?: string;
    snapshot?: SettingsSnapshot | null;
  };

  let { onClose, onConnected, defaultLocalPath = "~/code", snapshot = null }: Props = $props();

  type Mode = "local" | "remote";
  let mode = $state<Mode>("local");

  // Local mode
  let localDest = $state("");
  let localGitUrl = $state("");

  // Remote mode
  let sshTarget = $state("");
  let remoteWorkspace = $state("~");
  let remoteBinary = $state("");
  let remoteDest = $state("");
  let remoteGitUrl = $state("");

  let busy = $state(false);
  let status = $state<string | null>(null);
  let error = $state<string | null>(null);
  let sshErrorHint = $state<string | null>(null);
  let pickerOpen = $state(false);
  let pickerPath = $state("");
  let pickerEntries = $state<DirEntry[]>([]);
  let pickerLoading = $state(false);
  let pickerError = $state<string | null>(null);
  let pickerLoadGeneration = 0;
  let selectedProvider = $state("");

  function providerOptionsFor(source: SettingsSnapshot | null): ProviderSummary[] {
    const authenticatedProviderIds = (source?.auth ?? []).map((entry) => entry.providerId);
    return providerCatalogForSetup(source).filter((provider) =>
      source === null
        ? providerCanRunAgent(provider)
        : providerIsAvailableForAgent(provider, authenticatedProviderIds)
    );
  }

  let providerOptions = $derived(providerOptionsFor(snapshot));

  function defaultProviderIdFor(
    source: SettingsSnapshot | null,
    options: ProviderSummary[] = providerOptionsFor(source)
  ): string {
    const configured = source?.config.defaultProvider;
    const configuredProvider = options.find((provider) =>
      providerIdsEquivalent(provider.id, configured)
    );
    if (configuredProvider) {
      return configuredProvider.id;
    }
    return options[0]?.id ?? "openai";
  }

  function defaultProviderId(): string {
    return defaultProviderIdFor(snapshot, providerOptions);
  }

  function providerForSnapshot(
    source: SettingsSnapshot | null,
    requestedProviderId: string,
    context: "local" | "remote" = "local"
  ): string {
    const options = providerOptionsFor(source);
    if (options.length === 0) {
      throw new Error(
        context === "remote"
          ? "Connect an agent provider on the remote host before starting a remote project."
          : "Connect an agent provider in Settings before starting a project."
      );
    }
    const requested = requestedProviderId.trim();
    const requestedProvider = options.find((provider) =>
      providerIdsEquivalent(provider.id, requested)
    );
    return canonicalDaemonProviderId(
      requestedProvider?.id ?? defaultProviderIdFor(source, options)
    );
  }

  $effect(() => {
    if (!providerOptions.some((provider) => provider.id === selectedProvider)) {
      selectedProvider = defaultProviderId();
    }
  });

  let canSubmit = $derived(() => {
    if (busy) return false;
    if (providerOptions.length === 0) return false;
    if (mode === "local") return localDest.trim().length > 0;
    return sshTarget.trim().length > 0 && remoteDest.trim().length > 0;
  });

  function closePicker() {
    pickerLoadGeneration += 1;
    pickerOpen = false;
    pickerLoading = false;
    pickerError = null;
  }

  function selectMode(nextMode: Mode) {
    if (busy || mode === nextMode) return;
    if (pickerOpen) {
      closePicker();
    }
    status = null;
    error = null;
    sshErrorHint = null;
    mode = nextMode;
  }

  $effect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || busy) return;
      if (pickerOpen) {
        e.preventDefault();
        e.stopPropagation();
        if (!pickerLoading) {
          closePicker();
        }
        return;
      }
      onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  // PLUS-117 follow-up: the in-modal directory picker only applies to the
  // Local destination field — collapse it when the user flips to Remote so
  // a stale picker doesn't linger above the SSH-specific inputs.
  $effect(() => {
    if (mode !== "local" && pickerOpen) {
      pickerOpen = false;
    }
  });

  async function submit() {
    if (!canSubmit()) return;
    busy = true;
    error = null;
    sshErrorHint = null;
    try {
      if (mode === "local") {
        await doLocal();
      } else {
        await doRemote();
      }
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }

  /** Runs a clone through the streaming cloneRepo handle and updates the
   *  status line with each progress line as git emits it. Resolves with the
   *  final resolved destination path. */
  async function runStreamingClone(url: string, dest: string, label: string): Promise<string> {
    status = `Cloning ${url} into ${dest}…`;
    const handle = await cloneRepo(url, dest);
    const offProgress = handle.onProgress((line) => {
      // Many git progress lines carry carriage returns + percentages — keep
      // only the last non-empty token so the status bar reads cleanly.
      const trimmed = line.trim();
      if (trimmed) status = `${label}: ${trimmed}`;
    });
    try {
      const res = await handle.done;
      if (!res.ok) {
        throw new Error(`git clone failed: ${res.stderr || res.stdout || "unknown"}`);
      }
      return res.dest;
    } finally {
      offProgress();
    }
  }

  async function doLocal() {
    // 1. If a Git URL is provided, clone into the dest directory first.
    let targetCwd = localDest.trim();
    const url = localGitUrl.trim();
    if (url) {
      targetCwd = await runStreamingClone(url, targetCwd, "Cloning");
    }
    // 2. Create a new session rooted at that directory.
    status = `Creating agent in ${targetCwd}…`;
    const created = await createSession(
      targetCwd,
      providerForSnapshot(snapshot, selectedProvider || defaultProviderId())
    );
    status = `Ready — session ${created.sessionId.slice(0, 8)}`;
    await onConnected?.(created);
    onClose();
  }

  async function doRemote() {
    // 1. Spin up / attach to the remote daemon over SSH. From this point
    //    the app's shared DaemonClient points at the remote. Subsequent
    //    RPCs (clone + create_session) run on that host's filesystem.
    const previousHandshake = currentDaemonClient()?.handshake ?? null;
    let switchedToRemote = false;
    status = `Connecting to ${sshTarget}…`;
    try {
      await connectSshDaemon(sshTarget, {
        remoteWorkspace: remoteWorkspace.trim() || undefined,
        remoteBinary: remoteBinary.trim() || undefined
      });
      switchedToRemote = true;
    } catch (e) {
      sshErrorHint = deriveSshHint(String(e instanceof Error ? e.message : e), sshTarget);
      throw e;
    }

    let createdSession: CreatedSessionResult | null = null;
    try {
      // 2. Optional clone on the remote.
      let targetCwd = remoteDest.trim();
      const url = remoteGitUrl.trim();
      if (url) {
        targetCwd = await runStreamingClone(url, targetCwd, `Cloning on ${sshTarget}`);
      }

      // 3. Create a session on the remote. Only backend failures in this
      // phase should roll back to the prior daemon.
      status = `Creating agent on ${sshTarget}…`;
      const remoteSnapshot = await loadSettingsSnapshot();
      const created = await createSession(
        targetCwd,
        providerForSnapshot(remoteSnapshot, selectedProvider || defaultProviderId(), "remote")
      );
      createdSession = created;
    } catch (e) {
      if (switchedToRemote && previousHandshake) {
        try {
          await switchDaemonClient(previousHandshake);
        } catch (rollbackError) {
          console.warn("failed to restore previous daemon after remote connect failure", rollbackError);
        }
      }
      throw e;
    }
    if (!createdSession) {
      throw new Error("remote create_session did not return a session");
    }
    status = `Ready — session ${createdSession.sessionId.slice(0, 8)} on ${sshTarget}`;
    await onConnected?.(createdSession);
    onClose();
  }

  // SSH auth helper — when `start_ssh_daemon` surfaces a familiar failure
  // mode, render a specific remediation tip so users aren't stuck reading
  // raw sshd errors.
  function deriveSshHint(message: string, target: string): string | null {
    const m = message.toLowerCase();
    if (m.includes("permission denied") || m.includes("publickey")) {
      return "SSH key auth failed. Add your key to `~/.ssh/config` or run `ssh-add`.";
    }
    if (m.includes("host key verification")) {
      return `Host key verification failed. Run \`ssh ${target}\` once from a terminal to accept the fingerprint.`;
    }
    if (m.includes("command not found") || m.includes("not found")) {
      return "`puffer` binary not found on the remote. Install it, or set the Remote binary field if it lives elsewhere.";
    }
    if (m.includes("could not resolve") || m.includes("name or service not known")) {
      return "Couldn't resolve the SSH hostname. Check your `~/.ssh/config` and DNS.";
    }
    return null;
  }

  function initialPickerPath(): string {
    const candidate = (localDest || defaultLocalPath).trim();
    if (candidate.startsWith("/")) return candidate;
    if (candidate.startsWith("~")) {
      const parts = defaultLocalPath.split("/").filter(Boolean);
      if (defaultLocalPath.startsWith("/") && parts.length >= 2) {
        return `/${parts[0]}/${parts[1]}${candidate.slice(1)}`;
      }
    }
    return defaultLocalPath.startsWith("/") ? defaultLocalPath : "/";
  }

  function parentPath(path: string): string {
    const trimmed = path.replace(/\/+$/, "");
    if (!trimmed || trimmed === "/") return "/";
    const idx = trimmed.lastIndexOf("/");
    return idx <= 0 ? "/" : trimmed.slice(0, idx);
  }

  async function openBrowserPicker() {
    pickerOpen = true;
    await loadPickerPath(initialPickerPath());
  }

  async function loadPickerPath(path: string) {
    const nextPath = path.trim() || "/";
    const generation = ++pickerLoadGeneration;
    pickerLoading = true;
    pickerError = null;
    try {
      const entries = await listDir(nextPath);
      if (generation !== pickerLoadGeneration) return;
      pickerPath = nextPath;
      pickerEntries = entries.filter((entry) => entry.kind === "directory" || entry.kind === "symlink");
    } catch (e) {
      if (generation !== pickerLoadGeneration) return;
      pickerError = e instanceof Error ? e.message : String(e);
      pickerEntries = [];
    } finally {
      if (generation === pickerLoadGeneration) {
        pickerLoading = false;
      }
    }
  }

  async function browseLocalDirectory() {
    const picked = await pickDirectory();
    if (picked) {
      localDest = picked;
      return;
    }
    await openBrowserPicker();
  }
</script>

<div
  class="pf-modal-scrim"
  onclick={() => { if (!busy) onClose(); }}
  role="presentation"
  onkeydown={() => {}}
>
  <div
    class="pf-modal pf-connect-modal"
    onclick={(e) => e.stopPropagation()}
    role="dialog"
    aria-label="Create Project"
    aria-modal="true"
    tabindex="-1"
    use:focusTrap
    onkeydown={() => {}}
  >
    <div class="pf-modal-head">
      <div class="pf-modal-title-group">
        <div class="pf-modal-title">New Project</div>
      </div>
      <button type="button" class="pf-modal-close" onclick={onClose} aria-label="Close" disabled={busy}>
        <Icon name="x" size={14} />
      </button>
    </div>

    <div class="pf-modal-seg" role="tablist">
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
          <span class="pf-modal-seg-sub">This machine</span>
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
          <span class="pf-modal-seg-sub">SSH — run agent on another host</span>
        </div>
      </button>
    </div>

    <div class="pf-modal-body">
      <div class="pf-field">
        <div class="pf-field-label">Provider</div>
        <div class="pf-provider-seg" role="radiogroup" aria-label="Agent provider">
          {#each providerOptions as provider (provider.id)}
            <label
              class="pf-provider-seg-btn"
              data-active={selectedProvider === provider.id}
            >
              <input
                class="pf-provider-seg-input"
                type="radio"
                name="connect-project-provider"
                value={provider.id}
                checked={selectedProvider === provider.id}
                onchange={(event) => {
                  event.stopPropagation();
                  selectedProvider = provider.id;
                }}
                disabled={busy}
              />
              {provider.displayName}
            </label>
          {/each}
        </div>
        {#if providerOptions.length === 0}
          <div class="pf-field-hint">Connect a provider in Settings before starting a project.</div>
        {/if}
      </div>

      {#if mode === "local"}
        <div class="pf-field">
          <label class="pf-field-label" for="pf-local-dest">Directory</label>
          <div class="pf-field-row">
            <div class="pf-field-input pf-field-input-path">
              <Icon name="folder" size={12} />
              <input
                id="pf-local-dest"
                bind:value={localDest}
                placeholder={defaultLocalPath}
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
              onclick={browseLocalDirectory}
            >Browse…</button>
          </div>
          <div class="pf-field-hint">
            If this directory doesn't exist Puffer will create it. Must be empty if a git URL is set below.
          </div>
        </div>
        <div class="pf-field">
          <label class="pf-field-label" for="pf-local-giturl">
            Git URL <span class="pf-field-label-opt">optional</span>
          </label>
          <div class="pf-field-input">
            <Icon name="git" size={12} />
            <input
              id="pf-local-giturl"
              bind:value={localGitUrl}
              placeholder="git@github.com:acme/project.git"
              spellcheck="false"
              disabled={busy}
            />
          </div>
          <div class="pf-field-hint">
            Leave blank to create a fresh agent rooted at an existing directory.
          </div>
        </div>
      {:else}
        <div class="pf-field">
          <label class="pf-field-label" for="pf-ssh">SSH target</label>
          <div class="pf-field-input pf-field-input-path">
            <Icon name="globe" size={12} />
            <input
              id="pf-ssh"
              bind:value={sshTarget}
              placeholder="you@build-01.internal"
              spellcheck="false"
              disabled={busy}
            />
          </div>
          <div class="pf-field-hint">
            Puffer will connect over your existing SSH config and port-forward the workspace runtime locally.
          </div>
        </div>
        <div class="pf-field">
          <label class="pf-field-label" for="pf-remote-workspace">
            Remote workspace <span class="pf-field-label-opt">cd to</span>
          </label>
          <div class="pf-field-input pf-field-input-path">
            <Icon name="folder" size={12} />
            <input
              id="pf-remote-workspace"
              bind:value={remoteWorkspace}
              placeholder="~"
              spellcheck="false"
              disabled={busy}
            />
          </div>
        </div>
        <div class="pf-field">
          <label class="pf-field-label" for="pf-remote-binary">
            Remote binary <span class="pf-field-label-opt">advanced</span>
          </label>
          <div class="pf-field-input">
            <Icon name="terminal" size={12} />
            <input
              id="pf-remote-binary"
              bind:value={remoteBinary}
              placeholder="puffer"
              spellcheck="false"
              disabled={busy}
            />
          </div>
          <div class="pf-field-hint">
            Set this only when <span class="pf-mono">puffer</span> is installed outside the remote's $PATH.
          </div>
        </div>
        <div class="pf-field">
          <label class="pf-field-label" for="pf-remote-dest">Destination directory</label>
          <div class="pf-field-input pf-field-input-path">
            <Icon name="folder" size={12} />
            <input
              id="pf-remote-dest"
              bind:value={remoteDest}
              placeholder="~/src/project"
              spellcheck="false"
              disabled={busy}
            />
          </div>
        </div>
        <div class="pf-field">
          <label class="pf-field-label" for="pf-remote-giturl">
            Git URL <span class="pf-field-label-opt">optional</span>
          </label>
          <div class="pf-field-input">
            <Icon name="git" size={12} />
            <input
              id="pf-remote-giturl"
              bind:value={remoteGitUrl}
              placeholder="git@github.com:acme/project.git"
              spellcheck="false"
              disabled={busy}
            />
          </div>
        </div>
      {/if}

      {#if pickerOpen}
        <div class="pf-dir-picker" role="group" aria-label="Choose directory">
          <div class="pf-dir-picker-head">
            <div class="pf-dir-picker-title">Choose directory</div>
            <button
              type="button"
              class="pf-modal-close"
              onclick={closePicker}
              aria-label="Close directory picker"
              disabled={pickerLoading}
            >
              <Icon name="x" size={13} />
            </button>
          </div>
          <div class="pf-field-row">
            <div class="pf-field-input pf-field-input-path">
              <Icon name="folder" size={12} />
              <input
                bind:value={pickerPath}
                placeholder="/Users/me/src"
                spellcheck="false"
                disabled={pickerLoading}
                onkeydown={(e) => {
                  if (e.key === "Enter") void loadPickerPath(pickerPath);
                }}
              />
            </div>
            <button
              type="button"
              class="sc-btn"
              data-variant="outline"
              data-size="sm"
              disabled={pickerLoading}
              onclick={() => loadPickerPath(pickerPath)}
            >Go</button>
          </div>
          <div class="pf-dir-picker-toolbar">
            <button
              type="button"
              class="sc-btn"
              data-variant="ghost"
              data-size="sm"
              disabled={pickerLoading || pickerPath === "/"}
              onclick={() => loadPickerPath(parentPath(pickerPath))}
            >
              <Icon name="chevL" size={12} />Parent
            </button>
            <button
              type="button"
              class="sc-btn"
              data-variant="default"
              data-size="sm"
              disabled={pickerLoading || !pickerPath.trim() || !!pickerError}
              onclick={() => {
                localDest = pickerPath.trim();
                closePicker();
              }}
            >
              Use this directory
            </button>
          </div>
          {#if pickerError}
            <div class="pf-modal-status" data-error="true">{pickerError}</div>
          {:else}
            <div class="pf-dir-picker-list" aria-busy={pickerLoading}>
              {#if pickerLoading}
                <div class="pf-dir-picker-empty">Loading directories...</div>
              {:else if pickerEntries.length === 0}
                <div class="pf-dir-picker-empty">No child directories.</div>
              {:else}
                {#each pickerEntries as entry (entry.name)}
                  <button
                    type="button"
                    class="pf-dir-picker-row"
                    onclick={() => loadPickerPath(`${pickerPath.replace(/\/+$/, "")}/${entry.name}`)}
                  >
                    <Icon name="folder" size={12} />
                    <span>{entry.name}</span>
                  </button>
                {/each}
              {/if}
            </div>
          {/if}
        </div>
      {/if}
      {#if error && sshErrorHint}
        <div class="pf-modal-hint" role="note">
          {sshErrorHint}
        </div>
      {/if}
    </div>

    <!-- PLUS-117: status / progress row lives between body and footer with a
         reserved height so it cannot shift other content when it appears. -->
    <div
      class="pf-modal-status-row"
      data-active={!!(status || error)}
      data-error={!!error}
      aria-live="polite"
    >
      {#if status || error}
        <span class="pf-modal-status-text">{error ?? status}</span>
      {/if}
    </div>

    <div class="pf-modal-foot">
      <div class="pf-modal-foot-hint">
        {#if mode === "local"}
          Agent runs on this machine in <span class="pf-mono">{localDest || "(choose a directory)"}</span>
        {:else}
          Agent runs on <span class="pf-mono">{sshTarget || "(ssh target)"}</span>
        {/if}
      </div>
      <div class="pf-modal-foot-btns">
        <button type="button" class="sc-btn" data-variant="ghost" onclick={onClose} disabled={busy}>
          Cancel
        </button>
        <button
          type="button"
          class="sc-btn"
          data-variant="default"
          onclick={submit}
          disabled={!canSubmit()}
        >
          {#if busy}
            <Icon name="refresh" size={13} />Working…
          {:else if mode === "local" && localGitUrl.trim()}
            Clone &amp; create
          {:else if mode === "remote" && remoteGitUrl.trim()}
            Clone &amp; create remote
          {:else}
            Create
          {/if}
        </button>
      </div>
    </div>
  </div>
</div>

<style>
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
  /* PLUS-117 follow-up: a fixed-height reserved row between body and footer
     for the clone/progress status. The slot keeps its height even when
     empty so revealing the message does not shift surrounding layout. */
  .pf-modal-status-row {
    flex-shrink: 0;
    min-height: 32px;
    padding: 6px 20px 0;
    display: flex; align-items: center;
  }
  .pf-modal-status-text {
    flex: 1; min-width: 0;
    font-size: 12px;
    padding: 4px 10px;
    border-radius: 6px;
    background: color-mix(in oklab, var(--muted) 60%, var(--background));
    color: var(--muted-foreground);
    font-family: var(--font-mono);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .pf-modal-status-row[data-error="true"] .pf-modal-status-text {
    background: color-mix(in oklab, oklch(0.7 0.18 25) 12%, var(--background));
    color: oklch(0.5 0.2 25);
    border: 1px solid color-mix(in oklab, oklch(0.7 0.18 25) 30%, var(--border));
  }
  .pf-provider-seg {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 6px;
  }
  .pf-provider-seg-btn {
    min-width: 0;
    height: 32px;
    border: 1px solid var(--border);
    border-radius: 7px;
    background: var(--background);
    color: var(--foreground);
    display: grid;
    place-items: center;
    font: inherit;
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    position: relative;
    transition: background 120ms, border-color 120ms;
  }
  .pf-provider-seg-input {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    margin: 0;
    opacity: 0;
    cursor: inherit;
  }
  .pf-provider-seg-btn:has(.pf-provider-seg-input:disabled) {
    cursor: not-allowed;
    opacity: 0.6;
  }
  .pf-provider-seg-btn:hover:not(:has(.pf-provider-seg-input:disabled)) {
    border-color: transparent;
    background: var(--pf-selected-bg-hover);
  }
  .pf-provider-seg-btn:focus-within {
    outline: 2px solid color-mix(in oklab, var(--accent) 70%, transparent);
    outline-offset: 2px;
  }
  .pf-provider-seg-btn[data-active="true"] {
    border-color: transparent;
    background: var(--pf-selected-bg);
  }
  .pf-modal-hint {
    font-size: 12px;
    padding: 8px 10px;
    border-radius: 8px;
    background: color-mix(in oklab, oklch(0.72 0.12 240) 12%, var(--background));
    color: oklch(0.45 0.12 240);
    border: 1px solid color-mix(in oklab, oklch(0.72 0.12 240) 30%, var(--border));
    line-height: 1.45;
  }
  .pf-dir-picker {
    display: flex;
    flex-direction: column;
    gap: 10px;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: color-mix(in oklab, var(--muted) 70%, var(--background));
    padding: 12px;
  }
  .pf-dir-picker-head,
  .pf-dir-picker-toolbar {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .pf-dir-picker-title {
    flex: 1;
    font-size: 12px;
    font-weight: 600;
  }
  .pf-dir-picker-toolbar {
    justify-content: space-between;
  }
  .pf-dir-picker-list {
    display: flex;
    flex-direction: column;
    gap: 2px;
    /* Height fixed so Parent reloads don't collapse the row — see PLUS-117. */
    height: 200px;
    overflow: auto;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--background);
    padding: 4px;
  }
  .pf-dir-picker-row {
    display: flex;
    align-items: center;
    gap: 8px;
    border: 0;
    border-radius: 6px;
    background: transparent;
    color: var(--foreground);
    cursor: pointer;
    font: inherit;
    font-size: 12px;
    padding: 7px 8px;
    text-align: left;
  }
  .pf-dir-picker-row:hover {
    background: var(--muted);
  }
  .pf-dir-picker-row :global(svg) {
    color: var(--muted-foreground);
    flex-shrink: 0;
  }
  .pf-dir-picker-empty {
    padding: 14px 8px;
    text-align: center;
    color: var(--muted-foreground);
    font-size: 12px;
  }
</style>

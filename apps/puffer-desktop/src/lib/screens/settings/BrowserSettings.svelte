<script lang="ts">
  import Icon from "../../design/Icon.svelte";
  import { saveBrowserSettings, saveSecret } from "../../api/desktop";
  import type {
    BrowserCaptchaSolver,
    BrowserExtension,
    SaveBrowserSettingsInput,
    SecretSummary,
    SettingsSnapshot
  } from "../../types";

  type Props = {
    snapshot: SettingsSnapshot | null;
    daemonReachable: boolean;
    onSaved: (snapshot: SettingsSnapshot) => void;
    onRefresh: () => void;
  };

  type BrowserSubtab = "extensions" | "solvers";

  type SolverDraft = BrowserCaptchaSolver & {
    apiKeyDraft: string;
  };

  let props: Props = $props();

  let activeSubtab = $state<BrowserSubtab>("extensions");
  let extensionsEnabled = $state(true);
  let extensions = $state<BrowserExtension[]>([]);
  let extensionNameDraft = $state("");
  let extensionPathDraft = $state("");
  let captchaEnabled = $state(false);
  let selectedSolver = $state("nopecha");
  let solvers = $state<SolverDraft[]>([]);
  let lastSnapshotKey = $state("");
  let saving = $state(false);
  let error = $state<string | null>(null);
  let saved = $state<string | null>(null);

  let disabled = $derived(!props.daemonReachable || saving || !props.snapshot?.browser);
  let selectedSolverDraft = $derived(solvers.find((solver) => solver.id === selectedSolver) ?? solvers[0]);
  let selectedSolverName = $derived(selectedSolverDraft?.displayName ?? selectedSolver);
  let selectedSolverHasKey = $derived(Boolean(selectedSolverDraft?.hasApiKey || selectedSolverDraft?.apiKeyDraft.trim()));
  let selectedSolverReady = $derived(Boolean(captchaEnabled && selectedSolverHasKey));

  $effect(() => {
    const key = browserSnapshotKey(props.snapshot);
    if (key === lastSnapshotKey) return;
    lastSnapshotKey = key;
    const browser = props.snapshot?.browser;
    extensionsEnabled = browser?.extensionsEnabled ?? true;
    extensions = (browser?.extensions ?? []).map((extension) => ({ ...extension }));
    extensionNameDraft = "";
    extensionPathDraft = "";
    captchaEnabled = browser?.captcha.enabled ?? false;
    selectedSolver = browser?.captcha.selectedSolver ?? "nopecha";
    solvers = (browser?.captcha.solvers ?? []).map((solver) => ({
      ...solver,
      apiKeyDraft: ""
    }));
  });

  function browserSnapshotKey(snapshot: SettingsSnapshot | null): string {
    if (!snapshot?.browser) return "missing";
    return JSON.stringify(snapshot.browser);
  }

  function updateSolver(id: string, patch: Partial<SolverDraft>) {
    solvers = solvers.map((solver) => (solver.id === id ? { ...solver, ...patch } : solver));
  }

  function updateExtension(id: string, patch: Partial<BrowserExtension>) {
    extensions = extensions.map((extension) => (extension.id === id ? { ...extension, ...patch } : extension));
  }

  function addExtension() {
    const path = extensionPathDraft.trim();
    if (!path) {
      error = "Extension path cannot be empty.";
      return;
    }
    const displayName = extensionNameDraft.trim() || path.split(/[\\/]/).filter(Boolean).pop() || "Custom extension";
    const id = `custom-${Date.now().toString(36)}`;
    extensions = [
      ...extensions,
      {
        id,
        displayName,
        path,
        enabled: true,
        manifestPresent: false,
        source: "custom"
      }
    ];
    extensionNameDraft = "";
    extensionPathDraft = "";
    error = null;
  }

  function removeExtension(id: string) {
    extensions = extensions.filter((extension) => extension.id !== id);
  }

  function solverHasKey(solver: SolverDraft): boolean {
    return Boolean(solver.hasApiKey || solver.apiKeyDraft.trim());
  }

  function solverStatusLabel(solver: SolverDraft): string {
    if (selectedSolver !== solver.id) return "Solver extension";
    if (!captchaEnabled) return "Selected solver";
    return solverHasKey(solver) ? "Ready solver" : "Needs key";
  }

  function secretLabel(solver: BrowserCaptchaSolver): string {
    return `${solver.displayName} captcha solver API key`;
  }

  function secretDescription(solver: BrowserCaptchaSolver): string {
    return `${solver.displayName} browser extension credential`;
  }

  function matchingSecretId(items: SecretSummary[], solver: BrowserCaptchaSolver): string | null {
    const label = secretLabel(solver);
    const origin = solver.baseUrl.trim();
    const matches = items
      .filter((secret) => secret.label === label && (secret.origin ?? "") === origin)
      .sort((a, b) => b.updatedAtMs - a.updatedAtMs);
    return matches[0]?.id ?? null;
  }

  async function persistSecretIfNeeded(solver: SolverDraft): Promise<string | null> {
    const value = solver.apiKeyDraft.trim();
    if (!value) return solver.apiKeySecretId;
    const snapshot = await saveSecret({
      label: secretLabel(solver),
      value,
      description: secretDescription(solver),
      username: null,
      origin: solver.baseUrl.trim() || null
    });
    const secretId = matchingSecretId(snapshot.secrets.items, solver);
    if (!secretId) {
      throw new Error(`Saved ${solver.displayName} key, but the secret id was not returned.`);
    }
    return secretId;
  }

  async function saveSettings() {
    if (disabled) return;
    saving = true;
    error = null;
    saved = null;
    try {
      const solverInputs: SaveBrowserSettingsInput["captcha"]["solvers"] = [];
      for (const solver of solvers) {
        const apiKeySecretId = await persistSecretIfNeeded(solver);
        solverInputs.push({
          id: solver.id,
          enabled: solver.id === selectedSolver,
          baseUrl: solver.baseUrl.trim() || null,
          apiKeySecretId
        });
      }
      const input: SaveBrowserSettingsInput = {
        extensionsEnabled,
        extensions: extensions.map((extension) => ({
          id: extension.id,
          displayName: extension.displayName.trim() || extension.id,
          path: extension.path.trim(),
          enabled: extension.enabled
        })),
        captcha: {
          enabled: captchaEnabled,
          selectedSolver,
          solvers: solverInputs
        }
      };
      const snapshot = await saveBrowserSettings(input);
      saved =
        activeSubtab === "solvers"
          ? `Saved captcha solver settings for ${selectedSolverName}.`
          : "Saved browser extension settings.";
      props.onSaved(snapshot);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      saving = false;
    }
  }
</script>

<h2>Browser</h2>
<p class="lead">Manage browser extensions separately from captcha solver credentials.</p>

{#if error}
  <div class="pf-settings-note warn">{error}</div>
{/if}
{#if saved}
  <div class="pf-settings-note">{saved}</div>
{/if}
{#if !props.daemonReachable}
  <div class="pf-settings-note">Preview mode - connect the local daemon to edit browser settings.</div>
{/if}

<div class="pf-browser-toolbar">
  <div class="pf-browser-tabs" role="tablist" aria-label="Browser settings sections">
    <button
      type="button"
      role="tab"
      aria-selected={activeSubtab === "extensions"}
      data-active={activeSubtab === "extensions"}
      onclick={() => (activeSubtab = "extensions")}
    >
      Extensions
    </button>
    <button
      type="button"
      role="tab"
      aria-selected={activeSubtab === "solvers"}
      data-active={activeSubtab === "solvers"}
      onclick={() => (activeSubtab = "solvers")}
    >
      Captcha solvers
    </button>
  </div>
  <button
    type="button"
    class="sc-btn"
    data-variant="default"
    data-size="sm"
    disabled={disabled}
    onclick={saveSettings}
  >
    {saving ? "Saving..." : "Save"}
  </button>
</div>

{#if activeSubtab === "extensions"}
  <section class="pf-browser-panel" aria-label="Installed browser extensions">
    <div class="pf-settings-row pf-browser-row">
      <div class="meta">
        <div class="label">Extension loading</div>
        <div class="desc">Master switch for loading bundled and custom browser extensions.</div>
      </div>
      <input
        type="checkbox"
        class="sc-switch"
        checked={extensionsEnabled}
        disabled={disabled}
        onchange={(e) => (extensionsEnabled = (e.currentTarget as HTMLInputElement).checked)}
      />
    </div>

    <div class="pf-browser-header">
      <h3>Installed extensions</h3>
      <span>{solvers.length + extensions.length} total</span>
    </div>

    <div class="pf-browser-list">
      {#each solvers as solver (solver.id)}
        <div class="pf-mcp-card pf-extension-card" data-kind="builtin">
          <div class="ico"><Icon name="key" size={16} /></div>
          <div class="pf-browser-main">
            <div class="pf-browser-title-row">
              <div>
                <div class="title">{solver.displayName}</div>
                <div class="desc">{solver.extensionPath}</div>
              </div>
              <div class="pf-browser-badges">
                <span class:ready={solver.bundled} class="pf-status-pill">
                  {solver.bundled ? "Bundled" : "Missing"}
                </span>
                <span class:ready={selectedSolver === solver.id && captchaEnabled && solverHasKey(solver)} class="pf-status-pill">
                  {solverStatusLabel(solver)}
                </span>
              </div>
            </div>
            <div class="pf-browser-meta">
              <span>Built-in</span>
              <span>v{solver.version}</span>
              <span>{solver.license}</span>
              {#if solver.sha256}
                <span class="pf-browser-sha" title={solver.sha256}>{solver.sha256.slice(0, 12)}</span>
              {/if}
              <a href={solver.releaseUrl} target="_blank" rel="noreferrer">
                Release <Icon name="external" size={11} />
              </a>
            </div>
          </div>
        </div>
      {/each}

      {#each extensions as extension (extension.id)}
        <div class="pf-mcp-card pf-extension-card" data-kind="custom">
          <div class="ico"><Icon name="plug" size={16} /></div>
          <div class="pf-browser-main">
            <div class="pf-browser-title-row">
              <div>
                <div class="title">{extension.displayName}</div>
                <div class="desc">{extension.path}</div>
              </div>
              <div class="pf-browser-badges">
                <span class:ready={extension.manifestPresent} class="pf-status-pill">
                  {extension.manifestPresent ? "Manifest found" : "Manifest unchecked"}
                </span>
                <span class:ready={extension.enabled} class="pf-status-pill">
                  {extension.enabled ? "Enabled" : "Disabled"}
                </span>
              </div>
            </div>
            <div class="pf-browser-fields">
              <label>
                Name
                <input
                  class="sc-input"
                  value={extension.displayName}
                  disabled={disabled}
                  oninput={(e) => updateExtension(extension.id, { displayName: (e.currentTarget as HTMLInputElement).value })}
                />
              </label>
              <label>
                Folder
                <input
                  class="sc-input"
                  value={extension.path}
                  disabled={disabled}
                  oninput={(e) => updateExtension(extension.id, { path: (e.currentTarget as HTMLInputElement).value })}
                />
              </label>
            </div>
          </div>
          <input
            type="checkbox"
            class="sc-switch"
            checked={extension.enabled}
            disabled={disabled}
            title="Enable this extension"
            onchange={(e) => updateExtension(extension.id, { enabled: (e.currentTarget as HTMLInputElement).checked })}
          />
          <button
            type="button"
            class="sc-icon-btn"
            disabled={disabled}
            onclick={() => removeExtension(extension.id)}
            title="Remove extension"
            aria-label={`Remove ${extension.displayName}`}
          >
            <Icon name="trash" size={14} />
          </button>
        </div>
      {/each}
    </div>

    <div class="pf-browser-header">
      <h3>Add extension</h3>
    </div>
    <div class="pf-extension-add">
      <label>
        Name
        <input
          class="sc-input"
          value={extensionNameDraft}
          disabled={disabled}
          placeholder="Extension name"
          oninput={(e) => (extensionNameDraft = (e.currentTarget as HTMLInputElement).value)}
        />
      </label>
      <label>
        Folder
        <input
          class="sc-input"
          value={extensionPathDraft}
          disabled={disabled}
          placeholder="/absolute/path/to/extension"
          oninput={(e) => (extensionPathDraft = (e.currentTarget as HTMLInputElement).value)}
        />
      </label>
      <button
        type="button"
        class="sc-btn"
        data-variant="default"
        data-size="sm"
        disabled={disabled}
        onclick={addExtension}
      >
        <Icon name="plus" size={14} /> Add
      </button>
    </div>
  </section>
{:else}
  <section class="pf-browser-panel" aria-label="Captcha solver settings">
    <div class="pf-settings-row pf-browser-row">
      <div class="meta">
        <div class="label">Captcha solving</div>
        <div class="desc">Enables the selected built-in solver extension for browser sessions.</div>
      </div>
      <input
        type="checkbox"
        class="sc-switch"
        checked={captchaEnabled}
        disabled={disabled}
        onchange={(e) => (captchaEnabled = (e.currentTarget as HTMLInputElement).checked)}
      />
    </div>

    <div class="pf-browser-header">
      <h3>Solver</h3>
    </div>
    <div class="pf-solver-picker">
      {#each solvers as solver (solver.id)}
        <button
          type="button"
          class="pf-solver-option"
          data-selected={selectedSolver === solver.id}
          disabled={disabled}
          onclick={() => (selectedSolver = solver.id)}
        >
          <span>
            <strong>{solver.displayName}</strong>
            <small>{solver.description}</small>
          </span>
          <span class="pf-browser-badges">
            <span class:ready={solver.bundled} class="pf-status-pill">
              {solver.bundled ? "Bundled" : "Missing"}
            </span>
            <span class:ready={solverHasKey(solver)} class="pf-status-pill">
              {solverHasKey(solver) ? "Key stored" : "No key"}
            </span>
          </span>
        </button>
      {/each}
    </div>

    {#if selectedSolverDraft}
      <div class="pf-mcp-card pf-solver-detail">
        <div class="pf-browser-title-row">
          <div>
            <div class="title">{selectedSolverDraft.displayName}</div>
            <div class="desc">{selectedSolverDraft.extensionPath}</div>
          </div>
          <span class:ready={selectedSolverReady} class="pf-status-pill">
            {captchaEnabled ? (selectedSolverHasKey ? "Ready" : "Needs key") : "Inactive"}
          </span>
        </div>

        <div class="pf-browser-fields">
          <label>
            Base URL
            <input
              class="sc-input"
              value={selectedSolverDraft.baseUrl}
              disabled={disabled}
              placeholder="https://api.example.com"
              oninput={(e) => updateSolver(selectedSolverDraft.id, { baseUrl: (e.currentTarget as HTMLInputElement).value })}
            />
          </label>
          <label>
            API key
            <input
              class="sc-input"
              type="password"
              value={selectedSolverDraft.apiKeyDraft}
              disabled={disabled}
              placeholder={selectedSolverDraft.hasApiKey ? "Stored key present" : "Paste API key"}
              autocomplete="off"
              oninput={(e) =>
                updateSolver(selectedSolverDraft.id, { apiKeyDraft: (e.currentTarget as HTMLInputElement).value })}
            />
          </label>
        </div>

        <div class="pf-browser-meta">
          <span>v{selectedSolverDraft.version}</span>
          <span>{selectedSolverDraft.license}</span>
          <a href={selectedSolverDraft.releaseUrl} target="_blank" rel="noreferrer">
            Release <Icon name="external" size={11} />
          </a>
          <a href={selectedSolverDraft.downloadUrl} target="_blank" rel="noreferrer">
            Download <Icon name="external" size={11} />
          </a>
        </div>
      </div>
    {/if}
  </section>
{/if}

<style>
  .pf-browser-toolbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    margin: 14px 0 16px;
  }

  .pf-browser-tabs {
    display: inline-flex;
    gap: 4px;
    padding: 3px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--card);
  }

  .pf-browser-tabs button {
    height: 30px;
    padding: 0 12px;
    border: 0;
    border-radius: 6px;
    background: transparent;
    color: var(--muted-foreground);
    cursor: pointer;
    font: inherit;
  }

  .pf-browser-tabs button[data-active="true"] {
    background: var(--background);
    color: var(--foreground);
    box-shadow: inset 0 0 0 1px var(--border);
  }

  .pf-browser-panel {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .pf-browser-row {
    align-items: center;
  }

  .pf-browser-row > .sc-switch {
    justify-self: end;
  }

  .pf-browser-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    margin: 4px 0 0;
  }

  .pf-browser-header h3 {
    margin: 0;
  }

  .pf-browser-header span {
    color: var(--muted-foreground);
    font-size: 11.5px;
  }

  .pf-browser-list {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .pf-extension-card {
    grid-template-columns: 32px minmax(0, 1fr) auto auto;
    align-items: start;
  }

  .pf-extension-card[data-kind="builtin"] {
    grid-template-columns: 32px minmax(0, 1fr);
  }

  .pf-browser-main {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .pf-browser-title-row {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 12px;
  }

  .pf-browser-title-row > div:first-child {
    min-width: 0;
  }

  .pf-browser-title-row .desc {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .pf-browser-badges,
  .pf-browser-meta {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
  }

  .pf-browser-badges {
    flex-shrink: 0;
    justify-content: flex-end;
  }

  .pf-browser-fields {
    display: grid;
    grid-template-columns: repeat(2, minmax(160px, 1fr));
    gap: 8px;
  }

  .pf-browser-fields label,
  .pf-extension-add label {
    display: flex;
    flex-direction: column;
    gap: 4px;
    color: var(--muted-foreground);
    font-size: 11.5px;
  }

  .pf-browser-meta {
    color: var(--muted-foreground);
    font-size: 11.5px;
  }

  .pf-browser-meta a {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    color: var(--foreground);
    text-decoration: none;
  }

  .pf-browser-meta a:hover {
    text-decoration: underline;
  }

  .pf-browser-sha {
    font-family: var(--font-mono);
  }

  .pf-extension-add {
    display: grid;
    grid-template-columns: minmax(160px, 220px) minmax(260px, 1fr) auto;
    gap: 8px;
    align-items: end;
  }

  .sc-icon-btn {
    width: 28px;
    height: 28px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--background);
    color: var(--foreground);
    cursor: pointer;
  }

  .sc-icon-btn:disabled {
    cursor: not-allowed;
    opacity: 0.55;
  }

  .pf-solver-picker {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 8px;
  }

  .pf-solver-option {
    min-height: 86px;
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    align-items: start;
    gap: 12px;
    padding: 12px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--card);
    color: var(--foreground);
    text-align: left;
    cursor: pointer;
  }

  .pf-solver-option > span:first-child {
    display: contents;
  }

  .pf-solver-option .pf-browser-badges {
    grid-column: 2;
    grid-row: 1;
  }

  .pf-solver-option[data-selected="true"] {
    border-color: color-mix(in oklab, var(--puffer-accent) 45%, var(--border));
    background: color-mix(in oklab, var(--puffer-accent) 5%, var(--card));
  }

  .pf-solver-option strong,
  .pf-solver-option small {
    display: block;
  }

  .pf-solver-option strong {
    grid-column: 1;
    grid-row: 1;
    min-width: 0;
    font-size: 13px;
  }

  .pf-solver-option small {
    grid-column: 1 / -1;
    margin-top: 4px;
    color: var(--muted-foreground);
    font-size: 11.5px;
    line-height: 1.35;
  }

  .pf-solver-detail {
    grid-template-columns: minmax(0, 1fr);
    gap: 12px;
  }

  @media (max-width: 860px) {
    .pf-browser-toolbar,
    .pf-browser-title-row {
      align-items: stretch;
      flex-direction: column;
    }

    .pf-browser-tabs {
      width: 100%;
    }

    .pf-browser-tabs button {
      flex: 1;
    }

    .pf-extension-card,
    .pf-extension-card[data-kind="builtin"],
    .pf-extension-add,
    .pf-browser-fields,
    .pf-solver-picker {
      grid-template-columns: 1fr;
    }
  }
</style>

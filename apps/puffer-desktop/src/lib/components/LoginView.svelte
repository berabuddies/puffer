<script lang="ts">
  import Icon from "../design/Icon.svelte";
  import { providerVisual } from "../providerVisuals";
  import {
    providerIdsEquivalent,
    providerRunsWithoutAuth
  } from "../providerIds";
  import {
    providerCatalogForSetup,
    usesFallbackProviderCatalog
  } from "../providerFallbacks";
  import type { ExternalCredential, ProviderSummary, SettingsSnapshot } from "../types";

  const HIDDEN_PROVIDER_IDS = new Set([
    "puffer",
    "cerebras",
    "groq",
    "llama-cpp",
    "lmstudio",
    "vllm",
    "ollama"
  ]);

  const PROVIDER_DISPLAY_ORDER = [
    "anthropic",
    "github-copilot",
    "openai",
    "google",
    "openrouter",
    "vercel-ai-gateway",
    "custom"
  ];

  const EXTRA_SETUP_PROVIDERS: ProviderSummary[] = [
    {
      id: "github-copilot",
      displayName: "GitHub Copilot",
      baseUrl: "https://api.githubcopilot.com",
      defaultApi: "openai-completions",
      modelCount: 0,
      authModes: ["oauth"],
      sourceKind: "ui-setup",
      sourcePath: null
    },
    {
      id: "google",
      displayName: "Google",
      baseUrl: "https://generativelanguage.googleapis.com",
      defaultApi: "openai-completions",
      modelCount: 0,
      authModes: ["api_key"],
      sourceKind: "ui-setup",
      sourcePath: null
    },
    {
      id: "openrouter",
      displayName: "OpenRouter",
      baseUrl: "https://openrouter.ai/api/v1",
      defaultApi: "openai-completions",
      modelCount: 1,
      authModes: ["api_key"],
      sourceKind: "ui-setup",
      sourcePath: null
    },
    {
      id: "vercel-ai-gateway",
      displayName: "Vercel AI Gateway",
      baseUrl: "https://ai-gateway.vercel.sh/v1",
      defaultApi: "openai-completions",
      modelCount: 0,
      authModes: ["api_key"],
      sourceKind: "ui-setup",
      sourcePath: null
    },
    {
      id: "custom",
      displayName: "Custom provider",
      baseUrl: "",
      defaultApi: "openai-completions",
      modelCount: 0,
      authModes: ["api_key"],
      sourceKind: "ui-setup",
      sourcePath: null
    }
  ];

  export let snapshot: SettingsSnapshot | null = null;
  export let loading = false;
  export let remoteEnabled = false;
  export let busyProviderId: string | null = null;
  export let errorMessage: string | null = null;
  export let externals: ExternalCredential[] = [];
  export let busyImportKey: string | null = null;
  export let onLoginOauth: (providerId: string) => void = () => {};
  export let onLoginApiKey: (providerId: string, apiKey: string) => void = () => {};
  export let onImportExternal: (providerId: string, source: "claude" | "codex") => void = () => {};
  export let onRefresh: () => void = () => {};

  let apiKeys: Record<string, string> = {};
  let query = "";
  let pendingApiKeyProvider: string | null = null;
  let pendingApiKeyBusyObserved = false;
  let activeProviderId: string | null = null;
  type ProviderAuth = SettingsSnapshot["auth"][number];

  function updateApiKey(providerId: string, value: string) {
    apiKeys = { ...apiKeys, [providerId]: value };
  }

  function apiKeyValue(providerId: string): string {
    return apiKeys[providerId] ?? "";
  }

  function submitApiKey(providerId: string) {
    if (credentialBusy) return;
    const apiKey = apiKeyValue(providerId).trim();
    if (!apiKey) return;
    pendingApiKeyProvider = providerId;
    pendingApiKeyBusyObserved = false;
    onLoginApiKey(providerId, apiKey);
  }

  function submitOauth(providerId: string) {
    if (credentialBusy) return;
    onLoginOauth(providerId);
  }

  function openProviderModal(providerId: string) {
    activeProviderId = providerId;
  }

  function closeProviderModal() {
    activeProviderId = null;
  }

  function supports(provider: ProviderSummary, mode: string): boolean {
    return provider.authModes.includes(mode);
  }

  function authForProvider(providerId: string): ProviderAuth | null {
    return (
      snapshot?.auth.find((auth) => providerIdsEquivalent(auth.providerId, providerId)) ?? null
    );
  }

  function connectedHint(auth: ProviderAuth): string {
    const details = [auth.kind];
    if (auth.email) details.push(auth.email);
    if (auth.organizationName) details.push(auth.organizationName);
    return `connected via ${details.join(" · ")}`;
  }

  function setupProviderId(providerId: string): string {
    const normalized = providerId.trim().toLowerCase();
    if (normalized === "claude") return "anthropic";
    if (normalized === "codex") return "openai";
    return providerId;
  }

  function setupProviderDisplayName(providerId: string, fallback: string): string {
    const normalized = providerId.trim().toLowerCase();
    if (normalized === "claude" || normalized === "anthropic") return "Anthropic";
    if (normalized === "codex" || normalized === "openai") return "OpenAI";
    return fallback;
  }

  function normalizeSetupProvider(provider: ProviderSummary): ProviderSummary {
    const id = setupProviderId(provider.id);
    return {
      ...provider,
      id,
      displayName: setupProviderDisplayName(id, provider.displayName)
    };
  }

  function providerRank(provider: ProviderSummary): number {
    const index = PROVIDER_DISPLAY_ORDER.indexOf(provider.id);
    return index === -1 ? PROVIDER_DISPLAY_ORDER.length : index;
  }

  function providerSettingsCatalog(snapshot: SettingsSnapshot | null): ProviderSummary[] {
    const byId = new Map<string, ProviderSummary>();
    for (const provider of providerCatalogForSetup(snapshot)) {
      const normalized = normalizeSetupProvider(provider);
      if (HIDDEN_PROVIDER_IDS.has(normalized.id)) continue;
      if (!byId.has(normalized.id)) byId.set(normalized.id, normalized);
    }
    for (const provider of EXTRA_SETUP_PROVIDERS) {
      if (!byId.has(provider.id)) byId.set(provider.id, provider);
    }
    return [...byId.values()].sort((left, right) => {
      const rankDelta = providerRank(left) - providerRank(right);
      if (rankDelta !== 0) return rankDelta;
      return left.displayName.localeCompare(right.displayName);
    });
  }

  $: filteredProviders = (() => {
    const all = providerSettingsCatalog(snapshot);
    const needle = query.trim().toLowerCase();
    if (!needle) return all;
    return all.filter((provider) => {
      return (
        provider.id.toLowerCase().includes(needle) ||
        provider.displayName.toLowerCase().includes(needle) ||
        provider.defaultApi.toLowerCase().includes(needle)
      );
    });
  })();

  $: connectedAuth = snapshot?.auth ?? [];
  $: connectedProviders = providerSettingsCatalog(snapshot).filter((provider) =>
    connectedAuth.some((auth) => providerIdsEquivalent(auth.providerId, provider.id))
  );
  $: activeProvider = activeProviderId
    ? providerSettingsCatalog(snapshot).find((provider) => providerIdsEquivalent(provider.id, activeProviderId)) ?? null
    : null;
  $: usingFallbackProviders = usesFallbackProviderCatalog(snapshot);

  $: importsByProvider = (() => {
    const map: Record<string, ExternalCredential[]> = {};
    for (const candidate of externals) {
      (map[candidate.providerId] ??= []).push(candidate);
    }
    return map;
  })();

  function importKey(providerId: string, source: "claude" | "codex"): string {
    return `${providerId}::${source}`;
  }

  function submitImport(providerId: string, source: "claude" | "codex") {
    if (credentialBusy) return;
    onImportExternal(providerId, source);
  }

  function submitRefresh() {
    if (credentialBusy) return;
    onRefresh();
  }

  function sourceLabel(source: "claude" | "codex"): string {
    return source === "claude" ? "~/.claude" : "~/.codex";
  }

  $: authBusy = busyProviderId !== null;
  $: credentialBusy = authBusy || busyImportKey !== null;
  $: if (pendingApiKeyProvider && busyProviderId === pendingApiKeyProvider) {
    pendingApiKeyBusyObserved = true;
  }
  $: if (
    pendingApiKeyProvider &&
    pendingApiKeyBusyObserved &&
    busyProviderId !== pendingApiKeyProvider
  ) {
    if (!errorMessage) {
      const next = { ...apiKeys };
      delete next[pendingApiKeyProvider];
      apiKeys = next;
    }
    pendingApiKeyProvider = null;
    pendingApiKeyBusyObserved = false;
  }
</script>

<section class="login-page">
  {#if errorMessage}
    <div class="error-banner">{errorMessage}</div>
  {/if}

  {#if remoteEnabled}
    <div class="remote-banner">
      Remote mode is active. API keys are stored on the remote host; OAuth opens locally then
      syncs the credential back over SSH.
    </div>
  {/if}

  {#if !loading && snapshot && connectedProviders.length}
    <div class="provider-section">
      <h3>Connected providers</h3>
      <div class="provider-grid" data-section="connected">
        {#each connectedProviders as provider (provider.id)}
          {@const visual = providerVisual(provider)}
          {@const auth = authForProvider(provider.id)}
          {@const authFree = providerRunsWithoutAuth(provider)}
          <article class="provider-card" style="--provider-accent: {visual.accent};">
            <header class="card-head">
              <span class="logo" aria-hidden="true">
                <img src={visual.icon} alt="" />
              </span>
              <div class="head-text">
                <h2 class="name">{provider.displayName}</h2>
              </div>
              <span class="status" data-connected="true">
                {auth ? `Connected via ${auth.kind}` : authFree ? "Ready" : "Connected"}
              </span>
            </header>

            <div class="actions">
              <button
                class="oauth-btn"
                disabled={credentialBusy}
                on:click={() => openProviderModal(provider.id)}
              >
                Manage connection
              </button>
            </div>

          </article>
        {/each}
      </div>
    </div>
  {/if}

  {#if usingFallbackProviders}
    <div class="provider-fallback-note" role="status">
      Provider registry is empty. Built-in setup options are shown so you can connect a provider,
      then refresh when resources reload.
    </div>
  {/if}

  <div class="provider-section">
    <h3>Popular providers</h3>
    <div class="search-row">
      <input
        type="search"
        class="search-input"
        placeholder="Search providers"
        bind:value={query}
        autocomplete="off"
        spellcheck="false"
      />
      <button class="refresh-btn" disabled={credentialBusy} on:click={submitRefresh} title="Re-scan providers">
        Refresh
      </button>
    </div>

    <div class="provider-grid">
      {#if loading}
        <div class="empty-card">Loading providers and auth state...</div>
      {:else if !filteredProviders.length}
        <div class="empty-card">No providers match "{query}".</div>
      {:else}
        {#each filteredProviders as provider (provider.id)}
          {@const visual = providerVisual(provider)}
          {@const auth = authForProvider(provider.id)}
          {@const authFree = providerRunsWithoutAuth(provider)}
          <article class="provider-card" style="--provider-accent: {visual.accent};">
            <header class="card-head">
              <span class="logo" aria-hidden="true">
                <img src={visual.icon} alt="" />
              </span>
              <div class="head-text">
                <h2 class="name">{provider.displayName}</h2>
                <p class="meta">{provider.modelCount} model{provider.modelCount === 1 ? "" : "s"}</p>
              </div>
              {#if auth || authFree}
                <span class="status" data-connected="true">
                  {auth ? "Connected" : "Ready"}
                </span>
              {/if}
            </header>

            <div class="actions">
              <button
                class="oauth-btn"
                disabled={credentialBusy}
                on:click={() => openProviderModal(provider.id)}
              >
                {auth || authFree ? "Manage connection" : "Connect"}
              </button>
            </div>

          </article>
        {/each}
      {/if}
    </div>
  </div>

  {#if activeProvider}
    {@const visual = providerVisual(activeProvider)}
    {@const candidates = importsByProvider[activeProvider.id] ?? []}
    {@const auth = authForProvider(activeProvider.id)}
    {@const authFree = providerRunsWithoutAuth(activeProvider)}
    <div
      class="provider-modal-scrim"
      role="presentation"
      on:click={closeProviderModal}
      on:keydown={(event) => {
        if (event.key === "Escape") closeProviderModal();
      }}
    >
      <div
        class="provider-modal"
        role="dialog"
        aria-modal="true"
        aria-label={`Connect ${activeProvider.displayName}`}
        style="--provider-accent: {visual.accent};"
        tabindex="-1"
        on:click={(event) => event.stopPropagation()}
        on:keydown={(event) => {
          if (event.key === "Escape") closeProviderModal();
        }}
      >
        <header class="provider-modal-head">
          <span class="logo" aria-hidden="true">
            <img src={visual.icon} alt="" />
          </span>
          <div>
            <h2>{activeProvider.displayName}</h2>
            <p>
              {auth
                ? connectedHint(auth)
                : authFree
                  ? "This provider can run without saved credentials."
                  : `${activeProvider.modelCount} model${activeProvider.modelCount === 1 ? "" : "s"} available`}
            </p>
          </div>
          <button type="button" class="modal-close" aria-label="Close" on:click={closeProviderModal}>
            <Icon name="x" size={14} />
          </button>
        </header>

        <div class="provider-modal-body">
          {#if candidates.length}
            <div class="provider-modal-section">
              <h3>Saved credentials</h3>
              <div class="imports">
                {#each candidates as candidate (importKey(candidate.providerId, candidate.source))}
                  <button
                    type="button"
                    class="import"
                    disabled={credentialBusy}
                    on:click={() => submitImport(candidate.providerId, candidate.source)}
                    title={candidate.sourcePath}
                  >
                    {#if busyImportKey === importKey(candidate.providerId, candidate.source)}
                      Importing...
                    {:else}
                      Use credentials from {sourceLabel(candidate.source)}
                    {/if}
                  </button>
                {/each}
              </div>
            </div>
          {/if}

          {#if supports(activeProvider, "oauth")}
            <div class="provider-modal-section">
              <h3>OAuth</h3>
              <button
                class="oauth-btn"
                disabled={credentialBusy}
                on:click={() => submitOauth(activeProvider.id)}
              >
                {busyProviderId === activeProvider.id
                  ? "Opening browser..."
                  : auth
                    ? remoteEnabled
                      ? "Reconnect with OAuth (remote)"
                      : "Reconnect with OAuth"
                    : remoteEnabled
                      ? "Connect with OAuth (remote)"
                      : "Connect with OAuth"}
              </button>
            </div>
          {/if}

          {#if supports(activeProvider, "api_key")}
            <div class="provider-modal-section">
              <h3>API key</h3>
              <div class="api-key-row">
                <input
                  type="password"
                  aria-label={`API key for ${activeProvider.displayName}`}
                  value={apiKeys[activeProvider.id] ?? ""}
                  placeholder={auth ? "Replace API key" : "Paste API key"}
                  disabled={credentialBusy}
                  on:input={(event) =>
                    updateApiKey(activeProvider.id, (event.currentTarget as HTMLInputElement).value)}
                  on:keydown={(event) => {
                    if (event.key === "Enter") submitApiKey(activeProvider.id);
                  }}
                />
                <button
                  class="apikey-btn"
                  disabled={credentialBusy || !(apiKeys[activeProvider.id] ?? "").trim()}
                  on:click={() => submitApiKey(activeProvider.id)}
                >
                  {auth ? "Update key" : "Connect"}
                </button>
              </div>
            </div>
          {/if}

          {#if authFree && !supports(activeProvider, "oauth") && !supports(activeProvider, "api_key") && !candidates.length}
            <div class="provider-modal-section">
              <p class="provider-modal-note">No connection setup is required for this provider.</p>
            </div>
          {/if}
        </div>
      </div>
    </div>
  {/if}
</section>

<style>
  .login-page {
    min-height: 0;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .error-banner,
  .remote-banner {
    border-radius: 16px;
    border: 1px solid rgba(111, 101, 89, 0.14);
    padding: 0.85rem 1rem;
    font-size: 0.9rem;
    line-height: 1.45;
  }
  .error-banner {
    background: rgba(247, 225, 220, 0.76);
    border-color: rgba(157, 58, 43, 0.16);
    color: var(--danger);
  }
  .remote-banner {
    background: rgba(255, 255, 255, 0.7);
    color: var(--text-muted);
  }

  .provider-section {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }
  .provider-section h3 {
    margin: 0;
    color: var(--text);
    font-size: 14px;
    font-weight: 600;
    line-height: 18px;
  }
  .search-row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 86px;
    gap: 10px;
  }
  .search-input {
    width: 100%;
    height: 36px;
    padding: 0 12px;
    border: 1px solid rgba(111, 101, 89, 0.18);
    border-radius: 8px;
    background: rgba(255, 255, 255, 0.88);
    color: var(--text);
    font: inherit;
    font-size: 14px;
  }
  .search-input:focus-visible {
    outline: 2px solid color-mix(in oklab, var(--accent) 35%, transparent);
    outline-offset: 1px;
    border-color: var(--accent);
  }
  .refresh-btn {
    height: 36px;
    padding: 0 12px;
    border-radius: 8px;
    border: 1px solid rgba(111, 101, 89, 0.18);
    background: rgba(255, 255, 255, 0.88);
    color: var(--text);
    cursor: pointer;
    font: inherit;
    font-size: 14px;
  }
  .refresh-btn:disabled {
    opacity: 0.6;
    cursor: progress;
  }

  .provider-fallback-note {
    border-radius: 12px;
    border: 1px solid color-mix(in oklab, var(--accent) 22%, rgba(111, 101, 89, 0.16));
    background: color-mix(in oklab, var(--accent) 8%, rgba(255, 255, 255, 0.78));
    color: var(--text-muted);
    padding: 0.72rem 0.9rem;
    font-size: 0.82rem;
    line-height: 1.4;
  }

  .provider-grid {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 14px;
  }

  .provider-card {
    --provider-accent: #475569;
    border-radius: 16px;
    border: 1px solid rgba(111, 101, 89, 0.16);
    background: linear-gradient(
      180deg,
      color-mix(in oklab, var(--provider-accent) 7%, white) 0%,
      rgba(255, 255, 255, 0.92) 100%
    );
    box-shadow: var(--shadow-soft);
    padding: 18px 16px;
    display: flex;
    flex-direction: column;
    gap: 10px;
    min-height: 118px;
    justify-content: space-between;
    color: var(--text);
  }

  .card-head {
    display: flex;
    align-items: center;
    gap: 0.7rem;
  }
  .logo {
    width: 36px;
    height: 36px;
    flex: 0 0 36px;
    border-radius: 10px;
    background: rgba(255, 255, 255, 0.92);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    box-shadow:
      0 1px 0 rgba(255, 255, 255, 0.4) inset,
      0 0 0 1px color-mix(in oklab, var(--provider-accent) 35%, rgba(111, 101, 89, 0.25)) inset;
  }
  .logo img {
    width: 23px;
    height: 23px;
    object-fit: contain;
    display: block;
  }
  .head-text {
    display: grid;
    gap: 0.1rem;
    min-width: 0;
    flex: 1;
  }
  .name {
    margin: 0;
    font-size: 16px;
    line-height: 19px;
    letter-spacing: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .meta {
    margin: 0;
    font-size: 12.5px;
    color: var(--text-muted);
    font-family: var(--font-sans, system-ui, sans-serif);
    line-height: 18px;
  }
  .status {
    justify-self: end;
    border-radius: 999px;
    border: 1px solid rgba(111, 101, 89, 0.16);
    background: rgba(255, 255, 255, 0.74);
    color: var(--text-muted);
    flex: 0 0 auto;
    font-size: 11px;
    font-weight: 700;
    line-height: 14px;
    padding: 5px 10px;
  }
  .status[data-connected="true"] {
    border-color: color-mix(in oklab, var(--provider-accent) 42%, rgba(111, 101, 89, 0.18));
    background: color-mix(in oklab, var(--provider-accent) 13%, white);
    color: color-mix(in oklab, var(--provider-accent) 72%, black);
  }
  .imports {
    display: grid;
    gap: 0.4rem;
  }
  .import {
    text-align: left;
    padding: 0.5rem 0.7rem;
    border-radius: 10px;
    border: 1px dashed color-mix(in oklab, var(--provider-accent) 50%, rgba(111, 101, 89, 0.4));
    background: color-mix(in oklab, var(--provider-accent) 8%, white);
    color: color-mix(in oklab, var(--provider-accent) 70%, var(--text));
    font: inherit;
    font-size: 0.85rem;
    cursor: pointer;
  }
  .import:hover:not(:disabled) {
    background: color-mix(in oklab, var(--provider-accent) 14%, white);
  }
  .import:disabled {
    opacity: 0.7;
    cursor: progress;
  }

  .actions {
    display: grid;
    gap: 8px;
  }
  .oauth-btn,
  .apikey-btn {
    border: none;
    border-radius: 10px;
    min-height: 34px;
    padding: 0 12px;
    font: inherit;
    font-size: 14px;
    font-weight: 600;
    cursor: pointer;
  }
  .oauth-btn {
    background: var(--provider-accent);
    color: #fff;
  }
  .oauth-btn:hover:not(:disabled) {
    filter: brightness(1.05);
  }
  .provider-card .oauth-btn {
    border: 1px solid color-mix(in oklab, var(--provider-accent) 24%, rgba(111, 101, 89, 0.16));
    background: color-mix(in oklab, var(--provider-accent) 10%, rgba(255, 255, 255, 0.9));
    color: color-mix(in oklab, var(--provider-accent) 78%, black);
    box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--provider-accent) 8%, transparent);
  }
  .provider-card .oauth-btn:hover:not(:disabled) {
    background: color-mix(in oklab, var(--provider-accent) 14%, rgba(255, 255, 255, 0.92));
    border-color: color-mix(in oklab, var(--provider-accent) 34%, rgba(111, 101, 89, 0.16));
    filter: none;
  }
  .apikey-btn {
    background: color-mix(in oklab, var(--provider-accent) 14%, white);
    color: color-mix(in oklab, var(--provider-accent) 80%, black);
    border: 1px solid color-mix(in oklab, var(--provider-accent) 35%, rgba(111, 101, 89, 0.25));
  }
  .oauth-btn:disabled,
  .apikey-btn:disabled {
    opacity: 0.6;
    cursor: progress;
  }

  .api-key-row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    gap: 8px;
  }
  .api-key-row input {
    min-height: 34px;
    padding: 0 10px;
    border-radius: 10px;
    border: 1px solid rgba(111, 101, 89, 0.2);
    background: rgba(255, 255, 255, 0.92);
    color: var(--text);
    font: inherit;
    font-size: 13px;
    min-width: 0;
  }
  .api-key-row input:focus-visible {
    outline: 2px solid color-mix(in oklab, var(--provider-accent) 40%, transparent);
    outline-offset: 1px;
    border-color: var(--provider-accent);
  }

  .empty-card {
    grid-column: 1 / -1;
    border-radius: 16px;
    border: 1px dashed rgba(111, 101, 89, 0.25);
    background: rgba(255, 255, 255, 0.6);
    padding: 1.4rem;
    color: var(--text-muted);
    text-align: center;
  }

  .provider-modal-scrim {
    position: fixed;
    inset: 0;
    z-index: 80;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 24px;
    background: rgba(17, 17, 17, 0.28);
  }

  .provider-modal {
    width: min(520px, calc(100vw - 32px));
    max-height: min(720px, calc(100vh - 48px));
    overflow: auto;
    border: 1px solid rgba(111, 101, 89, 0.18);
    border-radius: 16px;
    background: var(--background);
    box-shadow: 0 22px 60px rgba(15, 23, 42, 0.18);
    color: var(--text);
  }

  .provider-modal-head {
    display: grid;
    grid-template-columns: 36px minmax(0, 1fr) 30px;
    gap: 12px;
    align-items: center;
    padding: 18px 18px 14px;
    border-bottom: 1px solid rgba(111, 101, 89, 0.14);
  }

  .provider-modal-head h2 {
    margin: 0;
    font-size: 18px;
    line-height: 22px;
    letter-spacing: 0;
  }

  .provider-modal-head p {
    margin: 2px 0 0;
    color: var(--text-muted);
    font-size: 12.5px;
    line-height: 18px;
  }

  .modal-close {
    width: 30px;
    height: 30px;
    border: 1px solid rgba(111, 101, 89, 0.14);
    border-radius: 8px;
    background: rgba(255, 255, 255, 0.72);
    color: var(--text-muted);
    cursor: pointer;
    font: inherit;
    font-size: 14px;
  }

  .provider-modal-body {
    display: flex;
    flex-direction: column;
    gap: 16px;
    padding: 16px 18px 18px;
  }

  .provider-modal-section {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .provider-modal-section h3 {
    margin: 0;
    color: var(--text);
    font-size: 13px;
    font-weight: 700;
    line-height: 16px;
  }

  .provider-modal-note {
    margin: 0;
    color: var(--text-muted);
    font-size: 13px;
    line-height: 18px;
  }

  @media (max-width: 980px) {
    .provider-grid {
      grid-template-columns: minmax(0, 1fr);
    }
    .api-key-row {
      grid-template-columns: 1fr;
    }
  }

  @media (max-width: 560px) {
    .search-row {
      grid-template-columns: minmax(0, 1fr);
    }
    .refresh-btn {
      width: 100%;
    }
  }
</style>

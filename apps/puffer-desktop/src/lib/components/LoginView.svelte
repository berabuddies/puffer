<script lang="ts">
  import { providerVisual } from "../providerVisuals";
  import {
    providerIsAvailableForAgent,
    providerIdsEquivalent,
    providerRunsWithoutAuth
  } from "../providerIds";
  import {
    providerCatalogForSetup,
    usesFallbackProviderCatalog
  } from "../providerFallbacks";
  import type { ExternalCredential, ProviderSummary, SettingsSnapshot } from "../types";

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

  function providerDisplayName(providerId: string): string {
    const provider = snapshot?.providers.find((candidate) =>
      providerIdsEquivalent(candidate.id, providerId)
    );
    return provider?.displayName ?? providerId;
  }

  $: filteredProviders = (() => {
    const all = providerCatalogForSetup(snapshot);
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
  $: authenticatedProviderIds = connectedAuth.map((auth) => auth.providerId);
  $: availableAgentProviders =
    providerCatalogForSetup(snapshot).filter((provider) =>
      providerIsAvailableForAgent(provider, authenticatedProviderIds)
    );
  $: showAvailableProviders = availableAgentProviders.length > connectedAuth.length;
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

  {#if !loading && snapshot}
    <div class="connection-summary" role="status" aria-label="Credential connections">
      <div class="connection-copy">
        <strong>
          {#if showAvailableProviders}
            {availableAgentProviders.length} agent provider{availableAgentProviders.length === 1 ? "" : "s"} ready
          {:else}
            {connectedAuth.length} provider{connectedAuth.length === 1 ? "" : "s"} connected
          {/if}
        </strong>
        <span>
          {availableAgentProviders.length
            ? "Ready for new sessions and provider switches."
            : "Connect a provider before starting an agent."}
        </span>
      </div>
      {#if showAvailableProviders}
        <div class="connection-pills" aria-label="Ready agent list">
          {#each availableAgentProviders as provider (provider.id)}
            {@const auth = authForProvider(provider.id)}
            <span class="connection-pill">
              <span class="pill-name">{provider.displayName}</span>
              <span class="pill-kind">{auth?.kind ?? "local"}</span>
            </span>
          {/each}
        </div>
      {:else if connectedAuth.length}
        <div class="connection-pills" aria-label="Credential list">
          {#each connectedAuth as auth (auth.providerId)}
            <span class="connection-pill">
              <span class="pill-name">{providerDisplayName(auth.providerId)}</span>
              <span class="pill-kind">{auth.kind}</span>
            </span>
          {/each}
        </div>
      {/if}
    </div>
  {/if}

  <div class="search-row">
    <input
      type="search"
      class="search-input"
      placeholder="Search providers (anthropic, openai, groq, …)"
      bind:value={query}
      autocomplete="off"
      spellcheck="false"
    />
    <button class="refresh-btn" disabled={credentialBusy} on:click={submitRefresh} title="Re-scan providers">
      Refresh
    </button>
  </div>

  {#if usingFallbackProviders}
    <div class="provider-fallback-note" role="status">
      Provider registry is empty. Built-in setup options are shown so you can connect a provider,
      then refresh when resources reload.
    </div>
  {/if}

  <div class="provider-grid">
    {#if loading}
      <div class="empty-card">Loading providers and auth state…</div>
    {:else if !filteredProviders.length}
      <div class="empty-card">No providers match "{query}".</div>
    {:else}
      {#each filteredProviders as provider (provider.id)}
        {@const visual = providerVisual(provider)}
        {@const candidates = importsByProvider[provider.id] ?? []}
        {@const auth = authForProvider(provider.id)}
        {@const authFree = providerRunsWithoutAuth(provider)}
        <article class="provider-card" style="--provider-accent: {visual.accent};">
          <header class="card-head">
            <span class="logo" aria-hidden="true">
              <img src={visual.icon} alt="" />
            </span>
            <div class="head-text">
              <h2 class="name">{provider.displayName}</h2>
              <p class="meta">{provider.id} · {provider.modelCount} model{provider.modelCount === 1 ? "" : "s"}</p>
            </div>
            <span class="status" data-connected={auth !== null || authFree}>
              {auth ? "Connected" : authFree ? "Ready" : "Not connected"}
            </span>
          </header>

          {#if candidates.length}
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
                    Importing…
                  {:else}
                    Use credentials from {sourceLabel(candidate.source)}
                  {/if}
                </button>
              {/each}
            </div>
          {/if}

          <div class="actions">
            {#if supports(provider, "oauth")}
              <button
                class="oauth-btn"
                disabled={credentialBusy}
                on:click={() => submitOauth(provider.id)}
              >
                {busyProviderId === provider.id
                  ? "Opening browser…"
                  : auth
                    ? remoteEnabled
                      ? "Reconnect with OAuth (remote)"
                      : "Reconnect with OAuth"
                    : remoteEnabled
                      ? "Connect with OAuth (remote)"
                      : "Connect with OAuth"}
              </button>
            {/if}

            {#if supports(provider, "api_key")}
              <div class="api-key-row">
                <input
                  type="password"
                  aria-label={`API key for ${provider.displayName}`}
                  value={apiKeys[provider.id] ?? ""}
                  placeholder={auth ? "Replace API key" : "Paste API key"}
                  disabled={credentialBusy}
                  on:input={(event) =>
                    updateApiKey(provider.id, (event.currentTarget as HTMLInputElement).value)}
                  on:keydown={(event) => {
                    if (event.key === "Enter") submitApiKey(provider.id);
                  }}
                />
                <button
                  class="apikey-btn"
                  disabled={credentialBusy || !(apiKeys[provider.id] ?? "").trim()}
                  on:click={() => submitApiKey(provider.id)}
                >
                  {auth ? "Update key" : "Connect"}
                </button>
              </div>
            {/if}
          </div>

          <p class="hint">
            {auth
              ? connectedHint(auth)
              : authFree
                ? "No credentials required"
                : `via ${provider.authModes.join(" · ")}`}
          </p>
        </article>
      {/each}
    {/if}
  </div>
</section>

<style>
  .login-page {
    min-height: 0;
    overflow: auto;
    padding: 1.4rem;
    display: grid;
    gap: 1rem;
    background: rgba(255, 252, 246, 0.46);
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

  .connection-summary {
    border-radius: 12px;
    border: 1px solid rgba(111, 101, 89, 0.14);
    background: rgba(255, 255, 255, 0.74);
    padding: 0.85rem 0.95rem;
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    align-items: center;
    gap: 0.85rem;
  }
  .connection-copy {
    min-width: 0;
    display: grid;
    gap: 0.18rem;
  }
  .connection-copy strong {
    font-size: 0.92rem;
    line-height: 1.2;
  }
  .connection-copy span {
    color: var(--text-muted);
    font-size: 0.8rem;
    line-height: 1.35;
  }
  .connection-pills {
    display: flex;
    justify-content: flex-end;
    flex-wrap: wrap;
    gap: 0.4rem;
  }
  .connection-pill {
    border-radius: 999px;
    border: 1px solid rgba(111, 101, 89, 0.14);
    background: color-mix(in oklab, var(--accent) 8%, white);
    padding: 0.38rem 0.55rem;
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
    max-width: 220px;
    font-size: 0.78rem;
    line-height: 1;
  }
  .pill-name {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-weight: 700;
  }
  .pill-kind {
    color: var(--text-muted);
    font-family: var(--font-mono, ui-monospace, monospace);
    font-size: 0.72rem;
  }

  .search-row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    gap: 0.6rem;
  }
  .search-input {
    width: 100%;
    padding: 0.7rem 0.95rem;
    border: 1px solid rgba(111, 101, 89, 0.18);
    border-radius: 999px;
    background: rgba(255, 255, 255, 0.88);
    color: var(--text);
    font: inherit;
  }
  .search-input:focus-visible {
    outline: 2px solid color-mix(in oklab, var(--accent) 35%, transparent);
    outline-offset: 1px;
    border-color: var(--accent);
  }
  .refresh-btn {
    padding: 0.7rem 1.1rem;
    border-radius: 999px;
    border: 1px solid rgba(111, 101, 89, 0.18);
    background: rgba(255, 255, 255, 0.88);
    color: var(--text);
    cursor: pointer;
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

  @media (max-width: 680px) {
    .connection-summary {
      grid-template-columns: 1fr;
      align-items: stretch;
    }
    .connection-pills {
      justify-content: flex-start;
    }
  }

  .provider-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
    gap: 0.9rem;
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
    padding: 0.95rem 1rem 0.85rem;
    display: grid;
    gap: 0.7rem;
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
    font-size: 1rem;
    line-height: 1.2;
    letter-spacing: -0.01em;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .meta {
    margin: 0;
    font-size: 0.78rem;
    color: var(--text-muted);
    font-family: var(--font-mono, ui-monospace, monospace);
  }
  .status {
    justify-self: end;
    border-radius: 999px;
    border: 1px solid rgba(111, 101, 89, 0.16);
    background: rgba(255, 255, 255, 0.74);
    color: var(--text-muted);
    flex: 0 0 auto;
    font-size: 0.72rem;
    font-weight: 600;
    line-height: 1;
    padding: 0.35rem 0.5rem;
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
    gap: 0.55rem;
  }
  .oauth-btn,
  .apikey-btn {
    border: none;
    border-radius: 10px;
    padding: 0.55rem 0.85rem;
    font: inherit;
    font-weight: 500;
    cursor: pointer;
  }
  .oauth-btn {
    background: var(--provider-accent);
    color: #fff;
  }
  .oauth-btn:hover:not(:disabled) {
    filter: brightness(1.05);
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
    gap: 0.5rem;
  }
  .api-key-row input {
    padding: 0.55rem 0.7rem;
    border-radius: 10px;
    border: 1px solid rgba(111, 101, 89, 0.2);
    background: rgba(255, 255, 255, 0.92);
    color: var(--text);
    font: inherit;
    min-width: 0;
  }
  .api-key-row input:focus-visible {
    outline: 2px solid color-mix(in oklab, var(--provider-accent) 40%, transparent);
    outline-offset: 1px;
    border-color: var(--provider-accent);
  }

  .hint {
    margin: 0;
    color: var(--text-muted);
    font-size: 0.74rem;
    font-family: var(--font-mono, ui-monospace, monospace);
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

  @media (max-width: 980px) {
    .api-key-row {
      grid-template-columns: 1fr;
    }
  }
</style>

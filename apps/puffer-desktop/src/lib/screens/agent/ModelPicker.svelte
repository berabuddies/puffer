<script lang="ts">
  import { listProviderModels, type ModelDescriptorInfo } from "../../api/desktop";
  import type { SettingsSnapshot } from "../../types";
  import Icon from "../../design/Icon.svelte";

  type Props = {
    snapshot: SettingsSnapshot | null;
    currentProvider?: string | null;
    currentModel?: string | null;
    allowProviderSwitch?: boolean;
    disabled?: boolean;
    onChange: (providerId: string, modelId: string) => void;
  };

  let {
    snapshot,
    currentProvider: currentProviderOverride = null,
    currentModel: currentModelOverride = null,
    allowProviderSwitch = true,
    disabled = false,
    onChange
  }: Props = $props();

  let open = $state(false);
  let busy = $state(false);
  let query = $state("");
  // Lazy-loaded per provider; the daemon's `list_provider_models` does the
  // discovery (which can hit the network), so we don't want to fan out
  // until the user actually opens the picker.
  let modelsByProvider = $state<Record<string, ModelDescriptorInfo[]>>({});
  let loadError = $state<string | null>(null);
  let triggerEl: HTMLButtonElement | null = $state(null);
  let menuEl: HTMLDivElement | null = $state(null);

  let currentProvider = $derived(
    currentProviderOverride ?? snapshot?.config?.defaultProvider ?? ""
  );
  let currentModel = $derived(
    currentModelOverride ?? snapshot?.config?.defaultModel ?? ""
  );
  let authedProviderIds = $derived(
    new Set((snapshot?.auth ?? []).map((entry) => entry.providerId))
  );
  let authedProviders = $derived(
    (snapshot?.providers ?? []).filter((provider) => authedProviderIds.has(provider.id))
  );
  let providerLabel = $derived(
    snapshot?.providers?.find((p) => p.id === currentProvider)?.displayName ?? currentProvider
  );

  let currentProviderModels = $derived(modelsByProvider[currentProvider] ?? []);

  // Filter models only within the selected provider. Provider switching is
  // explicit via the provider row above the model list.
  let filteredEntries = $derived.by(() => {
    const needle = query.trim().toLowerCase();
    const out: { provider: string; providerLabel: string; model: ModelDescriptorInfo }[] = [];
    const provider = authedProviders.find((entry) => entry.id === currentProvider);
    if (!provider) return out;
    for (const model of currentProviderModels) {
      if (
        !needle ||
        model.id.toLowerCase().includes(needle) ||
        model.displayName.toLowerCase().includes(needle)
      ) {
        out.push({ provider: provider.id, providerLabel: provider.displayName, model });
      }
    }
    return out;
  });

  async function loadModels() {
    busy = true;
    loadError = null;
    try {
      const next: Record<string, ModelDescriptorInfo[]> = { ...modelsByProvider };
      const providers = allowProviderSwitch
        ? authedProviders
        : authedProviders.filter((provider) => provider.id === currentProvider);
      for (const provider of providers) {
        if (next[provider.id]) continue;
        try {
          next[provider.id] = await listProviderModels(provider.id);
        } catch (error) {
          next[provider.id] = [];
          loadError = `${provider.id}: ${error}`;
        }
      }
      modelsByProvider = next;
    } finally {
      busy = false;
    }
  }

  async function selectProvider(providerId: string) {
    if (!allowProviderSwitch || disabled) return;
    if (providerId === currentProvider) return;
    query = "";
    let models = modelsByProvider[providerId] ?? [];
    if (models.length === 0) {
      busy = true;
      loadError = null;
      try {
        models = await listProviderModels(providerId);
        modelsByProvider = { ...modelsByProvider, [providerId]: models };
      } catch (error) {
        modelsByProvider = { ...modelsByProvider, [providerId]: [] };
        loadError = `${providerId}: ${error}`;
        models = [];
      } finally {
        busy = false;
      }
    }
    const defaultModel = models.find((model) => model.isDefault) ?? models[0];
    onChange(providerId, defaultModel?.id ?? "");
  }

  function toggle() {
    if (disabled) return;
    open = !open;
    if (open) {
      void loadModels();
    }
  }

  function pick(providerId: string, modelId: string) {
    if (disabled) return;
    open = false;
    query = "";
    onChange(providerId, modelId);
  }

  function handleDocumentClick(event: MouseEvent) {
    if (!open) return;
    const target = event.target as Node | null;
    if (!target) return;
    if (triggerEl?.contains(target)) return;
    if (menuEl?.contains(target)) return;
    open = false;
  }

  $effect(() => {
    if (typeof document === "undefined") return;
    document.addEventListener("mousedown", handleDocumentClick);
    return () => document.removeEventListener("mousedown", handleDocumentClick);
  });
</script>

<div class="picker">
  <button
    bind:this={triggerEl}
    type="button"
    class="trigger"
    class:open
    onclick={toggle}
    disabled={disabled}
    title={currentModel ? `${providerLabel} · ${currentModel}` : "Pick a model"}
  >
    <Icon name="sparkles" size={11} color="var(--muted-foreground)" />
    <span class="model" class:placeholder={!currentModel}>
      {currentModel || "Pick model"}
    </span>
    {#if providerLabel && currentModel}
      <span class="provider">{providerLabel}</span>
    {/if}
    <Icon name="chevD" size={10} color="var(--muted-foreground)" />
  </button>

  {#if open}
    <div bind:this={menuEl} class="menu" role="listbox">
      {#if allowProviderSwitch}
        <div class="providers" aria-label="Provider">
          {#each authedProviders as provider (provider.id)}
            <button
              type="button"
              class:on={provider.id === currentProvider}
              onclick={() => selectProvider(provider.id)}
            >
              {provider.displayName}
            </button>
          {/each}
        </div>
      {/if}
      <input
        type="search"
        class="search"
        placeholder={providerLabel ? `Filter ${providerLabel} models` : "Filter models"}
        bind:value={query}
        autocomplete="off"
        spellcheck="false"
      />
      <div class="results">
        {#if busy && filteredEntries.length === 0}
          <div class="hint">Loading models…</div>
        {:else if filteredEntries.length === 0}
          {#if authedProviders.length === 0}
            <div class="hint">Connect a provider first.</div>
          {:else if !currentProvider}
            <div class="hint">Pick a provider.</div>
          {:else if query}
            <div class="hint">No matches for "{query}".</div>
          {:else}
            <div class="hint">No {providerLabel} models available.</div>
          {/if}
        {:else}
          {#each filteredEntries as entry (entry.provider + "::" + entry.model.id)}
            {@const isCurrent =
              entry.provider === currentProvider && entry.model.id === currentModel}
            <button
              type="button"
              class="row"
              class:on={isCurrent}
              onclick={() => pick(entry.provider, entry.model.id)}
              role="option"
              aria-selected={isCurrent}
            >
              <span class="row-main">
                <span class="row-name">{entry.model.displayName || entry.model.id}</span>
                <span class="row-provider">{entry.providerLabel}</span>
              </span>
              <span class="row-id">{entry.model.id}</span>
              {#if isCurrent}
                <Icon name="check" size={11} color="var(--accent)" />
              {/if}
            </button>
          {/each}
        {/if}
      </div>
      {#if loadError}
        <div class="error" title={loadError}>Some models failed to load</div>
      {/if}
    </div>
  {/if}
</div>

<style>
  .picker {
    position: relative;
    display: inline-block;
    flex-shrink: 0;
  }

  .trigger {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 4px 8px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--background);
    color: var(--foreground);
    cursor: pointer;
    font: inherit;
    font-size: 11.5px;
    line-height: 1;
    max-width: 240px;
    transition: background 120ms, border-color 120ms;
  }
  .trigger:hover,
  .trigger.open {
    background: color-mix(in oklab, var(--background) 92%, var(--muted));
    border-color: color-mix(in oklab, var(--accent) 35%, var(--border));
  }
  .trigger:disabled {
    cursor: not-allowed;
    opacity: 0.6;
  }
  .trigger .model {
    font-family: var(--font-mono);
    font-weight: 500;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }
  .trigger .model.placeholder {
    color: var(--muted-foreground);
    font-weight: 400;
  }
  .trigger .provider {
    color: var(--muted-foreground);
    font-size: 10.5px;
    padding-left: 6px;
    border-left: 1px solid var(--border);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .menu {
    position: absolute;
    bottom: calc(100% + 4px);
    left: 0;
    z-index: 60;
    min-width: 320px;
    max-width: 420px;
    background: var(--background);
    border: 1px solid var(--border);
    border-radius: 10px;
    box-shadow: 0 12px 32px rgb(0 0 0 / 0.18);
    padding: 8px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .providers {
    display: flex;
    gap: 4px;
    padding: 2px;
    border: 1px solid var(--border);
    border-radius: 7px;
    background: color-mix(in oklab, var(--background) 94%, var(--muted));
  }
  .providers button {
    flex: 1;
    min-width: 0;
    border: 0;
    border-radius: 5px;
    background: transparent;
    color: var(--muted-foreground);
    cursor: pointer;
    font: inherit;
    font-size: 11.5px;
    padding: 5px 8px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .providers button:hover,
  .providers button.on {
    background: var(--background);
    color: var(--foreground);
  }

  .search {
    padding: 6px 9px;
    border-radius: 6px;
    border: 1px solid var(--border);
    background: var(--background);
    color: var(--foreground);
    font: inherit;
    font-size: 12px;
  }
  .search:focus-visible {
    outline: 2px solid color-mix(in oklab, var(--accent) 35%, transparent);
    outline-offset: 1px;
    border-color: var(--accent);
  }

  .results {
    max-height: 320px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }
  .hint {
    padding: 14px 10px;
    color: var(--muted-foreground);
    font-size: 12px;
    text-align: center;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 8px;
    border: 0;
    background: transparent;
    border-radius: 6px;
    cursor: pointer;
    color: var(--foreground);
    font: inherit;
    text-align: left;
  }
  .row:hover {
    background: color-mix(in oklab, var(--background) 90%, var(--muted));
  }
  .row.on {
    background: color-mix(in oklab, var(--accent) 12%, var(--background));
  }
  .row-main {
    display: flex;
    flex-direction: column;
    gap: 1px;
    flex: 1;
    min-width: 0;
  }
  .row-name {
    font-size: 12.5px;
    font-weight: 500;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .row-provider {
    font-size: 10.5px;
    color: var(--muted-foreground);
  }
  .row-id {
    font-family: var(--font-mono);
    font-size: 10.5px;
    color: var(--muted-foreground);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 140px;
  }

  .error {
    padding: 6px 8px;
    color: var(--danger, oklch(0.55 0.2 30));
    font-size: 11px;
    border-top: 1px solid var(--border);
    margin-top: 4px;
  }
</style>

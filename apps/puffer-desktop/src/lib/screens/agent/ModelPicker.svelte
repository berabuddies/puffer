<script lang="ts">
  import { listProviderModels, type ModelDescriptorInfo } from "../../api/desktop";
  import type { SettingsSnapshot } from "../../types";
  import Icon from "../../design/Icon.svelte";
  import { providerIsAvailableForAgent, providerIdsEquivalent } from "../../providerIds";

  type Props = {
    snapshot: SettingsSnapshot | null;
    currentProvider?: string | null;
    currentModel?: string | null;
    contextKey?: string | null;
    allowProviderSwitch?: boolean;
    disabled?: boolean;
    onChange: (providerId: string, modelId: string) => void;
  };

  let {
    snapshot,
    currentProvider: currentProviderOverride = null,
    currentModel: currentModelOverride = null,
    contextKey = null,
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
  let pendingProviderId = $state<string | null>(null);
  let activeContextKey = $state<string | null>(null);
  let providerSwitchRollback = $state<{ providerId: string; modelId: string } | null>(null);
  let modelLoadGeneration = 0;
  let providerSwitchGeneration = 0;

  let currentProvider = $derived(
    currentProviderOverride ?? snapshot?.config?.defaultProvider ?? ""
  );
  let currentModel = $derived(
    currentModelOverride ?? snapshot?.config?.defaultModel ?? ""
  );
  let activeProvider = $derived(pendingProviderId ?? currentProvider);
  let activeModel = $derived(pendingProviderId ? "" : currentModel);
  let authenticatedProviderIds = $derived((snapshot?.auth ?? []).map((entry) => entry.providerId));
  let availableProviders = $derived(
    (snapshot?.providers ?? []).filter(
      (provider) =>
        providerIsAvailableForAgent(provider, authenticatedProviderIds)
    )
  );
  let providerLabel = $derived(
    providerEntryFor(activeProvider)?.displayName ??
      activeProvider
  );

  let currentProviderEntry = $derived(
    providerEntryFor(activeProvider)
  );
  let currentProviderModels = $derived(
    modelsByProvider[activeProvider] ?? modelsByProvider[currentProviderEntry?.id ?? ""] ?? []
  );

  // Filter models only within the selected provider. Provider switching is
  // explicit via the provider row above the model list.
  let filteredEntries = $derived.by(() => {
    const needle = query.trim().toLowerCase();
    const out: { provider: string; providerLabel: string; model: ModelDescriptorInfo }[] = [];
    const provider = availableProviders.find((entry) => providerIdsEquivalent(entry.id, activeProvider));
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
    const generation = ++modelLoadGeneration;
    busy = true;
    loadError = null;
    try {
      const next: Record<string, ModelDescriptorInfo[]> = { ...modelsByProvider };
      const provider = currentProviderEntry ??
        availableProviders.find((entry) => providerIdsEquivalent(entry.id, currentProvider));
      const providers = provider ? [provider] : [];
      for (const provider of providers) {
        try {
          next[provider.id] = await listProviderModels(provider.id);
        } catch (error) {
          if (!Object.prototype.hasOwnProperty.call(next, provider.id)) {
            next[provider.id] = [];
          }
          loadError = `${provider.id}: ${error}`;
        }
      }
      if (generation !== modelLoadGeneration) return;
      modelsByProvider = next;
    } finally {
      if (generation === modelLoadGeneration) {
        busy = false;
      }
    }
  }

  function providerEntryFor(providerId: string | null | undefined) {
    const normalized = providerId?.trim().toLowerCase();
    if (!normalized) return null;
    const exact = availableProviders.find(
      (provider) => provider.id.trim().toLowerCase() === normalized
    );
    if (exact) return exact;
    return availableProviders.find((provider) => providerIdsEquivalent(provider.id, normalized)) ?? null;
  }

  async function selectProvider(providerId: string) {
    if (!allowProviderSwitch || disabled) return;
    if (providerIdsEquivalent(providerId, activeProvider)) return;
    const generation = ++providerSwitchGeneration;
    if (providerSwitchRollback === null) {
      providerSwitchRollback = { providerId: currentProvider, modelId: currentModel };
    }
    const rollback = providerSwitchRollback;
    modelLoadGeneration += 1;
    pendingProviderId = providerId;
    query = "";
    onChange(providerId, "");
    let models: ModelDescriptorInfo[] = [];
    busy = true;
    loadError = null;
    try {
      models = await listProviderModels(providerId);
      if (generation !== providerSwitchGeneration) return;
      modelsByProvider = { ...modelsByProvider, [providerId]: models };
    } catch (error) {
      if (generation !== providerSwitchGeneration) return;
      modelsByProvider = { ...modelsByProvider, [providerId]: [] };
      loadError = `${providerId}: ${error}`;
      onChange(rollback.providerId, rollback.modelId);
      providerSwitchRollback = null;
      pendingProviderId = null;
      return;
    } finally {
      if (generation === providerSwitchGeneration) {
        busy = false;
      }
    }
    if (generation !== providerSwitchGeneration) return;
    const defaultModel =
      models.find((model) => model.isDefault && modelSupportsAgentTools(model)) ??
      models.find(modelSupportsAgentTools);
    onChange(providerId, defaultModel?.id ?? "");
    providerSwitchRollback = null;
    pendingProviderId = null;
  }

  function modelSupportsAgentTools(model: ModelDescriptorInfo): boolean {
    return model.supportsTools !== false;
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
    const providerModels = modelsByProvider[providerId] ?? [];
    const model = providerModels.find((entry) => entry.id === modelId);
    if (model && !modelSupportsAgentTools(model)) return;
    pendingProviderId = null;
    providerSwitchRollback = null;
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

  function handleDocumentKeydown(event: KeyboardEvent) {
    if (!open || event.key !== "Escape") return;
    event.preventDefault();
    event.stopPropagation();
    open = false;
    triggerEl?.focus();
  }

  $effect(() => {
    const nextContextKey = contextKey ?? null;
    if (nextContextKey === activeContextKey) return;
    activeContextKey = nextContextKey;
    modelLoadGeneration += 1;
    providerSwitchGeneration += 1;
    pendingProviderId = null;
    providerSwitchRollback = null;
    busy = false;
    loadError = null;
    query = "";
    open = false;
  });

  $effect(() => {
    if (typeof document === "undefined") return;
    document.addEventListener("mousedown", handleDocumentClick);
    document.addEventListener("keydown", handleDocumentKeydown);
    return () => {
      document.removeEventListener("mousedown", handleDocumentClick);
      document.removeEventListener("keydown", handleDocumentKeydown);
    };
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
    aria-haspopup="listbox"
    aria-expanded={open}
    title={providerLabel ? `${providerLabel} · ${activeModel || "Pick model"}` : "Pick a model"}
  >
    <Icon name="sparkles" size={11} color="var(--muted-foreground)" />
    <span class="model" class:placeholder={!activeModel}>
      {activeModel || (busy ? "Loading models" : "Pick model")}
    </span>
    {#if providerLabel}
      <span class="provider">{providerLabel}</span>
    {/if}
    <Icon name="chevD" size={10} color="var(--muted-foreground)" />
  </button>

  {#if open}
    <div bind:this={menuEl} class="menu" role="listbox">
      {#if allowProviderSwitch}
        <div class="providers" role="group" aria-label="Model provider">
          {#each availableProviders as provider (provider.id)}
            <button
              type="button"
              class:on={providerIdsEquivalent(provider.id, activeProvider)}
              aria-pressed={providerIdsEquivalent(provider.id, activeProvider)}
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
          <div class="hint">Loading {providerLabel || "provider"} models…</div>
        {:else if filteredEntries.length === 0}
          {#if availableProviders.length === 0}
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
              providerIdsEquivalent(entry.provider, activeProvider) && entry.model.id === activeModel}
            {@const supportsAgentTools = modelSupportsAgentTools(entry.model)}
            <button
              type="button"
              class="row"
              class:on={isCurrent}
              class:unsupported={!supportsAgentTools}
              disabled={!supportsAgentTools}
              onclick={() => pick(entry.provider, entry.model.id)}
              role="option"
              aria-selected={isCurrent}
              title={supportsAgentTools ? entry.model.id : `${entry.model.id} does not support agent tools`}
            >
              <span class="row-main">
                <span class="row-name">{entry.model.displayName || entry.model.id}</span>
                <span class="row-provider">
                  {entry.providerLabel}{supportsAgentTools ? "" : " · No agent tools"}
                </span>
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
    line-height: 1.2;
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
  .row:disabled {
    cursor: not-allowed;
    opacity: 0.58;
  }
  .row.unsupported:hover {
    background: transparent;
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

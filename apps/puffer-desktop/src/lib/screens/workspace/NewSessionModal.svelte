<script lang="ts">
  import Icon from "../../design/Icon.svelte";
  import { focusTrap } from "../../focusTrap";
  import { providerCatalogForSetup } from "../../providerFallbacks";
  import {
    canonicalDaemonProviderId,
    providerCanRunAgent,
    providerIsAvailableForAgent,
    providerIdsEquivalent
  } from "../../providerIds";
  import type { ProviderSummary, SettingsSnapshot } from "../../types";

  type Props = {
    cwd: string;
    snapshot: SettingsSnapshot | null;
    busy?: boolean;
    error?: string | null;
    onClose: () => void;
    onCreate: (providerId: string) => void | Promise<void>;
  };

  let { cwd, snapshot, busy = false, error = null, onClose, onCreate }: Props = $props();
  let selectedProvider = $state("");
  let authenticatedProviderIds = $derived((snapshot?.auth ?? []).map((entry) => entry.providerId));

  let providerOptions = $derived(
    providerCatalogForSetup(snapshot).filter((provider) =>
      snapshot === null
        ? providerCanRunAgent(provider)
        : providerIsAvailableForAgent(provider, authenticatedProviderIds)
    )
  );

  function defaultProviderId(): string {
    const configured = snapshot?.config.defaultProvider;
    const configuredProvider = providerOptions.find((provider) =>
      providerIdsEquivalent(provider.id, configured)
    );
    if (configuredProvider) {
      return configuredProvider.id;
    }
    return providerOptions[0]?.id ?? "openai";
  }

  $effect(() => {
    if (!providerOptions.some((provider) => provider.id === selectedProvider)) {
      selectedProvider = defaultProviderId();
    }
  });

  function providerDetail(provider: ProviderSummary): string {
    if (provider.id === "codex" || provider.id === "openai") return "OpenAI Codex CLI";
    if (provider.id === "claude" || provider.id === "anthropic") return "Claude Code CLI";
    if (provider.id === "puffer") return "Puffer CLI";
    return provider.defaultApi ? `${provider.defaultApi} provider` : "Model provider";
  }

  $effect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !busy) onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });
</script>

<div
  class="pf-modal-scrim"
  onclick={() => { if (!busy) onClose(); }}
  role="presentation"
  onkeydown={() => {}}
>
  <div
    class="pf-modal pf-new-session-modal"
    onclick={(event) => event.stopPropagation()}
    role="dialog"
    aria-label="New agent"
    aria-modal="true"
    tabindex="-1"
    use:focusTrap
    onkeydown={() => {}}
  >
    <div class="pf-modal-head">
      <div class="pf-modal-title-group">
        <div class="pf-modal-title">Choose provider</div>
      </div>
      <button type="button" class="pf-modal-close" onclick={onClose} aria-label="Close" disabled={busy}>
        <Icon name="x" size={14} />
      </button>
    </div>

    <div class="pf-modal-body">
      <div class="pf-provider-choice" role="radiogroup" aria-label="Agent provider">
        {#each providerOptions as provider (provider.id)}
          <label
            class="pf-provider-choice-btn"
            data-active={selectedProvider === provider.id}
          >
            <input
              class="pf-provider-choice-input"
              type="radio"
              name="new-agent-provider"
              value={provider.id}
              checked={selectedProvider === provider.id}
              onchange={(event) => {
                event.stopPropagation();
                selectedProvider = provider.id;
              }}
              disabled={busy}
            />
            <span class="pf-provider-dot" data-provider={provider.id}></span>
            <span class="pf-provider-copy">
              <span class="name">{provider.displayName}</span>
              <span class="meta">{providerDetail(provider)}</span>
            </span>
          </label>
        {/each}
      </div>
      {#if providerOptions.length === 0}
        <div class="pf-field-hint">Connect a provider in Settings before starting an agent.</div>
      {/if}
      <div class="pf-field-hint">
        Session root: <span class="pf-mono">{cwd}</span>
      </div>
      {#if error}
        <div class="pf-new-session-error" role="alert" aria-live="assertive">{error}</div>
      {/if}
    </div>

    <div class="pf-modal-foot">
      <div class="pf-modal-foot-btns">
        <button type="button" class="sc-btn" data-variant="ghost" onclick={onClose} disabled={busy}>
          Cancel
        </button>
        <button
          type="button"
          class="sc-btn"
          data-variant="default"
          onclick={() => onCreate(canonicalDaemonProviderId(selectedProvider || defaultProviderId()))}
          disabled={busy || providerOptions.length === 0}
        >
          <Icon name="plus" size={13} />Start agent
        </button>
      </div>
    </div>
  </div>
</div>

<style>
  .pf-new-session-modal {
    width: min(480px, calc(100vw - 28px));
  }
  .pf-provider-choice {
    display: grid;
    grid-template-columns: 1fr;
    gap: 8px;
  }
  .pf-provider-choice-btn {
    min-height: 56px;
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 12px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--background);
    color: var(--foreground);
    text-align: left;
    cursor: pointer;
    position: relative;
    transition: background 100ms, border-color 100ms, box-shadow 100ms;
  }
  .pf-provider-choice-input {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    margin: 0;
    opacity: 0;
    cursor: inherit;
  }
  .pf-provider-choice-btn:has(.pf-provider-choice-input:disabled) {
    cursor: not-allowed;
    opacity: 0.6;
  }
  .pf-provider-choice-btn:hover:not(:has(.pf-provider-choice-input:disabled)) {
    background: var(--accent);
  }
  .pf-provider-choice-btn:focus-within {
    outline: 2px solid color-mix(in oklab, var(--accent) 70%, transparent);
    outline-offset: 2px;
  }
  .pf-provider-choice-btn[data-active="true"] {
    border-color: var(--foreground);
    box-shadow: 0 0 0 1px var(--foreground) inset;
  }
  .pf-provider-dot {
    width: 10px;
    height: 10px;
    border-radius: 999px;
    background: #2563eb;
    flex-shrink: 0;
    pointer-events: none;
  }
  .pf-provider-dot[data-provider="claude"] {
    background: #7c3aed;
  }
  .pf-provider-dot[data-provider="anthropic"] {
    background: #7c3aed;
  }
  .pf-provider-dot[data-provider="puffer"] {
    background: #15803d;
  }
  .pf-provider-copy {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 3px;
    pointer-events: none;
  }
  .pf-provider-copy .name {
    font-size: 13px;
    font-weight: 650;
  }
  .pf-provider-copy .meta {
    font-size: 12px;
    color: var(--muted-foreground);
  }
  .pf-new-session-error {
    font-size: 12px;
    line-height: 1.4;
    padding: 8px 10px;
    border-radius: 8px;
    background: color-mix(in oklab, oklch(0.7 0.18 25) 12%, var(--background));
    color: oklch(0.5 0.2 25);
    border: 1px solid color-mix(in oklab, oklch(0.7 0.18 25) 30%, var(--border));
  }
</style>

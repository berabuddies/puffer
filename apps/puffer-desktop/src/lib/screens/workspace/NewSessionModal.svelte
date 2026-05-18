<script lang="ts">
  import Icon from "../../design/Icon.svelte";
  import { canonicalDaemonProviderId } from "../../providerIds";
  import type { ProviderSummary, SettingsSnapshot } from "../../types";

  type Props = {
    cwd: string;
    snapshot: SettingsSnapshot | null;
    busy?: boolean;
    onClose: () => void;
    onCreate: (providerId: string) => void | Promise<void>;
  };

  let { cwd, snapshot, busy = false, onClose, onCreate }: Props = $props();
  let selectedProvider = $state("");

  const fallbackProviders: ProviderSummary[] = [
    {
      id: "openai",
      displayName: "Codex",
      baseUrl: "https://api.openai.com",
      defaultApi: "openai-responses",
      modelCount: 1,
      authModes: ["api_key", "oauth"],
      sourceKind: "builtin",
      sourcePath: null
    },
    {
      id: "anthropic",
      displayName: "Anthropic",
      baseUrl: "https://api.anthropic.com",
      defaultApi: "anthropic-messages",
      modelCount: 1,
      authModes: ["api_key", "oauth"],
      sourceKind: "builtin",
      sourcePath: null
    },
    {
      id: "puffer",
      displayName: "Puffer",
      baseUrl: "local-cli://puffer",
      defaultApi: "cli",
      modelCount: 1,
      authModes: ["native"],
      sourceKind: "builtin",
      sourcePath: null
    }
  ];
  const personalProviderIds = new Set(["codex", "openai", "claude", "anthropic", "puffer"]);
  let authenticatedProviderIds = $derived(
    new Set((snapshot?.auth ?? []).map((entry) => entry.providerId))
  );

  let providerOptions = $derived(
    (snapshot?.providers?.length ? snapshot.providers : fallbackProviders).filter((provider) =>
      personalProviderIds.has(provider.id) &&
      (snapshot === null || authenticatedProviderIds.has(provider.id))
    )
  );

  function defaultProviderId(): string {
    const configured = snapshot?.config.defaultProvider;
    if (configured && providerOptions.some((provider) => provider.id === configured)) {
      return configured;
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
    return "Puffer CLI";
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
    onkeydown={() => {}}
  >
    <div class="pf-modal-head">
      <div class="pf-modal-title-group">
        <div class="pf-modal-eyebrow">New agent</div>
        <div class="pf-modal-title">Choose provider</div>
      </div>
      <button type="button" class="pf-modal-close" onclick={onClose} aria-label="Close" disabled={busy}>
        <Icon name="x" size={14} />
      </button>
    </div>

    <div class="pf-modal-body">
      <div class="pf-provider-choice" role="radiogroup" aria-label="Agent provider">
        {#each providerOptions as provider (provider.id)}
          <button
            type="button"
            class="pf-provider-choice-btn"
            data-active={selectedProvider === provider.id}
            role="radio"
            aria-checked={selectedProvider === provider.id}
            onclick={() => (selectedProvider = provider.id)}
            disabled={busy}
          >
            <span class="pf-provider-dot" data-provider={provider.id}></span>
            <span class="pf-provider-copy">
              <span class="name">{provider.displayName}</span>
              <span class="meta">{providerDetail(provider)}</span>
            </span>
          </button>
        {/each}
      </div>
      <div class="pf-field-hint">
        Session root: <span class="pf-mono">{cwd}</span>
      </div>
    </div>

    <div class="pf-modal-foot">
      <div class="pf-modal-foot-hint">
        Provider is saved on this session and used for every turn in it.
      </div>
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
  }
  .pf-provider-choice-btn:hover:not(:disabled) {
    background: var(--accent);
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
  }
  .pf-provider-copy .name {
    font-size: 13px;
    font-weight: 650;
  }
  .pf-provider-copy .meta {
    font-size: 12px;
    color: var(--muted-foreground);
  }
</style>

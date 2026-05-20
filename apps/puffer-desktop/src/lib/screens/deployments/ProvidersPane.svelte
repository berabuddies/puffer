<script lang="ts">
  import { onDestroy } from "svelte";
  import Icon, { type IconName } from "../../design/Icon.svelte";
  import { INTEGRATIONS, type Deployment, type Integration } from "../../data/mockDeployments";

  type Props = { d: Deployment };
  let { d }: Props = $props();

  const providerKinds = ["postgres", "redis", "stripe", "sentry", "github", "slack", "s3", "openai", "webhook"];

  let baseItems = $derived(INTEGRATIONS[d.id] ?? INTEGRATIONS["d-prod-api"]);
  let draftItems = $state<Record<string, Integration[]>>({});
  let providerOverrides = $state<Record<string, Record<string, Integration>>>({});
  let deploymentOverrides = $derived(providerOverrides[d.id] ?? {});
  let items = $derived(
    [...(draftItems[d.id] ?? []), ...baseItems].map((provider) =>
      deploymentOverrides[provider.name] ?? provider
    )
  );
  let addProviderOpen = $state(false);
  let providerName = $state("");
  let providerKind = $state("webhook");
  let providerNote = $state("");
  let providerStatus = $state<Integration["status"]>("connected");
  let providerNameInput = $state<HTMLInputElement | null>(null);
  let editingProviderName = $state<string | null>(null);
  let editProviderKind = $state("webhook");
  let editProviderNote = $state("");
  let editProviderStatus = $state<Integration["status"]>("connected");
  let statusMessage = $state("");
  let statusDeploymentId = $state("");
  let statusTimer = 0;
  let canAddProvider = $derived(
    providerName.trim().length > 0 &&
      providerNote.trim().length > 0 &&
      !items.some((provider) => provider.name.toLowerCase() === providerName.trim().toLowerCase())
  );
  let canSaveProviderSettings = $derived(Boolean(editingProviderName && editProviderNote.trim()));

  const providerIcon: Record<string, IconName> = {
    postgres: "server", redis: "server", stripe: "coin", sentry: "bug",
    github: "git", slack: "plug", s3: "layers", openai: "sparkles", webhook: "link"
  };

  onDestroy(() => {
    if (statusTimer) window.clearTimeout(statusTimer);
  });

  $effect(() => {
    const deploymentId = d.id;
    if (deploymentId === statusDeploymentId) return;
    statusDeploymentId = deploymentId;
    resetAddProvider();
    resetProviderSettings();
    statusMessage = "";
    if (statusTimer) window.clearTimeout(statusTimer);
    statusTimer = 0;
  });

  $effect(() => {
    const sourceItems = [...(draftItems[d.id] ?? []), ...baseItems];
    const existing = editingProviderName
      ? sourceItems.some((provider) => provider.name === editingProviderName)
      : true;
    if (!existing) resetProviderSettings();
  });

  function showStatus(message: string): void {
    statusMessage = message;
    if (statusTimer) window.clearTimeout(statusTimer);
    statusTimer = window.setTimeout(() => {
      statusMessage = "";
      statusTimer = 0;
    }, 4000);
  }

  function resetAddProvider(): void {
    addProviderOpen = false;
    providerName = "";
    providerKind = "webhook";
    providerNote = "";
    providerStatus = "connected";
  }

  function openAddProvider(): void {
    resetAddProvider();
    resetProviderSettings();
    addProviderOpen = true;
    window.setTimeout(() => providerNameInput?.focus({ preventScroll: true }), 20);
  }

  function resetProviderSettings(): void {
    editingProviderName = null;
    editProviderKind = "webhook";
    editProviderNote = "";
    editProviderStatus = "connected";
  }

  function openProviderSettings(provider: Integration): void {
    resetAddProvider();
    editingProviderName = provider.name;
    editProviderKind = provider.kind;
    editProviderNote = provider.note;
    editProviderStatus = provider.status;
  }

  function createProvider(): void {
    if (!canAddProvider) return;
    const name = providerName.trim();
    const next: Integration = {
      kind: providerKind,
      name,
      note: providerNote.trim(),
      status: providerStatus
    };
    draftItems = {
      ...draftItems,
      [d.id]: [next, ...(draftItems[d.id] ?? [])]
    };
    showStatus(`Added ${name} provider to ${d.name}.`);
    resetAddProvider();
  }

  function saveProviderSettings(): void {
    if (!canSaveProviderSettings || !editingProviderName) return;
    const name = editingProviderName;
    const next: Integration = {
      kind: editProviderKind,
      name,
      note: editProviderNote.trim(),
      status: editProviderStatus
    };
    providerOverrides = {
      ...providerOverrides,
      [d.id]: {
        ...(providerOverrides[d.id] ?? {}),
        [name]: next
      }
    };
    showStatus(`Updated ${name} provider settings for ${d.name}.`);
    resetProviderSettings();
  }
</script>

<div class="pf-dep-pane">
  <div class="pf-dep-pane-head">
    <div>
      <h3>Providers &amp; integrations</h3>
      <p class="sub">External services this deployment talks to. Connection strings are injected at build time.</p>
    </div>
    <div class="pf-dep-pane-actions">
      {#if statusMessage}
        <div class="pf-dep-pane-status" role="status" aria-live="polite">
          {statusMessage}
        </div>
      {/if}
      <button type="button" class="sc-btn" data-variant="default" data-size="sm" onclick={openAddProvider}>
        <Icon name="plus" size={12} />Add provider
      </button>
    </div>
  </div>
  {#if editingProviderName}
    <form
      class="pf-dep-prov-form"
      aria-label={`Edit ${editingProviderName} provider settings`}
      onsubmit={(event) => {
        event.preventDefault();
        saveProviderSettings();
      }}
    >
      <label>
        <span>Name</span>
        <input aria-label="Provider name" value={editingProviderName} readonly />
      </label>
      <label>
        <span>Type</span>
        <select aria-label="Provider type" bind:value={editProviderKind}>
          {#each providerKinds as kind (kind)}
            <option value={kind}>{kind}</option>
          {/each}
        </select>
      </label>
      <label>
        <span>Status</span>
        <select aria-label="Provider status" bind:value={editProviderStatus}>
          <option value="connected">connected</option>
          <option value="degraded">degraded</option>
        </select>
      </label>
      <label class="wide">
        <span>Connection note</span>
        <input
          aria-label="Provider connection note"
          value={editProviderNote}
          oninput={(event) => (editProviderNote = event.currentTarget.value)}
        />
      </label>
      <div class="pf-dep-prov-form-actions">
        <button type="button" class="sc-btn" data-variant="ghost" data-size="sm" onclick={resetProviderSettings}>
          Cancel
        </button>
        <button type="submit" class="sc-btn" data-variant="default" data-size="sm" disabled={!canSaveProviderSettings}>
          Save settings
        </button>
      </div>
    </form>
  {/if}
  {#if addProviderOpen}
    <form
      class="pf-dep-prov-form"
      aria-label="Add deployment provider"
      onsubmit={(event) => {
        event.preventDefault();
        createProvider();
      }}
    >
      <label>
        <span>Name</span>
        <input
          bind:this={providerNameInput}
          aria-label="Provider name"
          value={providerName}
          placeholder="Webhook relay"
          oninput={(event) => (providerName = event.currentTarget.value)}
        />
      </label>
      <label>
        <span>Type</span>
        <select aria-label="Provider type" bind:value={providerKind}>
          {#each providerKinds as kind (kind)}
            <option value={kind}>{kind}</option>
          {/each}
        </select>
      </label>
      <label>
        <span>Status</span>
        <select aria-label="Provider status" bind:value={providerStatus}>
          <option value="connected">connected</option>
          <option value="degraded">degraded</option>
        </select>
      </label>
      <label class="wide">
        <span>Connection note</span>
        <input
          aria-label="Provider connection note"
          value={providerNote}
          placeholder="https://hooks.example.com/live"
          oninput={(event) => (providerNote = event.currentTarget.value)}
        />
      </label>
      <div class="pf-dep-prov-form-actions">
        <button type="button" class="sc-btn" data-variant="ghost" data-size="sm" onclick={resetAddProvider}>
          Cancel
        </button>
        <button type="submit" class="sc-btn" data-variant="default" data-size="sm" disabled={!canAddProvider}>
          Add provider
        </button>
      </div>
    </form>
  {/if}
  <div class="pf-dep-provs">
    {#each items as p (p.name)}
      <div class="pf-dep-prov">
        <div class="pf-dep-prov-ico">
          <Icon name={providerIcon[p.kind] ?? "plug"} size={16} />
        </div>
        <div class="pf-dep-prov-body">
          <div class="pf-dep-prov-name">{p.name}</div>
          <div class="pf-dep-prov-note">{p.note}</div>
        </div>
        <span class="pf-dep-prov-status" data-state={p.status === "connected" ? "healthy" : "degraded"}>
          <span class="dot"></span>{p.status}
        </span>
        <button
          type="button"
          class="pf-dep-ico"
          aria-label={`Edit ${p.name} provider settings`}
          title={`Edit ${p.name} provider settings`}
          aria-pressed={editingProviderName === p.name}
          onclick={() => openProviderSettings(p)}
        >
          <Icon name="settings" size={12} />
        </button>
      </div>
    {/each}
  </div>
</div>

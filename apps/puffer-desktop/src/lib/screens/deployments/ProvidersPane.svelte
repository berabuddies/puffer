<script lang="ts">
  import { onDestroy } from "svelte";
  import Icon, { type IconName } from "../../design/Icon.svelte";
  import { INTEGRATIONS, type Deployment, type Integration } from "../../data/mockDeployments";

  type Props = { d: Deployment };
  let { d }: Props = $props();

  const providerKinds = ["postgres", "redis", "stripe", "sentry", "github", "slack", "s3", "openai", "webhook"];

  let baseItems = $derived(INTEGRATIONS[d.id] ?? INTEGRATIONS["d-prod-api"]);
  let draftItems = $state<Record<string, Integration[]>>({});
  let items = $derived([...(draftItems[d.id] ?? []), ...baseItems]);
  let addProviderOpen = $state(false);
  let providerName = $state("");
  let providerKind = $state("webhook");
  let providerNote = $state("");
  let providerStatus = $state<Integration["status"]>("connected");
  let providerNameInput = $state<HTMLInputElement | null>(null);
  let statusMessage = $state("");
  let statusDeploymentId = $state("");
  let statusTimer = 0;
  let canAddProvider = $derived(
    providerName.trim().length > 0 &&
      providerNote.trim().length > 0 &&
      !items.some((provider) => provider.name.toLowerCase() === providerName.trim().toLowerCase())
  );

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
    statusMessage = "";
    if (statusTimer) window.clearTimeout(statusTimer);
    statusTimer = 0;
  });

  function resetAddProvider(): void {
    addProviderOpen = false;
    providerName = "";
    providerKind = "webhook";
    providerNote = "";
    providerStatus = "connected";
  }

  function openAddProvider(): void {
    resetAddProvider();
    addProviderOpen = true;
    window.setTimeout(() => providerNameInput?.focus({ preventScroll: true }), 20);
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
    statusMessage = `Added ${name} provider to ${d.name}.`;
    if (statusTimer) window.clearTimeout(statusTimer);
    statusTimer = window.setTimeout(() => {
      statusMessage = "";
      statusTimer = 0;
    }, 4000);
    resetAddProvider();
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
        <button type="button" class="pf-dep-ico" aria-label="Settings">
          <Icon name="settings" size={12} />
        </button>
      </div>
    {/each}
  </div>
</div>

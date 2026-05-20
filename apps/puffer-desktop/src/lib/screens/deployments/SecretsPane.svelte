<script lang="ts">
  import { onDestroy } from "svelte";
  import Icon from "../../design/Icon.svelte";
  import { SECRETS, type Deployment, type Secret } from "../../data/mockDeployments";

  type Props = { d: Deployment };
  let { d }: Props = $props();

  let baseSecrets = $derived(SECRETS[d.id] ?? SECRETS["d-prod-api"]);
  let draftSecrets = $state<Record<string, Secret[]>>({});
  let secrets = $derived([...(draftSecrets[d.id] ?? []), ...baseSecrets]);
  let revealed = $state<Record<string, boolean>>({});
  let syncState = $state<"idle" | "syncing" | "synced">("idle");
  let syncMessage = $state("");
  let syncTimer = 0;
  let statusDeploymentId = $state("");
  let addSecretOpen = $state(false);
  let newSecretKey = $state("");
  let newSecretPreview = $state("");
  let newSecretScope = $state<Secret["scope"]>("runtime");
  let newSecretKeyInput = $state<HTMLInputElement | null>(null);
  let canAddSecret = $derived(
    newSecretKey.trim().length > 0 &&
      newSecretPreview.trim().length > 0 &&
      !secrets.some((secret) => secret.key.toLowerCase() === newSecretKey.trim().toLowerCase())
  );

  onDestroy(() => {
    if (syncTimer) window.clearTimeout(syncTimer);
  });

  $effect(() => {
    const deploymentId = d.id;
    if (deploymentId === statusDeploymentId) return;
    statusDeploymentId = deploymentId;
    if (syncTimer) window.clearTimeout(syncTimer);
    syncTimer = 0;
    syncState = "idle";
    syncMessage = "";
    addSecretOpen = false;
    newSecretKey = "";
    newSecretPreview = "";
    newSecretScope = "runtime";
    revealed = {};
  });

  function toggle(key: string) {
    revealed = { ...revealed, [key]: !revealed[key] };
  }

  function syncSecrets(): void {
    if (syncTimer) window.clearTimeout(syncTimer);
    const deploymentId = d.id;
    const deploymentName = d.name;
    const keyCount = secrets.length;
    statusDeploymentId = deploymentId;
    syncState = "syncing";
    syncMessage = `Syncing ${deploymentName} secrets with Vault...`;
    syncTimer = window.setTimeout(() => {
      if (statusDeploymentId !== deploymentId) return;
      syncState = "synced";
      syncMessage = `Secrets synced: ${keyCount} keys refreshed for ${deploymentName}.`;
      syncTimer = 0;
    }, 250);
  }

  function openAddSecret(): void {
    addSecretOpen = true;
    newSecretKey = "";
    newSecretPreview = "";
    newSecretScope = "runtime";
    window.setTimeout(() => newSecretKeyInput?.focus({ preventScroll: true }), 20);
  }

  function closeAddSecret(): void {
    addSecretOpen = false;
  }

  function createSecret(): void {
    if (!canAddSecret) return;
    const key = newSecretKey.trim().toUpperCase().replace(/[^A-Z0-9_]+/g, "_");
    const preview = newSecretPreview.trim();
    const next: Secret = {
      key,
      preview,
      scope: newSecretScope,
      updated: "just now",
      by: "Otter"
    };
    draftSecrets = {
      ...draftSecrets,
      [d.id]: [next, ...(draftSecrets[d.id] ?? [])]
    };
    syncMessage = `Added ${key} to ${d.name}.`;
    syncState = "synced";
    closeAddSecret();
  }
</script>

<div class="pf-dep-pane">
  <div class="pf-dep-pane-head">
    <div>
      <h3>Secrets &amp; env</h3>
      <p class="sub">{secrets.length} keys · synced to Vault · masked for all roles except <code>owner</code></p>
    </div>
    <div class="pf-dep-pane-actions">
      {#if syncMessage}
        <div class="pf-dep-pane-status" role="status" aria-live="polite" data-state={syncState}>
          {syncMessage}
        </div>
      {/if}
      <button
        type="button"
        class="sc-btn"
        data-variant="ghost"
        data-size="sm"
        aria-label="Sync secrets"
        aria-busy={syncState === "syncing"}
        disabled={syncState === "syncing"}
        onclick={syncSecrets}
      >
        <Icon name="refresh" size={12} />{syncState === "syncing" ? "Syncing" : "Sync"}
      </button>
      <button type="button" class="sc-btn" data-variant="default" data-size="sm" onclick={openAddSecret}>
        <Icon name="plus" size={12} />Add secret
      </button>
    </div>
  </div>

  {#if addSecretOpen}
    <form
      class="pf-dep-secret-form"
      aria-label="Add deployment secret"
      onsubmit={(event) => {
        event.preventDefault();
        createSecret();
      }}
    >
      <label>
        <span>Key</span>
        <input
          bind:this={newSecretKeyInput}
          aria-label="Secret key"
          value={newSecretKey}
          placeholder="WEBHOOK_TOKEN"
          oninput={(event) => (newSecretKey = event.currentTarget.value)}
        />
      </label>
      <label>
        <span>Preview value</span>
        <input
          aria-label="Secret preview value"
          value={newSecretPreview}
          placeholder="tok_live_..."
          oninput={(event) => (newSecretPreview = event.currentTarget.value)}
        />
      </label>
      <label>
        <span>Scope</span>
        <select aria-label="Secret scope" bind:value={newSecretScope}>
          <option value="runtime">runtime</option>
          <option value="build">build</option>
        </select>
      </label>
      <div class="pf-dep-secret-form-actions">
        <button type="button" class="sc-btn" data-variant="ghost" data-size="sm" onclick={closeAddSecret}>
          Cancel
        </button>
        <button type="submit" class="sc-btn" data-variant="default" data-size="sm" disabled={!canAddSecret}>
          Add secret
        </button>
      </div>
    </form>
  {/if}

  <div class="pf-dep-secrets">
    <div class="pf-dep-secrets-head">
      <span>Key</span>
      <span>Value</span>
      <span>Scope</span>
      <span>Last rotated</span>
      <span></span>
    </div>
    {#each secrets as s (s.key)}
      {@const secretRevealed = revealed[s.key] === true}
      <div class="pf-dep-secrets-row" data-rotate={s.rotate ?? false}>
        <span class="mono key">
          <Icon name="key" size={11} color="var(--muted-foreground)" />{s.key}
        </span>
        <span class="mono val">
          {secretRevealed ? s.preview : "••••••••••••••"}
          <button
            type="button"
            class="pf-dep-ico"
            onclick={() => toggle(s.key)}
            aria-label={`${secretRevealed ? "Hide" : "Reveal"} ${s.key}`}
            aria-pressed={secretRevealed}
            title={`${secretRevealed ? "Hide" : "Reveal"} ${s.key}`}
          >
            <Icon name={secretRevealed ? "eyeOff" : "eye"} size={11} />
          </button>
        </span>
        <span class="pf-dep-scope" data-scope={s.scope}>{s.scope}</span>
        <span class="sub">{s.updated} · {s.by}</span>
        <div class="pf-dep-secrets-actions">
          {#if s.rotate}
            <span class="pf-dep-rotate-chip">needs rotation</span>
          {/if}
          <button type="button" class="pf-dep-ico" title="More" aria-label="More">
            <Icon name="moreH" size={11} />
          </button>
        </div>
      </div>
    {/each}
  </div>
</div>

<script lang="ts">
  import { onDestroy } from "svelte";
  import Icon from "../../design/Icon.svelte";
  import { SECRETS, type Deployment } from "../../data/mockDeployments";

  type Props = { d: Deployment };
  let { d }: Props = $props();

  let secrets = $derived(SECRETS[d.id] ?? SECRETS["d-prod-api"]);
  let revealed = $state<Record<string, boolean>>({});
  let syncState = $state<"idle" | "syncing" | "synced">("idle");
  let syncMessage = $state("");
  let syncTimer = 0;
  let statusDeploymentId = $state("");

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
      <button type="button" class="sc-btn" data-variant="default" data-size="sm">
        <Icon name="plus" size={12} />Add secret
      </button>
    </div>
  </div>

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

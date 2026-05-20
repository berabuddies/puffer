<script lang="ts">
  import Icon from "../../design/Icon.svelte";
  import { SECRETS, type Deployment } from "../../data/mockDeployments";

  type Props = { d: Deployment };
  let { d }: Props = $props();

  let secrets = $derived(SECRETS[d.id] ?? SECRETS["d-prod-api"]);
  let revealed = $state<Record<string, boolean>>({});

  function toggle(key: string) {
    revealed = { ...revealed, [key]: !revealed[key] };
  }
</script>

<div class="pf-dep-pane">
  <div class="pf-dep-pane-head">
    <div>
      <h3>Secrets &amp; env</h3>
      <p class="sub">{secrets.length} keys · synced to Vault · masked for all roles except <code>owner</code></p>
    </div>
    <div style="display: flex; gap: 6px;">
      <button type="button" class="sc-btn" data-variant="ghost" data-size="sm">
        <Icon name="refresh" size={12} />Sync
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

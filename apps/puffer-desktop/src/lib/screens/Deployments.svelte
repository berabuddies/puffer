<script lang="ts">
  import "../design/chat.css";
  import "../design/deployments.css";

  import Icon, { type IconName } from "../design/Icon.svelte";
  import StatePill from "./deployments/StatePill.svelte";
  import ProviderGlyph from "./deployments/ProviderGlyph.svelte";
  import AskPufferPane from "./deployments/AskPufferPane.svelte";
  import MemoryPane from "./deployments/MemoryPane.svelte";
  import SecretsPane from "./deployments/SecretsPane.svelte";
  import ProvidersPane from "./deployments/ProvidersPane.svelte";
  import DeploysPane from "./deployments/DeploysPane.svelte";
  import { DEPLOYMENTS } from "../data/mockDeployments";

  type Tab = "askpuffer" | "memory" | "secrets" | "providers" | "deploys";
  let selectedId = $state("d-prod-api");
  let tab = $state<Tab>("askpuffer");

  let selected = $derived(DEPLOYMENTS.find((d) => d.id === selectedId) ?? DEPLOYMENTS[0]);

  const tabs: { id: Tab; label: string; icon: IconName }[] = [
    { id: "askpuffer", label: "Ask Puffer", icon: "sparkles" },
    { id: "memory",    label: "Memory",    icon: "bolt" },
    { id: "secrets",   label: "Secrets",   icon: "key" },
    { id: "providers", label: "Providers", icon: "plug" },
    { id: "deploys",   label: "Deploys",   icon: "rocket" }
  ];

  function select(id: string) {
    selectedId = id;
    tab = "askpuffer";
  }

  function selectTab(id: Tab) {
    tab = id;
  }

  function focusTab(id: Tab) {
    document.querySelector<HTMLButtonElement>(`[data-dep-tab="${id}"]`)?.focus();
  }

  function moveTab(id: Tab, offset: number) {
    const idx = tabs.findIndex((item) => item.id === id);
    if (idx < 0) return;
    const next = tabs[(idx + offset + tabs.length) % tabs.length].id;
    selectTab(next);
    setTimeout(() => focusTab(next), 0);
  }

  function handleTabKeydown(event: KeyboardEvent, id: Tab) {
    if (event.key === "ArrowRight" || event.key === "ArrowDown") {
      event.preventDefault();
      moveTab(id, 1);
    } else if (event.key === "ArrowLeft" || event.key === "ArrowUp") {
      event.preventDefault();
      moveTab(id, -1);
    } else if (event.key === "Home") {
      event.preventDefault();
      const first = tabs[0].id;
      selectTab(first);
      setTimeout(() => focusTab(first), 0);
    } else if (event.key === "End") {
      event.preventDefault();
      const last = tabs[tabs.length - 1].id;
      selectTab(last);
      setTimeout(() => focusTab(last), 0);
    }
  }
</script>

<div class="pf-dep">
  <div class="pf-dep-top">
    <div class="pf-dep-top-title">
      <span class="pf-pipe-chip">Deployments</span>
      <strong>{DEPLOYMENTS.length} environments</strong>
      <span class="pf-dep-top-sub">across 4 providers · 6 workspaces</span>
    </div>
    <div class="pf-dep-top-right">
      <button type="button" class="sc-btn" data-variant="ghost" data-size="sm">
        <Icon name="search" size={12} />Search
      </button>
      <button type="button" class="sc-btn" data-variant="outline" data-size="sm">
        <Icon name="refresh" size={12} />Sync providers
      </button>
      <button type="button" class="sc-btn" data-variant="default" data-size="sm">
        <Icon name="plus" size={12} />New deployment
      </button>
    </div>
  </div>

  <div class="pf-dep-body">
    <div class="pf-dep-list">
      <div class="pf-dep-list-head">
        <span>Environment</span>
        <span>Status</span>
      </div>
      {#each DEPLOYMENTS as d (d.id)}
        <div
          class="pf-dep-row"
          data-selected={selectedId === d.id}
          role="button"
          tabindex="0"
          onclick={() => select(d.id)}
          onkeydown={(e) => {
            if (e.key === "Enter" || e.key === " ") {
              e.preventDefault();
              select(d.id);
            }
          }}
        >
          <div class="pf-dep-row-left">
            <span class="pf-dep-provider-chip" data-provider={d.provider}>
              <ProviderGlyph kind={d.provider} size={14} />
            </span>
            <div class="pf-dep-row-title">
              <div class="pf-dep-row-name">{d.name}</div>
              <div class="pf-dep-row-sub">
                <span class="pf-dep-row-url-inline">
                  <Icon name="globe" size={10} color="var(--muted-foreground)" />
                  <span class="mono">{d.url}</span>
                </span>
                <span class="sep">·</span>
                <span>{d.providerLabel}</span>
              </div>
              <div class="pf-dep-row-workspaces">
                {#each d.workspaces.slice(0, 3) as w (w.id)}
                  <span class="pf-dep-ws-chip" title={w.name}>{w.name}</span>
                {/each}
                {#if d.workspaces.length > 3}
                  <span class="pf-dep-ws-chip muted">+{d.workspaces.length - 3}</span>
                {/if}
              </div>
            </div>
          </div>
          <div class="pf-dep-row-state">
            <StatePill state={d.state} />
            <div class="pf-dep-row-meta mono">{d.lastDeploy}</div>
          </div>
        </div>
      {/each}
    </div>

    <div class="pf-dep-detail">
      <div class="pf-dep-detail-head">
        <div class="pf-dep-detail-head-left">
          <span class="pf-dep-provider-chip lg" data-provider={selected.provider}>
            <ProviderGlyph kind={selected.provider} size={18} />
          </span>
          <div>
            <div class="pf-dep-detail-name">
              {selected.name}
              <StatePill state={selected.state} />
            </div>
            <div class="pf-dep-detail-sub">
              {selected.providerLabel} · {selected.region} · <span class="mono">{selected.url}</span>
            </div>
          </div>
        </div>
        <div class="pf-dep-detail-head-right">
          <button type="button" class="sc-btn" data-variant="ghost" data-size="sm">
            <Icon name="external" size={12} />Open
          </button>
          <button type="button" class="sc-btn" data-variant="outline" data-size="sm">
            <Icon name="refresh" size={12} />Redeploy
          </button>
        </div>
      </div>

      <div class="pf-dep-tabs" role="tablist" aria-label="Deployment detail">
        {#each tabs as t (t.id)}
          <button
            type="button"
            class="pf-dep-tab"
            role="tab"
            id={`dep-tab-${t.id}`}
            data-active={tab === t.id}
            data-dep-tab={t.id}
            aria-selected={tab === t.id}
            aria-controls={`dep-panel-${t.id}`}
            tabindex={tab === t.id ? 0 : -1}
            onclick={() => selectTab(t.id)}
            onkeydown={(event) => handleTabKeydown(event, t.id)}
          >
            <Icon name={t.icon} size={12} />{t.label}
          </button>
        {/each}
      </div>

      <div
        class="pf-dep-pane-wrap"
        role="tabpanel"
        id={`dep-panel-${tab}`}
        aria-labelledby={`dep-tab-${tab}`}
      >
        {#if tab === "askpuffer"}
          <AskPufferPane d={selected} />
        {:else if tab === "memory"}
          <MemoryPane d={selected} />
        {:else if tab === "secrets"}
          <SecretsPane d={selected} />
        {:else if tab === "providers"}
          <ProvidersPane d={selected} />
        {:else if tab === "deploys"}
          <DeploysPane d={selected} />
        {/if}
      </div>
    </div>
  </div>
</div>

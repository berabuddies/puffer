<script lang="ts">
  import { onDestroy } from "svelte";

  import "../design/chat.css";
  import "../design/deployments.css";
  import "../design/workspace.css";

  import Icon, { type IconName } from "../design/Icon.svelte";
  import { focusTrap } from "../focusTrap";
  import StatePill from "./deployments/StatePill.svelte";
  import ProviderGlyph from "./deployments/ProviderGlyph.svelte";
  import AskPufferPane from "./deployments/AskPufferPane.svelte";
  import MemoryPane from "./deployments/MemoryPane.svelte";
  import SecretsPane from "./deployments/SecretsPane.svelte";
  import ProvidersPane from "./deployments/ProvidersPane.svelte";
  import DeploysPane from "./deployments/DeploysPane.svelte";
  import { DEPLOYMENTS, type DeployHistoryItem, type Deployment, type MemoryItem } from "../data/mockDeployments";

  type Tab = "askpuffer" | "memory" | "secrets" | "providers" | "deploys";
  let selectedId = $state("d-prod-api");
  let tab = $state<Tab>("askpuffer");
  let searchOpen = $state(false);
  let deploymentQuery = $state("");
  let searchInput = $state<HTMLInputElement | null>(null);
  let syncState = $state<"idle" | "syncing" | "synced">("idle");
  let syncMessage = $state("");
  let syncTimer = 0;
  let draftDeployments = $state<Deployment[]>([]);
  let showNewDeployment = $state(false);
  let newDeploymentName = $state("");
  let newDeploymentProvider = $state<Deployment["provider"]>("vercel");
  let newDeploymentEnvironment = $state("staging");
  let newDeploymentBranch = $state("main");
  let newDeploymentTrigger = $state<HTMLButtonElement | null>(null);
  let redeployingId = $state<string | null>(null);
  let detailActionStatus = $state("");
  let redeployTimer = 0;
  let redeploySequence = $state(1429);
  let redeployHistory = $state<Record<string, DeployHistoryItem[]>>({});
  let memoryDrafts = $state<Record<string, MemoryItem[]>>({});

  const providerOptions: { id: Deployment["provider"]; label: string; region: string }[] = [
    { id: "vercel", label: "Vercel", region: "iad1 - us-east" },
    { id: "aws", label: "AWS - ECS Fargate", region: "us-east-1" },
    { id: "fly", label: "Fly.io Machines", region: "iad" },
    { id: "railway", label: "Railway", region: "us-east" },
    { id: "cloudflare", label: "Cloudflare Workers", region: "global" },
    { id: "supabase", label: "Supabase - Postgres", region: "us-east-1" }
  ];

  let allDeployments = $derived([...draftDeployments, ...DEPLOYMENTS]);
  let filteredDeployments = $derived.by(() => {
    const query = deploymentQuery.trim().toLowerCase();
    return query ? allDeployments.filter((deployment) => deploymentMatchesQuery(deployment, query)) : allDeployments;
  });
  let selected = $derived(
    filteredDeployments.find((d) => d.id === selectedId)
      ?? allDeployments.find((d) => d.id === selectedId)
      ?? allDeployments[0]
  );
  let selectedForDetail = $derived(filteredDeployments.length === 0 ? null : selected);
  let providerCount = $derived(new Set(allDeployments.map((deployment) => deployment.provider)).size);
  let canCreateDeployment = $derived(newDeploymentName.trim().length > 0);

  const tabs: { id: Tab; label: string; icon: IconName }[] = [
    { id: "askpuffer", label: "Ask Puffer", icon: "sparkles" },
    { id: "memory",    label: "Memory",    icon: "bolt" },
    { id: "secrets",   label: "Secrets",   icon: "key" },
    { id: "providers", label: "Providers", icon: "plug" },
    { id: "deploys",   label: "Deploys",   icon: "rocket" }
  ];
  onDestroy(() => {
    if (syncTimer) window.clearTimeout(syncTimer);
    if (redeployTimer) window.clearTimeout(redeployTimer);
  });

  function select(id: string) {
    selectedId = id;
    tab = "askpuffer";
  }

  function selectTab(id: Tab) {
    tab = id;
  }

  function deploymentMatchesQuery(deployment: Deployment, query: string): boolean {
    const fields = [
      deployment.name,
      deployment.provider,
      deployment.providerLabel,
      deployment.region,
      deployment.url,
      deployment.branch,
      deployment.state,
      deployment.alert ?? "",
      ...deployment.workspaces.flatMap((workspace) => [workspace.name, workspace.role])
    ];
    return fields.some((field) => field.toLowerCase().includes(query));
  }

  function openSearch(): void {
    searchOpen = true;
    setTimeout(() => searchInput?.focus(), 0);
  }

  function closeSearch(): void {
    searchOpen = false;
    deploymentQuery = "";
  }

  function toggleSearch(): void {
    if (searchOpen) {
      closeSearch();
    } else {
      openSearch();
    }
  }

  function handleSearchKeydown(event: KeyboardEvent): void {
    if (event.key !== "Escape") return;
    event.preventDefault();
    closeSearch();
  }

  function syncProviders(): void {
    if (syncTimer) window.clearTimeout(syncTimer);
    syncState = "syncing";
    syncMessage = "Syncing providers...";
    syncTimer = window.setTimeout(() => {
      syncState = "synced";
      syncMessage = `Providers synced: ${allDeployments.length} environments across ${providerCount} providers refreshed.`;
      syncTimer = 0;
    }, 250);
  }

  function slugify(value: string): string {
    return value.trim().toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "") || "deployment";
  }

  function openNewDeployment(): void {
    newDeploymentName = "";
    newDeploymentProvider = "vercel";
    newDeploymentEnvironment = "staging";
    newDeploymentBranch = "main";
    showNewDeployment = true;
  }

  function closeNewDeployment(): void {
    showNewDeployment = false;
    window.requestAnimationFrame(() => {
      if (newDeploymentTrigger?.isConnected) {
        newDeploymentTrigger.focus({ preventScroll: true });
      }
    });
  }

  function createDraftDeployment(): void {
    if (!canCreateDeployment) return;
    const name = newDeploymentName.trim();
    const provider = providerOptions.find((option) => option.id === newDeploymentProvider) ?? providerOptions[0];
    const environment = newDeploymentEnvironment.trim() || "staging";
    const slug = slugify(`${name}-${environment}`);
    const deployment: Deployment = {
      id: `draft-${slug}-${Date.now()}`,
      name: `${name} · ${environment}`,
      provider: provider.id,
      providerLabel: provider.label,
      region: provider.region,
      url: `${slug}.puffer.app`,
      branch: newDeploymentBranch.trim() || "main",
      state: "deploying",
      lastDeploy: "draft",
      lastCommit: "new draft deployment",
      lastDeployer: "Otter",
      workspaces: [{ id: slug, name: slug, role: "service" }],
      envCount: 0,
      integrations: 0,
      metrics: { rps: "-", p95: "-", error: "-" },
      alert: "Draft deployment has not been pushed yet"
    };
    draftDeployments = [deployment, ...draftDeployments];
    selectedId = deployment.id;
    tab = "deploys";
    closeNewDeployment();
  }

  function deploymentPublicUrl(value: string): string | null {
    const trimmed = value.trim();
    if (!trimmed || trimmed === "—") return null;
    return /^https?:\/\//i.test(trimmed) ? trimmed : `https://${trimmed}`;
  }

  function openDeployment(deployment: Deployment = selected): void {
    const url = deploymentPublicUrl(deployment.url);
    if (!url) {
      detailActionStatus = `${deployment.name} has no public URL to open.`;
      return;
    }
    window.open(url, "_blank", "noopener,noreferrer");
    detailActionStatus = `Opened ${deployment.name} at ${url}.`;
  }

  function triggerRedeploy(deployment: Deployment = selected): void {
    if (!deployment || redeployingId) return;
    if (redeployTimer) window.clearTimeout(redeployTimer);
    const nextSequence = redeploySequence + 1;
    redeploySequence = nextSequence;
    const run: DeployHistoryItem = {
      id: `manual-${nextSequence}`,
      commit: deployment.lastCommit,
      branch: deployment.branch || "main",
      deployer: "Otter",
      state: "deploying",
      time: "just now",
      dur: "running",
      current: true
    };
    redeployingId = deployment.id;
    detailActionStatus = `Redeploying ${deployment.name} from ${run.branch}.`;
    redeployHistory = {
      ...redeployHistory,
      [deployment.id]: [run, ...(redeployHistory[deployment.id] ?? [])]
    };
    tab = "deploys";
    redeployTimer = window.setTimeout(() => {
      redeployHistory = {
        ...redeployHistory,
        [deployment.id]: (redeployHistory[deployment.id] ?? []).map((item) =>
          item.id === run.id ? { ...item, state: "healthy", dur: "0m 12s" } : item
        )
      };
      detailActionStatus = `Redeploy complete for ${deployment.name}.`;
      redeployingId = null;
      redeployTimer = 0;
    }, 350);
  }

  function addMemoryDraft(deploymentId: string, item: MemoryItem): void {
    memoryDrafts = {
      ...memoryDrafts,
      [deploymentId]: [item, ...(memoryDrafts[deploymentId] ?? [])]
    };
  }

  $effect(() => {
    if (filteredDeployments.length === 0) return;
    if (filteredDeployments.some((deployment) => deployment.id === selectedId)) return;
    selectedId = filteredDeployments[0].id;
    tab = "askpuffer";
  });

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
      <strong>{allDeployments.length} environments</strong>
      <span class="pf-dep-top-sub">across {providerCount} providers · 6 workspaces</span>
    </div>
    <div class="pf-dep-top-right">
      {#if searchOpen}
        <div class="pf-dep-search">
          <Icon name="search" size={12} color="var(--muted-foreground)" />
          <input
            bind:this={searchInput}
            type="search"
            bind:value={deploymentQuery}
            aria-label="Search deployments"
            placeholder="Search deployments"
            onkeydown={handleSearchKeydown}
          />
          {#if deploymentQuery.trim()}
            <button type="button" aria-label="Clear deployment search" onclick={() => (deploymentQuery = "")}>
              Clear
            </button>
          {/if}
        </div>
      {/if}
      <button type="button" class="sc-btn" data-variant="ghost" data-size="sm" aria-pressed={searchOpen} onclick={toggleSearch}>
        <Icon name="search" size={12} />Search
      </button>
      {#if syncMessage}
        <div class="pf-dep-sync-status" role="status" aria-live="polite" data-state={syncState}>
          {syncMessage}
        </div>
      {/if}
      <button
        type="button"
        class="sc-btn"
        data-variant="outline"
        data-size="sm"
        aria-label="Sync providers"
        aria-busy={syncState === "syncing"}
        disabled={syncState === "syncing"}
        onclick={syncProviders}
      >
        <Icon name="refresh" size={12} />{syncState === "syncing" ? "Syncing" : "Sync providers"}
      </button>
      <button
        bind:this={newDeploymentTrigger}
        type="button"
        class="sc-btn"
        data-variant="default"
        data-size="sm"
        onclick={openNewDeployment}
      >
        <Icon name="plus" size={12} />New deployment
      </button>
    </div>
  </div>

  {#if showNewDeployment}
    <div
      class="pf-modal-scrim"
      onclick={closeNewDeployment}
      role="presentation"
      onkeydown={() => {}}
    >
      <div
        class="pf-modal pf-dep-new-modal"
        onclick={(event) => event.stopPropagation()}
        role="dialog"
        aria-label="New deployment"
        aria-modal="true"
        tabindex="-1"
        use:focusTrap
        onkeydown={(event) => {
          if (event.key === "Escape") {
            event.preventDefault();
            closeNewDeployment();
          }
        }}
      >
        <form
          class="pf-dep-new-form"
          onsubmit={(event) => {
            event.preventDefault();
            createDraftDeployment();
          }}
        >
          <div class="pf-modal-head">
            <div class="pf-modal-title-group">
              <div class="pf-modal-eyebrow">Deployment</div>
              <div class="pf-modal-title">New deployment</div>
            </div>
            <button type="button" class="pf-modal-close" onclick={closeNewDeployment} aria-label="Close">
              <Icon name="x" size={14} />
            </button>
          </div>

          <div class="pf-modal-body">
            <label class="pf-field">
              <span class="pf-field-label">Service name</span>
              <span class="pf-field-input">
                <Icon name="rocket" size={13} />
                <input
                  aria-label="Service name"
                  data-autofocus
                  bind:value={newDeploymentName}
                  placeholder="checkout-worker"
                />
              </span>
            </label>

            <div class="pf-dep-new-grid">
              <label class="pf-field">
                <span class="pf-field-label">Provider</span>
                <span class="pf-field-input">
                  <Icon name="plug" size={13} />
                  <select aria-label="Provider" bind:value={newDeploymentProvider}>
                    {#each providerOptions as provider (provider.id)}
                      <option value={provider.id}>{provider.label}</option>
                    {/each}
                  </select>
                </span>
              </label>

              <label class="pf-field">
                <span class="pf-field-label">Environment</span>
                <span class="pf-field-input">
                  <Icon name="globe" size={13} />
                  <select aria-label="Environment" bind:value={newDeploymentEnvironment}>
                    <option value="staging">staging</option>
                    <option value="preview">preview</option>
                    <option value="production">production</option>
                  </select>
                </span>
              </label>
            </div>

            <label class="pf-field">
              <span class="pf-field-label">Branch</span>
              <span class="pf-field-input">
                <Icon name="branch" size={13} />
                <input aria-label="Branch" bind:value={newDeploymentBranch} placeholder="main" />
              </span>
            </label>

            <div class="pf-dep-new-summary" role="status" aria-live="polite">
              {#if canCreateDeployment}
                Draft will appear as {newDeploymentName.trim()} · {newDeploymentEnvironment}.
              {:else}
                Add a service name to create a deployment draft.
              {/if}
            </div>
          </div>

          <div class="pf-modal-foot">
            <div class="pf-modal-foot-hint">
              Drafts stay local until provider deployment RPCs are connected.
            </div>
            <div class="pf-modal-foot-btns">
              <button type="button" class="sc-btn" data-variant="ghost" onclick={closeNewDeployment}>
                Cancel
              </button>
              <button type="submit" class="sc-btn" data-variant="default" disabled={!canCreateDeployment}>
                <Icon name="plus" size={13} />Create deployment
              </button>
            </div>
          </div>
        </form>
      </div>
    </div>
  {/if}

  <div class="pf-dep-body">
    <div class="pf-dep-list">
      <div class="pf-dep-list-head">
        <span>Environment</span>
        <span>Status</span>
      </div>
      {#each filteredDeployments as d (d.id)}
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
      {#if filteredDeployments.length === 0}
        <div class="pf-dep-empty" role="status">
          <strong>No deployments match</strong>
          <span>Try a service name, provider, branch, region, status, or workspace.</span>
        </div>
      {/if}
    </div>

    <div class="pf-dep-detail">
      {#if selectedForDetail}
        {@const detail = selectedForDetail}
        <div class="pf-dep-detail-head">
          <div class="pf-dep-detail-head-left">
            <span class="pf-dep-provider-chip lg" data-provider={detail.provider}>
              <ProviderGlyph kind={detail.provider} size={18} />
            </span>
            <div>
              <div class="pf-dep-detail-name">
                {detail.name}
                <StatePill state={detail.state} />
              </div>
              <div class="pf-dep-detail-sub">
                {detail.providerLabel} · {detail.region} · <span class="mono">{detail.url}</span>
              </div>
            </div>
          </div>
          <div class="pf-dep-detail-head-right">
            {#if detailActionStatus}
              <div class="pf-dep-action-status" role="status" aria-live="polite">
                {detailActionStatus}
              </div>
            {/if}
            <button
              type="button"
              class="sc-btn"
              data-variant="ghost"
              data-size="sm"
              onclick={() => openDeployment(detail)}
            >
              <Icon name="external" size={12} />Open
            </button>
            <button
              type="button"
              class="sc-btn"
              data-variant="outline"
              data-size="sm"
              aria-label="Redeploy"
              aria-busy={redeployingId === detail.id}
              disabled={redeployingId !== null}
              onclick={() => triggerRedeploy(detail)}
            >
              <Icon name="refresh" size={12} />{redeployingId === detail.id ? "Redeploying" : "Redeploy"}
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
            <AskPufferPane
              d={detail}
              memoryDrafts={memoryDrafts[detail.id] ?? []}
              onAddMemory={(item) => addMemoryDraft(detail.id, item)}
            />
          {:else if tab === "memory"}
            <MemoryPane
              d={detail}
              drafts={memoryDrafts[detail.id] ?? []}
              onAddMemory={(item) => addMemoryDraft(detail.id, item)}
            />
          {:else if tab === "secrets"}
            <SecretsPane d={detail} />
          {:else if tab === "providers"}
            <ProvidersPane d={detail} />
          {:else if tab === "deploys"}
            <DeploysPane
              d={detail}
              localHistory={redeployHistory[detail.id] ?? []}
              triggerBusy={redeployingId === detail.id}
              onTriggerDeploy={() => triggerRedeploy(detail)}
            />
          {/if}
        </div>
      {:else}
        <div class="pf-dep-detail-empty" role="status">
          <Icon name="search" size={18} />
          <strong>No deployment selected</strong>
          <span>Clear or change the search to inspect deployment details.</span>
        </div>
      {/if}
    </div>
  </div>
</div>

<script lang="ts">
  import LoginView from "../components/LoginView.svelte";
  import BrandLogo from "../design/BrandLogo.svelte";
  import Puffer from "../design/Puffer.svelte";
  import Icon from "../design/Icon.svelte";
  import { providerCatalogForSetup } from "../providerFallbacks";
  import { providerIsAvailableForAgent } from "../providerIds";
  import type { ExternalCredential, SettingsSnapshot } from "../types";

  type Props = {
    snapshot: SettingsSnapshot | null;
    loading: boolean;
    remoteEnabled: boolean;
    busyProviderId: string | null;
    errorMessage: string | null;
    externals: ExternalCredential[];
    busyImportKey: string | null;
    onLoginOauth: (providerId: string) => void;
    onLoginApiKey: (providerId: string, apiKey: string) => void;
    onImportExternal: (providerId: string, source: "claude" | "codex") => void;
    onRefresh: () => void;
    onFinish: () => void;
  };

  let props: Props = $props();

  let authenticatedProviderIds = $derived((props.snapshot?.auth ?? []).map((auth) => auth.providerId));
  let agentProviderCount = $derived(
    providerCatalogForSetup(props.snapshot).filter((provider) =>
      providerIsAvailableForAgent(provider, authenticatedProviderIds)
    ).length
  );
  let signedIn = $derived(agentProviderCount > 0);

  let steps = $derived(
    signedIn
      ? [
          { label: "Connect a provider", done: true,  active: false },
          { label: "Connect GitHub",     done: true,  active: false },
          { label: "Open workspace",     done: false, active: true },
          { label: "Pick a model",       done: false, active: false },
          { label: "Set permissions",    done: false, active: false }
        ]
      : [
          { label: "Connect a provider", done: false, active: true },
          { label: "Connect GitHub",     done: false, active: false },
          { label: "Open workspace",     done: false, active: false },
          { label: "Pick a model",       done: false, active: false },
          { label: "Set permissions",    done: false, active: false }
        ]
  );
</script>

<div class="pf-onboard">
  <div class="pf-onboard-side">
    <div class="brand">
      <BrandLogo size={32} />
      Puffer
    </div>
    <h1>An agent that codes alongside you, not for you.</h1>
    <p class="lead">
      Puffer reads your repos, writes patches, and runs your tests. You stay in the loop with a
      permissions model that you actually understand.
    </p>
    <div style="flex: 1;"></div>
    <div class="pf-onboard-steps">
      {#each steps as s, i (s.label)}
        <div class="pf-onboard-step" data-done={s.done} data-active={s.active ?? false}>
          <span class="num">{s.done ? "✓" : i + 1}</span>{s.label}
        </div>
      {/each}
    </div>
  </div>
  <div class="pf-onboard-main">
    {#if signedIn}
      <h2>Workspace is ready</h2>
      <p class="lead">An agent provider is ready. Open the workspace to start or connect a project.</p>
      <div class="pf-onboard-ready">
        <div class="pf-onboard-ready-icon">
          <Icon name="check" size={18} color="var(--puffer-accent)" />
        </div>
        <div>
          <div class="pf-onboard-ready-title">
            {agentProviderCount} agent provider{agentProviderCount === 1 ? "" : "s"} ready
          </div>
          <div class="pf-onboard-ready-sub">
            Repository access is managed from the workspace and provider settings.
          </div>
        </div>
      </div>
      <div style="display: flex; margin-top: 28px; gap: 10px; justify-content: flex-end;">
        <button type="button" class="sc-btn" data-variant="default" onclick={props.onFinish}>
          Continue<Icon name="arrow" size={14} />
        </button>
      </div>
    {:else}
      <LoginView
        snapshot={props.snapshot}
        loading={props.loading}
        remoteEnabled={props.remoteEnabled}
        busyProviderId={props.busyProviderId}
        errorMessage={props.errorMessage}
        externals={props.externals}
        busyImportKey={props.busyImportKey}
        onLoginOauth={props.onLoginOauth}
        onLoginApiKey={props.onLoginApiKey}
        onImportExternal={props.onImportExternal}
        onRefresh={props.onRefresh}
      />
    {/if}
  </div>
</div>

<style>
  .pf-onboard {
    flex: 1;
    display: grid;
    grid-template-columns: 360px 1fr;
    background: var(--background);
    min-height: 0;
  }
  .pf-onboard-side {
    background: linear-gradient(
      180deg,
      color-mix(in oklab, var(--puffer-accent) 90%, oklch(0.2 0.05 280)) 0%,
      color-mix(in oklab, var(--puffer-accent) 70%, oklch(0.15 0.05 240)) 100%
    );
    color: white;
    padding: 36px 32px;
    display: flex;
    flex-direction: column;
    gap: 24px;
  }
  .pf-onboard-side .brand {
    display: flex; align-items: center; gap: 10px;
    font-size: 18px; font-weight: 600; letter-spacing: -0.02em;
  }
  .pf-onboard-side :global(h1) {
    font-size: 30px;
    line-height: 1.1;
    letter-spacing: -0.025em;
    color: white;
    text-wrap: balance;
    margin: 0;
  }
  .pf-onboard-side .lead {
    font-size: 14px; opacity: 0.85; line-height: 1.55; margin: 0;
  }
  .pf-onboard-steps {
    margin-top: auto;
    display: flex; flex-direction: column; gap: 8px;
    font-size: 13px; opacity: 0.85;
  }
  .pf-onboard-step { display: flex; align-items: center; gap: 10px; }
  .pf-onboard-step .num {
    width: 22px; height: 22px; border-radius: 50%;
    border: 1px solid rgba(255, 255, 255, 0.45);
    display: inline-flex; align-items: center; justify-content: center;
    font-size: 11px; font-family: var(--font-mono);
  }
  .pf-onboard-step[data-done="true"] .num {
    background: white;
    color: oklch(0.4 0.2 295);
    border-color: white;
  }
  .pf-onboard-step[data-active="true"] {
    opacity: 1;
  }

  .pf-onboard-main {
    padding: 48px 56px;
    overflow: auto;
    min-width: 0;
  }
  .pf-onboard-main :global(h2) {
    font-size: 22px; letter-spacing: -0.02em; margin-bottom: 6px; margin-top: 0;
    color: var(--foreground);
  }
  .pf-onboard-main .lead {
    color: var(--muted-foreground); font-size: 14px; margin: 0 0 24px;
  }
  .pf-onboard-ready {
    border: 1px solid var(--border);
    background: var(--background);
    border-radius: 10px;
    padding: 18px;
    display: flex;
    align-items: center;
    gap: 14px;
  }
  .pf-onboard-ready-icon {
    width: 38px;
    height: 38px;
    border-radius: 10px;
    display: flex;
    align-items: center;
    justify-content: center;
    background: color-mix(in oklab, var(--puffer-accent) 10%, var(--background));
    border: 1px solid color-mix(in oklab, var(--puffer-accent) 28%, var(--border));
  }
  .pf-onboard-ready-title {
    font-size: 14px;
    font-weight: 600;
    color: var(--foreground);
  }
  .pf-onboard-ready-sub {
    margin-top: 4px;
    font-size: 12.5px;
    color: var(--muted-foreground);
  }

  @media (max-width: 900px) {
    .pf-onboard { grid-template-columns: 1fr; }
    .pf-onboard-side { padding: 24px; }
    .pf-onboard-main { padding: 32px; }
  }
</style>

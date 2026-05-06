<script lang="ts">
  import LoginView from "../components/LoginView.svelte";
  import BrandLogo from "../design/BrandLogo.svelte";
  import Puffer from "../design/Puffer.svelte";
  import Icon from "../design/Icon.svelte";
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
    forceRepoStep?: boolean;
  };

  let props: Props = $props();

  let signedIn = $derived(props.forceRepoStep || (props.snapshot?.auth?.length ?? 0) > 0);

  const repos = [
    { name: "puffer-web", desc: "Marketing site + dashboard · Next.js",  lang: "TypeScript", commits: "342 commits", sel: true },
    { name: "stripe-api", desc: "Billing & webhooks · Node + Postgres",  lang: "TypeScript", commits: "1.2k commits", sel: true },
    { name: "infra-tf",   desc: "Terraform · AWS, Cloudflare",           lang: "HCL",        commits: "89 commits",  sel: false },
    { name: "ml-tools",   desc: "Internal scripts & notebooks",          lang: "Python",     commits: "210 commits", sel: false }
  ];

  let selected = $state(new Set(repos.filter((r) => r.sel).map((r) => r.name)));

  function toggle(name: string) {
    const next = new Set(selected);
    if (next.has(name)) next.delete(name);
    else next.add(name);
    selected = next;
  }

  let steps = $derived(
    signedIn
      ? [
          { label: "Connect a provider", done: true,  active: false },
          { label: "Connect GitHub",     done: true,  active: false },
          { label: "Choose your repos",  done: false, active: true },
          { label: "Pick a model",       done: false, active: false },
          { label: "Set permissions",    done: false, active: false }
        ]
      : [
          { label: "Connect a provider", done: false, active: true },
          { label: "Connect GitHub",     done: false, active: false },
          { label: "Choose your repos",  done: false, active: false },
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
      <h2>Choose the repos Puffer can see</h2>
      <p class="lead">You can change this any time. Corbina will only read what you grant.</p>
      <div class="pf-onboard-grid">
        {#each repos as r (r.name)}
          {@const sel = selected.has(r.name)}
          <button
            type="button"
            class="pf-onboard-pick"
            data-selected={sel}
            onclick={() => toggle(r.name)}
          >
            <div style="display: flex; align-items: center; gap: 8px;">
              <Icon name="repo" size={15} color="var(--puffer-accent)" />
              <span class="title">{r.name}</span>
              {#if sel}
                <span style="margin-left: auto;">
                  <Icon name="check" size={14} color="var(--puffer-accent)" />
                </span>
              {/if}
            </div>
            <div class="desc">{r.desc}</div>
            <div class="meta-row">
              <span>● {r.lang}</span>
              <span>{r.commits}</span>
            </div>
          </button>
        {/each}
      </div>
      <div style="display: flex; margin-top: 28px; gap: 10px; justify-content: flex-end;">
        <button type="button" class="sc-btn" data-variant="ghost">Skip for now</button>
        <button type="button" class="sc-btn" data-variant="default">
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
  .pf-onboard-grid {
    display: grid;
    grid-template-columns: repeat(2, 1fr);
    gap: 12px;
  }
  .pf-onboard-pick {
    border: 1px solid var(--border);
    background: var(--background);
    border-radius: 12px;
    padding: 16px;
    cursor: pointer;
    display: flex; flex-direction: column; gap: 8px;
    transition: all 120ms;
    text-align: left;
    color: var(--foreground);
    font: inherit;
  }
  .pf-onboard-pick[data-selected="true"] {
    border-color: var(--puffer-accent);
    background: color-mix(in oklab, var(--puffer-accent) 6%, var(--background));
    box-shadow: 0 0 0 3px color-mix(in oklab, var(--puffer-accent) 18%, transparent);
  }
  .pf-onboard-pick:hover {
    border-color: var(--puffer-accent);
  }
  .pf-onboard-pick .title { font-weight: 600; font-size: 14px; }
  .pf-onboard-pick .desc { font-size: 12.5px; color: var(--muted-foreground); }
  .pf-onboard-pick .meta-row {
    display: flex; align-items: center; gap: 8px; font-size: 11px;
    color: var(--muted-foreground); font-family: var(--font-mono);
  }

  @media (max-width: 900px) {
    .pf-onboard { grid-template-columns: 1fr; }
    .pf-onboard-side { padding: 24px; }
    .pf-onboard-main { padding: 32px; }
  }
</style>

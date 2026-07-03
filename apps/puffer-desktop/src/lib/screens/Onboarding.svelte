<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import LoginView from "../components/LoginView.svelte";
  import LocalModelSetupCard from "../components/LocalModelSetupCard.svelte";
  import RemoteSettings from "./settings/RemoteSettings.svelte";
  import BrandLogo from "../design/BrandLogo.svelte";
  import Icon from "../design/Icon.svelte";
  import { providerCatalogForSetup } from "../providerFallbacks";
  import { providerIsAvailableForAgent } from "../providerIds";
  import { isDesktopMac } from "../shell/platform";
  import {
    qwen35Recommend,
    qwen35Install,
    type Qwen35Recommendation
  } from "../api/desktop";
  import { listen } from "@tauri-apps/api/event";
  import type { ExternalCredential, SettingsSnapshot } from "../types";

  type OnboardingStepId =
    | "mode"
    | "provider"
    | "remote"
    | "tools"
    | "learn"
    | "analyzing"
    | "profile";

  type Props = {
    snapshot: SettingsSnapshot | null;
    loading: boolean;
    remoteEnabled: boolean;
    busyProviderId: string | null;
    errorMessage: string | null;
    externals: ExternalCredential[];
    busyImportKey: string | null;
    copilotLogin: { userCode: string; verificationUri: string } | null;
    onLoginCopilot: (providerId: string) => void;
    onCancelLogin: () => void;
    onLoginOauth: (providerId: string) => void;
    onLoginApiKey: (
      providerId: string,
      apiKey: string,
      options?: { baseUrl?: string | null; displayName?: string | null }
    ) => void;
    onLogout: (providerId: string) => void;
    onImportExternal: (providerId: string, source: "claude" | "codex") => void;
    onRefresh: () => void;
    onRemoteSettingsSaved: (snapshot: SettingsSnapshot) => void;
    onFinish: () => void;
  };

  let props: Props = $props();

  let currentStep = $state<OnboardingStepId>("mode");
  let runMode = $state<"cloud" | "local">("local");
  let toolAccess = $state({
    chrome: true,
    passwords: false,
    telegram: true,
    ssh: true
  });
  let learningAccess = $state({
    browserHistory: true,
    shellHistory: true,
    messages: false
  });
  let analyzeTimer: ReturnType<typeof setTimeout> | null = null;

  let authenticatedProviderIds = $derived((props.snapshot?.auth ?? []).map((auth) => auth.providerId));
  let agentProviderCount = $derived(
    providerCatalogForSetup(props.snapshot).filter((provider) =>
      providerIsAvailableForAgent(provider, authenticatedProviderIds)
    ).length
  );
  let signedIn = $derived(agentProviderCount > 0);

  let stepOrder: OnboardingStepId[] = [
    "mode",
    "provider",
    "remote",
    "tools",
    "learn",
    "analyzing",
    "profile"
  ];
  let currentStepIndex = $derived(stepOrder.indexOf(currentStep));
  let steps = $derived([
    {
      id: "mode",
      label: "Run mode",
      reachable: true,
      done: currentStepIndex > 0,
      active: currentStep === "mode"
    },
    {
      id: "provider",
      label: "Provider",
      reachable: currentStepIndex >= 1,
      done: signedIn || currentStepIndex > 1,
      active: currentStep === "provider"
    },
    {
      id: "remote",
      label: "AgentEnv",
      reachable: signedIn && currentStepIndex >= 2,
      done: currentStepIndex > 2,
      active: currentStep === "remote"
    },
    {
      id: "tools",
      label: "Connect",
      reachable: signedIn && currentStepIndex >= 3,
      done: currentStepIndex > 3,
      active: currentStep === "tools"
    },
    {
      id: "learn",
      label: "Learn",
      reachable: signedIn && currentStepIndex >= 4,
      done: currentStepIndex > 4,
      active: currentStep === "learn"
    },
    {
      id: "profile",
      label: currentStep === "analyzing" ? "Analyzing" : "Profile",
      reachable: signedIn && currentStepIndex >= 5,
      done: currentStep === "profile",
      active: currentStep === "analyzing" || currentStep === "profile"
    }
  ]);

  let mcp = $state<Qwen35Recommendation | null>(null);
  let mcpInstalling = $state(false);
  let mcpDone = $state<boolean | null>(null);
  let mcpLog = $state("");

  onMount(() => {
    if (!isDesktopMac()) return;
    let cancelled = false;
    let unlog: (() => void) | null = null;
    let undone: (() => void) | null = null;
    void qwen35Recommend()
      .then((r) => {
        if (!cancelled) mcp = r;
      })
      .catch(() => {});
    void listen<string>("qwen35://install-log", (e) => (mcpLog = e.payload)).then((u) => {
      if (cancelled) u();
      else unlog = u;
    });
    void listen<{ success: boolean }>("qwen35://install-done", (e) => {
      mcpInstalling = false;
      mcpDone = e.payload?.success ?? false;
      if (mcpDone) props.onRefresh();
    }).then((u) => {
      if (cancelled) u();
      else undone = u;
    });
    return () => {
      cancelled = true;
      unlog?.();
      undone?.();
    };
  });

  onDestroy(() => {
    if (analyzeTimer) clearTimeout(analyzeTimer);
  });

  async function installMcp() {
    mcpInstalling = true;
    mcpDone = null;
    mcpLog = "Starting...";
    try {
      await qwen35Install();
    } catch (error) {
      mcpInstalling = false;
      mcpDone = false;
      mcpLog = String(error);
    }
  }

  function goTo(step: OnboardingStepId) {
    currentStep = step;
  }

  function nextFromMode() {
    currentStep = "provider";
  }

  function nextFromProvider() {
    currentStep = "remote";
  }

  function nextFromRemote() {
    currentStep = "tools";
  }

  function startProfileAnalysis() {
    currentStep = "analyzing";
    if (analyzeTimer) clearTimeout(analyzeTimer);
    analyzeTimer = setTimeout(() => {
      currentStep = "profile";
      analyzeTimer = null;
    }, 700);
  }

  function toggleTool(key: keyof typeof toolAccess) {
    toolAccess[key] = !toolAccess[key];
  }

  function toggleLearning(key: keyof typeof learningAccess) {
    learningAccess[key] = !learningAccess[key];
  }

  let showMcpCard = $derived(mcp?.recommend === true || mcpInstalling || mcpDone !== null);
</script>

<div class="pf-onboard">
  <aside class="pf-onboard-side">
    <div class="brand">
      <BrandLogo size={32} />
      Puffer
    </div>
    <h1>Set up Puffer Code</h1>
    <p class="lead">
      Pick where Puffer runs, connect an agent provider, and choose what local context it may use.
    </p>
    <div class="pf-onboard-steps" aria-label="Onboarding progress">
      {#each steps as s, i (s.id)}
        <button
          type="button"
          class="pf-onboard-step"
          data-done={s.done}
          data-active={s.active}
          disabled={!s.reachable}
          onclick={() => goTo(s.id as OnboardingStepId)}
        >
          <span class="num">{s.done ? "✓" : i + 1}</span>{s.label}
        </button>
      {/each}
    </div>
  </aside>

  <main class="pf-onboard-main">
    {#if currentStep === "mode"}
      <section class="pf-onboard-panel" aria-labelledby="onboard-mode-title">
        <div class="pf-onboard-kicker">Step 1</div>
        <h2 id="onboard-mode-title">How should Puffer run?</h2>
        <p class="lead">Start locally today. Cloud mode is represented as a stub until hosted workspaces land.</p>

        <div class="pf-choice-list">
          <button
            type="button"
            class="pf-choice-card"
            data-selected={runMode === "cloud"}
            onclick={() => (runMode = "cloud")}
          >
            <span class="pf-choice-icon"><Icon name="globe" size={17} /></span>
            <span>
              <strong>Puffer Cloud</strong>
              <small>Managed workspace sign-in stub. Opens no browser yet.</small>
            </span>
            <span class="pf-stub-pill">Stub</span>
          </button>
          <button
            type="button"
            class="pf-choice-card"
            data-selected={runMode === "local"}
            onclick={() => (runMode = "local")}
          >
            <span class="pf-choice-icon"><Icon name="server" size={17} /></span>
            <span>
              <strong>Local / bring your own</strong>
              <small>Use your provider and keep setup on this machine.</small>
            </span>
          </button>
        </div>

        <div class="pf-onboard-actions">
          <button type="button" class="sc-btn" data-variant="default" onclick={nextFromMode}>
            Next<Icon name="arrow" size={14} />
          </button>
        </div>
      </section>
    {:else if currentStep === "provider"}
      <section class="pf-onboard-panel" aria-labelledby="onboard-provider-title">
        <div class="pf-onboard-kicker">Step 2</div>
        <h2 id="onboard-provider-title">Pick your agent provider</h2>
        <p class="lead">Real Anthropic, OpenAI, Codex, and local provider setup stays wired here.</p>

        {#if signedIn}
          <div class="pf-onboard-ready">
            <div class="pf-onboard-ready-icon">
              <Icon name="check" size={18} color="var(--puffer-accent)" />
            </div>
            <div>
              <div class="pf-onboard-ready-title">
                {agentProviderCount} agent provider{agentProviderCount === 1 ? "" : "s"} ready
              </div>
              <div class="pf-onboard-ready-sub">
                Add more providers here now, or continue with the connected provider.
              </div>
            </div>
          </div>
          <LocalModelSetupCard compact={true} onRefresh={props.onRefresh} />
          <div class="pf-onboard-provider-setup">
            <LoginView
              snapshot={props.snapshot}
              loading={props.loading}
              remoteEnabled={props.remoteEnabled}
              busyProviderId={props.busyProviderId}
              errorMessage={props.errorMessage}
              externals={props.externals}
              busyImportKey={props.busyImportKey}
              copilotLogin={props.copilotLogin}
              onLoginCopilot={props.onLoginCopilot}
              onCancelLogin={props.onCancelLogin}
              onLoginOauth={props.onLoginOauth}
              onLoginApiKey={props.onLoginApiKey}
              onLogout={props.onLogout}
              onImportExternal={props.onImportExternal}
              onRefresh={props.onRefresh}
            />
          </div>
          <div class="pf-onboard-actions">
            <button type="button" class="sc-btn" data-variant="ghost" onclick={() => goTo("mode")}>
              Back
            </button>
            <button type="button" class="sc-btn" data-variant="default" onclick={nextFromProvider}>
              Next<Icon name="arrow" size={14} />
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
            copilotLogin={props.copilotLogin}
            onLoginCopilot={props.onLoginCopilot}
            onCancelLogin={props.onCancelLogin}
            onLoginOauth={props.onLoginOauth}
            onLoginApiKey={props.onLoginApiKey}
            onLogout={props.onLogout}
            onImportExternal={props.onImportExternal}
            onRefresh={props.onRefresh}
          />
        {/if}
      </section>
    {:else if currentStep === "remote"}
      <section class="pf-onboard-panel" aria-labelledby="onboard-remote-title">
        <div class="pf-onboard-kicker">Step 3</div>
        <h2 id="onboard-remote-title">Connect AgentEnv</h2>
        <p class="lead">Use an existing AgentEnv account for remote tool execution, or skip it for now.</p>

        <div class="pf-onboard-remote">
          <RemoteSettings
            variant="onboarding"
            snapshot={props.snapshot}
            daemonReachable={!props.loading}
            onSaved={props.onRemoteSettingsSaved}
            onRefresh={props.onRefresh}
          />
        </div>

        <div class="pf-onboard-actions">
          <button type="button" class="sc-btn" data-variant="ghost" onclick={() => goTo("provider")}>
            Back
          </button>
          <button type="button" class="sc-btn" data-variant="default" onclick={nextFromRemote}>
            Next<Icon name="arrow" size={14} />
          </button>
        </div>
      </section>
    {:else if currentStep === "tools"}
      <section class="pf-onboard-panel" aria-labelledby="onboard-tools-title">
        <div class="pf-onboard-kicker">Step 4</div>
        <h2 id="onboard-tools-title">Connect your tools</h2>
        <p class="lead">Optional tool switches are stubs for now, so the flow can land before integrations are final.</p>

        <div class="pf-toggle-list">
          <button type="button" class="pf-toggle-row" onclick={() => toggleTool("chrome")}>
            <span class="pf-choice-icon"><Icon name="globe" size={16} /></span>
            <span><strong>Chrome import</strong><small>Local only, will request Keychain access.</small></span>
            <span class="pf-toggle" data-on={toolAccess.chrome}></span>
          </button>
          <button type="button" class="pf-toggle-row" onclick={() => toggleTool("passwords")}>
            <span class="pf-choice-icon"><Icon name="key" size={16} /></span>
            <span><strong>1Password</strong><small>Stubbed connector.</small></span>
            <span class="pf-toggle" data-on={toolAccess.passwords}></span>
          </button>
          <button type="button" class="pf-toggle-row" onclick={() => toggleTool("telegram")}>
            <span class="pf-choice-icon"><Icon name="plug" size={16} /></span>
            <span><strong>Telegram</strong><small>Out-of-process connector setup later.</small></span>
            <span class="pf-toggle" data-on={toolAccess.telegram}></span>
          </button>
          <button type="button" class="pf-toggle-row" onclick={() => toggleTool("ssh")}>
            <span class="pf-choice-icon"><Icon name="terminal" size={16} /></span>
            <span><strong>SSH config</strong><small>Host discovery stub.</small></span>
            <span class="pf-toggle" data-on={toolAccess.ssh}></span>
          </button>
        </div>

        <div class="pf-onboard-actions">
          <button type="button" class="sc-btn" data-variant="ghost" onclick={() => goTo("remote")}>
            Back
          </button>
          <button type="button" class="sc-btn" data-variant="default" onclick={() => goTo("learn")}>
            Next<Icon name="arrow" size={14} />
          </button>
        </div>
      </section>
    {:else if currentStep === "learn"}
      <section class="pf-onboard-panel" aria-labelledby="onboard-learn-title">
        <div class="pf-onboard-kicker">Step 5</div>
        <h2 id="onboard-learn-title">What may Puffer learn?</h2>
        <p class="lead">These local-only permissions shape the generated profile preview.</p>

        <div class="pf-toggle-list">
          <button type="button" class="pf-check-row" onclick={() => toggleLearning("browserHistory")}>
            <span class="pf-check" data-on={learningAccess.browserHistory}><Icon name="check" size={13} /></span>
            <span><strong>Browser history</strong><small>Recommended local profile signal.</small></span>
          </button>
          <button type="button" class="pf-check-row" onclick={() => toggleLearning("shellHistory")}>
            <span class="pf-check" data-on={learningAccess.shellHistory}><Icon name="check" size={13} /></span>
            <span><strong>Shell history</strong><small>On by default, local only.</small></span>
          </button>
          <button type="button" class="pf-check-row" onclick={() => toggleLearning("messages")}>
            <span class="pf-check" data-on={learningAccess.messages}><Icon name="check" size={13} /></span>
            <span><strong>Read messages</strong><small>Telegram and Slack stubs.</small></span>
          </button>
        </div>

        <div class="pf-onboard-actions">
          <button type="button" class="sc-btn" data-variant="ghost" onclick={() => goTo("tools")}>
            Back
          </button>
          <button type="button" class="sc-btn" data-variant="default" onclick={startProfileAnalysis}>
            Build profile<Icon name="arrow" size={14} />
          </button>
        </div>
      </section>
    {:else if currentStep === "analyzing"}
      <section class="pf-onboard-panel pf-analyzing" aria-labelledby="onboard-analyzing-title">
        <div class="pf-onboard-kicker">Step 6</div>
        <h2 id="onboard-analyzing-title">Reading you in...</h2>
        <div class="pf-analysis-card" aria-label="Profile analysis in progress">
          <span></span><span></span><span></span><span></span><span></span>
        </div>
        <p class="lead">Scanning 1,204 shell commands, recent git repos, and selected local history stubs.</p>
      </section>
    {:else}
      <section class="pf-onboard-panel" aria-labelledby="onboard-profile-title">
        <div class="pf-onboard-kicker">Step 6</div>
        <h2 id="onboard-profile-title">Workspace is ready</h2>
        <p class="lead">Meet your local profile. It is editable later from Settings.</p>

        <div class="pf-profile-card">
          <div><span>Languages</span><strong>Rust, TypeScript</strong></div>
          <div><span>Hosts</span><strong>prod-web-1, gpu-box</strong></div>
          <div><span>Focus</span><strong>payments refactor</strong></div>
          <div><span>Run mode</span><strong>{runMode === "cloud" ? "Cloud stub" : "Local"}</strong></div>
        </div>

        <div class="pf-onboard-actions">
          <button type="button" class="sc-btn" data-variant="ghost" onclick={() => goTo("learn")}>
            Edit
          </button>
          <button type="button" class="sc-btn" data-variant="default" onclick={props.onFinish}>
            Start using Puffer<Icon name="arrow" size={14} />
          </button>
        </div>
      </section>
    {/if}

    {#if showMcpCard && currentStep === "provider"}
      <div class="pf-mcp">
        <div class="pf-mcp-top">
          <span class="pf-mcp-dot"></span>
          <div class="pf-mcp-text">
            <div class="pf-mcp-title">{mcp?.display_name ?? "Qwen3.5-0.8B (local)"}</div>
            <div class="pf-mcp-sub">
              {mcp?.why ?? "on-device - private, free, always-on"}{mcp?.size
                ? ` - ${mcp.size}`
                : ""}
            </div>
          </div>
        </div>
        {#if mcpDone === true}
          <div class="pf-mcp-status" data-state="ok">
            Installed - available as a local provider.
          </div>
        {:else if mcpInstalling}
          <div class="pf-mcp-status">Installing... <code>{mcpLog}</code></div>
        {:else if mcpDone === false}
          <div class="pf-mcp-status" data-state="err">Install failed. <code>{mcpLog}</code></div>
          <button type="button" class="sc-btn" data-variant="ghost" onclick={installMcp}>
            Retry
          </button>
        {:else}
          <button type="button" class="sc-btn" data-variant="default" onclick={installMcp}>
            Install local model
          </button>
        {/if}
      </div>
    {/if}
  </main>
</div>

<style>
  .pf-onboard {
    flex: 1;
    display: grid;
    grid-template-columns: 320px 1fr;
    background: var(--background);
    min-height: 0;
  }
  .pf-onboard-side {
    background: color-mix(in oklab, var(--background) 92%, var(--puffer-accent));
    border-right: 1px solid var(--border);
    color: var(--foreground);
    padding: 34px 28px;
    display: flex;
    flex-direction: column;
    gap: 22px;
  }
  .pf-onboard-side .brand {
    display: flex;
    align-items: center;
    gap: 10px;
    font-size: 18px;
    font-weight: 650;
  }
  .pf-onboard-side :global(h1) {
    font-size: 29px;
    line-height: 1.12;
    color: var(--foreground);
    margin: 0;
  }
  .pf-onboard-side .lead {
    font-size: 13.5px;
    color: var(--muted-foreground);
    line-height: 1.55;
    margin: 0;
  }
  .pf-onboard-steps {
    margin-top: auto;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .pf-onboard-step {
    appearance: none;
    border: 0;
    background: transparent;
    color: var(--muted-foreground);
    display: flex;
    align-items: center;
    gap: 10px;
    text-align: left;
    padding: 4px 0;
    font: inherit;
    font-size: 13px;
    cursor: pointer;
  }
  .pf-onboard-step .num {
    width: 22px;
    height: 22px;
    border-radius: 50%;
    border: 1px solid var(--border);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    font-size: 11px;
    font-family: var(--font-mono);
    background: var(--background);
    flex: none;
  }
  .pf-onboard-step[data-done="true"] .num {
    background: var(--puffer-accent);
    color: white;
    border-color: var(--puffer-accent);
  }
  .pf-onboard-step[data-active="true"] {
    color: var(--foreground);
    font-weight: 600;
  }
  .pf-onboard-step:disabled {
    cursor: default;
    opacity: 0.55;
  }
  .pf-onboard-main {
    padding: 46px 54px;
    overflow: auto;
    min-width: 0;
  }
  .pf-onboard-panel {
    max-width: 720px;
  }
  .pf-onboard-kicker {
    color: var(--puffer-accent);
    font-size: 11px;
    font-weight: 700;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    margin-bottom: 8px;
  }
  .pf-onboard-main :global(h2) {
    font-size: 23px;
    margin: 0 0 6px;
    color: var(--foreground);
  }
  .pf-onboard-main .lead {
    color: var(--muted-foreground);
    font-size: 14px;
    margin: 0 0 24px;
    line-height: 1.55;
  }
  .pf-choice-list,
  .pf-toggle-list {
    display: grid;
    gap: 10px;
  }
  .pf-choice-card,
  .pf-toggle-row,
  .pf-check-row {
    appearance: none;
    width: 100%;
    border: 1px solid var(--border);
    background: var(--background);
    color: var(--foreground);
    border-radius: 8px;
    padding: 14px;
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto;
    align-items: center;
    gap: 12px;
    text-align: left;
    cursor: pointer;
  }
  .pf-choice-card[data-selected="true"],
  .pf-toggle-row:hover,
  .pf-check-row:hover {
    border-color: color-mix(in oklab, var(--puffer-accent) 56%, var(--border));
    background: color-mix(in oklab, var(--puffer-accent) 7%, var(--background));
  }
  .pf-choice-icon {
    width: 34px;
    height: 34px;
    border-radius: 8px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    background: color-mix(in oklab, var(--muted) 55%, var(--background));
    border: 1px solid var(--border);
    color: var(--puffer-accent);
  }
  .pf-choice-card strong,
  .pf-toggle-row strong,
  .pf-check-row strong {
    display: block;
    font-size: 14px;
  }
  .pf-choice-card small,
  .pf-toggle-row small,
  .pf-check-row small {
    display: block;
    color: var(--muted-foreground);
    font-size: 12.5px;
    margin-top: 3px;
    line-height: 1.35;
  }
  .pf-stub-pill {
    border-radius: 999px;
    background: color-mix(in oklab, var(--puffer-accent) 12%, var(--background));
    color: var(--puffer-accent);
    border: 1px solid color-mix(in oklab, var(--puffer-accent) 30%, var(--border));
    font-size: 11px;
    padding: 3px 8px;
    font-weight: 700;
  }
  .pf-toggle {
    width: 38px;
    height: 22px;
    border-radius: 999px;
    border: 1px solid var(--border);
    background: color-mix(in oklab, var(--muted) 80%, var(--background));
    position: relative;
  }
  .pf-toggle::after {
    content: "";
    position: absolute;
    width: 16px;
    height: 16px;
    top: 2px;
    left: 2px;
    border-radius: 50%;
    background: var(--background);
    border: 1px solid var(--border);
    transition: transform 120ms ease;
  }
  .pf-toggle[data-on="true"] {
    background: var(--puffer-accent);
    border-color: var(--puffer-accent);
  }
  .pf-toggle[data-on="true"]::after {
    transform: translateX(16px);
  }
  .pf-check {
    width: 24px;
    height: 24px;
    border-radius: 6px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border: 1px solid var(--border);
    color: transparent;
  }
  .pf-check[data-on="true"] {
    color: white;
    background: var(--puffer-accent);
    border-color: var(--puffer-accent);
  }
  .pf-onboard-actions {
    display: flex;
    margin-top: 26px;
    gap: 10px;
    justify-content: flex-end;
  }
  .pf-onboard-ready,
  .pf-profile-card {
    border: 1px solid var(--border);
    background: var(--background);
    border-radius: 8px;
    padding: 18px;
  }
  .pf-onboard-ready {
    display: flex;
    align-items: center;
    gap: 14px;
  }
  .pf-onboard-ready-icon {
    width: 38px;
    height: 38px;
    border-radius: 8px;
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
  .pf-onboard-provider-setup {
    margin-top: 22px;
  }
  .pf-onboard-provider-setup :global(.login-page) {
    gap: 18px;
  }
  .pf-onboard-provider-setup :global(.provider-grid) {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
  .pf-analyzing {
    display: flex;
    min-height: 360px;
    flex-direction: column;
    justify-content: center;
  }
  .pf-analysis-card {
    width: min(360px, 100%);
    height: 176px;
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 30px;
    display: grid;
    gap: 10px;
    margin-bottom: 20px;
    background: color-mix(in oklab, var(--muted) 30%, var(--background));
  }
  .pf-analysis-card span {
    height: 10px;
    border-radius: 999px;
    background: linear-gradient(
      90deg,
      color-mix(in oklab, var(--puffer-accent) 16%, var(--background)),
      color-mix(in oklab, var(--puffer-accent) 42%, var(--background)),
      color-mix(in oklab, var(--puffer-accent) 16%, var(--background))
    );
    background-size: 220% 100%;
    animation: pf-scan 900ms linear infinite;
  }
  .pf-analysis-card span:nth-child(2) { width: 82%; }
  .pf-analysis-card span:nth-child(3) { width: 94%; }
  .pf-analysis-card span:nth-child(4) { width: 70%; }
  .pf-analysis-card span:nth-child(5) { width: 88%; }
  .pf-profile-card {
    display: grid;
    gap: 0;
    max-width: 560px;
  }
  .pf-profile-card div {
    display: grid;
    grid-template-columns: 120px minmax(0, 1fr);
    gap: 12px;
    padding: 11px 0;
    border-bottom: 1px solid var(--border);
  }
  .pf-profile-card div:last-child {
    border-bottom: 0;
  }
  .pf-profile-card span {
    color: var(--muted-foreground);
    font-size: 12px;
  }
  .pf-profile-card strong {
    font-size: 13px;
    color: var(--foreground);
  }
  .pf-mcp {
    margin-top: 24px;
    padding: 16px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--card, var(--background));
    display: flex;
    flex-direction: column;
    gap: 12px;
    max-width: 520px;
  }
  .pf-mcp-top {
    display: flex;
    align-items: flex-start;
    gap: 10px;
  }
  .pf-mcp-dot {
    width: 8px;
    height: 8px;
    margin-top: 5px;
    border-radius: 50%;
    background: var(--puffer-accent);
    flex: none;
  }
  .pf-mcp-title {
    font-size: 14px;
    font-weight: 600;
  }
  .pf-mcp-sub,
  .pf-mcp-status {
    font-size: 12px;
    color: var(--muted-foreground);
    margin-top: 2px;
  }
  .pf-mcp-status[data-state="ok"] {
    color: var(--puffer-accent);
  }
  .pf-mcp-status[data-state="err"] {
    color: var(--destructive, #d33);
  }
  .pf-mcp-status code {
    font-family: var(--font-mono);
    font-size: 11px;
    opacity: 0.8;
    word-break: break-all;
  }
  .pf-mcp .sc-btn {
    align-self: flex-start;
  }
  .pf-onboard-remote {
    margin-top: 24px;
    max-width: 900px;
  }
  @keyframes pf-scan {
    from { background-position: 0% 50%; }
    to { background-position: -220% 50%; }
  }
  @media (max-width: 900px) {
    .pf-onboard {
      grid-template-columns: 1fr;
    }
    .pf-onboard-side,
    .pf-onboard-main {
      padding: 24px;
    }
    .pf-profile-card div {
      grid-template-columns: 1fr;
      gap: 4px;
    }
  }
</style>

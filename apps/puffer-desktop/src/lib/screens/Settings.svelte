<script lang="ts">
  import "../design/settings.css";

  import Icon, { type IconName } from "../design/Icon.svelte";
  import LoginView from "../components/LoginView.svelte";
  import {
    providerIdCanRunAgent,
    providerIsAvailableForAgent,
    providerIdsEquivalent
  } from "../providerIds";
  import { providerCatalogForSetup, usesFallbackProviderCatalog } from "../providerFallbacks";
  import type { AccentKey, DensityKey, FontMixKey, ThemeKey, Tweaks } from "../shell/tweaks";
  import {
    addMcpServer,
    isDaemonReachable,
    listMcpServers,
    listPermissions,
    listProviderModels,
    savePermissions,
    updateConfig,
    type McpServerInfo,
    type ModelDescriptorInfo,
    type PermissionsSnapshot
  } from "../api/desktop";
  import type {
    DesktopPreferences,
    ExternalCredential,
    RemoteOperation,
    SettingsSnapshot
  } from "../types";

  type Props = {
    snapshot: SettingsSnapshot | null;
    loading: boolean;
    tweaks: Tweaks;
    preferences: DesktopPreferences;
    daemonUrl: string | null;
    daemonWorkspaceRoot: string | null;
    remoteEnabled: boolean;
    remotePassword: string;
    remoteBusy: boolean;
    remoteResult: RemoteOperation | null;
    onPreferenceChange: <K extends keyof DesktopPreferences>(key: K, value: DesktopPreferences[K]) => void;
    onRemotePasswordChange: (value: string) => void;
    onResetPreferences: () => void;
    onTweakChange: <K extends keyof Tweaks>(key: K, value: Tweaks[K]) => void;
    onResetAppearance: () => void;
    onRefresh: () => void;
    onLogout: (providerId: string) => void;
    onLoginOauth?: (providerId: string) => void;
    onApiKeyLogin?: (providerId: string, apiKey: string) => void;
    onImportExternal?: (providerId: string, source: "claude" | "codex") => void;
    busyProviderId?: string | null;
    authError?: string | null;
    externals?: ExternalCredential[];
    busyImportKey?: string | null;
    onRunRemoteBash: (command: string) => void;
    onReadRemoteFile: (path: string) => void;
    onWriteRemoteFile: (path: string, contents: string) => void;
  };

  let props: Props = $props();
  let credentialBusy = $derived(props.busyProviderId != null || props.busyImportKey != null);

  function refreshIfIdle() {
    if (credentialBusy) return;
    props.onRefresh();
  }

  type Section = "general" | "providers" | "permissions" | "mcp" | "git" | "appearance" | "shortcuts";
  let section = $state<Section>("general");

  const navItems: { id: Section; label: string; icon: IconName }[] = [
    { id: "general",     label: "General",    icon: "settings" },
    { id: "providers",   label: "Providers",  icon: "plug" },
    { id: "permissions", label: "Permissions", icon: "bolt" },
    { id: "mcp",         label: "MCP Servers", icon: "plug" },
    { id: "git",         label: "Git & PRs",  icon: "git" },
    { id: "appearance",  label: "Appearance", icon: "layers" },
    { id: "shortcuts",   label: "Shortcuts",  icon: "panel" }
  ];

  // Live permissions loaded from the daemon. `permissionRows` is the
  // editable working copy — changes are staged in memory and flushed on
  // Save. Loading state and generation guards keep late responses from
  // clobbering in-progress edits.
  let permissionSnapshot = $state<PermissionsSnapshot | null>(null);
  let permissionRows = $state<{ tool: string; mode: string }[]>([]);
  let permissionLoading = $state(false);
  let permissionLoadGeneration = 0;
  let permissionSaving = $state(false);
  let permissionError = $state<string | null>(null);
  let permissionDirty = $state(false);

  // MCP servers discovered on disk plus a small manifest writer for new
  // workspace/user entries.
  let mcpServers = $state<McpServerInfo[]>([]);
  let mcpLoaded = $state(false);
  let mcpLoading = $state(false);
  let mcpLoadGeneration = 0;
  let mcpSaving = $state(false);
  let mcpError = $state<string | null>(null);
  let mcpSaved = $state<string | null>(null);
  let mcpForm = $state({
    id: "",
    displayName: "",
    transport: "stdio" as "stdio" | "sse" | "http",
    commandOrUrl: "",
    args: "",
    description: "",
    scope: "local" as "local" | "user"
  });

  // Per-provider model listings cached by providerId. Populated on demand
  // when the user expands the Providers pane.
  let providerModels = $state<Record<string, ModelDescriptorInfo[]>>({});
  let modelLoadingByProvider = $state<Record<string, boolean>>({});
  let modelPickerProvider = $state<string>("");
  let modelPickerModel = $state<string>("");
  let modelSaving = $state(false);
  let modelError = $state<string | null>(null);

  function ruleIcon(tool: string): IconName {
    if (tool === "read_file") return "file";
    if (tool === "edit_file") return "edit";
    if (tool.includes("bash") || tool.includes("shell")) return "terminal";
    if (tool.includes("fetch") || tool.includes("http")) return "globe";
    return "bolt";
  }

  function mcpIcon(server: McpServerInfo): IconName {
    const id = server.id.toLowerCase();
    if (id.includes("github") || id.includes("git")) return "git";
    if (id.includes("postgres") || id.includes("db")) return "cpu";
    if (id.includes("sentry") || id.includes("error")) return "flame";
    if (id.includes("figma") || id.includes("design")) return "panel";
    return "layers";
  }

  async function loadMcpServers() {
    const generation = ++mcpLoadGeneration;
    mcpLoading = true;
    mcpError = null;
    try {
      const servers = await listMcpServers();
      if (generation !== mcpLoadGeneration) return;
      mcpServers = servers;
    } catch (e) {
      if (generation === mcpLoadGeneration) {
        mcpError = (e as Error).message ?? String(e);
      }
    } finally {
      if (generation === mcpLoadGeneration) {
        mcpLoaded = true;
        mcpLoading = false;
      }
    }
  }

  function mcpTargetValue(): string {
    const command = mcpForm.commandOrUrl.trim();
    if (mcpForm.transport !== "stdio") return command;
    const args = mcpForm.args.trim();
    return args ? `${command} ${args}` : command;
  }

  async function saveMcpServer() {
    const id = mcpForm.id.trim();
    const targetOrUrl = mcpTargetValue();
    if (mcpSaving || !id || !targetOrUrl) return;
    mcpLoadGeneration += 1;
    mcpLoading = false;
    mcpSaving = true;
    mcpError = null;
    mcpSaved = null;
    try {
      mcpServers = await addMcpServer({
        id,
        displayName: mcpForm.displayName.trim() || undefined,
        description: mcpForm.description.trim() || undefined,
        transport: mcpForm.transport,
        endpoint: mcpForm.transport === "stdio" ? undefined : targetOrUrl,
        target: mcpForm.transport === "stdio" ? targetOrUrl : undefined,
        scope: mcpForm.scope
      });
      mcpLoaded = true;
      mcpSaved = `Added ${id}`;
      mcpForm = {
        id: "",
        displayName: "",
        transport: "stdio",
        commandOrUrl: "",
        args: "",
        description: "",
        scope: mcpForm.scope
      };
      props.onRefresh();
    } catch (e) {
      mcpError = (e as Error).message ?? String(e);
    } finally {
      mcpSaving = false;
    }
  }

  async function loadPermissionSnapshot() {
    const generation = ++permissionLoadGeneration;
    permissionLoading = true;
    permissionError = null;
    try {
      const snap = await listPermissions();
      if (generation !== permissionLoadGeneration) return;
      permissionSnapshot = snap;
      if (!permissionDirty && !permissionSaving) {
        permissionRows = Object.entries(snap.tools)
          .sort(([a], [b]) => a.localeCompare(b))
          .map(([tool, mode]) => ({ tool, mode }));
        permissionDirty = false;
      }
    } catch (e) {
      if (generation === permissionLoadGeneration) {
        permissionError = (e as Error).message ?? String(e);
      }
    } finally {
      if (generation === permissionLoadGeneration) {
        permissionLoading = false;
      }
    }
  }

  async function loadModelsForProvider(providerId: string) {
    if (
      !providerId ||
      !providerIdCanRunAgent(providerId, props.snapshot?.providers ?? [])
    ) {
      return;
    }
    const cachedModels = providerModels[providerId];
    if (cachedModels) {
      if (modelPickerProvider === providerId && !modelIdInList(modelPickerModel, cachedModels)) {
        modelPickerModel = defaultModelId(cachedModels);
      }
      return;
    }
    if (modelLoadingByProvider[providerId]) return;
    modelLoadingByProvider = { ...modelLoadingByProvider, [providerId]: true };
    modelError = null;
    try {
      const models = await listProviderModels(providerId);
      providerModels = { ...providerModels, [providerId]: models };
      if (modelPickerProvider === providerId && !modelIdInList(modelPickerModel, models)) {
        modelPickerModel = defaultModelId(models);
      }
    } catch (e) {
      if (modelPickerProvider === providerId) {
        modelError = (e as Error).message ?? String(e);
      }
    } finally {
      modelLoadingByProvider = { ...modelLoadingByProvider, [providerId]: false };
    }
  }

  function modelSupportsAgentTools(model: ModelDescriptorInfo): boolean {
    return model.supportsTools !== false;
  }

  function agentToolModels(models: ModelDescriptorInfo[]): ModelDescriptorInfo[] {
    return models.filter(modelSupportsAgentTools);
  }

  function defaultModelId(models: ModelDescriptorInfo[]): string {
    const availableModels = agentToolModels(models);
    return (availableModels.find((model) => model.isDefault) ?? availableModels[0])?.id ?? "";
  }

  function modelIdInList(modelId: string, models: ModelDescriptorInfo[]): boolean {
    return Boolean(modelId && agentToolModels(models).some((model) => model.id === modelId));
  }

  function addPermissionRow() {
    permissionRows = [...permissionRows, { tool: "", mode: "ask" }];
    permissionDirty = true;
  }

  function removePermissionRow(index: number) {
    permissionRows = permissionRows.filter((_, i) => i !== index);
    permissionDirty = true;
  }

  function updatePermissionRow(index: number, field: "tool" | "mode", value: string) {
    permissionRows = permissionRows.map((row, i) =>
      i === index ? { ...row, [field]: value } : row
    );
    permissionDirty = true;
  }

  async function savePermissionRows() {
    if (permissionSaving || permissionLoading || !permissionDirty) return;
    permissionSaving = true;
    permissionError = null;
    try {
      const tools: Record<string, string> = {};
      for (const row of permissionRows) {
        const tool = row.tool.trim();
        if (!tool) continue;
        tools[tool] = row.mode;
      }
      const snap = await savePermissions(tools);
      permissionSnapshot = snap;
      permissionRows = Object.entries(snap.tools)
        .sort(([a], [b]) => a.localeCompare(b))
        .map(([tool, mode]) => ({ tool, mode }));
      permissionDirty = false;
    } catch (e) {
      permissionError = (e as Error).message ?? String(e);
    } finally {
      permissionSaving = false;
    }
  }

  async function saveDefaultModel() {
    if (!modelPickerProvider || !modelPickerModel || modelPickerLoading || modelSaving) return;
    modelSaving = true;
    modelError = null;
    try {
      await updateConfig({
        defaultProvider: modelPickerProvider,
        defaultModel: modelPickerModel || null
      });
      // Parent owns the snapshot; ask it to refetch so the header values
      // line up with the new config.
      props.onRefresh();
    } catch (e) {
      modelError = (e as Error).message ?? String(e);
    } finally {
      modelSaving = false;
    }
  }

  let authedProviderIds = $derived(new Set((props.snapshot?.auth ?? []).map((a) => a.providerId)));
  let defaultRouteProviders = $derived.by(() => {
    const authIds = (props.snapshot?.auth ?? []).map((auth) => auth.providerId);
    return providerCatalogForSetup(props.snapshot).filter(
      (provider) => providerIsAvailableForAgent(provider, authIds)
    );
  });
  let usingFallbackProviders = $derived(usesFallbackProviderCatalog(props.snapshot));

  function defaultRouteProviderId(): string {
    const configured = props.snapshot?.config.defaultProvider;
    const configuredProvider = defaultRouteProviders.find((provider) =>
      providerIdsEquivalent(provider.id, configured)
    );
    return configuredProvider?.id ?? defaultRouteProviders[0]?.id ?? "";
  }

  // Shortcuts the app actually wires up today. Keep this honest — when we
  // add more we'll add them here, not before.
  const shortcuts: { combo: string; action: string }[] = [
    { combo: "Enter",            action: "Send composer message" },
    { combo: "Shift + Enter",    action: "Insert newline in composer" },
    { combo: "Esc",              action: "Close modal / cancel" },
    { combo: "Cmd/Ctrl + ,",     action: "Open settings" }
  ];

  // Well-known git providers we surface on the Git & PRs pane. If the user
  // is logged in to one of these we show their auth status inline.
  const GIT_PROVIDER_IDS: string[] = ["github", "gitlab"];
  const themes: ThemeKey[] = ["light", "dark"];
  const accents: { k: AccentKey; c: string }[] = [
    { k: "violet", c: "oklch(0.55 0.22 295)" },
    { k: "cyan", c: "oklch(0.62 0.14 215)" },
    { k: "amber", c: "oklch(0.72 0.18 70)" },
    { k: "rose", c: "oklch(0.62 0.22 15)" },
    { k: "lime", c: "oklch(0.72 0.18 130)" },
    { k: "mono", c: "oklch(0.205 0 0)" }
  ];
  const fonts: { k: FontMixKey; label: string }[] = [
    { k: "sans-mono", label: "sans + mono" },
    { k: "all-mono", label: "all mono" }
  ];
  const densities: DensityKey[] = ["compact", "comfortable", "airy"];

  // Reset daemon-scoped local pane state when the parent refreshes to a
  // different daemon/workspace/config source. Otherwise Settings can show
  // permissions, MCP servers, or default model choices from the prior daemon.
  let settingsSourceKey = $state("");
  $effect(() => {
    const nextKey = [
      props.daemonUrl ?? "",
      props.snapshot?.workspaceRoot ?? "",
      (props.snapshot?.auth ?? []).map((auth) => auth.providerId).sort().join(","),
      (props.snapshot?.providers ?? []).map((provider) => provider.id).sort().join(","),
      props.snapshot?.config.defaultProvider ?? "",
      props.snapshot?.config.defaultModel ?? ""
    ].join("\0");
    if (nextKey === settingsSourceKey) return;
    settingsSourceKey = nextKey;

    permissionLoadGeneration += 1;
    permissionSnapshot = null;
    permissionRows = [];
    permissionLoading = false;
    permissionError = null;
    permissionDirty = false;

    mcpServers = [];
    mcpLoaded = false;
    mcpLoading = false;
    mcpLoadGeneration += 1;
    mcpError = null;
    mcpSaved = null;

    providerModels = {};
    modelLoadingByProvider = {};
    const nextProvider = defaultRouteProviderId();
    modelPickerProvider = nextProvider;
    modelPickerModel = providerIdsEquivalent(nextProvider, props.snapshot?.config.defaultProvider)
      ? props.snapshot?.config.defaultModel ?? ""
      : "";
    modelError = null;
  });

  // Skip RPC calls when the daemon isn't reachable — web previews render
  // static panes with a friendly "connect daemon" banner instead of a red
  // error. In Tauri the singleton connects on first `ensureLocalDaemonClient`.
  let daemonReachable = isDaemonReachable();
  let mcpFormDisabled = $derived(!daemonReachable || mcpLoading || mcpSaving);
  let modelPickerLoading = $derived(
    Boolean(modelPickerProvider && modelLoadingByProvider[modelPickerProvider])
  );
  let modelPickerModels = $derived(agentToolModels(providerModels[modelPickerProvider] ?? []));
  let modelPickerModelsLoaded = $derived(
    Boolean(modelPickerProvider && providerModels[modelPickerProvider])
  );
  let modelPickerNoAgentModels = $derived(
    modelPickerModelsLoaded && !modelPickerLoading && modelPickerModels.length === 0 && !modelError
  );
  let modelPickerProviderName = $derived(
    defaultRouteProviders.find((p) => providerIdsEquivalent(p.id, modelPickerProvider))
      ?.displayName ?? modelPickerProvider
  );
  let modelPickerDisabled = $derived(!daemonReachable || modelSaving);
  let canSaveDefaultModel = $derived(
    Boolean(
      daemonReachable &&
        modelPickerProvider &&
        modelPickerModel &&
        modelIdInList(modelPickerModel, providerModels[modelPickerProvider] ?? []) &&
        !modelPickerLoading &&
        !modelSaving
    )
  );

  // Lazy-load per-pane data when the user actually opens the tab so the
  // initial settings render stays a single RPC (the snapshot).
  $effect(() => {
    if (!daemonReachable) return;
    if (section === "providers" && !modelPickerProvider && defaultRouteProviders.length > 0) {
      modelPickerProvider = defaultRouteProviderId();
    }
    if (section === "permissions" && permissionSnapshot === null && !permissionLoading) {
      void loadPermissionSnapshot();
    }
    if (section === "mcp" && !mcpLoaded && !mcpLoading) {
      void loadMcpServers();
    }
    if (section === "providers" && modelPickerProvider) {
      void loadModelsForProvider(modelPickerProvider);
    }
  });
</script>

<div class="pf-settings">
  <div class="pf-settings-nav">
    {#each navItems as n (n.id)}
      <button type="button" class="pf-settings-nav-item" data-active={section === n.id} onclick={() => (section = n.id)}>
        <Icon name={n.icon} size={14} color="var(--muted-foreground)" />{n.label}
      </button>
    {/each}
  </div>

  <div class="pf-settings-pane">
    {#if section === "general"}
      <h2>General</h2>
      <p class="lead">Workspace roots, configuration files, and session-level preferences.</p>

      <div class="pf-settings-row">
        <div class="meta">
          <div class="label">App name</div>
          <div class="desc">From the active Puffer config.</div>
        </div>
        <div style="justify-self: end; font-family: var(--font-sans); font-size: 12.5px; color: var(--muted-foreground);">
          {props.snapshot?.config.appName ?? "—"}
        </div>
      </div>

      <div class="pf-settings-row">
        <div class="meta">
          <div class="label">Workspace root</div>
          <div class="desc">Where session records and the auth store live.</div>
        </div>
        <div class="pf-path" title={props.snapshot?.workspaceRoot ?? ""}>
          {props.snapshot?.workspaceRoot ?? "—"}
        </div>
      </div>

      <div class="pf-settings-row">
        <div class="meta">
          <div class="label">Daemon</div>
          <div class="desc">The WebSocket endpoint this window is connected to.</div>
        </div>
        <div class="pf-path" title={props.daemonUrl ?? ""}>
          {#if props.daemonUrl}
            <span style="color: var(--foreground);">{props.daemonUrl}</span>
            {#if props.daemonWorkspaceRoot && props.daemonWorkspaceRoot !== props.snapshot?.workspaceRoot}
              <div style="color: var(--muted-foreground); font-size: 11px; margin-top: 2px;">
                -> {props.daemonWorkspaceRoot}
              </div>
            {/if}
          {:else}
            <span style="color: var(--muted-foreground);">not connected</span>
          {/if}
        </div>
      </div>

      <div class="pf-settings-row">
        <div class="meta">
          <div class="label">Config files</div>
          <div class="desc">Resolved workspace + user config paths.</div>
        </div>
        <div class="pf-path-list">
          <div><span class="pf-path-label">workspace</span> <span class="pf-path-value">{props.snapshot?.workspaceConfigFile ?? "—"}</span></div>
          <div><span class="pf-path-label">user</span> <span class="pf-path-value">{props.snapshot?.userConfigFile ?? "—"}</span></div>
          <div><span class="pf-path-label">auth</span> <span class="pf-path-value">{props.snapshot?.authStoreFile ?? "—"}</span></div>
        </div>
      </div>

      <div class="pf-settings-row">
        <div class="meta">
          <div class="label">Account</div>
          <div class="desc">{(props.snapshot?.auth.length ?? 0) === 0 ? "No providers signed in." : "Signed-in providers and session controls."}</div>
        </div>
        <div style="display: flex; flex-direction: column; gap: 6px; justify-self: end; align-items: flex-end;">
          <button
            type="button"
            class="sc-btn"
            data-variant="outline"
            data-size="sm"
            disabled={credentialBusy}
            onclick={refreshIfIdle}
          >
            <Icon name="refresh" size={13} />Refresh
          </button>
          {#each props.snapshot?.auth ?? [] as a (a.providerId)}
            <div style="display: flex; align-items: center; gap: 8px; font-size: 12px;">
              <span style="font-family: var(--font-mono);">{a.providerId}</span>
              <span style="color: var(--muted-foreground);">· {a.kind}{a.email ? ` · ${a.email}` : ""}</span>
              <button
                type="button"
                class="sc-btn"
                data-variant="ghost"
                data-size="sm"
                disabled={credentialBusy}
                onclick={() => props.onLogout(a.providerId)}
              >
                {props.busyProviderId === a.providerId ? "Signing out..." : "Sign out"}
              </button>
            </div>
          {/each}
        </div>
      </div>

      <div class="pf-settings-row">
        <div class="meta">
          <div class="label">Remember last session</div>
          <div class="desc">Reopen the last project + agent when you launch the app.</div>
        </div>
        <div style="display: flex; justify-content: flex-end;">
          <input
            type="checkbox"
            class="sc-switch"
            checked={props.preferences.rememberSession}
            onchange={(e) =>
              props.onPreferenceChange("rememberSession", (e.currentTarget as HTMLInputElement).checked)}
          />
        </div>
      </div>

      <div class="pf-settings-row" style="border-bottom: 0;">
        <div class="meta">
          <div class="label">Reset preferences</div>
          <div class="desc">Clear in-memory desktop tweaks and session-remember flags. Does not touch credentials.</div>
        </div>
        <div style="display: flex; justify-content: flex-end;">
          <button type="button" class="sc-btn" data-variant="outline" data-size="sm" onclick={props.onResetPreferences}>
            Reset
          </button>
        </div>
      </div>

    {:else if section === "providers"}
      <h2>Providers</h2>
      <p class="lead">Connect providers, review their available models, and choose the default route for new turns.</p>

      {#if !daemonReachable}
        <div class="pf-settings-note">
          Preview mode — launch Puffer in the desktop app to edit live routing.
        </div>
      {:else if usingFallbackProviders}
        <div class="pf-settings-note">
          Provider registry is empty. Built-in setup options are available below; refresh after
          resources reload.
        </div>
      {/if}
      <div class="pf-settings-row" style="align-items: start;">
        <div class="meta">
          <div class="label">Default routing</div>
          <div class="desc">Which provider + model new agent turns use when the session doesn't override.</div>
        </div>
        <div style="display: flex; flex-direction: column; gap: 8px; justify-self: end; min-width: 300px;">
          <label style="display: flex; flex-direction: column; gap: 4px; font-size: 11.5px; color: var(--muted-foreground);">
            Provider
            <select
              class="sc-input"
              value={modelPickerProvider}
              disabled={modelPickerDisabled}
              onchange={(e) => {
                modelPickerProvider = (e.currentTarget as HTMLSelectElement).value;
                modelPickerModel = "";
                void loadModelsForProvider(modelPickerProvider);
              }}
            >
              <option value="">— none —</option>
              {#each defaultRouteProviders as p (p.id)}
                <option value={p.id}>{p.displayName} ({p.id})</option>
              {/each}
            </select>
          </label>
          <label style="display: flex; flex-direction: column; gap: 4px; font-size: 11.5px; color: var(--muted-foreground);">
            Model
            <select
              class="sc-input"
              value={modelPickerModel}
              onchange={(e) => (modelPickerModel = (e.currentTarget as HTMLSelectElement).value)}
              disabled={modelPickerDisabled || !modelPickerProvider || modelPickerLoading}
            >
              <option value="">
                {modelPickerLoading
                  ? "Loading models..."
                  : modelPickerNoAgentModels
                    ? "No agent-capable models"
                    : "— pick a model —"}
              </option>
              {#each modelPickerModels as m (m.id)}
                <option value={m.id}>{m.displayName} ({m.id})</option>
              {/each}
            </select>
          </label>
          {#if modelPickerLoading}
            <div class="pf-model-loading-note">
              Fetching {modelPickerProviderName} models...
            </div>
          {:else if modelPickerNoAgentModels}
            <div class="pf-model-loading-note" data-error="true">
              No {modelPickerProviderName} models support agent tools.
            </div>
          {/if}
          <div style="display: flex; justify-content: flex-end; gap: 8px;">
            {#if modelError}
              <span style="color: var(--destructive, #c03232); font-size: 11.5px; align-self: center;">{modelError}</span>
            {/if}
            <button
              type="button"
              class="sc-btn"
              data-variant="default"
              data-size="sm"
              disabled={!canSaveDefaultModel}
              onclick={saveDefaultModel}
            >
              {modelSaving ? "Saving…" : "Save default"}
            </button>
          </div>
        </div>
      </div>

      <LoginView
        snapshot={props.snapshot}
        loading={props.loading}
        remoteEnabled={props.remoteEnabled}
        busyProviderId={props.busyProviderId ?? null}
        errorMessage={props.authError ?? null}
        externals={props.externals ?? []}
        busyImportKey={props.busyImportKey ?? null}
        onLoginOauth={props.onLoginOauth ?? (() => {})}
        onLoginApiKey={props.onApiKeyLogin ?? (() => {})}
        onImportExternal={props.onImportExternal ?? (() => {})}
        onRefresh={props.onRefresh}
      />

    {:else if section === "permissions"}
      <h2>Permissions</h2>
      <p class="lead">
        Tool policies applied before every call. Modes: <strong>allow</strong>
        runs silently, <strong>ask</strong> pauses for approval,
        <strong>deny</strong>/<strong>disabled</strong> blocks the call.
      </p>
      {#if !daemonReachable}
        <div class="pf-settings-note">
          Preview mode — launch Puffer in the desktop app to edit workspace permissions.
        </div>
      {:else if permissionLoading}
        <div class="pf-settings-note">
          Loading permissions...
        </div>
      {:else if permissionSnapshot}
        <div class="pf-settings-note">
          Stored at <code>{permissionSnapshot.path}</code>.
        </div>
      {/if}
      {#if permissionError}
        <div class="pf-settings-note warn">{permissionError}</div>
      {/if}

      <div class="pf-perm-table">
        <div class="pf-perm-row head">
          <span></span>
          <span>Tool id</span>
          <span>Mode</span>
          <span></span>
        </div>
        {#each permissionRows as row, i (i)}
          <div class="pf-perm-row">
            <Icon name={ruleIcon(row.tool)} size={14} color="var(--muted-foreground)" />
            <input
              class="sc-input"
              type="text"
              placeholder="bash, read_file, edit_file…"
              value={row.tool}
              disabled={permissionLoading || permissionSaving || !daemonReachable}
              oninput={(e) => updatePermissionRow(i, "tool", (e.currentTarget as HTMLInputElement).value)}
            />
            <select
              class="sc-input"
              value={row.mode}
              disabled={permissionLoading || permissionSaving || !daemonReachable}
              onchange={(e) => updatePermissionRow(i, "mode", (e.currentTarget as HTMLSelectElement).value)}
            >
              <option value="allow">allow</option>
              <option value="ask">ask</option>
              <option value="deny">deny</option>
              <option value="disabled">disabled</option>
            </select>
            <button
              type="button"
              class="sc-btn"
              data-variant="ghost"
              data-size="sm"
              disabled={permissionLoading || permissionSaving || !daemonReachable}
              onclick={() => removePermissionRow(i)}
              title="Remove rule"
            >
              <Icon name="x" size={12} />
            </button>
          </div>
        {/each}
        {#if permissionRows.length === 0}
          <div class="pf-empty">No tool overrides. Defaults apply to every call.</div>
        {/if}
      </div>

      <div style="display: flex; gap: 8px; margin-top: 14px; justify-content: flex-end;">
        <button
          type="button"
          class="sc-btn"
          data-variant="outline"
          data-size="sm"
          disabled={!daemonReachable || permissionLoading || permissionSaving}
          onclick={addPermissionRow}
        >
          <Icon name="plus" size={12} />Add rule
        </button>
        <button
          type="button"
          class="sc-btn"
          data-variant="default"
          data-size="sm"
          disabled={!permissionDirty || permissionSaving || permissionLoading || !daemonReachable}
          onclick={savePermissionRows}
        >
          {permissionSaving ? "Saving…" : "Save"}
        </button>
      </div>

    {:else if section === "mcp"}
      <h2>MCP Servers</h2>
      <p class="lead">External tools Puffer can pull context from and take actions on.</p>
      {#if mcpError}
        <div class="pf-settings-note warn">{mcpError}</div>
      {/if}
      {#if mcpSaved}
        <div class="pf-settings-note">{mcpSaved}</div>
      {/if}
      <div class="pf-settings-note">
        {#if !daemonReachable}
          Preview mode — launch Puffer in the desktop app to see your workspace's MCP servers.
        {:else if mcpLoading}
          Loading MCP servers…
        {:else}
          {mcpServers.length} MCP server{mcpServers.length === 1 ? "" : "s"} discovered across this workspace's resource roots.
        {/if}
      </div>

      <div class="pf-settings-row" style="align-items: start;">
        <div class="meta">
          <div class="label">Add server</div>
          <div class="desc">Create a declarative MCP manifest in this workspace or your user resource directory.</div>
        </div>
        <div class="pf-mcp-form">
          <div class="pf-mcp-form-grid">
            <label>
              ID
              <input
                class="sc-input"
                placeholder="github"
                value={mcpForm.id}
                disabled={mcpFormDisabled}
                oninput={(e) => (mcpForm.id = (e.currentTarget as HTMLInputElement).value)}
              />
            </label>
            <label>
              Name
              <input
                class="sc-input"
                placeholder="GitHub"
                value={mcpForm.displayName}
                disabled={mcpFormDisabled}
                oninput={(e) => (mcpForm.displayName = (e.currentTarget as HTMLInputElement).value)}
              />
            </label>
            <label>
              Transport
              <select
                class="sc-input"
                value={mcpForm.transport}
                disabled={mcpFormDisabled}
                onchange={(e) =>
                  (mcpForm.transport = (e.currentTarget as HTMLSelectElement).value as "stdio" | "sse" | "http")}
              >
                <option value="stdio">stdio</option>
                <option value="sse">sse</option>
                <option value="http">http</option>
              </select>
            </label>
            <label>
              Scope
              <select
                class="sc-input"
                value={mcpForm.scope}
                disabled={mcpFormDisabled}
                onchange={(e) =>
                  (mcpForm.scope = (e.currentTarget as HTMLSelectElement).value as "local" | "user")}
              >
                <option value="local">workspace</option>
                <option value="user">user</option>
              </select>
            </label>
          </div>
          <label>
            {mcpForm.transport === "stdio" ? "Command" : "URL"}
            <input
              class="sc-input"
              placeholder={mcpForm.transport === "stdio"
                ? "npx @modelcontextprotocol/server-github"
                : "http://127.0.0.1:3000/mcp"}
              value={mcpForm.commandOrUrl}
              disabled={mcpFormDisabled}
              oninput={(e) => (mcpForm.commandOrUrl = (e.currentTarget as HTMLInputElement).value)}
            />
          </label>
          {#if mcpForm.transport === "stdio"}
            <label>
              Arguments
              <input
                class="sc-input"
                placeholder="--flag value"
                value={mcpForm.args}
                disabled={mcpFormDisabled}
                oninput={(e) => (mcpForm.args = (e.currentTarget as HTMLInputElement).value)}
              />
            </label>
          {/if}
          <label>
            Description
            <input
              class="sc-input"
              placeholder="Optional note"
              value={mcpForm.description}
              disabled={mcpFormDisabled}
              oninput={(e) => (mcpForm.description = (e.currentTarget as HTMLInputElement).value)}
            />
          </label>
          <div style="display: flex; justify-content: flex-end;">
            <button
              type="button"
              class="sc-btn"
              data-variant="default"
              data-size="sm"
              disabled={mcpFormDisabled || !mcpForm.id.trim() || !mcpTargetValue()}
              onclick={saveMcpServer}
            >
              <Icon name="plus" size={12} />{mcpSaving ? "Adding…" : "Add server"}
            </button>
          </div>
        </div>
      </div>

      <div class="pf-mcp-list">
        {#each mcpServers as s (s.id)}
          <div class="pf-mcp-card">
            <span class="ico"><Icon name={mcpIcon(s)} size={16} /></span>
            <div>
              <div class="title">{s.displayName}
                <span style="color: var(--muted-foreground); font-family: var(--font-mono); font-size: 11px; margin-left: 6px;">{s.id}</span>
              </div>
              <div class="desc">
                {s.description || `${s.transport || "stdio"} transport`}
                {#if s.endpoint}· {s.endpoint}{/if}
                {#if s.target}· {s.target}{/if}
              </div>
            </div>
            <div style="color: var(--muted-foreground); font-family: var(--font-sans); font-size: 11px;" title={s.sourcePath ?? ""}>
              {s.sourceKind}
            </div>
            <input type="checkbox" class="sc-switch" checked disabled />
          </div>
        {/each}
        {#if !mcpLoading && mcpServers.length === 0}
          <div class="pf-empty">No MCP servers configured.</div>
        {/if}
      </div>

    {:else if section === "git"}
      <h2>Git &amp; PRs</h2>
      <p class="lead">The agent uses these credentials to push branches and open PRs.</p>

      <div class="pf-settings-row">
        <div class="meta">
          <div class="label">Workspace root</div>
          <div class="desc">Sessions created in this workspace inherit this path as their default cwd.</div>
        </div>
        <div class="pf-path" title={props.snapshot?.workspaceRoot ?? ""}>
          {props.snapshot?.workspaceRoot ?? "—"}
        </div>
      </div>

      <div class="pf-settings-row" style="align-items: start;">
        <div class="meta">
          <div class="label">Forge accounts</div>
          <div class="desc">
            Git-hosting providers Puffer recognizes. To add one, connect it from the
            Providers pane using an API key.
          </div>
        </div>
        <div style="display: flex; flex-direction: column; gap: 6px; justify-self: end; align-items: flex-end;">
          {#each GIT_PROVIDER_IDS as providerId (providerId)}
            {@const status = props.snapshot?.auth.find((a) => a.providerId === providerId) ?? null}
            <div style="display: flex; align-items: center; gap: 8px; font-size: 12px;">
              <span style="font-family: var(--font-mono); min-width: 64px; display: inline-block;">{providerId}</span>
              {#if status}
                <span class="pf-model-badge ok"><span class="dot"></span>signed in{status.email ? ` · ${status.email}` : ""}</span>
              {:else}
                <span style="color: var(--muted-foreground);">not connected</span>
              {/if}
            </div>
          {/each}
          {#if !GIT_PROVIDER_IDS.some((id) => authedProviderIds.has(id))}
            <div style="color: var(--muted-foreground); font-size: 11.5px; max-width: 260px; text-align: right;">
              No git provider connected. PR creation still works via <code>gh</code> if it's
              authenticated on the host shell.
            </div>
          {/if}
        </div>
      </div>

      <div class="pf-settings-row" style="border-bottom: 0;">
        <div class="meta">
          <div class="label">Default branch prefix</div>
          <div class="desc">Coming soon — the agent picks branch names freely today.</div>
        </div>
        <input class="sc-input" disabled value="puffer/" style="width: 140px;" />
      </div>

    {:else if section === "appearance"}
      <h2>Appearance</h2>
      <p class="lead">Theme, accent, density, and font mixing.</p>

      <div class="pf-settings-row">
        <div class="meta">
          <div class="label">Theme</div>
          <div class="desc">Choose the base color mode for the app shell.</div>
        </div>
        <div class="pf-appearance-control">
          {#each themes as t (t)}
            <button
              type="button"
              class="pf-choice-pill"
              data-active={props.tweaks.theme === t}
              onclick={() => props.onTweakChange("theme", t)}
            >{t}</button>
          {/each}
        </div>
      </div>

      <div class="pf-settings-row">
        <div class="meta">
          <div class="label">Accent</div>
          <div class="desc">Set the accent color used for selection and emphasis.</div>
        </div>
        <div class="pf-appearance-control">
          {#each accents as a (a.k)}
            <button
              type="button"
              class="pf-color-swatch"
              data-active={props.tweaks.accent === a.k}
              style="background: {a.c};"
              onclick={() => props.onTweakChange("accent", a.k)}
              aria-label={a.k}
              title={a.k}
            ></button>
          {/each}
        </div>
      </div>

      <div class="pf-settings-row">
        <div class="meta">
          <div class="label">Density</div>
          <div class="desc">Adjust spacing for list-heavy and repeated workflows.</div>
        </div>
        <div class="pf-appearance-control">
          {#each densities as d (d)}
            <button
              type="button"
              class="pf-choice-pill"
              data-active={props.tweaks.density === d}
              onclick={() => props.onTweakChange("density", d)}
            >{d}</button>
          {/each}
        </div>
      </div>

      <div class="pf-settings-row">
        <div class="meta">
          <div class="label">Font mix</div>
          <div class="desc">Choose whether interface text stays mixed or uses mono throughout.</div>
        </div>
        <div class="pf-appearance-control">
          {#each fonts as f (f.k)}
            <button
              type="button"
              class="pf-choice-pill"
              data-active={props.tweaks.fontMix === f.k}
              onclick={() => props.onTweakChange("fontMix", f.k)}
            >{f.label}</button>
          {/each}
        </div>
      </div>

      <div class="pf-settings-row">
        <div class="meta">
          <div class="label">User name</div>
          <div class="desc">Shown beside your chat messages.</div>
        </div>
        <input
          class="sc-input"
          value={props.tweaks.userName}
          placeholder="Otter"
          oninput={(e) => props.onTweakChange("userName", (e.currentTarget as HTMLInputElement).value)}
          style="width: 220px;"
        />
      </div>

      <div class="pf-settings-row" style="border-bottom: 0;">
        <div class="meta">
          <div class="label">Reset appearance</div>
          <div class="desc">Restore the default theme, accent, density, font mix, and chat name.</div>
        </div>
        <div style="display: flex; justify-content: flex-end;">
          <button type="button" class="sc-btn" data-variant="outline" data-size="sm" onclick={props.onResetAppearance}>
            Reset
          </button>
        </div>
      </div>

    {:else if section === "shortcuts"}
      <h2>Shortcuts</h2>
      <p class="lead">Keyboard shortcuts the desktop app wires up today.</p>

      <div class="pf-shortcut-list">
        {#each shortcuts as s (s.combo)}
          <div class="pf-shortcut-row">
            <span class="pf-shortcut-combo">{s.combo}</span>
            <span class="pf-shortcut-action">{s.action}</span>
          </div>
        {/each}
      </div>

      <div class="pf-settings-note" style="margin-top: 20px;">
        That's the full list. When the command palette lands we'll document
        its bindings here too — no Lorem-Ipsum placeholders.
      </div>
    {/if}
  </div>
</div>

<style>
  .pf-path {
    justify-self: end;
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--muted-foreground);
    max-width: 320px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    text-align: right;
  }
  .pf-path-list {
    justify-self: end;
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-family: var(--font-mono);
    font-size: 11.5px;
    color: var(--muted-foreground);
    text-align: right;
    max-width: 340px;
    min-width: 0;
  }
  .pf-path-label {
    color: var(--muted-foreground);
    display: inline-block;
    min-width: 74px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    font-size: 10px;
    font-family: var(--font-sans);
  }
  .pf-path-value {
    color: var(--foreground);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    display: inline-block;
    max-width: 240px;
    vertical-align: bottom;
  }
  .pf-settings-note {
    border: 1px solid var(--border);
    background: color-mix(in oklab, var(--muted) 30%, var(--background));
    border-radius: 10px;
    padding: 10px 14px;
    font-size: 12.5px;
    line-height: 1.55;
    color: var(--muted-foreground);
    margin-bottom: 18px;
  }
  .pf-settings-note.warn {
    border-color: color-mix(in oklab, oklch(0.72 0.18 70) 45%, var(--border));
    background: color-mix(in oklab, oklch(0.72 0.18 70) 10%, var(--background));
    color: oklch(0.42 0.15 70);
  }
  .pf-settings-note code {
    font-family: var(--font-mono);
    font-size: 11.5px;
    background: color-mix(in oklab, var(--muted) 55%, transparent);
    padding: 1px 5px;
    border-radius: 4px;
    color: var(--foreground);
  }
  .pf-model-loading-note {
    border: 1px solid color-mix(in oklab, var(--accent) 28%, var(--border));
    border-radius: 8px;
    background: color-mix(in oklab, var(--accent) 7%, var(--background));
    color: var(--muted-foreground);
    font-size: 11.5px;
    line-height: 1.4;
    padding: 7px 9px;
  }
  .pf-model-loading-note[data-error="true"] {
    border-color: color-mix(in oklab, var(--destructive, #c03232) 32%, var(--border));
    background: color-mix(in oklab, var(--destructive, #c03232) 7%, var(--background));
    color: var(--destructive, #c03232);
  }
  .pf-model-badge {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 2px 8px;
    font-family: var(--font-sans);
    font-size: 10.5px;
    border-radius: 999px;
  }
  .pf-model-badge.ok {
    background: color-mix(in oklab, oklch(0.7 0.18 145) 16%, transparent);
    color: oklch(0.42 0.15 145);
  }
  .pf-model-badge .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: oklch(0.65 0.18 145);
  }
  .pf-empty {
    padding: 16px;
    border: 1px dashed var(--border);
    border-radius: 10px;
    color: var(--muted-foreground);
    font-size: 13px;
    text-align: center;
  }
  .pf-shortcut-list {
    display: flex;
    flex-direction: column;
    gap: 4px;
    border: 1px solid var(--border);
    border-radius: 10px;
    overflow: hidden;
  }
  .pf-shortcut-row {
    display: grid;
    grid-template-columns: 180px 1fr;
    padding: 10px 14px;
    font-size: 13px;
    border-bottom: 1px solid var(--border);
    align-items: center;
  }
  .pf-shortcut-row:last-child {
    border-bottom: 0;
  }
  .pf-shortcut-combo {
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--foreground);
  }
  .pf-shortcut-action {
    color: var(--muted-foreground);
    font-size: 12.5px;
  }
  .pf-appearance-control {
    justify-self: end;
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 6px;
    flex-wrap: wrap;
    max-width: 360px;
  }
  .pf-choice-pill {
    padding: 4px 9px;
    border-radius: 999px;
    border: 1px solid var(--border);
    background: transparent;
    font-size: 11px;
    cursor: pointer;
    color: var(--foreground);
    font-family: var(--font-sans);
    font-weight: 500;
  }
  .pf-choice-pill:hover {
    background: var(--pf-selected-bg-hover);
    font-weight: 700;
  }
  .pf-choice-pill[data-active="true"] {
    background: var(--pf-selected-bg);
    color: var(--foreground);
    border-color: transparent;
    font-weight: 700;
  }
  .pf-color-swatch {
    width: 26px;
    height: 26px;
    border-radius: 7px;
    border: 1px solid var(--border);
    cursor: pointer;
    position: relative;
  }
  .pf-color-swatch[data-active="true"]::after {
    content: "";
    position: absolute;
    inset: -4px;
    border: 2px solid var(--foreground);
    border-radius: 11px;
  }
</style>

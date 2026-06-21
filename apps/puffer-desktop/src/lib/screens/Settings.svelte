<script lang="ts">
  import "../design/settings.css";

  import { onDestroy } from "svelte";
  import Icon, { type IconName } from "../design/Icon.svelte";
  import LoginView from "../components/LoginView.svelte";
  import LocalModelSetupCard from "../components/LocalModelSetupCard.svelte";
  import BrowserSettings from "./settings/BrowserSettings.svelte";
  import NetworkSettings from "./settings/NetworkSettings.svelte";
  import SecretsSettings from "./settings/SecretsSettings.svelte";
  import BrowserPane from "./agent/BrowserPane.svelte";
  import { focusTrap } from "../focusTrap";
  import {
    providerIdCanRunAgent,
    providerIsAvailableForAgent,
    providerIdsEquivalent
  } from "../providerIds";
  import { providerCatalogForSetup, usesFallbackProviderCatalog } from "../providerFallbacks";
  import type { AccentKey, DensityKey, FontMixKey, ThemeKey, Tweaks } from "../shell/tweaks";
  import {
    addMcpServer,
    cancelTurn,
    createMonitor,
    deleteMonitor,
    deleteWorkflowConnection,
    isDaemonReachable,
    listLambdaSkillLibraries,
    listMcpServers,
    listPermissions,
    listProviderModels,
    loadWorkflowSnapshot,
    removeLambdaSkillLibrary,
    resolveUserQuestion,
    saveLambdaSkillLibrary,
    savePermissions,
    setLambdaSkillApproval,
    setLambdaSkillEnabled,
    startConnectorSetupCommand,
    updateConfig,
    type LambdaSkillLibraryInfo,
    type LambdaSkillLibrariesSnapshot,
    type LambdaVerifiedSkillInfo,
    type McpServerInfo,
    type ModelDescriptorInfo,
    type PermissionsSnapshot
  } from "../api/desktop";
  import { canInvokeTauri, currentDaemonClient } from "../api/daemonClient";
  import { subscribeConnectorSetupEvents, type SessionStreamEvent } from "../api/sessionEvents";
  import type {
    BrowserRenderer,
    DesktopPreferences,
    ExternalCredential,
    RemoteOperation,
    SettingsSnapshot,
    WorkflowConnector,
    WorkflowSnapshot
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
    onApiKeyLogin?: (
      providerId: string,
      apiKey: string,
      options?: { baseUrl?: string | null; displayName?: string | null }
    ) => void;
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
    if (section === "mcp" && daemonReachable && !mcpLoading && !mcpSaving) {
      void loadMcpServers();
    }
    if (section === "skills" && daemonReachable && !lambdaLoading && !lambdaSaving && lambdaRemovingLibrary === null) {
      void loadLambdaSkillLibraries();
    }
    if (section === "connectors" && daemonReachable && !connectorLoading && !connectorCreating) {
      void loadConnectorSnapshot();
    }
    props.onRefresh();
  }

  type Section = "general" | "providers" | "secrets" | "network" | "browser" | "connectors" | "permissions" | "skills" | "mcp" | "git" | "appearance" | "shortcuts";
  let section = $state<Section>("general");

  const navItems: { id: Section; label: string; icon: IconName }[] = [
    { id: "general",     label: "General",    icon: "settings" },
    { id: "providers",   label: "Providers",  icon: "plug" },
    { id: "secrets",     label: "Secrets",    icon: "key" },
    { id: "network",     label: "Network",    icon: "globe" },
    { id: "browser",     label: "Browser",    icon: "globe" },
    { id: "connectors",  label: "Connectors", icon: "server" },
    { id: "permissions", label: "Permissions", icon: "bolt" },
    { id: "skills",      label: "Verified Skills", icon: "shield" },
    { id: "mcp",         label: "MCP Servers", icon: "plug" },
    { id: "git",         label: "Git & PRs",  icon: "git" },
    { id: "appearance",  label: "Appearance", icon: "layers" },
    { id: "shortcuts",   label: "Shortcuts",  icon: "panel" }
  ];

  type ConnectorQuestionOption = {
    label: string;
    description?: string;
    preview?: string;
  };

  type ConnectorQuestion = {
    header: string;
    question: string;
    type: "input" | "choice";
    options: ConnectorQuestionOption[];
    multiSelect: boolean;
    secret?: boolean;
  };

  type ConnectorMarkdownPart =
    | { kind: "text"; text: string }
    | { kind: "image"; alt: string; src: string };

  type ConnectorQuestionRequest = {
    turnId: string;
    requestId: string;
    questions: ConnectorQuestion[];
    browserSessionId?: string;
    browserTabId?: string;
    browserUrl?: string;
  };

  type ConnectorTab = "connections" | "catalog";

  // Live permissions loaded from the daemon. `permissionRows` is the
  // editable working copy — changes are staged in memory and flushed on
  // Save. Loading state and generation guards keep late responses from
  // clobbering in-progress edits.
  let permissionSnapshot = $state<PermissionsSnapshot | null>(null);
  let permissionRows = $state<{ tool: string; mode: string }[]>([]);
  let permissionLoading = $state(false);
  let permissionLoaded = $state(false);
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

  let connectorSnapshot = $state<WorkflowSnapshot | null>(null);
  let connectorLoaded = $state(false);
  let connectorLoading = $state(false);
  let connectorLoadGeneration = 0;
  let connectorError = $state<string | null>(null);
  let connectorSaved = $state<string | null>(null);
  let connectorCreateOpen = $state(false);
  let connectorTab = $state<ConnectorTab>("connections");
  let connectorSlug = $state("");
  let connectorConnectionSlug = $state("");
  let connectorCreating = $state(false);
  let connectorTurnId = $state<string | null>(null);
  let connectorSetupSlug = $state<string | null>(null);
  let connectorQuestionRequest = $state<ConnectorQuestionRequest | null>(null);
  let connectorQuestionAnswers = $state<Record<string, string | string[]>>({});
  let connectorUnlisten: (() => void) | null = null;
  let connectorCancelledTurnIds = new Set<string>();
  let connectorDeleting = $state<string | null>(null);
  let connectorMonitoring = $state<string | null>(null);
  let lastConnectorSlug = "";

  let lambdaSnapshot = $state<LambdaSkillLibrariesSnapshot | null>(null);
  let lambdaLoaded = $state(false);
  let lambdaLoading = $state(false);
  let lambdaLoadGeneration = 0;
  let lambdaSaving = $state(false);
  let lambdaRemovingLibrary = $state<string | null>(null);
  let lambdaTogglingSkill = $state<string | null>(null);
  let lambdaTogglingApproval = $state<string | null>(null);
  let lambdaError = $state<string | null>(null);
  let lambdaSaved = $state<string | null>(null);

  // Per-provider model listings cached by providerId. Populated on demand
  // when the user expands the Providers pane.
  let providerModels = $state<Record<string, ModelDescriptorInfo[]>>({});
  let modelLoadingByProvider = $state<Record<string, boolean>>({});
  let modelPickerProvider = $state<string>("");
  let modelPickerModel = $state<string>("");
  let modelSaving = $state(false);
  let modelError = $state<string | null>(null);
  let connectors = $derived(connectorSnapshot?.connectors ?? []);
  let connections = $derived(connectorSnapshot?.connections ?? []);
  let selectedConnector = $derived(
    connectors.find((connector) => connector.connector_slug === connectorSlug) ?? connectors[0] ?? null
  );
  let selectedConnectorConnections = $derived(
    selectedConnector
      ? connections.filter((connection) => connection.connector_slug === selectedConnector.connector_slug)
      : []
  );
  let connectorConnectionSlugInvalid = $derived(!connectionSlugValid(connectorConnectionSlug));
  let connectorCommandPreview = $derived(
    selectedConnector && !connectorConnectionSlugInvalid
      ? `/connect ${selectedConnector.connector_slug} ${connectorConnectionSlug.trim()}`
      : ""
  );

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

  async function loadConnectorSnapshot() {
    const generation = ++connectorLoadGeneration;
    connectorLoading = true;
    connectorError = null;
    try {
      const snap = await loadWorkflowSnapshot();
      if (generation !== connectorLoadGeneration) return;
      connectorSnapshot = snap;
      connectorLoaded = true;
      ensureConnectorSelection(snap.connectors ?? []);
    } catch (e) {
      if (generation === connectorLoadGeneration) {
        connectorError = (e as Error).message ?? String(e);
      }
    } finally {
      if (generation === connectorLoadGeneration) {
        connectorLoading = false;
      }
    }
  }

  function ensureConnectorSelection(list = connectors) {
    const selected = list.find((connector) => connector.connector_slug === connectorSlug) ?? list[0] ?? null;
    if (!selected) {
      connectorSlug = "";
      connectorConnectionSlug = "";
      return;
    }
    if (connectorSlug !== selected.connector_slug) {
      connectorSlug = selected.connector_slug;
    }
    if (!connectorConnectionSlug.trim() || lastConnectorSlug !== connectorSlug) {
      connectorConnectionSlug = connectorConnectionHint(selected);
    }
    lastConnectorSlug = connectorSlug;
  }

  function selectConnector(slug: string) {
    const next = connectors.find((connector) => connector.connector_slug === slug) ?? null;
    connectorSlug = slug;
    connectorConnectionSlug = next ? connectorConnectionHint(next) : "";
    connectorQuestionRequest = null;
    connectorQuestionAnswers = {};
    connectorSaved = null;
    lastConnectorSlug = slug;
  }

  function openConnectorCreate() {
    if (!daemonReachable || connectorLoading || connectorCreating || connectors.length === 0) return;
    ensureConnectorSelection();
    connectorError = null;
    connectorSaved = null;
    connectorCreateOpen = true;
  }

  function closeConnectorCreate() {
    connectorCreateOpen = false;
  }

  function connectorConnectionHint(connector: WorkflowConnector): string {
    return connector.suggested_connection_slug || connector.connector_slug;
  }

  function connectionSlugValid(value: string): boolean {
    return /^[a-z0-9][a-z0-9-]*$/.test(value.trim());
  }

  function connectorStatusLabel(connector: WorkflowConnector): string {
    if (connector.can_trigger_workflow ?? connector.can_subscribe) return "workflow";
    if (connector.can_proxy_agent) return "proxy";
    return connector.requires_auth ? "auth" : "available";
  }

  function connectorQuestionKey(question: ConnectorQuestion): string {
    return question.question;
  }

  function connectorMarkdownParts(value: string | undefined): ConnectorMarkdownPart[] {
    if (!value) return [];
    const parts: ConnectorMarkdownPart[] = [];
    const imagePattern = /!\[([^\]\n]*)\]\(([^)\s]+)\)/g;
    let cursor = 0;
    for (const match of value.matchAll(imagePattern)) {
      const start = match.index ?? 0;
      if (start > cursor) {
        parts.push({ kind: "text", text: value.slice(cursor, start) });
      }
      const src = safeConnectorImageSrc(match[2] ?? "");
      if (src) {
        parts.push({ kind: "image", alt: match[1] ?? "", src });
      } else {
        parts.push({ kind: "text", text: match[0] });
      }
      cursor = start + match[0].length;
    }
    if (cursor < value.length) {
      parts.push({ kind: "text", text: value.slice(cursor) });
    }
    return parts.length > 0 ? parts : [{ kind: "text", text: value }];
  }

  function safeConnectorImageSrc(value: string): string | null {
    const src = value.trim();
    if (/^data:image\/(?:png|jpe?g|gif|webp|svg\+xml);base64,[a-z0-9+/=]+$/i.test(src)) return src;
    if (/^https?:\/\//i.test(src)) return src;
    return null;
  }

  function normalizeConnectorQuestions(rawQuestions: unknown[]): ConnectorQuestion[] {
    return rawQuestions
      .map((raw): ConnectorQuestion | null => {
        if (typeof raw !== "object" || raw === null) return null;
        const record = raw as Record<string, unknown>;
        const question = typeof record.question === "string" ? record.question : "";
        if (!question.trim()) return null;
        const options = Array.isArray(record.options)
          ? record.options
              .map((option): ConnectorQuestionOption | null => {
                if (typeof option !== "object" || option === null) return null;
                const optionRecord = option as Record<string, unknown>;
                const label = typeof optionRecord.label === "string" ? optionRecord.label : "";
                if (!label.trim()) return null;
                return {
                  label,
                  description: typeof optionRecord.description === "string" ? optionRecord.description : undefined,
                  preview: typeof optionRecord.preview === "string" ? optionRecord.preview : undefined
                };
              })
              .filter((option): option is ConnectorQuestionOption => option !== null)
          : [];
        const type = record.type === "input"
          ? "input"
          : record.type === "choice"
            ? "choice"
            : options.length === 0
              ? "input"
              : "choice";
        return {
          header: typeof record.header === "string" && record.header.trim() ? record.header : "Question",
          question,
          type,
          options,
          multiSelect: record.multiSelect === true,
          secret: record.secret === true
        };
      })
      .filter((question): question is ConnectorQuestion => question !== null);
  }

  function defaultConnectorAnswers(questions: ConnectorQuestion[]): Record<string, string | string[]> {
    const answers: Record<string, string | string[]> = {};
    for (const question of questions) {
      const key = connectorQuestionKey(question);
      answers[key] = question.multiSelect
        ? []
        : question.type === "choice"
          ? question.options[0]?.label ?? ""
          : "";
    }
    return answers;
  }

  function connectorQuestionInputType(question: ConnectorQuestion): "text" | "password" {
    if (question.secret === true) return "password";
    const text = `${question.header} ${question.question}`.toLowerCase();
    return /password|secret|token|cookie|key|code|xox|credential/.test(text) ? "password" : "text";
  }

  function updateConnectorAnswer(question: ConnectorQuestion, value: string) {
    connectorQuestionAnswers = {
      ...connectorQuestionAnswers,
      [connectorQuestionKey(question)]: value
    };
  }

  function connectorAnswerText(question: ConnectorQuestion): string {
    const value = connectorQuestionAnswers[connectorQuestionKey(question)];
    return typeof value === "string" ? value : "";
  }

  function connectorAnswerIncludes(question: ConnectorQuestion, value: string): boolean {
    const current = connectorQuestionAnswers[connectorQuestionKey(question)];
    return Array.isArray(current) ? current.includes(value) : current === value;
  }

  function toggleConnectorMultiAnswer(question: ConnectorQuestion, value: string, checked: boolean) {
    const key = connectorQuestionKey(question);
    const current = Array.isArray(connectorQuestionAnswers[key])
      ? connectorQuestionAnswers[key] as string[]
      : [];
    const next = checked
      ? Array.from(new Set([...current, value]))
      : current.filter((item) => item !== value);
    connectorQuestionAnswers = {
      ...connectorQuestionAnswers,
      [key]: next
    };
  }

  function connectorAnswersComplete(): boolean {
    if (!connectorQuestionRequest) return false;
    return connectorQuestionRequest.questions.every((question) => {
      const value = connectorQuestionAnswers[connectorQuestionKey(question)];
      if (Array.isArray(value)) return question.options.length === 0 || value.length > 0;
      return question.type === "choice" || Boolean(value?.trim());
    });
  }

  function connectorQuestionIsActionOnly(question: ConnectorQuestion): boolean {
    return question.type === "choice" && question.options.length === 0;
  }

  function connectorQuestionSubmitLabel(): string {
    if (connectorCreating) return "Continuing...";
    if (!connectorQuestionRequest) return "Submit answers";
    if (activeConnectorSetupUsesBrowser() && connectorQuestionRequest.questions.some(connectorQuestionIsActionOnly)) {
      return "Continue";
    }
    return connectorQuestionRequest.browserSessionId ? "Continue setup" : "Submit answers";
  }

  function activeConnectorSetupSlug(): string {
    return connectorSetupSlug ?? selectedConnector?.connector_slug ?? connectorSlug;
  }

  function activeConnectorSetupUsesBrowser(): boolean {
    return ["gmail-browser", "gcal-browser", "wechat-login"].includes(activeConnectorSetupSlug());
  }

  function connectorQuestionStatusMessage(questions: ConnectorQuestion[]): string {
    if (activeConnectorSetupSlug() === "gmail-browser") {
      const prompt = questions
        .map((question) => `${question.header} ${question.question}`)
        .join(" ")
        .toLowerCase();
      if (prompt.includes("sign in")) {
        return "Gmail setup is waiting for browser sign-in in the Puffer profile.";
      }
      if (prompt.includes("accounts")) {
        return "Gmail setup found Google accounts in the Puffer profile.";
      }
      return "Gmail setup is waiting for the next browser-profile answer.";
    }
    if (activeConnectorSetupSlug() === "gcal-browser") {
      const prompt = questions
        .map((question) => `${question.header} ${question.question}`)
        .join(" ")
        .toLowerCase();
      if (prompt.includes("sign in")) {
        return "Google Calendar setup is waiting for browser sign-in in the Puffer profile.";
      }
      if (prompt.includes("accounts")) {
        return "Google Calendar setup found Google accounts in the Puffer profile.";
      }
      return "Google Calendar setup is waiting for the next browser-profile answer.";
    }
    if (activeConnectorSetupSlug() !== "telegram-login") {
      return "Answer the connector setup questions to continue.";
    }
    const prompt = questions
      .map((question) => `${question.header} ${question.question}`)
      .join(" ")
      .toLowerCase();
    if (prompt.includes("authenticate")) {
      return "Telegram setup is waiting for you to choose Desktop import, QR login, or phone login.";
    }
    if (prompt.includes("qr")) {
      return "Telegram setup is waiting for QR approval in an already logged-in Telegram app.";
    }
    if (prompt.includes("phone")) {
      return "Telegram setup is waiting for the account phone number.";
    }
    if (prompt.includes("code")) {
      return "Telegram setup is waiting for the one-time login code from Telegram.";
    }
    if (prompt.includes("password")) {
      return "Telegram setup is waiting for the account's Telegram 2FA cloud password.";
    }
    return "Telegram setup is waiting for the next login answer.";
  }

  function connectorSetupErrorMessage(error: string): string {
    if (activeConnectorSetupSlug() !== "telegram-login") return error;
    const lower = error.toLowerCase();
    if (lower.includes("telegram-user subscriber manifest not found")) {
      return [
        "Telegram setup could not start because the local telegram-user subscriber manifest is unavailable.",
        error,
        "The daemon log now includes a [telegram-connect] line with the searched paths."
      ].join(" ");
    }
    if (lower.includes("telegram subscriber manifest")) {
      return [
        "Telegram setup found a subscriber manifest but could not read it.",
        error,
        "Check the manifest path and daemon stderr for [telegram-connect] details."
      ].join(" ");
    }
    if (lower.includes("start telegram subscriber")) {
      return [
        "Telegram setup found its manifest but could not start the local subscriber process.",
        error,
        "Check the daemon stderr for [telegram-connect] details."
      ].join(" ");
    }
    return error;
  }

  function handleConnectorSessionEvent(event: SessionStreamEvent) {
    if (connectorTurnId && "turnId" in event && event.turnId !== connectorTurnId) return;
    // Progress lines from long-running setups (e.g. WeChat container bringup /
    // install) that block before the first question; keep the user informed.
    const progress = event as unknown as { type?: string; status?: string };
    if (progress.type === "connector-setup-status") {
      if (typeof progress.status === "string" && progress.status.length > 0) {
        connectorSaved = progress.status;
      }
      return;
    }
    if (event.type === "user-question-request") {
      const questions = normalizeConnectorQuestions(event.questions);
      connectorQuestionRequest = {
        turnId: event.turnId,
        requestId: event.requestId,
        questions,
        browserSessionId: typeof event.browserSessionId === "string" ? event.browserSessionId : undefined,
        browserTabId: typeof event.browserTabId === "string" ? event.browserTabId : undefined,
        browserUrl: typeof event.browserUrl === "string" ? event.browserUrl : undefined
      };
      connectorQuestionAnswers = defaultConnectorAnswers(questions);
      connectorCreating = false;
      connectorCreateOpen = false;
      connectorSaved = connectorQuestionStatusMessage(questions);
      return;
    }
    if (event.type === "turn-complete") {
      connectorCreating = false;
      connectorCreateOpen = false;
      connectorQuestionRequest = null;
      connectorQuestionAnswers = {};
      connectorSaved = `Connector setup finished for ${connectorConnectionSlug.trim()}.`;
      connectorSetupSlug = null;
      connectorTab = "connections";
      void loadConnectorSnapshot();
      return;
    }
    if (event.type === "turn-error") {
      connectorCreating = false;
      connectorCreateOpen = false;
      connectorQuestionRequest = null;
      if ("turnId" in event && connectorCancelledTurnIds.has(event.turnId)) {
        connectorCancelledTurnIds.delete(event.turnId);
        return;
      }
      connectorError = connectorSetupErrorMessage(event.error);
      connectorSaved = null;
      connectorSetupSlug = null;
    }
  }

  function newConnectorSetupId(): string {
    if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
      return crypto.randomUUID();
    }
    return `connector-${Date.now()}-${Math.random().toString(36).slice(2)}`;
  }

  async function startConnectorSetup() {
    if (!canStartConnectorSetup) return;
    connectorCreating = true;
    connectorError = null;
    connectorSaved = `Starting ${connectorCommandPreview}...`;
    connectorSetupSlug = selectedConnector?.connector_slug ?? connectorSlug;
    connectorQuestionRequest = null;
    connectorQuestionAnswers = {};
    connectorCreateOpen = false;
    try {
      connectorUnlisten?.();
      connectorUnlisten = null;
      const setupId = newConnectorSetupId();
      connectorTurnId = setupId;
      connectorUnlisten = await subscribeConnectorSetupEvents(setupId, handleConnectorSessionEvent);
      connectorTurnId = await startConnectorSetupCommand(setupId, connectorCommandPreview);
    } catch (e) {
      connectorCreating = false;
      connectorUnlisten?.();
      connectorUnlisten = null;
      connectorTurnId = null;
      connectorError = connectorSetupErrorMessage((e as Error).message ?? String(e));
      connectorSaved = null;
      connectorSetupSlug = null;
    }
  }

  async function submitConnectorAnswers() {
    if (!connectorQuestionRequest || !connectorAnswersComplete()) return;
    connectorCreating = true;
    connectorSaved = "Continuing connector setup...";
    try {
      await resolveUserQuestion(
        connectorQuestionRequest.turnId,
        connectorQuestionRequest.requestId,
        connectorQuestionAnswers,
        {}
      );
    } catch (e) {
      connectorCreating = false;
      connectorError = connectorSetupErrorMessage((e as Error).message ?? String(e));
      connectorSaved = null;
    }
  }

  async function cancelConnectorSetup() {
    const turnId = connectorTurnId;
    connectorQuestionRequest = null;
    connectorQuestionAnswers = {};
    connectorCreating = false;
    connectorSaved = "Connector setup cancelled.";
    connectorSetupSlug = null;
    if (turnId) {
      connectorCancelledTurnIds.add(turnId);
      try {
        await cancelTurn(turnId);
      } catch (e) {
        connectorError = (e as Error).message ?? String(e);
      }
    }
    connectorTurnId = null;
  }

  async function removeConnectorConnection(slug: string) {
    if (!daemonReachable || !slug || connectorDeleting) return;
    connectorDeleting = slug;
    connectorError = null;
    try {
      const snap = await deleteWorkflowConnection(slug);
      connectorSnapshot = snap;
      ensureConnectorSelection(snap.connectors ?? []);
      connectorSaved = `Removed connection ${slug}.`;
    } catch (e) {
      connectorError = (e as Error).message ?? String(e);
    } finally {
      connectorDeleting = null;
    }
  }

  // Starts (or stops) a read-only monitor for a connection. Creating a monitor
  // adds an enabled, no-auto-reply consumer, which is what actually spawns the
  // connector's subscribe stream (a connection alone is authenticated but idle).
  async function toggleConnectionMonitor(slug: string, active: boolean) {
    if (!daemonReachable || !slug || connectorMonitoring) return;
    connectorMonitoring = slug;
    connectorError = null;
    try {
      const snap = active ? await deleteMonitor(`monitor-${slug}`) : await createMonitor(slug);
      connectorSnapshot = snap;
      ensureConnectorSelection(snap.connectors ?? []);
      connectorSaved = active
        ? `Stopped monitoring ${slug}.`
        : `Started monitoring ${slug}. New incoming messages will be picked up.`;
    } catch (e) {
      connectorError = (e as Error).message ?? String(e);
    } finally {
      connectorMonitoring = null;
    }
  }

  async function loadLambdaSkillLibraries() {
    const generation = ++lambdaLoadGeneration;
    lambdaLoading = true;
    lambdaError = null;
    try {
      const snap = await withTimeout(
        listLambdaSkillLibraries(),
        15000,
        "Verified Skills are still loading. Try Refresh again in a moment."
      );
      if (generation !== lambdaLoadGeneration) return;
      lambdaSnapshot = snap;
    } catch (e) {
      if (generation === lambdaLoadGeneration) {
        lambdaError = (e as Error).message ?? String(e);
      }
    } finally {
      if (generation === lambdaLoadGeneration) {
        lambdaLoaded = true;
        lambdaLoading = false;
      }
    }
  }

  function basenameFromPath(path: string): string {
    return path.split(/[\\/]/).filter(Boolean).at(-1) ?? "Verified Skills";
  }

  function slugFromPath(path: string): string {
    const slug = basenameFromPath(path)
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "");
    return slug || "verified-skills";
  }

  function shortPathHash(path: string): string {
    let hash = 2166136261;
    for (let i = 0; i < path.length; i += 1) {
      hash ^= path.charCodeAt(i);
      hash = Math.imul(hash, 16777619);
    }
    return (hash >>> 0).toString(36).slice(0, 6);
  }

  function verifiedSkillIdFromPath(path: string): string {
    return `${slugFromPath(path)}-${shortPathHash(path)}`;
  }

  function normalizedFolderPath(path: string): string {
    return path.replace(/\\/g, "/").replace(/\/+$/g, "");
  }

  function pathContains(parent: string, child: string): boolean {
    const normalizedParent = normalizedFolderPath(parent);
    const normalizedChild = normalizedFolderPath(child);
    return normalizedChild === normalizedParent || normalizedChild.startsWith(`${normalizedParent}/`);
  }

  function coveringVerifiedSkillLibrary(path: string): LambdaSkillLibraryInfo | null {
    const libraries = lambdaSnapshot?.libraries ?? [];
    return libraries.find((library) => pathContains(library.root, path)) ?? null;
  }

  function lambdaLibraryKey(library: LambdaSkillLibraryInfo): string {
    return `${library.sourceKind}:${library.id}`;
  }

  function verifiedSkillStatus(library: LambdaSkillLibraryInfo): string {
    if (library.disableModelInvocation) return "Installed";
    if (library.allowedTools.length > 0 && library.hostCatalogueSubpath) {
      return "Ready for model use";
    }
    return "Needs verification output";
  }

  function verifiedSkillScopeLabel(library: LambdaSkillLibraryInfo): string {
    return library.sourceKind === "user" ? "User" : "Workspace";
  }

  function verifiedSkillKey(skill: LambdaVerifiedSkillInfo): string {
    return `${skill.sourceKind ?? "unknown"}:${skill.libraryId ?? "none"}:${skill.name}`;
  }

  function verifiedSkillReadinessLabel(skill: LambdaVerifiedSkillInfo): string {
    if (skill.modelInvocable) return "Enabled";
    if (!skill.enabled) return "Disabled";
    if (skill.ready) return "Configured";
    return "Needs attention";
  }

  function verifiedSkillDetailLabel(skill: LambdaVerifiedSkillInfo): string {
    const scope = skill.sourceKind === "user" ? "User" : "Workspace";
    const counts = [
      skill.tools != null ? `${skill.tools} tools` : null,
      skill.actions != null ? `${skill.actions} actions` : null
    ].filter(Boolean);
    const approval = skill.requireApproval ? "asks before tools" : "no extra approval";
    return counts.length > 0
      ? `${scope} Verified Skill · ${counts.join(" · ")} · ${approval}`
      : `${scope} Verified Skill · ${approval}`;
  }

  function withTimeout<T>(promise: Promise<T>, ms: number, message: string): Promise<T> {
    return Promise.race([
      promise,
      new Promise<T>((_, reject) => {
        setTimeout(() => reject(new Error(message)), ms);
      })
    ]);
  }

  async function pickVerifiedSkillsFolder(): Promise<string | null> {
    if (!canInvokeTauri()) {
      throw new Error("Folder picker is only available in the desktop app.");
    }
    const { open } = await import("@tauri-apps/plugin-dialog");
    const picked = await open({ directory: true, multiple: false });
    return typeof picked === "string" && picked.length > 0 ? picked : null;
  }

  async function addVerifiedSkillsFolder() {
    if (lambdaSaving) return;
    let root: string | null = null;
    try {
      root = await pickVerifiedSkillsFolder();
    } catch (e) {
      lambdaError = (e as Error).message ?? String(e);
      return;
    }
    if (!root) return;
    const coveringLibrary = coveringVerifiedSkillLibrary(root);
    if (coveringLibrary) {
      lambdaError = null;
      lambdaSaved = pathContains(root, coveringLibrary.root)
        ? `${basenameFromPath(root)} is already added.`
        : `${basenameFromPath(root)} is already covered by ${basenameFromPath(coveringLibrary.root)}.`;
      return;
    }
    lambdaLoadGeneration += 1;
    lambdaLoading = false;
    lambdaSaving = true;
    lambdaError = null;
    lambdaSaved = null;
    try {
      lambdaSnapshot = await withTimeout(
        saveLambdaSkillLibrary({
          id: verifiedSkillIdFromPath(root),
          root,
          scope: "workspace",
          userInvocable: true,
          disableModelInvocation: false,
          requireApproval: false
        }),
        15000,
        "Adding this Verified Skills folder is still running. Try Refresh again in a moment."
      );
      lambdaLoaded = true;
      lambdaSaved = `Added ${basenameFromPath(root)}`;
      props.onRefresh();
    } catch (e) {
      lambdaError = (e as Error).message ?? String(e);
    } finally {
      lambdaSaving = false;
    }
  }

  async function removeVerifiedSkillsFolder(library: LambdaSkillLibraryInfo) {
    if (lambdaRemovingLibrary || lambdaSaving) return;
    if (library.sourceKind !== "workspace" && library.sourceKind !== "user") return;
    const key = lambdaLibraryKey(library);
    lambdaLoadGeneration += 1;
    lambdaLoading = false;
    lambdaRemovingLibrary = key;
    lambdaError = null;
    lambdaSaved = null;
    try {
      lambdaSnapshot = await withTimeout(
        removeLambdaSkillLibrary({
          libraryId: library.id,
          sourceKind: library.sourceKind
        }),
        15000,
        "Removing this Verified Skills folder is still running. Try Refresh again in a moment."
      );
      lambdaLoaded = true;
      lambdaSaved = `Removed ${basenameFromPath(library.root)}`;
      props.onRefresh();
    } catch (e) {
      lambdaError = (e as Error).message ?? String(e);
      void loadLambdaSkillLibraries();
    } finally {
      lambdaRemovingLibrary = null;
    }
  }

  async function toggleVerifiedSkill(skill: LambdaVerifiedSkillInfo, enabled: boolean) {
    if (!skill.libraryId || (skill.sourceKind !== "workspace" && skill.sourceKind !== "user")) {
      lambdaError = "This Verified Skill is not backed by an editable folder config.";
      return;
    }
    const key = verifiedSkillKey(skill);
    lambdaTogglingSkill = key;
    lambdaError = null;
    lambdaSaved = null;
    try {
      lambdaSnapshot = await setLambdaSkillEnabled({
        libraryId: skill.libraryId,
        sourceKind: skill.sourceKind,
        skillName: skill.name,
        enabled
      });
      lambdaLoaded = true;
      lambdaSaved = `${enabled ? "Enabled" : "Disabled"} ${skill.name}`;
      props.onRefresh();
    } catch (e) {
      lambdaError = (e as Error).message ?? String(e);
      void loadLambdaSkillLibraries();
    } finally {
      lambdaTogglingSkill = null;
    }
  }

  async function toggleVerifiedSkillApproval(library: LambdaSkillLibraryInfo, requireApproval: boolean) {
    if (library.sourceKind !== "workspace" && library.sourceKind !== "user") {
      lambdaError = "This Verified Skills folder is not backed by an editable config.";
      return;
    }
    const key = lambdaLibraryKey(library);
    lambdaTogglingApproval = key;
    lambdaError = null;
    lambdaSaved = null;
    try {
      lambdaSnapshot = await withTimeout(
        setLambdaSkillApproval({
          libraryId: library.id,
          sourceKind: library.sourceKind,
          requireApproval
        }),
        15000,
        "Updating this Verified Skills approval setting is still running. Try Refresh again in a moment."
      );
      lambdaLoaded = true;
      lambdaSaved = requireApproval
        ? `Extra approval enabled for ${basenameFromPath(library.root)}`
        : `Verified calls now run without extra approval for ${basenameFromPath(library.root)}`;
      props.onRefresh();
    } catch (e) {
      lambdaError = (e as Error).message ?? String(e);
      void loadLambdaSkillLibraries();
    } finally {
      lambdaTogglingApproval = null;
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
        permissionLoaded = true;
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
    if (!modelPickerProvider || !modelPickerModel || modelPickerLoading || modelSaving || credentialBusy) return;
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
  const browserRenderers: { k: BrowserRenderer; label: string }[] = [
    { k: "cef", label: "CEF" },
    { k: "screencast", label: "Screencast" }
  ];

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

    lambdaSnapshot = null;
    lambdaLoaded = false;
    lambdaLoading = false;
    lambdaLoadGeneration += 1;
    lambdaError = null;
    lambdaSaved = null;

    permissionLoadGeneration += 1;
    permissionSnapshot = null;
    permissionRows = [];
    permissionLoading = false;
    permissionLoaded = false;
    permissionError = null;
    permissionDirty = false;

    mcpServers = [];
    mcpLoaded = false;
    mcpLoading = false;
    mcpLoadGeneration += 1;
    mcpError = null;
    mcpSaved = null;

    connectorUnlisten?.();
    connectorUnlisten = null;
    connectorSnapshot = null;
    connectorLoaded = false;
    connectorLoading = false;
    connectorLoadGeneration += 1;
    connectorError = null;
    connectorSaved = null;
    connectorCreateOpen = false;
    connectorTab = "connections";
    connectorSlug = "";
    connectorConnectionSlug = "";
    connectorCreating = false;
    connectorTurnId = null;
    connectorSetupSlug = null;
    connectorQuestionRequest = null;
    connectorQuestionAnswers = {};
    lastConnectorSlug = "";

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
  let daemonReachable = $state(isDaemonReachable());
  $effect(() => {
    daemonReachable = isDaemonReachable();
    const client = currentDaemonClient();
    if (!client) return;
    return client.onConnectionChange(() => {
      daemonReachable = isDaemonReachable();
    });
  });
  let mcpFormDisabled = $derived(!daemonReachable || mcpLoading || mcpSaving);
  let lambdaRefreshDisabled = $derived(!daemonReachable || lambdaLoading || lambdaSaving || lambdaRemovingLibrary !== null);
  let lambdaChooseDisabled = $derived(lambdaSaving || lambdaRemovingLibrary !== null || !canInvokeTauri());
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
  let modelPickerDisabled = $derived(!daemonReachable || modelSaving || credentialBusy);
  let canSaveDefaultModel = $derived(
    Boolean(
      daemonReachable &&
        modelPickerProvider &&
        modelPickerModel &&
        modelIdInList(modelPickerModel, providerModels[modelPickerProvider] ?? []) &&
        !modelPickerLoading &&
        !modelSaving &&
        !credentialBusy
    )
  );
  let canStartConnectorSetup = $derived(
    Boolean(
      daemonReachable &&
        selectedConnector &&
        !connectorLoading &&
        !connectorCreating &&
        !connectorConnectionSlugInvalid &&
        connectorCommandPreview
    )
  );

  // Lazy-load per-pane data when the user actually opens the tab so the
  // initial settings render stays a single RPC (the snapshot).
  $effect(() => {
    if (!daemonReachable) return;
    if (section === "providers" && !modelPickerProvider && defaultRouteProviders.length > 0) {
      modelPickerProvider = defaultRouteProviderId();
    }
    if (section === "permissions" && permissionSnapshot === null && !permissionLoading && !permissionLoaded) {
      void loadPermissionSnapshot();
    }
    if (section === "skills" && !lambdaLoaded && !lambdaLoading) {
      void loadLambdaSkillLibraries();
    }
    if (section === "mcp" && !mcpLoaded && !mcpLoading) {
      void loadMcpServers();
    }
    if (section === "connectors" && !connectorLoaded && !connectorLoading) {
      void loadConnectorSnapshot();
    }
    if (section === "providers" && modelPickerProvider) {
      void loadModelsForProvider(modelPickerProvider);
    }
  });

  onDestroy(() => {
    connectorUnlisten?.();
    connectorUnlisten = null;
  });
</script>

{#snippet connectorMarkdown(value: string | undefined, fallbackAlt: string)}
  {#each connectorMarkdownParts(value) as part}
    {#if part.kind === "image"}
      <img class="pf-connector-question-image" src={part.src} alt={part.alt || fallbackAlt} />
    {:else}
      <span class="pf-connector-markdown-text">{part.text}</span>
    {/if}
  {/each}
{/snippet}

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

      <div class="pf-settings-row">
        <div class="meta">
          <div class="label">Browser renderer</div>
          <div class="desc">Use CEF by default, with screencast available as the compatibility path.</div>
        </div>
        <div class="pf-appearance-control">
          {#each browserRenderers as renderer (renderer.k)}
            <button
              type="button"
              class="pf-choice-pill"
              data-active={props.preferences.browserRenderer === renderer.k}
              onclick={() => props.onPreferenceChange("browserRenderer", renderer.k)}
            >{renderer.label}</button>
          {/each}
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
      <div class="pf-provider-page">
        <header class="pf-provider-title">
          <h2>Providers</h2>
          <p class="lead">Connect providers, review their available models, and choose the default route for new turns.</p>
        </header>

        {#if !daemonReachable}
          <div class="pf-settings-note">
            Preview mode - launch Puffer in the desktop app to edit live routing.
          </div>
        {:else if usingFallbackProviders}
          <div class="pf-settings-note">
            Provider registry is empty. Built-in setup options are available below; refresh after
            resources reload.
          </div>
        {/if}
        <LocalModelSetupCard onRefresh={props.onRefresh} />

        <section class="pf-provider-routing">
          <div class="pf-provider-routing-copy">
            <div class="label">Default routing</div>
            <div class="desc">Select the AI provider and model your agent uses by default.</div>
          </div>
          <div class="pf-provider-routing-controls">
            <label>
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
                <option value="">No provider</option>
                {#each defaultRouteProviders as p (p.id)}
                  <option value={p.id}>{p.displayName} ({p.id})</option>
                {/each}
              </select>
            </label>
            <label>
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
                      : "Pick a model"}
                </option>
                {#each modelPickerModels as m (m.id)}
                  <option value={m.id}>{m.displayName} ({m.id})</option>
                {/each}
              </select>
            </label>
            <button
              type="button"
              class="sc-btn pf-provider-save"
              data-variant="default"
              data-size="sm"
              disabled={!canSaveDefaultModel}
              onclick={saveDefaultModel}
            >
              {modelSaving ? "Saving..." : "Save default"}
            </button>
          </div>
          {#if modelPickerLoading}
            <div class="pf-model-loading-note">
              Fetching {modelPickerProviderName} models...
            </div>
          {:else if modelPickerNoAgentModels}
            <div class="pf-model-loading-note" data-error="true">
              No {modelPickerProviderName} models support agent tools.
            </div>
          {/if}
          {#if modelError}
            <div class="pf-model-loading-note" data-error="true">{modelError}</div>
          {/if}
        </section>

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
          onLogout={props.onLogout}
          onImportExternal={props.onImportExternal ?? (() => {})}
          onRefresh={props.onRefresh}
        />
      </div>

    {:else if section === "secrets"}
      <SecretsSettings
        snapshot={props.snapshot}
        daemonReachable={daemonReachable}
        onRefresh={props.onRefresh}
      />

    {:else if section === "network"}
      <NetworkSettings snapshot={props.snapshot} onSaved={(_next) => props.onRefresh()} />

    {:else if section === "browser"}
      <BrowserSettings
        snapshot={props.snapshot}
        daemonReachable={daemonReachable}
        onSaved={(_next) => props.onRefresh()}
        onRefresh={props.onRefresh}
      />

    {:else if section === "connectors"}
      <h2>Connectors</h2>
      <p class="lead">Connector catalog, saved connections, and setup flows backed by AskUserQuestion.</p>
      <div class="pf-connector-toolbar">
        <button
          type="button"
          class="sc-btn"
          data-variant="outline"
          data-size="sm"
          disabled={!daemonReachable || connectorLoading || connectorCreating}
          onclick={refreshIfIdle}
        >
          <Icon name="refresh" size={13} />Refresh connectors
        </button>
        <button
          type="button"
          class="sc-btn"
          data-variant="default"
          data-size="sm"
          disabled={!daemonReachable || connectorLoading || connectorCreating || connectors.length === 0}
          onclick={openConnectorCreate}
        >
          <Icon name="plug" size={13} />New connection
        </button>
      </div>
      {#if connectorError}
        <div class="pf-settings-note warn">{connectorError}</div>
      {:else if connectorSaved}
        <div class="pf-settings-note" class:busy={connectorCreating} aria-live="polite" aria-busy={connectorCreating}>
          {#if connectorCreating}<span class="pf-note-spinner" aria-hidden="true"></span>{/if}{connectorSaved}
        </div>
      {:else if !daemonReachable}
        <div class="pf-settings-note">Preview mode - launch Puffer in the desktop app to configure connectors.</div>
      {:else if connectorLoading}
        <div class="pf-settings-note">Loading connector catalog...</div>
      {:else}
        <div class="pf-settings-note">{connectors.length} connector{connectors.length === 1 ? "" : "s"} and {connections.length} connection{connections.length === 1 ? "" : "s"}.</div>
      {/if}

      {#if connectorCreateOpen && !connectorQuestionRequest}
        <div
          class="pf-modal-scrim pf-connector-create-scrim"
          role="presentation"
          onclick={closeConnectorCreate}
          onkeydown={() => {}}
        >
          <div
            class="pf-modal pf-connector-create-modal"
            role="dialog"
            aria-label="Create connector connection"
            aria-modal="true"
            tabindex="-1"
            use:focusTrap
            onclick={(event) => event.stopPropagation()}
            onkeydown={(event) => {
              if (event.key === "Escape") {
                event.preventDefault();
                closeConnectorCreate();
              }
            }}
          >
            <form
              class="pf-connector-create-form"
              onsubmit={(event) => {
                event.preventDefault();
                void startConnectorSetup();
              }}
            >
              <div class="pf-modal-head">
                <div class="pf-modal-title-group">
                  <div class="pf-modal-title">Create connection</div>
                </div>
                <button type="button" class="pf-modal-close" onclick={closeConnectorCreate} aria-label="Close">
                  <Icon name="x" size={14} />
                </button>
              </div>
              <div class="pf-modal-body pf-connector-create-body">
                <div class="pf-connector-form">
                  <label>
                    Connector
                    <select
                      class="sc-input"
                      value={connectorSlug}
                      disabled={!daemonReachable || connectorLoading || connectorCreating || connectors.length === 0}
                      data-autofocus
                      onchange={(e) => selectConnector((e.currentTarget as HTMLSelectElement).value)}
                    >
                      {#if connectors.length === 0}
                        <option value="">No connectors</option>
                      {/if}
                      {#each connectors as connector (connector.connector_slug)}
                        <option value={connector.connector_slug}>{connector.connector_slug}</option>
                      {/each}
                    </select>
                  </label>
                  <label>
                    Connection slug
                    <input
                      class="sc-input"
                      aria-label="Connector connection slug"
                      aria-invalid={connectorConnectionSlugInvalid}
                      value={connectorConnectionSlug}
                      disabled={!daemonReachable || connectorLoading || connectorCreating || !selectedConnector}
                      oninput={(e) => {
                        connectorConnectionSlug = (e.currentTarget as HTMLInputElement).value;
                        connectorSaved = null;
                      }}
                    />
                  </label>
                  {#if connectorConnectionSlugInvalid && connectorConnectionSlug.trim()}
                    <div class="pf-connector-validation">Use lowercase letters, digits, and hyphens.</div>
                  {/if}
                  {#if selectedConnector}
                    <div class="pf-connector-selected">
                      <div>
                        <strong>{selectedConnector.connector_slug}</strong>
                        <span>{selectedConnector.description}</span>
                        <span>{selectedConnectorConnections.length} existing connection{selectedConnectorConnections.length === 1 ? "" : "s"}</span>
                      </div>
                      <span class="pf-status-pill ready">{connectorStatusLabel(selectedConnector)}</span>
                    </div>
                  {/if}
                  <div class="pf-connector-command" aria-label="Connector setup command">
                    <Icon name="terminal" size={12} />
                    <code>{connectorCommandPreview || "Enter a valid connection slug."}</code>
                  </div>
                </div>
              </div>
              <div class="pf-modal-foot">
                <div class="pf-modal-foot-btns">
                  <button
                    type="button"
                    class="sc-btn"
                    data-variant="outline"
                    data-size="sm"
                    onclick={closeConnectorCreate}
                  >
                    Cancel
                  </button>
                  <button
                    type="submit"
                    class="sc-btn"
                    data-variant="default"
                    data-size="sm"
                    disabled={!canStartConnectorSetup}
                    aria-busy={connectorCreating && !connectorQuestionRequest}
                  >
                    <Icon name="plug" size={12} />{connectorCreating && !connectorQuestionRequest ? "Starting..." : "Start setup"}
                  </button>
                </div>
              </div>
            </form>
          </div>
        </div>
      {/if}

      {#if connectorQuestionRequest}
        <div class="pf-modal-scrim pf-connector-question-scrim" role="presentation" onkeydown={() => {}}>
          <div
            class={connectorQuestionRequest.browserSessionId
              ? "pf-modal pf-connector-question-modal pf-connector-question-modal-browser"
              : "pf-modal pf-connector-question-modal"}
            role="dialog"
            aria-label="Connector setup questions"
            aria-modal="true"
            tabindex="-1"
            use:focusTrap
            onclick={(event) => event.stopPropagation()}
            onkeydown={(event) => {
              if (event.key === "Escape") {
                event.preventDefault();
                void cancelConnectorSetup();
              }
            }}
          >
            <div class="pf-connector-question-form">
              <div class="pf-modal-head pf-connector-question-head">
                <div class="pf-modal-title-group">
                  <div class="pf-modal-title">{connectorQuestionRequest.browserSessionId ? "Browser setup" : "Setup questions"}</div>
                </div>
                <span class="pf-status-pill">
                  {connectorQuestionRequest.questions.length}
                  {connectorQuestionRequest.browserSessionId ? " step" : " question"}{connectorQuestionRequest.questions.length === 1 ? "" : "s"}
                </span>
              </div>
              <div class={connectorQuestionRequest.browserSessionId
                ? "pf-modal-body pf-connector-question-list pf-connector-question-list-browser"
                : "pf-modal-body pf-connector-question-list"}
              >
                {#if connectorCreating}
                  <div class="pf-connector-question-loading" role="status" aria-live="polite">
                    <span class="pf-connector-loading-spinner" aria-hidden="true"></span>
                    <div>
                      <strong>Checking connector auth...</strong>
                      <span>{connectorSaved || "Waiting for the connector to finish."}</span>
                    </div>
                  </div>
                {:else if connectorQuestionRequest.browserSessionId}
                  <div class="pf-connector-browser-auth">
                    <BrowserPane
                      sessionId={connectorQuestionRequest.browserSessionId}
                      browserRenderer={props.preferences.browserRenderer}
                    />
                  </div>
                  <div class="pf-connector-question-column">
                    {#each connectorQuestionRequest.questions as question, questionIndex (connectorQuestionKey(question))}
                      {@const key = connectorQuestionKey(question)}
                      <fieldset class="pf-connector-question" data-action-only={connectorQuestionIsActionOnly(question)}>
                        <legend>
                          <span>{question.header}</span>
                          <strong>{@render connectorMarkdown(question.question, "Question image")}</strong>
                        </legend>
                        {#if question.type === "input"}
                          <input
                            class="sc-input"
                            type={connectorQuestionInputType(question)}
                            value={connectorAnswerText(question)}
                            data-autofocus={questionIndex === 0 ? "true" : undefined}
                            oninput={(e) => updateConnectorAnswer(question, (e.currentTarget as HTMLInputElement).value)}
                          />
                        {:else if question.multiSelect}
                          <div class="pf-connector-options">
                            {#each question.options as option, optionIndex (option.label)}
                              <label>
                                <input
                                  type="checkbox"
                                  checked={connectorAnswerIncludes(question, option.label)}
                                  data-autofocus={questionIndex === 0 && optionIndex === 0 ? "true" : undefined}
                                  onchange={(e) => toggleConnectorMultiAnswer(question, option.label, (e.currentTarget as HTMLInputElement).checked)}
                                />
                                <span>
                                  <strong>{option.label}</strong>
                                  {#if option.description}<small>{option.description}</small>{/if}
                                  {#if option.preview}<code>{option.preview}</code>{/if}
                                </span>
                              </label>
                            {/each}
                          </div>
                        {:else if !connectorQuestionIsActionOnly(question)}
                          <div class="pf-connector-options">
                            {#each question.options as option, optionIndex (option.label)}
                              <label>
                                <input
                                  type="radio"
                                  name={`connector-${connectorQuestionRequest.requestId}-${key}`}
                                  checked={connectorAnswerIncludes(question, option.label)}
                                  data-autofocus={questionIndex === 0 && optionIndex === 0 ? "true" : undefined}
                                  onchange={() => updateConnectorAnswer(question, option.label)}
                                />
                                <span>
                                  <strong>{option.label}</strong>
                                  {#if option.description}<small>{option.description}</small>{/if}
                                  {#if option.preview}<code>{option.preview}</code>{/if}
                                </span>
                              </label>
                            {/each}
                          </div>
                        {/if}
                      </fieldset>
                    {/each}
                    <div class="pf-connector-browser-actions">
                      <button
                        type="button"
                        class="sc-btn"
                        data-variant="outline"
                        data-size="sm"
                        onclick={() => void cancelConnectorSetup()}
                      >
                        Cancel
                      </button>
                      <button
                        type="button"
                        class="sc-btn"
                        data-variant="default"
                        data-size="sm"
                        disabled={!connectorAnswersComplete() || connectorCreating}
                        aria-busy={connectorCreating}
                        onclick={() => void submitConnectorAnswers()}
                      >
                        <Icon name="check" size={12} />{connectorQuestionSubmitLabel()}
                      </button>
                    </div>
                  </div>
                {:else}
                  {#each connectorQuestionRequest.questions as question, questionIndex (connectorQuestionKey(question))}
                    {@const key = connectorQuestionKey(question)}
                    <fieldset class="pf-connector-question" data-action-only={connectorQuestionIsActionOnly(question)}>
                      <legend>
                        <span>{question.header}</span>
                        <strong>{@render connectorMarkdown(question.question, "Question image")}</strong>
                      </legend>
                      {#if question.type === "input"}
                        <input
                          class="sc-input"
                          type={connectorQuestionInputType(question)}
                          value={connectorAnswerText(question)}
                          data-autofocus={questionIndex === 0 ? "true" : undefined}
                          oninput={(e) => updateConnectorAnswer(question, (e.currentTarget as HTMLInputElement).value)}
                        />
                      {:else if question.multiSelect}
                        <div class="pf-connector-options">
                          {#each question.options as option, optionIndex (option.label)}
                            <label>
                              <input
                                type="checkbox"
                                checked={connectorAnswerIncludes(question, option.label)}
                                data-autofocus={questionIndex === 0 && optionIndex === 0 ? "true" : undefined}
                                onchange={(e) => toggleConnectorMultiAnswer(question, option.label, (e.currentTarget as HTMLInputElement).checked)}
                              />
                              <span>
                                <strong>{option.label}</strong>
                                {#if option.description}<small>{option.description}</small>{/if}
                                {#if option.preview}<code>{option.preview}</code>{/if}
                              </span>
                            </label>
                          {/each}
                        </div>
                      {:else if !connectorQuestionIsActionOnly(question)}
                        <div class="pf-connector-options">
                          {#each question.options as option, optionIndex (option.label)}
                            <label>
                              <input
                                type="radio"
                                name={`connector-${connectorQuestionRequest.requestId}-${key}`}
                                checked={connectorAnswerIncludes(question, option.label)}
                                data-autofocus={questionIndex === 0 && optionIndex === 0 ? "true" : undefined}
                                onchange={() => updateConnectorAnswer(question, option.label)}
                              />
                              <span>
                                <strong>{option.label}</strong>
                                {#if option.description}<small>{option.description}</small>{/if}
                                {#if option.preview}<code>{option.preview}</code>{/if}
                              </span>
                            </label>
                          {/each}
                        </div>
                      {/if}
                    </fieldset>
                  {/each}
                {/if}
              </div>
              {#if !connectorQuestionRequest.browserSessionId}
                <div class="pf-modal-foot pf-connector-question-actions">
                  <div class="pf-modal-foot-btns">
                    <button
                      type="button"
                      class="sc-btn"
                      data-variant="outline"
                      data-size="sm"
                      onclick={() => void cancelConnectorSetup()}
                    >
                      Cancel
                    </button>
                    <button
                      type="button"
                      class="sc-btn"
                      data-variant="default"
                      data-size="sm"
                      disabled={!connectorAnswersComplete() || connectorCreating}
                      aria-busy={connectorCreating}
                      onclick={() => void submitConnectorAnswers()}
                    >
                      <Icon name="check" size={12} />{connectorQuestionSubmitLabel()}
                    </button>
                  </div>
                </div>
              {/if}
            </div>
          </div>
        </div>
      {/if}

      <div class="pf-connector-tabs" role="tablist" aria-label="Connector views">
        <button
          type="button"
          role="tab"
          aria-selected={connectorTab === "connections"}
          data-active={connectorTab === "connections"}
          onclick={() => connectorTab = "connections"}
        >
          Connections
          <span>{connections.length}</span>
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={connectorTab === "catalog"}
          data-active={connectorTab === "catalog"}
          onclick={() => connectorTab = "catalog"}
        >
          Catalog
          <span>{connectors.length}</span>
        </button>
      </div>

      {#if connectorTab === "connections"}
        <section class="pf-connector-panel" aria-label="Connector connections">
          <div class="pf-mcp-list">
            {#each connections as connection (connection.slug)}
              <div class="pf-mcp-card pf-connector-card-settings">
                <span class="ico"><Icon name="plug" size={16} /></span>
                <div>
                  <div class="title">{connection.slug}</div>
                  <div class="desc">{connection.description || connection.connector_slug}</div>
                </div>
                <span class:ready={connection.state === "active" || connection.state === "authenticated"} class="pf-status-pill">
                  {connection.state}
                </span>
                <span class="pf-connector-source">{connection.connector_slug}</span>
                <button
                  type="button"
                  class="sc-btn"
                  data-variant="outline"
                  data-size="sm"
                  aria-label={connection.state === "active"
                    ? `Stop monitoring ${connection.slug}`
                    : `Start monitoring ${connection.slug}`}
                  disabled={!daemonReachable || connectorMonitoring !== null || connection.state === "pending"}
                  aria-busy={connectorMonitoring === connection.slug}
                  onclick={() => void toggleConnectionMonitor(connection.slug, connection.state === "active")}
                >
                  <Icon name="eye" size={12} />{connectorMonitoring === connection.slug
                    ? "Working..."
                    : connection.state === "active"
                      ? "Stop monitoring"
                      : "Start monitoring"}
                </button>
                <button
                  type="button"
                  class="sc-btn"
                  data-variant="outline"
                  data-size="sm"
                  aria-label={`Remove connection ${connection.slug}`}
                  disabled={!daemonReachable || connectorDeleting !== null}
                  aria-busy={connectorDeleting === connection.slug}
                  onclick={() => void removeConnectorConnection(connection.slug)}
                >
                  <Icon name="x" size={12} />{connectorDeleting === connection.slug ? "Removing..." : "Remove"}
                </button>
              </div>
            {/each}
            {#if !connectorLoading && connections.length === 0}
              <div class="pf-empty">No connector connections configured.</div>
            {/if}
          </div>
        </section>
      {:else}
        <section class="pf-connector-panel" aria-label="Connector catalog">
          <div class="pf-mcp-list">
            {#each connectors as connector (connector.connector_slug)}
              <button
                type="button"
                class="pf-mcp-card pf-connector-card-settings pf-connector-catalog-button"
                data-selected={connector.connector_slug === connectorSlug}
                onclick={() => {
                  selectConnector(connector.connector_slug);
                  openConnectorCreate();
                }}
              >
                <span class="ico"><Icon name="server" size={16} /></span>
                <div>
                  <div class="title">{connector.connector_slug}</div>
                  <div class="desc">{connector.description}</div>
                </div>
                <span class:ready={connector.requires_auth} class="pf-status-pill">{connectorStatusLabel(connector)}</span>
                <span class="pf-connector-source">{connector.skill}</span>
              </button>
            {/each}
            {#if !connectorLoading && connectors.length === 0}
              <div class="pf-empty">No connectors discovered.</div>
            {/if}
          </div>
        </section>
      {/if}

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
        <div class="pf-settings-note warn">
          <span>{permissionError}</span>
          <button
            type="button"
            class="sc-btn"
            data-variant="outline"
            data-size="sm"
            disabled={!daemonReachable || permissionLoading || permissionSaving}
            onclick={() => void loadPermissionSnapshot()}
          >
            Retry
          </button>
        </div>
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

    {:else if section === "skills"}
      <h2>Verified Skills</h2>
      <p class="lead">Add folders that contain Verified Skills with generated host catalogues. Puffer keeps the files outside the app and loads them from config.</p>
      <div style="display: flex; justify-content: flex-end; margin-bottom: 10px;">
        <button
          type="button"
          class="sc-btn"
          data-variant="outline"
          data-size="sm"
          disabled={lambdaRefreshDisabled}
          onclick={refreshIfIdle}
        >
          <Icon name="refresh" size={13} />Refresh
        </button>
      </div>
      {#if lambdaError}
        <div class="pf-settings-note warn">{lambdaError}</div>
      {/if}
      {#if lambdaSaved}
        <div class="pf-settings-note">{lambdaSaved}</div>
      {/if}
      <div class="pf-settings-note">
        {#if !daemonReachable}
          Launch Puffer in the desktop app to add Verified Skills folders.
        {:else if lambdaLoading}
          Loading Verified Skills folders…
        {:else}
          Choose a folder once. Puffer will keep using it from this workspace.
        {/if}
      </div>

      {#if lambdaSnapshot?.warnings.length}
        <div class="pf-settings-note warn">
          Some Verified Skills need generated verification output before the model can use them.
        </div>
      {/if}

      <div class="pf-settings-row">
        <div class="meta">
          <div class="label">Add folder</div>
          <div class="desc">Choose the folder that contains your Verified Skills. It must already include generated verification files and host catalogues.</div>
        </div>
        <button
          type="button"
          class="sc-btn"
          data-variant="default"
          data-size="sm"
          disabled={lambdaChooseDisabled}
          onclick={addVerifiedSkillsFolder}
        >
          <Icon name="folder" size={13} />{lambdaSaving ? "Adding…" : "Choose folder"}
        </button>
      </div>

      <h3 class="pf-settings-subhead">Folders</h3>
      <div class="pf-mcp-list">
        {#each lambdaSnapshot?.libraries ?? [] as library (library.sourcePath)}
          <div class="pf-mcp-card pf-verified-folder-card">
            <span class="ico"><Icon name="shield" size={16} /></span>
            <div>
              <div class="title">{basenameFromPath(library.root)}</div>
              <div class="desc">
                {verifiedSkillScopeLabel(library)} Verified Skills folder
                · {library.requireApproval ? "asks before verified tools" : "runs verified tools without extra approval"}
              </div>
            </div>
            <span class:ready={verifiedSkillStatus(library) === "Ready for model use"} class="pf-status-pill">
              {verifiedSkillStatus(library)}
            </span>
            <label class="pf-inline-switch">
              <span>Ask before tools</span>
              <input
                type="checkbox"
                class="sc-switch"
                checked={library.requireApproval}
                disabled={lambdaTogglingApproval === lambdaLibraryKey(library) || lambdaSaving || (library.sourceKind !== "workspace" && library.sourceKind !== "user")}
                aria-label={`Require approval for ${basenameFromPath(library.root)} verified tools`}
                onchange={(e) => toggleVerifiedSkillApproval(library, (e.currentTarget as HTMLInputElement).checked)}
              />
            </label>
            <input
              type="checkbox"
              class="sc-switch"
              checked={lambdaRemovingLibrary !== lambdaLibraryKey(library)}
              disabled={lambdaRemovingLibrary === lambdaLibraryKey(library) || lambdaSaving || (library.sourceKind !== "workspace" && library.sourceKind !== "user")}
              aria-label={`Remove ${basenameFromPath(library.root)} Verified Skills folder`}
              onchange={(e) => {
                if (!(e.currentTarget as HTMLInputElement).checked) {
                  void removeVerifiedSkillsFolder(library);
                }
              }}
            />
          </div>
        {/each}
        {#if !lambdaLoading && (lambdaSnapshot?.libraries.length ?? 0) === 0}
          <div class="pf-empty">No Verified Skills folders added.</div>
        {/if}
      </div>

      <h3 class="pf-settings-subhead">Recognized Verified Skills</h3>
      <div class="pf-mcp-list">
        {#each lambdaSnapshot?.skills ?? [] as skill (verifiedSkillKey(skill))}
          <div class="pf-mcp-card pf-skill-card">
            <span class="ico"><Icon name="shield" size={16} /></span>
            <div>
              <div class="title">{skill.name}</div>
              {#if skill.description}
                <div class="desc">{skill.description}</div>
              {/if}
              <div class="desc">
                {verifiedSkillDetailLabel(skill)}
                {#if skill.failureReason && skill.enabled}
                  · {skill.failureReason}
                {/if}
              </div>
            </div>
            <span
              class:ready={skill.modelInvocable}
              class="pf-status-pill"
            >
              {verifiedSkillReadinessLabel(skill)}
            </span>
            <input
              type="checkbox"
              class="sc-switch"
              checked={skill.enabled}
              disabled={lambdaTogglingSkill === verifiedSkillKey(skill) || !skill.libraryId || (skill.sourceKind !== "workspace" && skill.sourceKind !== "user")}
              onchange={(e) => toggleVerifiedSkill(skill, (e.currentTarget as HTMLInputElement).checked)}
            />
          </div>
        {/each}
        {#if !lambdaLoading && (lambdaSnapshot?.skills.length ?? 0) === 0}
          <div class="pf-empty">No Verified Skills recognized yet.</div>
        {/if}
      </div>

    {:else if section === "mcp"}
      <h2>MCP Servers</h2>
      <p class="lead">External tools Puffer can pull context from and take actions on.</p>
      <div style="display: flex; justify-content: flex-end; margin-bottom: 10px;">
        <button
          type="button"
          class="sc-btn"
          data-variant="outline"
          data-size="sm"
          disabled={!daemonReachable || mcpLoading || mcpSaving}
          onclick={refreshIfIdle}
        >
          <Icon name="refresh" size={13} />Refresh MCP servers
        </button>
      </div>
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

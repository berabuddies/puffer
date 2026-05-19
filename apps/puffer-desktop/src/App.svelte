<script lang="ts">
  import { onMount } from "svelte";

  import TitleBar from "./lib/shell/TitleBar.svelte";
  import Sidebar, { type ActiveAgent, type UserChip } from "./lib/shell/Sidebar.svelte";
  import {
    applyTweaksToDocument,
    defaultTweaks,
    loadTweaks,
    persistTweaks,
    type ScreenId,
    type Tweaks,
    type AgentState
  } from "./lib/shell/tweaks";

  import Workspace from "./lib/screens/Workspace.svelte";
  import ProjectDetail from "./lib/screens/workspace/ProjectDetail.svelte";
  import NewSessionModal from "./lib/screens/workspace/NewSessionModal.svelte";
  import WorkspacePicker from "./lib/screens/WorkspacePicker.svelte";
  import AgentDetail from "./lib/screens/agent/AgentDetail.svelte";
  import Pipelines from "./lib/screens/Pipelines.svelte";
  import Deployments from "./lib/screens/Deployments.svelte";
  import Settings from "./lib/screens/Settings.svelte";
  import Onboarding from "./lib/screens/Onboarding.svelte";

  import {
    createPullRequest,
    importExternalCredential,
    listExternalCredentials,
    loginWithApiKey,
    loginWithApiKeyViaDaemon,
    loginWithOauth,
    listGroupedSessionsFromDaemon,
    loadSettingsSnapshot,
    loadSessionDetailFromDaemon,
    renameSession,
    mergePullRequest,
    logoutProvider,
    logoutProviderViaDaemon,
    readRemoteFile,
    refreshRepoStatus,
    runRemoteBash,
    writeRemoteFile,
    runAgentTurn,
    resolvePermission as resolveTurnPermission,
    resolveUserQuestion as resolveTurnUserQuestion,
    cancelTurn,
    createSession,
    loadDefaultWorkspace,
    loadDesktopPins,
    setDesktopPin,
    type AgentTurnOptions
  } from "./lib/api/desktop";
  import {
    subscribeSessionEvents,
    type SessionStreamEvent
  } from "./lib/api/sessionEvents";
  import {
    currentDaemonClient,
    ensureLocalDaemonClient,
    type DaemonClient,
    type ConnectionState
  } from "./lib/api/daemonClient";
  import { sessionDisplayName, sessionDisplayTitle } from "./lib/sessionDisplay";
  import { providerIdInSet } from "./lib/providerIds";
  import type { UnlistenFn } from "@tauri-apps/api/event";
  import type {
    DesktopPreferences,
    DesktopPinState,
    AgentActivityStatus,
    ExternalCredential,
    FolderGroup,
    AskUserQuestionItem,
    PermissionTimelineItem,
    RemoteConnection,
    RemoteOperation,
    SessionDetail,
    SessionListItem,
    SettingsSnapshot,
    TimelineItem,
    UserQuestionTimelineItem
  } from "./lib/types";

  // ─────────────────────────────────────────────────────────────
  // Shell state
  // ─────────────────────────────────────────────────────────────
  let tweaks = $state<Tweaks>({ ...defaultTweaks });
  let onboarding = $state(true);
  let onboardingCompleted = $state(false);
  // Dev bypass so we can screenshot every screen without live auth.
  const urlParams = typeof window !== "undefined" ? new URLSearchParams(window.location.search) : new URLSearchParams();
  const skipOnboarding =
    typeof window !== "undefined" &&
    (urlParams.has("skipOnboarding") ||
      window.localStorage.getItem("puffer-desktop:skip-onboarding") === "1");
  const forceOnboarding = urlParams.has("forceOnboarding");
  let statusMessage = $state("Desktop workspace ready.");
  // Auto-dismiss the status strip a few seconds after each message so it
  // doesn't linger in the sidebar corner looking like a truncated widget.
  let statusDismissTimer: ReturnType<typeof setTimeout> | null = null;
  $effect(() => {
    // Re-arm whenever `statusMessage` changes to a non-default value.
    if (!statusMessage || statusMessage === "Desktop workspace ready.") return;
    if (statusDismissTimer) clearTimeout(statusDismissTimer);
    statusDismissTimer = setTimeout(() => {
      statusMessage = "Desktop workspace ready.";
      statusDismissTimer = null;
    }, 4000);
  });
  let showWorkspacePicker = $state(false);
  let newSessionCwd = $state<string | null>(null);
  let newSessionBusy = $state(false);

  // Backend-backed state
  let groups = $state<FolderGroup[]>([]);
  let groupsLoading = $state(false);
  let selectedSession = $state<SessionListItem | null>(null);
  let sessionDetail = $state<SessionDetail | null>(null);
  let sessionLoading = $state(false);

  // Drill-in marker: which session id is currently expanded in AgentDetail.
  // Cleared when the user backs out to the workspace board.
  let openAgentSessionId = $state<string | null>(null);
  let openProjectId = $state<string | null>(null);
  let submittedMessages = $state<TimelineItem[]>([]);
  let submitMessageInFlightSessionIds = $state<string[]>([]);
  let dismissedPermissionIds = $state<string[]>([]);
  let dismissedQuestionIds = $state<string[]>([]);
  let resolvingPermissionIds = $state<string[]>([]);
  let resolvingQuestionIds = $state<string[]>([]);

  // Live turn state: items synthesized from streaming events while a turn is
  // running. When the turn finishes we reload the session detail so the real
  // persisted transcript replaces these placeholders.
  let currentTurnId = $state<string | null>(null);
  let cancelingTurnId = $state<string | null>(null);
  let turnStartedAtMs = $state<number | null>(null);
  let turnThinking = $state(false);
  let turnStatusHint = $state<string | null>(null);
  let liveStreamItems = $state<TimelineItem[]>([]);
  let settledTurnIds = new Set<string>();
  let turnPermissionLookup = $state<Record<string, { turnId: string; requestId: string }>>({});
  let turnQuestionLookup = $state<Record<string, { turnId: string; requestId: string }>>({});
  let replayTextByTurn: Record<string, string> = {};
  let sessionEventUnlisten: UnlistenFn | null = null;
  let subscribedSessionId: string | null = null;
  let connectionState = $state<ConnectionState>("idle");
  let daemonUrl = $state<string | null>(null);
  let daemonWorkspaceRoot = $state<string | null>(null);
  let daemonClientFingerprint = $state<string | null>(null);
  let daemonClientUnlisteners: Array<() => void> = [];
  let sessionLoadGeneration = 0;
  let liveErrorSeq = 0;
  let desktopPins = $state<DesktopPinState>({ pinnedAgentIds: [], pinnedWorkspacePaths: [] });
  let desktopPinInFlightKeys = $state<string[]>([]);

  let settingsSnapshot = $state<SettingsSnapshot | null>(null);
  let settingsLoading = $state(false);
  let settingsRefreshGeneration = 0;
  let groupsRefreshGeneration = 0;
  let authBusyProviderId = $state<string | null>(null);
  let authError = $state<string | null>(null);
  let externalCredentials = $state<ExternalCredential[]>([]);
  let importBusyKey = $state<string | null>(null);
  let actionBusy = $state(false);
  let remoteOperation = $state<RemoteOperation | null>(null);
  let remoteBusy = $state(false);
  let remotePassword = $state("");

  const defaultDesktopPreferences: DesktopPreferences = {
    rememberSession: false,
    rememberInspectorLayout: false,
    launchInspectorOpen: true,
    defaultInspectorTab: "latest-diff",
    defaultInspectorWidth: 50,
    remoteEnabled: false,
    remoteTarget: "",
    remoteCwd: ""
  };
  const DESKTOP_PREFERENCES_KEY = "puffer-desktop:preferences";
  const REMEMBERED_SESSION_KEY = "puffer-desktop:remembered-session";
  type RememberedSession = {
    workspaceRoot: string;
    sessionId: string;
  };
  let desktopPreferences = $state<DesktopPreferences>(loadDesktopPreferences());

  // The daemon's default workspace (host, path). Shown in the sidebar /
  // workspace header; new sessions default to this cwd.
  let defaultWorkspaceCwd = $state<string>("");

  let remoteConnection = $derived<RemoteConnection>({
    enabled:
      desktopPreferences.remoteEnabled && desktopPreferences.remoteTarget.trim().length > 0,
    target: desktopPreferences.remoteTarget.trim(),
    cwd: desktopPreferences.remoteCwd.trim(),
    password: remotePassword
  });

  // ─────────────────────────────────────────────────────────────
  // Active agent mapping (sessions → sidebar agents)
  //   review  = open PR OR pending manual approval
  //   running = active work (unresolved permission / uncommitted changes)
  //   done    = merged / clean on a branch with closed PR
  //   idle    = otherwise
  // For Phase 1 we can only distinguish by session metadata at this shell level,
  // so we mark everything idle until AgentDetail in Phase 2 has per-session state.
  // ─────────────────────────────────────────────────────────────
  let renderedSubmittedMessages = $derived<TimelineItem[]>(
    stillMissingFromPersisted(sessionDetail?.timeline ?? [], submittedMessages)
  );
  let renderedLiveStreamItems = $derived<TimelineItem[]>(
    stillMissingFromPersisted(
      [...(sessionDetail?.timeline ?? []), ...renderedSubmittedMessages],
      liveStreamItems
    )
  );
  let combinedTimeline = $derived<TimelineItem[]>([
    ...(sessionDetail?.timeline ?? []),
    ...renderedSubmittedMessages,
    ...renderedLiveStreamItems
  ]);
  function isPendingPermission(item: PermissionTimelineItem): boolean {
    const status = item.status?.toLowerCase() ?? "";
    const state = item.permissionDialog.state?.toLowerCase() ?? "";
    return status === "pending" || state === "pending";
  }

  let pendingPermissions = $derived<PermissionTimelineItem[]>(
    combinedTimeline.filter(
      (t): t is PermissionTimelineItem =>
        t.kind === "permission" &&
        isPendingPermission(t) &&
        !dismissedPermissionIds.includes(t.id)
    )
  );
  let pendingQuestions = $derived<UserQuestionTimelineItem[]>(
    combinedTimeline.filter(
      (t): t is UserQuestionTimelineItem =>
        t.kind === "question" && t.status === "pending" && !dismissedQuestionIds.includes(t.id)
    )
  );
  let turnRunning = $derived(currentTurnId !== null || turnStartedAtMs !== null);

  function sidebarAgentState(status: AgentActivityStatus): AgentState {
    if (status === "awaiting") return "awaiting";
    if (status === "running") return "running";
    return "idle";
  }

  function liveSidebarAgentState(session: SessionListItem): AgentState {
    if (selectedSession?.id !== session.id) return sidebarAgentState(session.activityStatus);
    if (pendingPermissions.length > 0 || pendingQuestions.length > 0) return "awaiting";
    if (turnRunning) return turnThinking ? "thinking" : "running";
    return sidebarAgentState(session.activityStatus);
  }

  function latestGroupMs(group: FolderGroup): number {
    return group.sessions.reduce((latest, session) => Math.max(latest, session.updatedAtMs), 0);
  }

  function pinnedIndex(values: string[], id: string): number {
    const index = values.indexOf(id);
    return index === -1 ? Number.MAX_SAFE_INTEGER : index;
  }

  function basenameFromPath(path: string): string {
    return path.split(/[\\/]+/).filter(Boolean).at(-1) ?? path;
  }

  function fallbackProjectLabel(session: SessionListItem): string {
    return basenameFromPath(session.folderPath || session.cwd || defaultWorkspaceCwd) || "Workspace";
  }

  function groupPathForSession(session: SessionListItem): string {
    return session.folderPath || session.cwd || defaultWorkspaceCwd || "Workspace";
  }

  function compareFolderGroups(left: FolderGroup, right: FolderGroup): number {
    const leftPin = Math.min(
      pinnedIndex(desktopPins.pinnedWorkspacePaths, left.path),
      pinnedIndex(desktopPins.pinnedWorkspacePaths, left.id)
    );
    const rightPin = Math.min(
      pinnedIndex(desktopPins.pinnedWorkspacePaths, right.path),
      pinnedIndex(desktopPins.pinnedWorkspacePaths, right.id)
    );
    return leftPin - rightPin
      || latestGroupMs(right) - latestGroupMs(left)
      || left.label.localeCompare(right.label);
  }

  function compareSessionsByRecency(left: SessionListItem, right: SessionListItem): number {
    return (
      right.updatedAtMs - left.updatedAtMs ||
      sessionDisplayName(left).localeCompare(sessionDisplayName(right))
    );
  }

  function withSelectedSessionFallback(sourceGroups: FolderGroup[]): FolderGroup[] {
    const session = selectedSession;
    if (!session) return sourceGroups;
    if (sourceGroups.some((group) => group.sessions.some((item) => item.id === session.id))) {
      return sourceGroups;
    }
    const path = groupPathForSession(session);
    const existingIndex = sourceGroups.findIndex((group) => group.path === path || group.id === path);
    if (existingIndex >= 0) {
      return sourceGroups.map((group, index) =>
        index === existingIndex
          ? {
              ...group,
              sessionCount: group.sessionCount + 1,
              sessions: [session, ...group.sessions].sort(compareSessionsByRecency)
            }
          : group
      );
    }
    return [
      {
        id: path,
        label: fallbackProjectLabel(session),
        path,
        sessionCount: 1,
        sessions: [session]
      },
      ...sourceGroups
    ].sort(compareFolderGroups);
  }

  function activeAgentFromSession(session: SessionListItem, project: string): ActiveAgent {
    return {
      id: session.id,
      name: sessionDisplayName(session).slice(0, 24),
      title: sessionDisplayTitle(session),
      project,
      branch: "",
      state: liveSidebarAgentState(session),
      updatedAtMs: session.updatedAtMs,
      pinned: desktopPins.pinnedAgentIds.includes(session.id)
    };
  }

  let sortedGroups = $derived<FolderGroup[]>(
    groups.slice().sort(compareFolderGroups)
  );
  let workspaceGroups = $derived<FolderGroup[]>(withSelectedSessionFallback(sortedGroups));

  let realAgents = $derived<ActiveAgent[]>(
    sortedGroups
      .flatMap((g) =>
        g.sessions.map((s) => activeAgentFromSession(s, g.label))
    )
      .slice()
      .sort((left, right) =>
        pinnedIndex(desktopPins.pinnedAgentIds, left.id) - pinnedIndex(desktopPins.pinnedAgentIds, right.id)
        || right.updatedAtMs - left.updatedAtMs
        || left.project.localeCompare(right.project)
      )
  );

  let selectedSessionGroup = $derived<FolderGroup | null>(
    selectedSession
      ? sortedGroups.find((group) =>
          group.sessions.some((session) => session.id === selectedSession?.id)
        ) ?? null
      : null
  );
  let selectedSessionFallbackAgent = $derived<ActiveAgent | null>(
    selectedSession && !realAgents.some((agent) => agent.id === selectedSession?.id)
      ? activeAgentFromSession(
          selectedSession,
          selectedSessionGroup?.label ?? fallbackProjectLabel(selectedSession)
        )
      : null
  );
  let activeAgents = $derived<ActiveAgent[]>(
    selectedSessionFallbackAgent ? [selectedSessionFallbackAgent, ...realAgents] : realAgents
  );

  let userChip = $derived<UserChip | null>(
    settingsSnapshot?.auth.length
      ? {
          initials: (settingsSnapshot.auth[0].email ?? "you").slice(0, 2).toUpperCase(),
          name: settingsSnapshot.auth[0].email ?? "You",
          meta: `${settingsSnapshot.auth[0].providerId}${
            settingsSnapshot.auth[0].planType ? " · " + settingsSnapshot.auth[0].planType : ""
          }`
        }
      : null
  );

  // ─────────────────────────────────────────────────────────────
  // Init
  // ─────────────────────────────────────────────────────────────
  // Auto-recap: when the window loses focus for `RECAP_IDLE_MS`, submit
  // `/recap` so the session shows a 1-2 sentence summary by the time the
  // user comes back. Matches the TUI's idle-timer auto-trigger; mirrors
  // claude-code's `tengu_sedge_lantern` blur path. The slash command
  // dispatcher inside puffer-core decides whether to actually run (gates
  // on `config.recap.enabled` + skip checks); this layer just deals with
  // "is the window away long enough to be worth asking."
  const RECAP_IDLE_MS = 180_000;
  let recapBlurTimer: ReturnType<typeof setTimeout> | null = null;
  let composerHasDraft = $state(false);

  function armRecapBlurTimer() {
    if (turnRunning || composerHasDraft) return;
    if (recapBlurTimer != null) return;
    recapBlurTimer = setTimeout(() => {
      recapBlurTimer = null;
      if (!selectedSession || turnRunning || composerHasDraft) return;
      void submitMessage("/recap", {});
    }, RECAP_IDLE_MS);
  }

  function cancelRecapBlurTimer() {
    if (recapBlurTimer != null) {
      clearTimeout(recapBlurTimer);
      recapBlurTimer = null;
    }
  }

  function handleShellKeydown(event: KeyboardEvent) {
    if (event.defaultPrevented || onboarding) return;
    if ((event.metaKey || event.ctrlKey) && event.key === ",") {
      event.preventDefault();
      onSelectScreen("settings");
    }
  }

  function loadDesktopPreferences(): DesktopPreferences {
    if (typeof window === "undefined") return { ...defaultDesktopPreferences };
    try {
      const raw = window.localStorage.getItem(DESKTOP_PREFERENCES_KEY);
      if (!raw) return { ...defaultDesktopPreferences };
      const parsed = JSON.parse(raw) as Partial<DesktopPreferences>;
      return {
        ...defaultDesktopPreferences,
        rememberSession: parsed.rememberSession === true,
        rememberInspectorLayout: parsed.rememberInspectorLayout === true,
        launchInspectorOpen:
          typeof parsed.launchInspectorOpen === "boolean"
            ? parsed.launchInspectorOpen
            : defaultDesktopPreferences.launchInspectorOpen,
        defaultInspectorTab: parsed.defaultInspectorTab ?? defaultDesktopPreferences.defaultInspectorTab,
        defaultInspectorWidth:
          typeof parsed.defaultInspectorWidth === "number"
            ? parsed.defaultInspectorWidth
            : defaultDesktopPreferences.defaultInspectorWidth,
        remoteEnabled: parsed.remoteEnabled === true,
        remoteTarget: typeof parsed.remoteTarget === "string" ? parsed.remoteTarget : "",
        remoteCwd: typeof parsed.remoteCwd === "string" ? parsed.remoteCwd : ""
      };
    } catch {
      return { ...defaultDesktopPreferences };
    }
  }

  function persistDesktopPreferences(preferences: DesktopPreferences) {
    if (typeof window === "undefined") return;
    window.localStorage.setItem(DESKTOP_PREFERENCES_KEY, JSON.stringify(preferences));
  }

  function workspaceIdentity(): string {
    return settingsSnapshot?.workspaceRoot || daemonWorkspaceRoot || defaultWorkspaceCwd || "";
  }

  function loadRememberedSession(): RememberedSession | null {
    if (typeof window === "undefined") return null;
    try {
      const raw = window.localStorage.getItem(REMEMBERED_SESSION_KEY);
      if (!raw) return null;
      const parsed = JSON.parse(raw) as Partial<RememberedSession>;
      if (typeof parsed.sessionId !== "string" || !parsed.sessionId) return null;
      return {
        sessionId: parsed.sessionId,
        workspaceRoot: typeof parsed.workspaceRoot === "string" ? parsed.workspaceRoot : ""
      };
    } catch {
      return null;
    }
  }

  function clearRememberedSession() {
    if (typeof window === "undefined") return;
    window.localStorage.removeItem(REMEMBERED_SESSION_KEY);
  }

  function rememberSession(sessionId: string) {
    if (!desktopPreferences.rememberSession || typeof window === "undefined") return;
    const workspaceRoot = workspaceIdentity();
    if (!workspaceRoot || !sessionId) return;
    window.localStorage.setItem(
      REMEMBERED_SESSION_KEY,
      JSON.stringify({ workspaceRoot, sessionId } satisfies RememberedSession)
    );
  }

  function findSessionById(sessionId: string): SessionListItem | null {
    return groups.flatMap((g) => g.sessions).find((session) => session.id === sessionId) ?? null;
  }

  async function openRememberedSessionIfAvailable(): Promise<boolean> {
    if (!desktopPreferences.rememberSession) return false;
    const remembered = loadRememberedSession();
    if (!remembered) return false;
    const workspaceRoot = workspaceIdentity();
    if (remembered.workspaceRoot && workspaceRoot && remembered.workspaceRoot !== workspaceRoot) {
      return false;
    }
    const session = findSessionById(remembered.sessionId);
    if (!session) return false;
    await openSession(session);
    openAgentSessionId = session.id;
    tweaks = { ...tweaks, screen: "workspace" };
    return true;
  }

  function hasProviderAuth(snapshot: SettingsSnapshot | null): boolean {
    return (snapshot?.auth?.length ?? 0) > 0;
  }

  function shouldShowOnboarding(snapshot: SettingsSnapshot | null): boolean {
    if (!hasProviderAuth(snapshot)) return true;
    if (forceOnboarding && !onboardingCompleted) return true;
    return !skipOnboarding;
  }

  function daemonFingerprint(client: DaemonClient): string {
    const hs = client.handshake;
    return [hs.url, hs.token, hs.protocolVersion, hs.workspaceRoot].join("\n");
  }

  function updateDaemonIdentity(client: DaemonClient | null = currentDaemonClient()) {
    daemonUrl = client?.handshake.url ?? null;
    daemonWorkspaceRoot = client?.handshake.workspaceRoot ?? null;
    daemonClientFingerprint = client ? daemonFingerprint(client) : null;
  }

  async function adoptCurrentDaemonClient(client: DaemonClient, workspaceRoot: string) {
    defaultWorkspaceCwd = workspaceRoot;
    resetDaemonScopedSessionState();
    attachDaemonClient(client);
    await refreshSettings();
    await refreshPins();
  }

  function clearDaemonClientListeners() {
    for (const unlisten of daemonClientUnlisteners) {
      unlisten();
    }
    daemonClientUnlisteners = [];
  }

  function attachDaemonClient(client: DaemonClient) {
    clearDaemonClientListeners();
    updateDaemonIdentity(client);
    daemonClientUnlisteners = [
      client.onConnectionChange((s) => {
        connectionState = s;
        updateDaemonIdentity(client);
        // When we reconnect after a drop, refresh groups + re-open the
        // selected session so the UI catches up.
        if (s === "open" && !onboarding) {
          void refreshPins();
          void refreshSettings();
          void refreshGroups();
          if (selectedSession) void openSession(selectedSession);
        }
      }),
      // Any time a session is created or a turn finishes, refresh the
      // workspace board + sidebar. Coalesced by `refreshGroups`'s own
      // loading guard.
      client.on<{ sessionId?: string; reason?: string }>("workspace:sessions:changed", (event) => {
        void refreshGroups();
        if (
          selectedSession &&
          event?.sessionId === selectedSession.id &&
          (event.reason === "generated_title" || event.reason === "rename_session")
        ) {
          void openSession(selectedSession, { showLoading: false, resetLiveState: false });
        }
      }),
      client.on<DesktopPinState>("desktop:pins:changed", (pins) => {
        desktopPins = {
          pinnedAgentIds: Array.isArray(pins?.pinnedAgentIds) ? pins.pinnedAgentIds : [],
          pinnedWorkspacePaths: Array.isArray(pins?.pinnedWorkspacePaths) ? pins.pinnedWorkspacePaths : []
        };
      })
    ];
  }

  onMount(() => {
    tweaks = loadTweaks();
    applyTweaksToDocument(tweaks);
    if (forceOnboarding && !onboardingCompleted) {
      onboarding = true;
    }
    window.addEventListener("blur", armRecapBlurTimer);
    window.addEventListener("focus", cancelRecapBlurTimer);
    window.addEventListener("keydown", handleShellKeydown, true);
    void init();
    return () => {
      cancelRecapBlurTimer();
      clearDaemonClientListeners();
      if (sessionEventUnlisten) {
        sessionEventUnlisten();
        sessionEventUnlisten = null;
      }
      window.removeEventListener("blur", armRecapBlurTimer);
      window.removeEventListener("focus", cancelRecapBlurTimer);
      window.removeEventListener("keydown", handleShellKeydown, true);
    };
  });

  $effect(() => {
    applyTweaksToDocument(tweaks);
    persistTweaks(tweaks); // Tweaks are renderer ergonomics, not workspace data.
  });

  $effect(() => {
    persistDesktopPreferences(desktopPreferences);
  });

  async function init() {
    void loadDefaultWorkspace()
      .then((info) => {
        defaultWorkspaceCwd = info.cwd;
      })
      .catch(() => {
        /* daemon might be remote / unavailable; keep default empty */
      });
    // Observe daemon connection state so the banner reflects reality.
    void ensureLocalDaemonClient()
      .then((client) => {
        attachDaemonClient(client);
      })
      .catch(() => {
        /* connection may be unavailable (web preview); stay idle */
      });
    await refreshSettings();
    if (!onboarding) {
      await refreshPins();
      await refreshGroups();
      // When drilled into a mock agent via the screenshot harness (or the
      // user just landed after login without picking a session), auto-open
      // the most recent real session so the Chat tab renders a transcript
      // instead of the empty state.
      if (!selectedSession) {
        const restored = await openRememberedSessionIfAvailable();
        if (restored) return;
        const firstReal = sortedGroups
          .flatMap((g) => g.sessions)
          .sort((a, b) => b.updatedAtMs - a.updatedAtMs)[0];
        if (firstReal) {
          await openSession(firstReal);
        }
      }
    }
  }

  // ─────────────────────────────────────────────────────────────
  // Handlers — mostly lifted from the prior App.svelte
  // ─────────────────────────────────────────────────────────────
  async function refreshSettings() {
    const generation = ++settingsRefreshGeneration;
    settingsLoading = true;
    try {
      const snapshot = await loadSettingsSnapshot(remoteConnection);
      if (generation !== settingsRefreshGeneration) return;
      settingsSnapshot = snapshot;
      onboarding = shouldShowOnboarding(settingsSnapshot);
      // Re-scan ~/.claude / ~/.codex so the LoginView can offer one-click
      // imports for credentials the user already has on disk. Failure is
      // non-fatal — the manual API-key path still works.
      void listExternalCredentials()
        .then((found) => {
          if (generation !== settingsRefreshGeneration) return;
          externalCredentials = found;
        })
        .catch(() => {
          if (generation !== settingsRefreshGeneration) return;
          externalCredentials = [];
        });
      statusMessage = "Settings snapshot refreshed.";
    } catch (error) {
      if (generation !== settingsRefreshGeneration) return;
      statusMessage = String(error);
      if (!skipOnboarding) onboarding = true;
    } finally {
      if (generation === settingsRefreshGeneration) {
        settingsLoading = false;
      }
    }
  }

  async function handleImportExternal(providerId: string, source: "claude" | "codex") {
    if (importBusyKey) return;
    importBusyKey = `${providerId}::${source}`;
    authError = null;
    try {
      settingsSnapshot = await importExternalCredential(providerId, source);
      onboardingCompleted = true;
      onboarding = false;
      tweaks = { ...tweaks, screen: "workspace" };
      statusMessage = `Imported ${source} credential into ${providerId}.`;
      void listExternalCredentials()
        .then((found) => {
          externalCredentials = found;
        })
        .catch(() => {});
      await refreshGroups();
    } catch (error) {
      authError = String(error);
      statusMessage = authError;
    } finally {
      importBusyKey = null;
    }
  }

  async function finishOnboarding() {
    onboardingCompleted = true;
    onboarding = false;
    tweaks = { ...tweaks, screen: "workspace" };
    if (typeof window !== "undefined") {
      window.localStorage.setItem("puffer-desktop:skip-onboarding", "1");
    }
    statusMessage = "Onboarding complete.";
    await refreshPins();
    await refreshGroups();
  }

  async function handleOauthLogin(providerId: string) {
    if (authBusyProviderId) return;
    authBusyProviderId = providerId;
    authError = null;
    try {
      settingsSnapshot = await loginWithOauth(providerId, remoteConnection);
      onboardingCompleted = true;
      onboarding = false;
      tweaks = { ...tweaks, screen: "workspace" };
      statusMessage = `Connected to ${providerId}.`;
      await refreshGroups();
    } catch (error) {
      authError = String(error);
      statusMessage = authError;
    } finally {
      authBusyProviderId = null;
    }
  }

  async function handleApiKeyLogin(providerId: string, apiKey: string) {
    if (authBusyProviderId) return;
    authBusyProviderId = providerId;
    authError = null;
    try {
      // Prefer the daemon path; it reuses the workspace auth store and
      // lets remote daemons (SSH) pick up credentials server-side. Falls
      // back to the Tauri-invoke path inside the wrapper when no daemon
      // is reachable. For genuinely remote connections we stay on the
      // Tauri path so `remoteConnection` (SSH command) is honored.
      if (remoteConnection.enabled) {
        settingsSnapshot = await loginWithApiKey(providerId, apiKey, remoteConnection);
      } else {
        settingsSnapshot = await loginWithApiKeyViaDaemon(providerId, apiKey);
      }
      onboardingCompleted = true;
      onboarding = false;
      tweaks = { ...tweaks, screen: "workspace" };
      statusMessage = `Stored API key for ${providerId}.`;
      await refreshGroups();
    } catch (error) {
      authError = String(error);
      statusMessage = authError;
    } finally {
      authBusyProviderId = null;
    }
  }

  async function handleLogout(providerId: string) {
    if (authBusyProviderId) return;
    authBusyProviderId = providerId;
    authError = null;
    try {
      if (remoteConnection.enabled) {
        settingsSnapshot = await logoutProvider(providerId, remoteConnection);
      } else {
        settingsSnapshot = await logoutProviderViaDaemon(providerId);
      }
      statusMessage = `Disconnected ${providerId}.`;
      if ((settingsSnapshot.auth?.length ?? 0) === 0) {
        groups = [];
        selectedSession = null;
        sessionDetail = null;
        onboarding = true;
      }
    } catch (error) {
      authError = String(error);
      statusMessage = authError;
    } finally {
      authBusyProviderId = null;
    }
  }

  async function refreshGroups() {
    const generation = ++groupsRefreshGeneration;
    groupsLoading = true;
    try {
      const nextGroups = await listGroupedSessionsFromDaemon();
      if (generation !== groupsRefreshGeneration) return;
      groups = nextGroups;
      statusMessage =
        groups.length === 0
          ? "No sessions in this workspace yet."
          : `${groups.length} project${groups.length === 1 ? "" : "s"} loaded.`;
    } catch (error) {
      if (generation !== groupsRefreshGeneration) return;
      statusMessage = String(error);
    } finally {
      if (generation === groupsRefreshGeneration) {
        groupsLoading = false;
      }
    }
  }

  async function refreshPins() {
    try {
      desktopPins = await loadDesktopPins();
    } catch (error) {
      statusMessage = `Failed to load pins: ${error}`;
    }
  }

  function applyPin(kind: "agent" | "workspace", id: string, pinned: boolean) {
    if (kind === "agent") {
      const next = desktopPins.pinnedAgentIds.filter((value) => value !== id);
      desktopPins = {
        ...desktopPins,
        pinnedAgentIds: pinned ? [id, ...next] : next
      };
      return;
    }
    const next = desktopPins.pinnedWorkspacePaths.filter((value) => value !== id);
    desktopPins = {
      ...desktopPins,
      pinnedWorkspacePaths: pinned ? [id, ...next] : next
    };
  }

  function desktopPinKey(kind: "agent" | "workspace", id: string): string {
    return `${kind}:${id}`;
  }

  async function toggleDesktopPin(kind: "agent" | "workspace", id: string, pinned: boolean) {
    const key = desktopPinKey(kind, id);
    if (desktopPinInFlightKeys.includes(key)) return;
    desktopPinInFlightKeys = [...desktopPinInFlightKeys, key];
    applyPin(kind, id, pinned);
    try {
      desktopPins = await setDesktopPin(kind, id, pinned);
      statusMessage = `${pinned ? "Pinned" : "Unpinned"} ${kind}.`;
    } catch (error) {
      applyPin(kind, id, !pinned);
      statusMessage = `Failed to update pin: ${error}`;
    } finally {
      desktopPinInFlightKeys = desktopPinInFlightKeys.filter((value) => value !== key);
    }
  }

  type OpenSessionOptions = {
    showLoading?: boolean;
    resetLiveState?: boolean;
  };

  function resetLiveTurnState() {
    submittedMessages = [];
    liveStreamItems = [];
    replayTextByTurn = {};
    turnPermissionLookup = {};
    turnQuestionLookup = {};
    resolvingPermissionIds = [];
    resolvingQuestionIds = [];
    desktopPinInFlightKeys = [];
    currentTurnId = null;
    cancelingTurnId = null;
    turnStartedAtMs = null;
    turnThinking = false;
    turnStatusHint = null;
    settledTurnIds = new Set();
  }

  async function openSession(session: SessionListItem, options: OpenSessionOptions = {}) {
    const showLoading = options.showLoading ?? selectedSession?.id !== session.id;
    const resetLiveState = options.resetLiveState ?? true;
    const loadGeneration = ++sessionLoadGeneration;
    if (showLoading) sessionLoading = true;
    if (resetLiveState && selectedSession?.id !== session.id) {
      selectedSession = session;
      sessionDetail = null;
      rememberSession(session.id);
      resetLiveTurnState();
    }
    try {
      const detail = await loadSessionDetailFromDaemon(session.id);
      if (loadGeneration !== sessionLoadGeneration) return;
      const timeline = resetLiveState
        ? detail.timeline
        : reuseTransientMessageIds(detail.timeline, [...submittedMessages, ...liveStreamItems]);
      selectedSession = detail.session;
      sessionDetail = { ...detail, timeline };
      rememberSession(detail.session.id);
      if (resetLiveState) {
        // New session lands: drop any lingering live-stream items + local draft
        // so the composer feels fresh.
        resetLiveTurnState();
      } else {
        submittedMessages = stillMissingFromPersisted(timeline, submittedMessages);
        liveStreamItems = stillMissingFromPersisted(timeline, liveStreamItems);
      }
      statusMessage = `Loaded ${detail.timeline.length} conversation items.`;
    } catch (error) {
      if (loadGeneration !== sessionLoadGeneration) return;
      const detail = errorText(error);
      statusMessage = detail;
      if (selectedSession?.id === session.id || openAgentSessionId === session.id) {
        appendAgentError("Conversation load failed", detail, "load-session");
      }
    } finally {
      if (showLoading && loadGeneration === sessionLoadGeneration) sessionLoading = false;
    }
  }

  /** Creates a blank session via the daemon in the given cwd (or the daemon's
   *  default workspace if unset) and opens AgentDetail on it. The workspace
   *  list refreshes so the new session appears as an agent card. */
  function requestNewAgent(cwd: string) {
    newSessionCwd = cwd || defaultWorkspaceCwd || "";
  }

  async function handleNewAgent(cwd: string, providerId?: string): Promise<boolean> {
    try {
      const created = await createSession(cwd || undefined, providerId);
      await refreshGroups();
      const newSession =
        groups.flatMap((g) => g.sessions).find((s) => s.id === created.sessionId) ?? null;
      if (newSession) {
        await openSession({
          ...newSession,
          providerId: created.providerId ?? providerId ?? newSession.providerId,
          modelId: created.modelId ?? newSession.modelId
        });
      } else {
        // Fall back to a synthetic SessionListItem so the AgentDetail can
        // still open; reloading later will pick up the real record.
        const fallback: SessionListItem = {
          id: created.sessionId,
          displayName: null,
          generatedTitle: null,
          title: "New Session",
          cwd: created.cwd,
          folderPath: created.cwd,
          updatedAtMs: created.createdAtMs,
          createdAtMs: created.createdAtMs,
          eventCount: 0,
          activityStatus: "idle",
          slug: null,
          tags: [],
          note: null,
          parentSessionId: null,
          providerId: created.providerId ?? providerId ?? "codex",
          modelId: created.modelId ?? null
        };
        await openSession(fallback);
      }
      openAgentSessionId = created.sessionId;
      tweaks = { ...tweaks, screen: "workspace" };
      statusMessage = `New ${created.providerId ?? providerId ?? "agent"} session in ${cwd || defaultWorkspaceCwd || "default workspace"}.`;
      return true;
    } catch (error) {
      statusMessage = `Failed to create session: ${error}`;
      return false;
    }
  }

  function updateDesktopPreference<K extends keyof DesktopPreferences>(key: K, value: DesktopPreferences[K]) {
    desktopPreferences = { ...desktopPreferences, [key]: value };
    if (key === "rememberSession") {
      if (value === true && selectedSession) {
        rememberSession(selectedSession.id);
      } else if (value === false) {
        clearRememberedSession();
      }
    }
  }

  function resetDesktopPreferences() {
    desktopPreferences = { ...defaultDesktopPreferences };
    clearRememberedSession();
    statusMessage = "Desktop preferences reset.";
  }

  function resetAppearanceTweaks() {
    tweaks = {
      ...tweaks,
      theme: defaultTweaks.theme,
      accent: defaultTweaks.accent,
      density: defaultTweaks.density,
      fontMix: defaultTweaks.fontMix,
      userName: defaultTweaks.userName
    };
    statusMessage = "Appearance reset.";
  }

  function resetDaemonScopedSessionState() {
    selectedSession = null;
    sessionDetail = null;
    openAgentSessionId = null;
    openProjectId = null;
    submittedMessages = [];
    submitMessageInFlightSessionIds = [];
    dismissedPermissionIds = [];
    dismissedQuestionIds = [];
    resolvingPermissionIds = [];
    resolvingQuestionIds = [];
    desktopPinInFlightKeys = [];
    liveStreamItems = [];
    replayTextByTurn = {};
    turnPermissionLookup = {};
    turnQuestionLookup = {};
    currentTurnId = null;
    cancelingTurnId = null;
    turnStartedAtMs = null;
    turnThinking = false;
    turnStatusHint = null;
    settledTurnIds = new Set();
    sessionLoadGeneration += 1;
    if (sessionEventUnlisten) {
      sessionEventUnlisten();
      sessionEventUnlisten = null;
    }
    subscribedSessionId = null;
  }

  async function handleWorkspaceSwitched(hs: {
    url: string;
    workspaceRoot: string;
  }) {
    showWorkspacePicker = false;
    const client = currentDaemonClient();
    if (client) {
      await adoptCurrentDaemonClient(client, hs.workspaceRoot);
    } else {
      defaultWorkspaceCwd = hs.workspaceRoot;
      resetDaemonScopedSessionState();
      daemonUrl = hs.url;
      daemonWorkspaceRoot = hs.workspaceRoot;
      daemonClientFingerprint = null;
      await refreshSettings();
      await refreshPins();
    }
    await refreshGroups();
    await openRememberedSessionIfAvailable();
    statusMessage = `Switched workspace to ${hs.workspaceRoot}.`;
  }

  async function handleRemoteBash(command: string) {
    if (!remoteConnection.enabled) return;
    remoteBusy = true;
    try {
      remoteOperation = await runRemoteBash(remoteConnection, command);
      statusMessage = remoteOperation.success ? "Remote bash finished." : "Remote bash failed.";
    } catch (error) {
      statusMessage = String(error);
      remoteOperation = { success: false, stdout: "", stderr: String(error) };
    } finally {
      remoteBusy = false;
    }
  }

  async function handleRemoteRead(path: string) {
    if (!remoteConnection.enabled) return;
    remoteBusy = true;
    try {
      remoteOperation = await readRemoteFile(remoteConnection, path);
      statusMessage = remoteOperation.success ? `Read remote file ${path}.` : `Reading ${path} failed.`;
    } catch (error) {
      statusMessage = String(error);
      remoteOperation = { success: false, stdout: "", stderr: String(error) };
    } finally {
      remoteBusy = false;
    }
  }

  async function handleRemoteWrite(path: string, contents: string) {
    if (!remoteConnection.enabled) return;
    remoteBusy = true;
    try {
      remoteOperation = await writeRemoteFile(remoteConnection, path, contents);
      statusMessage = remoteOperation.success ? `Wrote remote file ${path}.` : `Writing ${path} failed.`;
    } catch (error) {
      statusMessage = String(error);
      remoteOperation = { success: false, stdout: "", stderr: String(error) };
    } finally {
      remoteBusy = false;
    }
  }

  // Phase 2+: these aren't surfaced yet in the new UI, but we keep the handlers
  // live so PR / repo actions continue to work through whatever embeds them.
  // Referenced via the noop below so TS/svelte-check don't treat them as dead.
  const _keepAlive = { createPullRequest, mergePullRequest, refreshRepoStatus, cancelTurn };
  void _keepAlive;

  function updateTweak<K extends keyof Tweaks>(key: K, value: Tweaks[K]) {
    tweaks = { ...tweaks, [key]: value };
  }

  function onSelectScreen(id: ScreenId) {
    tweaks = { ...tweaks, screen: id };
    openProjectId = null;
    openAgentSessionId = null;
  }

  function onOpenAgent(id: string) {
    const realTarget = groups.flatMap((g) => g.sessions).find((s) => s.id === id);
    if (!realTarget) {
      if (selectedSession?.id === id) {
        openAgentSessionId = id;
        tweaks = { ...tweaks, screen: "workspace" };
      }
      return;
    }
    openAgentSessionId = realTarget.id;
    tweaks = { ...tweaks, screen: "workspace" };
    void openSession(realTarget);
  }

  function onCloseAgent() {
    openAgentSessionId = null;
  }

  function onOpenProject(id: string) {
    openProjectId = id;
    openAgentSessionId = null;
    tweaks = { ...tweaks, screen: "workspace" };
  }

  /** Fired by ConnectProjectModal once a clone+create has landed. Refreshes
   *  the workspace board and drills straight into the new session. */
  async function handleSessionReady(sessionId: string) {
    const client = currentDaemonClient();
    const currentFingerprint = client ? daemonFingerprint(client) : null;
    if (client && currentFingerprint !== daemonClientFingerprint) {
      await adoptCurrentDaemonClient(client, client.handshake.workspaceRoot);
    } else {
      void loadDefaultWorkspace()
        .then((info) => {
          defaultWorkspaceCwd = info.cwd;
        })
        .catch(() => {});
    }
    await refreshGroups();
    const session = groups.flatMap((g) => g.sessions).find((s) => s.id === sessionId);
    if (session) {
      await openSession(session);
    }
    openAgentSessionId = sessionId;
    tweaks = { ...tweaks, screen: "workspace" };
  }

  function providerIsAuthenticated(providerId: string | null | undefined): boolean {
    if (!settingsSnapshot || !providerId) return true;
    return providerIdInSet(
      providerId,
      settingsSnapshot.auth.map((auth) => auth.providerId)
    );
  }

  function submitMessageInFlightFor(sessionId: string): boolean {
    return submitMessageInFlightSessionIds.includes(sessionId);
  }

  function setSubmitMessageInFlight(sessionId: string, inFlight: boolean) {
    if (inFlight) {
      if (!submitMessageInFlightSessionIds.includes(sessionId)) {
        submitMessageInFlightSessionIds = [...submitMessageInFlightSessionIds, sessionId];
      }
      return;
    }
    submitMessageInFlightSessionIds = submitMessageInFlightSessionIds.filter(
      (id) => id !== sessionId
    );
  }

  async function submitMessage(message: string, options: AgentTurnOptions = {}) {
    if (!selectedSession) {
      statusMessage = "Select a session to send a message.";
      return false;
    }
    const sessionAtSubmit = selectedSession;
    const submitSessionId = sessionAtSubmit.id;
    if (
      submitMessageInFlightFor(submitSessionId) ||
      turnStartedAtMs !== null ||
      currentTurnId !== null
    ) {
      statusMessage = "Wait for the current turn to finish before sending another message.";
      return false;
    }
    const requestedProviderId =
      options.providerId ?? sessionAtSubmit.providerId ?? settingsSnapshot?.config.defaultProvider;
    if (!providerIsAuthenticated(requestedProviderId)) {
      const detail = `Reconnect ${requestedProviderId} before continuing this session.`;
      statusMessage = detail;
      appendAgentError("Provider disconnected", detail, "provider-auth");
      return false;
    }
    setSubmitMessageInFlight(submitSessionId, true);
    const now = Date.now();
    const localUserId = `local-user-${now}`;
    submittedMessages = [
      ...submittedMessages,
      {
        id: localUserId,
        kind: "user",
        createdAtMs: now,
        title: "User",
        summary: message,
        body: message,
        meta: []
      }
    ];
    turnStartedAtMs = now;
    turnThinking = true;
    turnStatusHint = "Thinking";
    try {
      const turnId = await runAgentTurn(submitSessionId, message, options);
      if (selectedSession?.id !== submitSessionId) {
        submittedMessages = submittedMessages.filter((item) => item.id !== localUserId);
        return false;
      }
      currentTurnId = turnId;
      cancelingTurnId = null;
      settledTurnIds.delete(turnId);
      statusMessage = `Agent turn ${turnId.slice(0, 8)} started.`;
      return true;
    } catch (error) {
      if (selectedSession?.id !== submitSessionId) return false;
      submittedMessages = submittedMessages.filter((item) => item.id !== localUserId);
      currentTurnId = null;
      cancelingTurnId = null;
      turnStartedAtMs = null;
      turnThinking = false;
      turnStatusHint = null;
      const detail = errorText(error);
      statusMessage = `run_agent_turn failed: ${detail}`;
      appendAgentError("Agent start failed", detail, "turn-start-error");
      return false;
    } finally {
      setSubmitMessageInFlight(submitSessionId, false);
    }
  }

  async function renameSelectedSession(title: string) {
    if (!selectedSession) return;
    const previous = selectedSession;
    const renameSessionId = previous.id;
    try {
      const detail = await renameSession(renameSessionId, title);
      if (selectedSession?.id !== renameSessionId) return;
      selectedSession = detail.session;
      sessionDetail = detail;
      await refreshGroups();
      statusMessage = title.trim() ? "Session title updated." : "Session title reset.";
    } catch (error) {
      if (selectedSession?.id !== renameSessionId) return;
      selectedSession = previous;
      statusMessage = `Failed to rename session: ${errorText(error)}`;
      throw error;
    }
  }

  function mapPermissionAction(choice: string): "allow_once" | "allow_session" | "allow_all_session" | "deny" {
    const n = choice.toLowerCase();
    if (n.includes("always") && n.includes("session")) return "allow_all_session";
    if (n.includes("always")) return "allow_all_session";
    if (n.includes("session")) return "allow_session";
    if (n.includes("deny") || n.includes("never")) return "deny";
    return "allow_once";
  }

  async function resolvePermission(permissionId: string, choice: string) {
    if (resolvingPermissionIds.includes(permissionId)) return;
    resolvingPermissionIds = [...resolvingPermissionIds, permissionId];
    try {
      const mapping = turnPermissionLookup[permissionId];
      if (mapping) {
        try {
          await resolveTurnPermission(mapping.turnId, mapping.requestId, mapPermissionAction(choice));
          dismissedPermissionIds = [...dismissedPermissionIds, permissionId];
          statusMessage = `${choice} sent to agent.`;
          if (currentTurnId === mapping.turnId) {
            turnThinking = false;
            turnStatusHint = "Running";
          }
          const { [permissionId]: _drop, ...rest } = turnPermissionLookup;
          turnPermissionLookup = rest;
        } catch (error) {
          const detail = errorText(error);
          statusMessage = `resolve_permission failed: ${detail}`;
          appendAgentError("Permission response failed", detail, "permission-error");
        }
      } else {
        dismissedPermissionIds = [...dismissedPermissionIds, permissionId];
        statusMessage = `${choice} selected (no in-flight turn).`;
      }
    } finally {
      resolvingPermissionIds = resolvingPermissionIds.filter((id) => id !== permissionId);
    }
  }

  async function resolveUserQuestion(
    questionId: string,
    answers: Record<string, string | string[]>,
    annotations: Record<string, Record<string, string>> = {}
  ) {
    if (resolvingQuestionIds.includes(questionId)) return;
    resolvingQuestionIds = [...resolvingQuestionIds, questionId];
    try {
      const mapping = turnQuestionLookup[questionId];
      if (mapping) {
        try {
          await resolveTurnUserQuestion(mapping.turnId, mapping.requestId, answers, annotations);
          dismissedQuestionIds = [...dismissedQuestionIds, questionId];
          statusMessage = "Answer sent to agent.";
          if (currentTurnId === mapping.turnId) {
            turnThinking = false;
            turnStatusHint = "Running";
          }
          const { [questionId]: _drop, ...rest } = turnQuestionLookup;
          turnQuestionLookup = rest;
        } catch (error) {
          const detail = errorText(error);
          statusMessage = `resolve_user_question failed: ${detail}`;
          appendAgentError("Question response failed", detail, "question-error");
        }
      } else {
        dismissedQuestionIds = [...dismissedQuestionIds, questionId];
        statusMessage = "Answer selected (no in-flight turn).";
      }
    } finally {
      resolvingQuestionIds = resolvingQuestionIds.filter((id) => id !== questionId);
    }
  }

  async function cancelCurrentTurn() {
    const turnId = currentTurnId;
    if (!turnId || cancelingTurnId === turnId) return;
    cancelingTurnId = turnId;
    turnStatusHint = "Cancel requested";
    try {
      await cancelTurn(turnId);
      statusMessage = `Cancel requested for turn ${turnId.slice(0, 8)}.`;
    } catch (error) {
      if (currentTurnId !== turnId) return;
      cancelingTurnId = null;
      turnStatusHint = "Running";
      const detail = errorText(error);
      statusMessage = `cancel_turn failed: ${detail}`;
      appendAgentError("Cancel failed", detail, "cancel-error");
    }
  }

  function appendLive(item: TimelineItem) {
    const existingIdx = liveStreamItems.findIndex((existing) => existing.id === item.id);
    if (existingIdx >= 0) {
      liveStreamItems = [
        ...liveStreamItems.slice(0, existingIdx),
        item,
        ...liveStreamItems.slice(existingIdx + 1)
      ];
      return;
    }
    liveStreamItems = [...liveStreamItems, item];
  }

  function errorText(error: unknown): string {
    return error instanceof Error ? error.message : String(error);
  }

  function appendAgentError(title: string, body: string, code: string) {
    const trimmed = body.trim() || "Unknown error.";
    liveErrorSeq += 1;
    appendLive({
      id: `live-error-${code}-${Date.now()}-${liveErrorSeq}`,
      kind: "system",
      title,
      summary: trimmed,
      body: trimmed,
      meta: ["error", code],
      status: "error"
    });
  }

  function timelineItemBody(item: TimelineItem): string {
    return "body" in item && typeof item.body === "string" ? item.body : "";
  }

  function timelineHasBody(items: TimelineItem[], kind: TimelineItem["kind"], body: string): boolean {
    const trimmed = body.trim();
    if (!trimmed) return true;
    return items.some((item) => item.kind === kind && timelineItemBody(item).includes(trimmed));
  }

  function transientMessageSignature(item: TimelineItem): string | null {
    if (item.kind !== "user" && item.kind !== "assistant") return null;
    const body = timelineItemBody(item).trim();
    if (!body) return null;
    return `${item.kind}:${body}`;
  }

  function stableJsonText(value: unknown): string {
    if (Array.isArray(value)) {
      return `[${value.map(stableJsonText).join(",")}]`;
    }
    if (value && typeof value === "object") {
      const record = value as Record<string, unknown>;
      return `{${Object.keys(record)
        .sort()
        .map((key) => `${JSON.stringify(key)}:${stableJsonText(record[key])}`)
        .join(",")}}`;
    }
    return JSON.stringify(value) ?? "undefined";
  }

  function normalizedToolInput(item: TimelineItem): string | null {
    if (item.kind !== "tool") return null;
    const raw = item.input?.trim();
    if (raw) {
      try {
        return stableJsonText(JSON.parse(raw) as unknown);
      } catch {
        return raw;
      }
    }
    return item.inputJson ? stableJsonText(item.inputJson) : null;
  }

  function transientToolSignature(item: TimelineItem): string | null {
    if (item.kind !== "tool") return null;
    const input = normalizedToolInput(item);
    return input ? `${item.toolName}:${input}` : null;
  }

  function reuseTransientMessageIds(
    persisted: TimelineItem[],
    transient: TimelineItem[]
  ): TimelineItem[] {
    const transientIds = new Map<string, string[]>();
    for (let index = transient.length - 1; index >= 0; index -= 1) {
      const item = transient[index];
      const signature = transientMessageSignature(item);
      if (!signature) continue;
      transientIds.set(signature, [...(transientIds.get(signature) ?? []), item.id]);
    }
    const keyed = [...persisted];
    for (let index = keyed.length - 1; index >= 0; index -= 1) {
      const item = keyed[index];
      const signature = transientMessageSignature(item);
      const candidates = signature ? transientIds.get(signature) : null;
      const replacement = candidates?.shift();
      if (replacement) keyed[index] = { ...item, id: replacement };
    }
    return keyed;
  }

  function timelineItemCreatedAtMs(item: TimelineItem): number | null {
    return typeof item.createdAtMs === "number" && Number.isFinite(item.createdAtMs)
      ? item.createdAtMs
      : null;
  }

  function transientTimestampsMatch(persisted: TimelineItem, pending: TimelineItem): boolean {
    const persistedAt = timelineItemCreatedAtMs(persisted);
    const pendingAt = timelineItemCreatedAtMs(pending);
    if (persistedAt === null || pendingAt === null) return true;
    return Math.abs(persistedAt - pendingAt) <= 5 * 60 * 1000;
  }

  function timelineHasTransientMatch(items: TimelineItem[], pending: TimelineItem): boolean {
    const body = timelineItemBody(pending).trim();
    if (!body) {
      const toolSignature = transientToolSignature(pending);
      if (toolSignature) {
        return items.some((item) => transientToolSignature(item) === toolSignature);
      }
      return items.some((item) => item.kind === pending.kind && item.id === pending.id);
    }
    return items.some(
      (item) =>
        item.kind === pending.kind &&
        ((item.id && item.id === pending.id) ||
          (timelineItemBody(item).trim() === body && transientTimestampsMatch(item, pending)))
    );
  }

  function stillMissingFromPersisted(items: TimelineItem[], pending: TimelineItem[]): TimelineItem[] {
    return pending.filter((item) => !timelineHasTransientMatch(items, item));
  }

  function withCompletionAssistantFallback(items: TimelineItem[], text: string): TimelineItem[] {
    const trimmed = text.trim();
    if (!trimmed || timelineHasBody(items, "assistant", trimmed)) return items;
    return [
      ...items,
      {
        id: `live-complete-assistant-${Date.now()}`,
        kind: "assistant",
        title: "Assistant",
        summary: trimmed,
        body: trimmed,
        meta: []
      }
    ];
  }

  async function refreshSessionAfterTurn(
    completedTurnId: string,
    sessionToRefresh: SessionListItem,
    liveItemsAtCompletion: TimelineItem[],
    submittedAtCompletion: TimelineItem[],
    preservedErrorItems: TimelineItem[],
    turnEndedWithError: boolean
  ) {
    const loadGeneration = ++sessionLoadGeneration;
    try {
      const detail = await loadSessionDetailFromDaemon(sessionToRefresh.id);
      if (loadGeneration !== sessionLoadGeneration || selectedSession?.id !== sessionToRefresh.id) {
        return;
      }
      if (currentTurnId !== null && currentTurnId !== completedTurnId) {
        return;
      }
      const persistedTimeline = reuseTransientMessageIds(detail.timeline, [
        ...submittedAtCompletion,
        ...liveItemsAtCompletion
      ]);
      selectedSession = detail.session;
      sessionDetail = { ...detail, timeline: persistedTimeline };
      statusMessage = `Loaded ${detail.timeline.length} conversation items.`;
      if (turnEndedWithError) {
        liveStreamItems = stillMissingFromPersisted(persistedTimeline, preservedErrorItems);
        submittedMessages = stillMissingFromPersisted(persistedTimeline, submittedAtCompletion);
        return;
      }
      liveStreamItems = stillMissingFromPersisted(persistedTimeline, liveItemsAtCompletion);
      submittedMessages = stillMissingFromPersisted(persistedTimeline, submittedAtCompletion);
    } catch (error) {
      if (loadGeneration !== sessionLoadGeneration || selectedSession?.id !== sessionToRefresh.id) {
        return;
      }
      const detail = errorText(error);
      statusMessage = detail;
      appendAgentError("Conversation load failed", detail, "load-session");
    }
  }

  function streamingAssistantId(turnId: string): string {
    return `live-stream-assistant-${turnId}`;
  }

  function livePermissionId(turnId: string, requestId: string): string {
    return `live-perm-${turnId}-${requestId}`;
  }

  function liveQuestionId(turnId: string, requestId: string): string {
    return `live-question-${turnId}-${requestId}`;
  }

  function liveToolId(turnId: string, callId: string): string {
    return `live-tool-${turnId}-${callId}`;
  }

  function upsertStreamingAssistant(turnId: string, delta: string) {
    const id = streamingAssistantId(turnId);
    const existingIdx = liveStreamItems.findIndex((item) => item.id === id && item.kind === "assistant");
    if (existingIdx >= 0) {
      const existing = liveStreamItems[existingIdx];
      const updated = { ...existing, body: existing.body + delta, summary: existing.body + delta };
      liveStreamItems = [
        ...liveStreamItems.slice(0, existingIdx),
        updated,
        ...liveStreamItems.slice(existingIdx + 1)
      ];
    } else {
      appendLive({
        id,
        kind: "assistant",
        title: "Assistant",
        summary: delta,
        body: delta,
        meta: []
      });
    }
  }

  function shouldIgnoreTurnEvent(turnId: string): boolean {
    if (settledTurnIds.has(turnId)) return true;
    return currentTurnId !== null && currentTurnId !== turnId;
  }

  function markTurnActive(turnId: string) {
    if (currentTurnId !== null && currentTurnId !== turnId) {
      cancelingTurnId = null;
    }
    currentTurnId = turnId;
    settledTurnIds.delete(turnId);
  }

  function markTurnSettled(turnId: string) {
    settledTurnIds.add(turnId);
    const { [turnId]: _drop, ...rest } = replayTextByTurn;
    replayTextByTurn = rest;
    if (cancelingTurnId === turnId) {
      cancelingTurnId = null;
    }
    if (currentTurnId === turnId) {
      currentTurnId = null;
    }
  }

  function replaySafeDelta(turnId: string, delta: string): string {
    const replayText = `${replayTextByTurn[turnId] ?? ""}${delta}`;
    replayTextByTurn = { ...replayTextByTurn, [turnId]: replayText };
    const currentItem = liveStreamItems.find((item) => item.id === streamingAssistantId(turnId));
    if (!currentItem || currentItem.kind !== "assistant") {
      return delta;
    }
    const current = currentItem.body;
    if (current.startsWith(replayText)) return "";
    if (replayText.startsWith(current)) return replayText.slice(current.length);
    return delta;
  }

  function handleSessionEvent(sid: string, ev: SessionStreamEvent) {
    if (!selectedSession || selectedSession.id !== sid) return;
    if (shouldIgnoreTurnEvent(ev.turnId)) return;
    switch (ev.type) {
      case "turn-start":
        markTurnActive(ev.turnId);
        turnStartedAtMs = Date.now();
        turnThinking = true;
        turnStatusHint = "Thinking";
        if (!ev.replay) {
          const { [ev.turnId]: _drop, ...rest } = replayTextByTurn;
          replayTextByTurn = rest;
        }
        break;
      case "thinking-delta":
        markTurnActive(ev.turnId);
        turnThinking = true;
        turnStatusHint = "Thinking";
        break;
      case "text-delta":
        markTurnActive(ev.turnId);
        turnThinking = false;
        turnStatusHint = null;
        {
          const delta = ev.replay ? replaySafeDelta(ev.turnId, ev.delta) : ev.delta;
          if (delta) upsertStreamingAssistant(ev.turnId, delta);
        }
        break;
      case "tool-calls-requested":
        markTurnActive(ev.turnId);
        turnThinking = false;
        turnStatusHint = "Running tools";
        // Render an immediate pending card per requested call so the user
        // sees *what* the agent is doing before it finishes. The id is
        // scoped to the turn and call id, so backend call id reuse in a later
        // turn does not replace a previous live card while transcript reloads.
        for (const req of ev.requests) {
          const id = liveToolId(ev.turnId, req.callId);
          if (liveStreamItems.some((x) => x.id === id)) continue;
          appendLive({
            id,
            kind: "tool",
            title: req.toolId,
            summary: `${req.toolId} · running`,
            body: "",
            meta: [],
            toolName: req.toolId,
            status: "running",
            input: req.input,
            output: "",
            inputJson: safeParseJson(req.input)
          });
        }
        break;
      case "tool-invocations":
        markTurnActive(ev.turnId);
        turnThinking = false;
        turnStatusHint = null;
        for (const inv of ev.invocations) {
          const id = liveToolId(ev.turnId, inv.callId);
          const existingIdx = liveStreamItems.findIndex((x) => x.id === id);
          const payload: TimelineItem = {
            id,
            kind: "tool",
            title: inv.toolId,
            summary: `${inv.toolId} · ${inv.success ? "success" : "error"}`,
            body: inv.output,
            meta: [],
            toolName: inv.toolId,
            status: inv.success ? "success" : "error",
            input: inv.input,
            output: inv.output,
            inputJson: safeParseJson(inv.input)
          };
          if (existingIdx >= 0) {
            // Upgrade the pending card in place. Svelte needs a new array
            // reference to observe the change.
            liveStreamItems = [
              ...liveStreamItems.slice(0, existingIdx),
              payload,
              ...liveStreamItems.slice(existingIdx + 1)
            ];
          } else {
            appendLive(payload);
          }
        }
        break;
      case "reflection-checkpoint":
        markTurnActive(ev.turnId);
        turnThinking = true;
        turnStatusHint = "Thinking";
        break;
      case "retry-attempt":
        markTurnActive(ev.turnId);
        turnThinking = true;
        turnStatusHint = `Retrying ${ev.attempt}/${ev.maxAttempts}`;
        break;
      case "usage":
        markTurnActive(ev.turnId);
        break;
      case "permission-request": {
        markTurnActive(ev.turnId);
        turnThinking = false;
        turnStatusHint = "Awaiting approval";
        const id = livePermissionId(ev.turnId, ev.requestId);
        appendLive({
          id,
          kind: "permission",
          title: `Permission · ${ev.toolId}`,
          summary: ev.summary,
          body: ev.reason ?? ev.summary,
          meta: [],
          toolName: ev.toolId,
          status: "pending",
          permissionDialog: {
            state: "pending",
            reason: ev.reason ?? ev.summary,
            summary: ev.summary,
            inputText: null,
            toolName: ev.toolId,
            choices: ["Allow once", "Always allow", "Deny"]
          },
          scopeLabel: null,
          choices: ["Allow once", "Always allow", "Deny"]
        });
        turnPermissionLookup = {
          ...turnPermissionLookup,
          [id]: { turnId: ev.turnId, requestId: ev.requestId }
        };
        break;
      }
      case "user-question-request": {
        markTurnActive(ev.turnId);
        turnThinking = false;
        turnStatusHint = "Waiting for answer";
        const id = liveQuestionId(ev.turnId, ev.requestId);
        const questions = normalizeUserQuestions(ev.questions);
        appendLive({
          id,
          kind: "question",
          title: "Question",
          summary: questions.map((q) => q.question).join("\n"),
          body: "",
          meta: [],
          status: "pending",
          questions
        });
        turnQuestionLookup = {
          ...turnQuestionLookup,
          [id]: { turnId: ev.turnId, requestId: ev.requestId }
        };
        break;
      }
      case "turn-complete":
      case "turn-error":
        markTurnSettled(ev.turnId);
        turnStartedAtMs = null;
        turnThinking = false;
        turnStatusHint = null;
        if (ev.type === "turn-error") {
          // Surface the daemon's error so the user sees *why* the agent
          // didn't reply — otherwise we'd silently reload an empty
          // transcript. Renders inline as a system-style timeline item
          // and a status-strip toast.
          const detail = ev.error?.trim() || "Unknown agent error.";
          statusMessage = `Agent error: ${detail}`;
          appendAgentError("Agent error", detail, "turn-error");
        }
        // Reload the persisted transcript; then drop live items.
        if (selectedSession) {
          const sessionToRefresh = selectedSession;
          const completionText = ev.type === "turn-complete" ? ev.assistantText : "";
          const liveItemsAtCompletion = withCompletionAssistantFallback(liveStreamItems, completionText);
          const submittedAtCompletion = submittedMessages;
          const turnEndedWithError = ev.type === "turn-error";
          const preservedErrorItems = liveItemsAtCompletion.filter(
            (item) => item.kind === "system" && item.meta.includes("error")
          );
          void refreshSessionAfterTurn(
            ev.turnId,
            sessionToRefresh,
            liveItemsAtCompletion,
            submittedAtCompletion,
            preservedErrorItems,
            turnEndedWithError
          );
        }
        break;
    }
  }

  function safeParseJson(text: string): Record<string, unknown> | null {
    try {
      const v = JSON.parse(text);
      return typeof v === "object" && v !== null ? (v as Record<string, unknown>) : null;
    } catch {
      return null;
    }
  }

  function normalizeUserQuestions(raw: unknown[]): AskUserQuestionItem[] {
    return raw
      .map((item) => (typeof item === "object" && item !== null ? item as Record<string, unknown> : null))
      .filter((item): item is Record<string, unknown> => item !== null)
      .map((item) => ({
        question: typeof item.question === "string" ? item.question : "Question",
        header: typeof item.header === "string" ? item.header : "Question",
        multiSelect: item.multiSelect === true,
        options: Array.isArray(item.options)
          ? item.options
              .map((option) =>
                typeof option === "object" && option !== null
                  ? option as Record<string, unknown>
                  : null
              )
              .filter((option): option is Record<string, unknown> => option !== null)
              .map((option) => ({
                label: typeof option.label === "string" ? option.label : "Option",
                description: typeof option.description === "string" ? option.description : "",
                preview: typeof option.preview === "string" ? option.preview : null
              }))
          : []
      }))
      .filter((item) => item.options.length > 0);
  }

  async function ensureSessionSubscription() {
    if (!selectedSession) {
      if (sessionEventUnlisten) {
        sessionEventUnlisten();
        sessionEventUnlisten = null;
      }
      subscribedSessionId = null;
      return;
    }
    if (subscribedSessionId === selectedSession.id && sessionEventUnlisten) return;
    if (sessionEventUnlisten) sessionEventUnlisten();
    const sid = selectedSession.id;
    subscribedSessionId = sid;
    sessionEventUnlisten = await subscribeSessionEvents(sid, (ev) => handleSessionEvent(sid, ev));
  }

  $effect(() => {
    void ensureSessionSubscription();
  });
</script>

<div class="pf-mac">
  <TitleBar />
  {#if onboarding}
    <div class="pf-app-body">
      <div class="pf-main">
        <div class="pf-stage">
          <Onboarding
            snapshot={settingsSnapshot}
            loading={settingsLoading}
            remoteEnabled={remoteConnection.enabled}
            busyProviderId={authBusyProviderId}
            errorMessage={authError}
            externals={externalCredentials}
            busyImportKey={importBusyKey}
            onLoginOauth={(providerId) => void handleOauthLogin(providerId)}
            onLoginApiKey={(providerId, apiKey) => void handleApiKeyLogin(providerId, apiKey)}
            onImportExternal={(providerId, source) =>
              void handleImportExternal(providerId, source)}
            onRefresh={() => void refreshSettings()}
            onFinish={() => void finishOnboarding()}
            forceRepoStep={forceOnboarding}
          />
        </div>
      </div>
    </div>
  {:else}
    <div class="pf-app-body">
      {#if tweaks.showSidebar}
        <Sidebar
          screen={tweaks.screen}
          collapsed={tweaks.collapsedSidebar}
          onSelectScreen={onSelectScreen}
          agents={activeAgents}
          activeAgentId={openAgentSessionId}
          onOpenAgent={onOpenAgent}
          onToggleAgentPin={(id, pinned) => void toggleDesktopPin("agent", id, pinned)}
          onToggleCollapse={() => updateTweak("collapsedSidebar", !tweaks.collapsedSidebar)}
          user={userChip}
        />
      {/if}
      <div class="pf-main">
        <div class="pf-stage">
          {#if tweaks.screen === "workspace"}
            {#if openAgentSessionId}
              <AgentDetail
                session={selectedSession}
                sessionDetail={sessionDetail}
                timeline={combinedTimeline}
                pendingPermissions={pendingPermissions}
                pendingQuestions={pendingQuestions}
                loading={sessionLoading}
                turnRunning={turnRunning}
                turnCancelable={currentTurnId !== null && cancelingTurnId !== currentTurnId}
                turnStartedAtMs={turnStartedAtMs}
                turnThinking={turnThinking}
                turnStatusHint={turnStatusHint}
                settingsSnapshot={settingsSnapshot}
                userDisplayName={tweaks.userName}
                onBack={onCloseAgent}
                onSubmitMessage={submitMessage}
                onResolvePermission={resolvePermission}
                onResolveUserQuestion={resolveUserQuestion}
                onCancelTurn={() => void cancelCurrentTurn()}
                onDraftChange={(hasDraft) => (composerHasDraft = hasDraft)}
                onRenameTitle={renameSelectedSession}
              />
            {:else if openProjectId && workspaceGroups.find((g) => g.id === openProjectId)}
              <ProjectDetail
                group={workspaceGroups.find((g) => g.id === openProjectId)!}
                pinnedAgentIds={desktopPins.pinnedAgentIds}
                onBack={() => (openProjectId = null)}
                onOpenAgent={(id) => onOpenAgent(id)}
                onNewAgent={(cwd) => requestNewAgent(cwd)}
              />
            {:else}
              <Workspace
                groups={workspaceGroups}
                settingsSnapshot={settingsSnapshot}
                defaultWorkspaceCwd={defaultWorkspaceCwd}
                loading={groupsLoading}
                onOpenAgent={(id) => onOpenAgent(id)}
                onOpenBoard={onOpenProject}
                onNewAgent={(cwd) => requestNewAgent(cwd)}
                onSessionReady={(sessionId) => handleSessionReady(sessionId)}
                onOpenWorkspacePicker={() => (showWorkspacePicker = true)}
                pinnedWorkspacePaths={desktopPins.pinnedWorkspacePaths}
                onToggleWorkspacePin={(path, pinned) => void toggleDesktopPin("workspace", path, pinned)}
              />
            {/if}
          {:else if tweaks.screen === "pipelines"}
            <Pipelines />
          {:else if tweaks.screen === "deployments"}
            <Deployments />
          {:else if tweaks.screen === "settings"}
            <Settings
              snapshot={settingsSnapshot}
              loading={settingsLoading}
              tweaks={tweaks}
              preferences={desktopPreferences}
              daemonUrl={daemonUrl}
              daemonWorkspaceRoot={daemonWorkspaceRoot}
              remoteEnabled={remoteConnection.enabled}
              remotePassword={remotePassword}
              remoteBusy={remoteBusy}
              remoteResult={remoteOperation}
              onPreferenceChange={updateDesktopPreference}
              onRemotePasswordChange={(value) => (remotePassword = value)}
              onResetPreferences={resetDesktopPreferences}
              onTweakChange={updateTweak}
              onResetAppearance={resetAppearanceTweaks}
              onRefresh={() => void refreshSettings()}
              onLogout={(providerId) => void handleLogout(providerId)}
              onLoginOauth={(providerId) => void handleOauthLogin(providerId)}
              onApiKeyLogin={(providerId, apiKey) => void handleApiKeyLogin(providerId, apiKey)}
              onImportExternal={(providerId, source) =>
                void handleImportExternal(providerId, source)}
              busyProviderId={authBusyProviderId}
              authError={authError}
              externals={externalCredentials}
              busyImportKey={importBusyKey}
              onRunRemoteBash={(command) => void handleRemoteBash(command)}
              onReadRemoteFile={(path) => void handleRemoteRead(path)}
              onWriteRemoteFile={(path, contents) => void handleRemoteWrite(path, contents)}
            />
          {/if}
        </div>
      </div>
    </div>
  {/if}

</div>

{#if showWorkspacePicker}
  <WorkspacePicker
    onClose={() => (showWorkspacePicker = false)}
    onSwitched={handleWorkspaceSwitched}
  />
{/if}

{#if newSessionCwd}
  <NewSessionModal
    cwd={newSessionCwd}
    snapshot={settingsSnapshot}
    busy={newSessionBusy}
    onClose={() => {
      if (!newSessionBusy) newSessionCwd = null;
    }}
    onCreate={async (providerId) => {
      if (!newSessionCwd || newSessionBusy) return;
      newSessionBusy = true;
      try {
        const ok = await handleNewAgent(newSessionCwd, providerId);
        if (ok) newSessionCwd = null;
      } finally {
        newSessionBusy = false;
      }
    }}
  />
{/if}

{#if !skipOnboarding && !forceOnboarding && !onboarding && statusMessage && statusMessage !== "Desktop workspace ready." && statusMessage !== "Settings snapshot refreshed."}
  <div class="status-strip" aria-live="polite">{statusMessage}</div>
{/if}

{#if connectionState === "reconnecting" || connectionState === "closed"}
  <div class="connection-banner" role="status" aria-live="polite">
    {#if connectionState === "reconnecting"}
      <span class="dot"></span>
      Lost connection to Corbina backend. Reconnecting…
    {:else}
      <span class="dot err"></span>
      Corbina backend disconnected.
      <button
        type="button"
        class="sc-btn"
        data-variant="outline"
        data-size="sm"
        onclick={() => void ensureLocalDaemonClient().then((c) => c.connect()).catch(() => {})}
      >Reconnect</button>
    {/if}
  </div>
{/if}

<style>
  .status-strip {
    position: fixed;
    bottom: 8px;
    left: 12px;
    font-size: 11px;
    color: var(--muted-foreground);
    font-family: var(--font-sans);
    max-width: 60vw;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    pointer-events: none;
    z-index: 5;
  }
  .connection-banner {
    position: fixed;
    top: 8px;
    left: 50%;
    transform: translateX(-50%);
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 14px;
    font-size: 12px;
    color: var(--foreground);
    background: color-mix(in oklab, oklch(0.72 0.18 70) 18%, var(--background));
    border: 1px solid color-mix(in oklab, oklch(0.72 0.18 70) 40%, var(--border));
    border-radius: 999px;
    box-shadow: var(--shadow-md);
    z-index: 80;
    font-family: var(--font-sans);
  }
  .connection-banner .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: oklch(0.72 0.18 70);
    animation: pf-breathe 1.6s ease-in-out infinite;
  }
  .connection-banner .dot.err {
    background: oklch(0.62 0.22 25);
    animation: none;
  }
</style>

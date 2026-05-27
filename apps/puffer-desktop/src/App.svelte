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
  import Workflows from "./lib/screens/Workflows.svelte";
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
  import { providerIdCanRunAgent, providerIdInSet, providerIsAvailableForAgent } from "./lib/providerIds";
  import { providerCatalogForSetup } from "./lib/providerFallbacks";
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

  type LiveSidebarAgentOverlay = {
    session: SessionListItem;
    state: AgentState;
    turnId: string | null;
  };

  type CreatedSessionResult = Awaited<ReturnType<typeof createSession>>;

  type TransientConversationState = {
    submittedMessages: TimelineItem[];
    submittedMessageBaselineIds: Record<string, string[]>;
    liveStreamItems: TimelineItem[];
    replayTextByTurn: Record<string, string>;
    turnPermissionLookup: Record<string, { turnId: string; requestId: string }>;
    turnQuestionLookup: Record<string, { turnId: string; requestId: string }>;
    resolvingPermissionIds: string[];
    resolvingQuestionIds: string[];
    currentTurnId: string | null;
    cancelingTurnId: string | null;
    turnStartedAtMs: number | null;
    turnThinking: boolean;
    turnStatusHint: string | null;
  };

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
  const allowUnauthenticatedWorkspaceHarness =
    typeof window !== "undefined" &&
    Boolean(
      (window as unknown as { __PUFFER_DESKTOP_ALLOW_UNAUTHENTICATED_WORKSPACE?: boolean })
        .__PUFFER_DESKTOP_ALLOW_UNAUTHENTICATED_WORKSPACE
    );
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
    return () => {
      if (statusDismissTimer) {
        clearTimeout(statusDismissTimer);
        statusDismissTimer = null;
      }
    };
  });
  let showWorkspacePicker = $state(false);
  let newSessionCwd = $state<string | null>(null);
  let newSessionBusy = $state(false);
  let newSessionError = $state<string | null>(null);

  // Backend-backed state
  let groups = $state<FolderGroup[]>([]);
  let groupsLoading = $state(false);
  let selectedSession = $state<SessionListItem | null>(null);
  let fallbackSessionsById = $state<Record<string, SessionListItem>>({});
  let sessionDetail = $state<SessionDetail | null>(null);
  let sessionLoading = $state(false);

  // Drill-in marker: which session id is currently expanded in AgentDetail.
  // Cleared when the user backs out to the workspace board.
  let openAgentSessionId = $state<string | null>(null);
  let openProjectId = $state<string | null>(null);
  let submittedMessages = $state<TimelineItem[]>([]);
  let submittedMessageBaselineIds: Record<string, string[]> = {};
  let transientConversationStates = $state<Record<string, TransientConversationState>>({});
  let submitMessageInFlightSessionIds = $state<string[]>([]);
  let dismissedPermissionIds = $state<string[]>([]);
  let dismissedQuestionIds = $state<string[]>([]);
  const DISMISSED_IDS_CAP = 200;
  const SETTLED_TURN_KEYS_CAP = 500;
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
  let liveSidebarAgentsById = $state<Record<string, LiveSidebarAgentOverlay>>({});
  let liveStreamItems = $state<TimelineItem[]>([]);
  let settledTurnKeys = new Set<string>();
  let turnPermissionLookup = $state<Record<string, { turnId: string; requestId: string }>>({});
  let turnQuestionLookup = $state<Record<string, { turnId: string; requestId: string }>>({});
  let replayTextByTurn: Record<string, string> = {};
  let sessionEventUnlisten: UnlistenFn | null = null;
  let subscribedSessionId: string | null = null;
  let sessionSubscriptionGeneration = 0;
  let liveSidebarSessionEventUnlisteners: Record<string, UnlistenFn> = {};
  let liveSidebarSessionSubscriptionGeneration = 0;
  let connectionState = $state<ConnectionState>("idle");
  let reconnectBusy = $state(false);
  let reconnectError = $state<string | null>(null);
  let daemonUrl = $state<string | null>(null);
  let daemonWorkspaceRoot = $state<string | null>(null);
  let daemonClientFingerprint = $state<string | null>(null);
  let daemonClientUnlisteners: Array<() => void> = [];
  let sessionLoadGeneration = 0;
  let liveErrorSeq = 0;
  let desktopPins = $state<DesktopPinState>({ pinnedAgentIds: [], pinnedWorkspacePaths: [] });
  let desktopPinInFlightKeys = $state<string[]>([]);
  let desktopPinInFlightStates = $state<Record<string, boolean>>({});
  let desktopPinQueuedStates = $state<Record<string, boolean>>({});
  const submitMessageInFlightGuards = new Set<string>();
  const PENDING_SUBMITTED_MESSAGE_PREFIX = "puffer-desktop:pending-submitted:";
  const PENDING_SUBMITTED_MESSAGE_TTL_MS = 10 * 60_000;

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

  function scopedSessionItemId(sessionId: string | null | undefined, itemId: string): string {
    return sessionId ? `${sessionId}::${itemId}` : itemId;
  }

  function activeSessionItemId(itemId: string): string {
    return scopedSessionItemId(selectedSession?.id, itemId);
  }

  let pendingPermissions = $derived<PermissionTimelineItem[]>(
    combinedTimeline.filter(
      (t): t is PermissionTimelineItem =>
        t.kind === "permission" &&
        isPendingPermission(t) &&
        !dismissedPermissionIds.includes(scopedSessionItemId(selectedSession?.id, t.id))
    )
  );
  let pendingQuestions = $derived<UserQuestionTimelineItem[]>(
    combinedTimeline.filter(
      (t): t is UserQuestionTimelineItem =>
        t.kind === "question" &&
        t.status === "pending" &&
        !dismissedQuestionIds.includes(scopedSessionItemId(selectedSession?.id, t.id))
    )
  );
  let turnRunning = $derived(currentTurnId !== null || turnStartedAtMs !== null);

  function sidebarAgentState(status: AgentActivityStatus): AgentState {
    if (status === "awaiting") return "awaiting";
    if (status === "running") return "running";
    if (status === "review") return "review";
    return "idle";
  }

  function liveSidebarAgentState(session: SessionListItem): AgentState {
    const live = liveSidebarAgentsById[session.id];
    if (live) return live.state;
    if (selectedSession?.id !== session.id) return sidebarAgentState(session.activityStatus);
    if (pendingPermissions.length > 0 || pendingQuestions.length > 0) return "awaiting";
    if (turnRunning) return turnThinking ? "thinking" : "running";
    return sidebarAgentState(session.activityStatus);
  }

  function activityStatusIsActive(status: AgentActivityStatus): boolean {
    return status === "running" || status === "awaiting";
  }

  function isTopLevelSession(session: SessionListItem): boolean {
    return !session.parentSessionId;
  }

  function topLevelFolderGroup(group: FolderGroup): FolderGroup | null {
    const sessions = group.sessions.filter(isTopLevelSession);
    if (sessions.length === 0) return null;
    return {
      ...group,
      sessionCount: sessions.length,
      sessions
    };
  }

  function findSidebarSession(sessionId: string, fallback?: SessionListItem | null): SessionListItem | null {
    if (fallback?.id === sessionId) return fallback;
    if (selectedSession?.id === sessionId) return selectedSession;
    for (const group of groups) {
      const session = group.sessions.find((item) => item.id === sessionId);
      if (session) return session;
    }
    return liveSidebarAgentsById[sessionId]?.session ?? null;
  }

  function setLiveSidebarAgentState(
    sessionId: string,
    state: AgentState,
    turnId: string | null,
    fallback?: SessionListItem | null
  ) {
    const session = findSidebarSession(sessionId, fallback);
    if (!session) return;
    liveSidebarAgentsById = {
      ...liveSidebarAgentsById,
      [sessionId]: {
        session,
        state,
        turnId
      }
    };
  }

  function clearLiveSidebarAgentState(sessionId: string, turnId: string | null) {
    const live = liveSidebarAgentsById[sessionId];
    if (!live) return;
    if (turnId && live.turnId && live.turnId !== turnId) return;
    const { [sessionId]: _drop, ...rest } = liveSidebarAgentsById;
    liveSidebarAgentsById = rest;
  }

  function clearLiveSidebarSessionSubscriptions() {
    liveSidebarSessionSubscriptionGeneration += 1;
    for (const unlisten of Object.values(liveSidebarSessionEventUnlisteners)) {
      unlisten();
    }
    liveSidebarSessionEventUnlisteners = {};
  }

  function liveSidebarSessionSubscriptionTargets(): string[] {
    const targets = new Set(Object.keys(liveSidebarAgentsById));
    for (const group of groups) {
      for (const session of group.sessions) {
        if (isTopLevelSession(session) && activityStatusIsActive(session.activityStatus)) {
          targets.add(session.id);
        }
      }
    }
    return Array.from(targets).sort();
  }

  async function ensureLiveSidebarSessionSubscriptions(targetIds: string[]) {
    const generation = ++liveSidebarSessionSubscriptionGeneration;
    const targets = new Set(targetIds);
    const retained: Record<string, UnlistenFn> = {};
    for (const [sessionId, unlisten] of Object.entries(liveSidebarSessionEventUnlisteners)) {
      if (targets.has(sessionId)) {
        retained[sessionId] = unlisten;
      } else {
        unlisten();
      }
    }
    liveSidebarSessionEventUnlisteners = retained;

    for (const sessionId of targetIds) {
      if (liveSidebarSessionEventUnlisteners[sessionId]) continue;
      const unlisten = await subscribeSessionEvents(sessionId, (ev) => {
        if (selectedSession?.id === sessionId) return;
        handleSessionEvent(sessionId, ev);
      });
      if (
        generation !== liveSidebarSessionSubscriptionGeneration ||
        !liveSidebarSessionSubscriptionTargets().includes(sessionId)
      ) {
        unlisten();
        continue;
      }
      liveSidebarSessionEventUnlisteners = {
        ...liveSidebarSessionEventUnlisteners,
        [sessionId]: unlisten
      };
    }
  }

  function applySidebarSessionEvent(sid: string, ev: SessionStreamEvent) {
    switch (ev.type) {
      case "turn-start":
      case "thinking-delta":
      case "reflection-checkpoint":
      case "retry-attempt":
        setLiveSidebarAgentState(sid, "thinking", ev.turnId);
        break;
      case "text-delta":
      case "tool-calls-requested":
      case "tool-invocations":
      case "lambda-gate":
        setLiveSidebarAgentState(sid, "running", ev.turnId);
        break;
      case "permission-request":
      case "user-question-request":
        setLiveSidebarAgentState(sid, "awaiting", ev.turnId);
        break;
      case "turn-complete":
      case "turn-error":
        clearLiveSidebarAgentState(sid, ev.turnId);
        break;
      case "usage":
        break;
    }
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

  function groupsContainSession(sourceGroups: FolderGroup[], sessionId: string): boolean {
    return sourceGroups.some((group) => group.sessions.some((item) => item.id === sessionId));
  }

  function insertSessionFallback(
    sourceGroups: FolderGroup[],
    session: SessionListItem
  ): FolderGroup[] {
    if (!isTopLevelSession(session)) return sourceGroups;
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

  function withSelectedSessionFallback(sourceGroups: FolderGroup[]): FolderGroup[] {
    const byId = new Map<string, SessionListItem>();
    for (const session of Object.values(fallbackSessionsById)) {
      byId.set(session.id, session);
    }
    if (selectedSession) {
      byId.set(selectedSession.id, selectedSession);
    }
    let nextGroups = sourceGroups;
    for (const session of byId.values()) {
      nextGroups = insertSessionFallback(nextGroups, session);
    }
    return nextGroups;
  }

  function rememberFallbackSession(session: SessionListItem) {
    if (groupsContainSession(groups, session.id)) {
      if (fallbackSessionsById[session.id]) {
        const { [session.id]: _drop, ...rest } = fallbackSessionsById;
        fallbackSessionsById = rest;
      }
      return;
    }
    fallbackSessionsById = { ...fallbackSessionsById, [session.id]: session };
  }

  function pruneFallbackSessions(sourceGroups: FolderGroup[]) {
    const next = Object.fromEntries(
      Object.entries(fallbackSessionsById).filter(
        ([sessionId]) => !groupsContainSession(sourceGroups, sessionId)
      )
    );
    if (Object.keys(next).length !== Object.keys(fallbackSessionsById).length) {
      fallbackSessionsById = next;
    }
  }

  function activeAgentFromSession(
    session: SessionListItem,
    project: string,
    projectKey: string
  ): ActiveAgent {
    return {
      id: session.id,
      name: sessionDisplayName(session),
      title: sessionDisplayTitle(session),
      project,
      projectKey,
      branch: "",
      state: liveSidebarAgentState(session),
      updatedAtMs: session.updatedAtMs,
      pinned: desktopPins.pinnedAgentIds.includes(session.id),
      pinBusy: desktopPinInFlightKeys.includes(desktopPinKey("agent", session.id))
    };
  }

  function activeAgentProjectLabel(group: FolderGroup, sourceGroups: FolderGroup[]): string {
    const duplicateLabel = sourceGroups.some(
      (candidate) => candidate !== group && candidate.label === group.label
    );
    return duplicateLabel ? group.path || group.id || group.label : group.label;
  }

  function activeAgentProjectKey(group: FolderGroup): string {
    return group.path || group.id || group.label;
  }

  function projectLabelForSession(session: SessionListItem): string {
    const group = workspaceGroups.find((item) =>
      item.sessions.some((candidate) => candidate.id === session.id)
    );
    return group ? activeAgentProjectLabel(group, workspaceGroups) : fallbackProjectLabel(session);
  }

  function projectKeyForSession(session: SessionListItem): string {
    const group = workspaceGroups.find((item) =>
      item.sessions.some((candidate) => candidate.id === session.id)
    );
    return group ? activeAgentProjectKey(group) : groupPathForSession(session);
  }

  let sortedGroups = $derived<FolderGroup[]>(
    groups
      .map(topLevelFolderGroup)
      .filter((group): group is FolderGroup => group !== null)
      .sort(compareFolderGroups)
  );
  let workspaceGroups = $derived<FolderGroup[]>(withSelectedSessionFallback(sortedGroups));

  let realAgents = $derived<ActiveAgent[]>(
    workspaceGroups
      .flatMap((g) =>
        g.sessions
          .filter(isTopLevelSession)
          .map((s) =>
            activeAgentFromSession(
              s,
              activeAgentProjectLabel(g, workspaceGroups),
              activeAgentProjectKey(g)
            ))
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
    selectedSession &&
    isTopLevelSession(selectedSession) &&
    !realAgents.some((agent) => agent.id === selectedSession?.id)
      ? activeAgentFromSession(
          selectedSession,
          selectedSessionGroup
            ? activeAgentProjectLabel(selectedSessionGroup, workspaceGroups)
            : fallbackProjectLabel(selectedSession),
          selectedSessionGroup
            ? activeAgentProjectKey(selectedSessionGroup)
            : groupPathForSession(selectedSession)
        )
      : null
  );
  let liveSidebarFallbackAgents = $derived<ActiveAgent[]>(
    Object.values(liveSidebarAgentsById)
      .filter(
        (live) =>
          isTopLevelSession(live.session) &&
          !realAgents.some((agent) => agent.id === live.session.id) &&
          selectedSessionFallbackAgent?.id !== live.session.id
      )
      .map((live) =>
        activeAgentFromSession(
          live.session,
          projectLabelForSession(live.session),
          projectKeyForSession(live.session)
        ))
  );
  let activeAgents = $derived<ActiveAgent[]>(
    selectedSessionFallbackAgent
      ? [selectedSessionFallbackAgent, ...liveSidebarFallbackAgents, ...realAgents]
      : [...liveSidebarFallbackAgents, ...realAgents]
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
  let recapBlurSessionId: string | null = null;
  let composerHasDraft = $state(false);

  function recapIdleMs(): number {
    if (typeof window === "undefined") return RECAP_IDLE_MS;
    const override = (window as unknown as { __RECAP_IDLE_MS_OVERRIDE?: unknown })
      .__RECAP_IDLE_MS_OVERRIDE;
    return typeof override === "number" && override > 0 ? override : RECAP_IDLE_MS;
  }

  function selectedSessionHasConversation(): boolean {
    if (!selectedSession) return false;
    if ((sessionDetail?.timeline.length ?? 0) > 0) return true;
    return selectedSession.eventCount > 0;
  }

  function armRecapBlurTimer() {
    if (turnRunning || composerHasDraft) return;
    if (recapBlurTimer != null) return;
    const sessionIdAtBlur = selectedSession?.id ?? null;
    if (!selectedSessionHasConversation()) return;
    if (!sessionIdAtBlur || openAgentSessionId !== sessionIdAtBlur) return;
    recapBlurSessionId = sessionIdAtBlur;
    recapBlurTimer = setTimeout(() => {
      recapBlurTimer = null;
      recapBlurSessionId = null;
      if (!selectedSession || turnRunning || composerHasDraft) return;
      if (selectedSession.id !== sessionIdAtBlur) return;
      if (!selectedSessionHasConversation()) return;
      if (openAgentSessionId !== sessionIdAtBlur) return;
      void submitMessage("/recap", {});
    }, recapIdleMs());
  }

  function cancelRecapBlurTimer() {
    if (recapBlurTimer != null) {
      clearTimeout(recapBlurTimer);
      recapBlurTimer = null;
      recapBlurSessionId = null;
    }
  }

  function modalDialogOpen(): boolean {
    if (showWorkspacePicker || newSessionCwd !== null) return true;
    if (typeof document === "undefined") return false;
    return document.querySelector("[role='dialog'][aria-modal='true']") !== null;
  }

  function handleShellKeydown(event: KeyboardEvent) {
    if (event.defaultPrevented || onboarding) return;
    if ((event.metaKey || event.ctrlKey) && event.key === ",") {
      if (modalDialogOpen()) return;
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
      if (typeof parsed.workspaceRoot !== "string" || !parsed.workspaceRoot) {
        clearRememberedSession();
        return null;
      }
      return {
        sessionId: parsed.sessionId,
        workspaceRoot: parsed.workspaceRoot
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
    if (remembered.workspaceRoot && remembered.workspaceRoot !== workspaceRoot) {
      return false;
    }
    if (!workspaceRoot && remembered.workspaceRoot) {
      return false;
    }
    const session = findSessionById(remembered.sessionId);
    if (session) {
      await openSession(session);
      openAgentSessionId = session.id;
      tweaks = { ...tweaks, screen: "workspace" };
      return true;
    }

    const loadGeneration = ++sessionLoadGeneration;
    sessionLoading = true;
    try {
      const detail = await loadSessionDetailFromDaemon(remembered.sessionId);
      if (loadGeneration !== sessionLoadGeneration) return false;
      saveCurrentTransientConversationState(selectedSession?.id);
      selectedSession = detail.session;
      rememberFallbackSession(detail.session);
      sessionDetail = detail;
      rememberSession(detail.session.id);
      resetLiveTurnState();
      statusMessage = `Loaded ${detail.timeline.length} conversation items.`;
      openAgentSessionId = detail.session.id;
    } catch {
      return false;
    } finally {
      if (loadGeneration === sessionLoadGeneration) sessionLoading = false;
    }
    tweaks = { ...tweaks, screen: "workspace" };
    return true;
  }

  function hasAvailableAgentProvider(snapshot: SettingsSnapshot | null): boolean {
    const authenticatedProviderIds = (snapshot?.auth ?? []).map((auth) => auth.providerId);
    return providerCatalogForSetup(snapshot).some((provider) =>
      providerIsAvailableForAgent(provider, authenticatedProviderIds)
    );
  }

  function shouldShowOnboarding(snapshot: SettingsSnapshot | null): boolean {
    if (!hasAvailableAgentProvider(snapshot)) return !allowUnauthenticatedWorkspaceHarness;
    if (onboardingCompleted) return false;
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
        if (s === "open" || s === "reconnecting") reconnectError = null;
        updateDaemonIdentity(client);
        // When we reconnect after a drop, refresh groups + reload the session
        // detail only if the user still has that detail view open.
        if (s === "open" && !onboarding) {
          desktopPinInFlightKeys = [];
          desktopPinInFlightStates = {};
          desktopPinQueuedStates = {};
          if (sessionEventUnlisten) {
            sessionEventUnlisten();
            sessionEventUnlisten = null;
          }
          subscribedSessionId = null;
          void refreshPins();
          void refreshSettings();
          void refreshGroups();
          void ensureSessionSubscription();
          if (selectedSession && openAgentSessionId === selectedSession.id) {
            void openSession(selectedSession, { showLoading: false, resetLiveState: true });
          }
        }
      }),
      // Any time a session is created or a turn finishes, refresh the
      // workspace board + sidebar. Coalesced by `refreshGroups`'s own
      // loading guard.
      client.on<{ sessionId?: string; reason?: string }>("workspace:sessions:changed", (event) => {
        const settled =
          event?.reason === "turn_complete" || event?.reason === "turn_error";
        if (
          event?.sessionId &&
          settled
        ) {
          clearLiveSidebarAgentState(event.sessionId, null);
          clearCachedTurnRuntimeState(event.sessionId);
          if (selectedSession?.id === event.sessionId) {
            clearTurnRuntimeState(event.sessionId, currentTurnId);
          }
        }
        void refreshGroups();
        if (
          selectedSession &&
          openAgentSessionId === selectedSession.id &&
          event?.sessionId === selectedSession.id
        ) {
          if (settled) {
            void openSession(selectedSession, { showLoading: false, resetLiveState: true });
          } else if (
            event.reason === "generated_title" ||
            event.reason === "rename_session" ||
            event.reason === "session_routing"
          ) {
            void openSession(selectedSession, { showLoading: false, resetLiveState: false });
          }
        }
      }),
      client.on<DesktopPinState>("desktop:pins:changed", (pins) => {
        desktopPins = {
          pinnedAgentIds: Array.isArray(pins?.pinnedAgentIds) ? pins.pinnedAgentIds : [],
          pinnedWorkspacePaths: Array.isArray(pins?.pinnedWorkspacePaths) ? pins.pinnedWorkspacePaths : []
        };
        const remainingInFlight = desktopPinInFlightKeys.filter((key) => {
          if (key in desktopPinQueuedStates) return true;
          const [kind, ...idParts] = key.split(":");
          const id = idParts.join(":");
          const expected = desktopPinInFlightStates[key];
          if (expected === undefined) return true;
          const confirmed = isDesktopPinned(kind === "workspace" ? "workspace" : "agent", id);
          return confirmed !== expected;
        });
        if (remainingInFlight.length !== desktopPinInFlightKeys.length) {
          desktopPinInFlightKeys = remainingInFlight;
          desktopPinInFlightStates = Object.fromEntries(
            Object.entries(desktopPinInFlightStates).filter(([key]) =>
              remainingInFlight.includes(key)
            )
          );
        }
      })
    ];
  }

  function reconnectFailureMessage(error: unknown): string {
    return error instanceof Error ? error.message : String(error);
  }

  async function reconnectBackend(): Promise<void> {
    if (reconnectBusy) return;
    reconnectBusy = true;
    reconnectError = null;
    try {
      const client = await ensureLocalDaemonClient();
      attachDaemonClient(client);
      await client.connect();
    } catch (error) {
      reconnectError = `Reconnect failed: ${reconnectFailureMessage(error)}`;
      connectionState = "closed";
    } finally {
      reconnectBusy = false;
    }
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
      sessionSubscriptionGeneration += 1;
      if (sessionEventUnlisten) {
        sessionEventUnlisten();
        sessionEventUnlisten = null;
      }
      clearLiveSidebarSessionSubscriptions();
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
    try {
      const info = await loadDefaultWorkspace();
      defaultWorkspaceCwd = info.cwd;
    } catch {
      /* daemon might be remote / unavailable; keep default empty */
    }
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
    if (authBusyProviderId || importBusyKey) return;
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
    if (importBusyKey || authBusyProviderId) return;
    importBusyKey = `${providerId}::${source}`;
    authError = null;
    const wasOnboarding = onboarding;
    try {
      settingsSnapshot = await importExternalCredential(providerId, source);
      onboardingCompleted = hasAvailableAgentProvider(settingsSnapshot);
      onboarding = shouldShowOnboarding(settingsSnapshot);
      if (wasOnboarding && !onboarding) {
        tweaks = { ...tweaks, screen: "workspace" };
      }
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
    if (authBusyProviderId || importBusyKey) return;
    authBusyProviderId = providerId;
    authError = null;
    const wasOnboarding = onboarding;
    try {
      settingsSnapshot = await loginWithOauth(providerId, remoteConnection);
      onboardingCompleted = hasAvailableAgentProvider(settingsSnapshot);
      onboarding = shouldShowOnboarding(settingsSnapshot);
      if (wasOnboarding && !onboarding) {
        tweaks = { ...tweaks, screen: "workspace" };
      }
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
    if (authBusyProviderId || importBusyKey) return;
    authBusyProviderId = providerId;
    authError = null;
    const wasOnboarding = onboarding;
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
      onboardingCompleted = hasAvailableAgentProvider(settingsSnapshot);
      onboarding = shouldShowOnboarding(settingsSnapshot);
      if (wasOnboarding && !onboarding) {
        tweaks = { ...tweaks, screen: "workspace" };
      }
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
    if (authBusyProviderId || importBusyKey) return;
    authBusyProviderId = providerId;
    authError = null;
    try {
      if (remoteConnection.enabled) {
        settingsSnapshot = await logoutProvider(providerId, remoteConnection);
      } else {
        settingsSnapshot = await logoutProviderViaDaemon(providerId);
      }
      statusMessage = `Disconnected ${providerId}.`;
      resetDaemonScopedSessionState();
      if (hasAvailableAgentProvider(settingsSnapshot)) {
        onboarding = false;
        await refreshGroups();
      } else {
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
      if (selectedSession) rememberFallbackSession(selectedSession);
      pruneFallbackSessions(nextGroups);
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

  function isDesktopPinned(kind: "agent" | "workspace", id: string): boolean {
    return kind === "agent"
      ? desktopPins.pinnedAgentIds.includes(id)
      : desktopPins.pinnedWorkspacePaths.includes(id);
  }

  function removeDesktopPinKey<T>(record: Record<string, T>, key: string): Record<string, T> {
    const { [key]: _removed, ...rest } = record;
    return rest;
  }

  async function toggleDesktopPin(kind: "agent" | "workspace", id: string, pinned: boolean) {
    const key = desktopPinKey(kind, id);
    if (desktopPinInFlightKeys.includes(key)) {
      if (isDesktopPinned(kind, id) !== pinned) applyPin(kind, id, pinned);
      desktopPinQueuedStates = { ...desktopPinQueuedStates, [key]: pinned };
      statusMessage = `${pinned ? "Pin" : "Unpin"} ${kind} queued.`;
      return;
    }
    desktopPinInFlightKeys = [...desktopPinInFlightKeys, key];
    desktopPinInFlightStates = { ...desktopPinInFlightStates, [key]: pinned };
    applyPin(kind, id, pinned);
    try {
      desktopPins = await setDesktopPin(kind, id, pinned);
      statusMessage = `${pinned ? "Pinned" : "Unpinned"} ${kind}.`;
    } catch (error) {
      applyPin(kind, id, !pinned);
      statusMessage = `Failed to update pin: ${error}`;
    } finally {
      desktopPinInFlightKeys = desktopPinInFlightKeys.filter((value) => value !== key);
      desktopPinInFlightStates = removeDesktopPinKey(desktopPinInFlightStates, key);
      const queued = desktopPinQueuedStates[key];
      desktopPinQueuedStates = removeDesktopPinKey(desktopPinQueuedStates, key);
      if (queued !== undefined && isDesktopPinned(kind, id) !== queued) {
        void toggleDesktopPin(kind, id, queued);
      }
    }
  }

  type OpenSessionOptions = {
    showLoading?: boolean;
    resetLiveState?: boolean;
  };

  function resetLiveTurnState() {
    submittedMessages = [];
    submittedMessageBaselineIds = {};
    liveStreamItems = [];
    replayTextByTurn = {};
    turnPermissionLookup = {};
    turnQuestionLookup = {};
    resolvingPermissionIds = [];
    resolvingQuestionIds = [];
    currentTurnId = null;
    cancelingTurnId = null;
    turnStartedAtMs = null;
    turnThinking = false;
    turnStatusHint = null;
  }

  function captureTransientConversationState(): TransientConversationState {
    return {
      submittedMessages,
      submittedMessageBaselineIds: { ...submittedMessageBaselineIds },
      liveStreamItems,
      replayTextByTurn: { ...replayTextByTurn },
      turnPermissionLookup: { ...turnPermissionLookup },
      turnQuestionLookup: { ...turnQuestionLookup },
      resolvingPermissionIds: [...resolvingPermissionIds],
      resolvingQuestionIds: [...resolvingQuestionIds],
      currentTurnId,
      cancelingTurnId,
      turnStartedAtMs,
      turnThinking,
      turnStatusHint
    };
  }

  function emptyTransientConversationState(): TransientConversationState {
    return {
      submittedMessages: [],
      submittedMessageBaselineIds: {},
      liveStreamItems: [],
      replayTextByTurn: {},
      turnPermissionLookup: {},
      turnQuestionLookup: {},
      resolvingPermissionIds: [],
      resolvingQuestionIds: [],
      currentTurnId: null,
      cancelingTurnId: null,
      turnStartedAtMs: null,
      turnThinking: false,
      turnStatusHint: null
    };
  }

  function transientStateHasContent(state: TransientConversationState): boolean {
    return (
      state.submittedMessages.length > 0 ||
      state.liveStreamItems.length > 0 ||
      state.currentTurnId !== null ||
      state.turnStartedAtMs !== null ||
      Object.keys(state.turnPermissionLookup).length > 0 ||
      Object.keys(state.turnQuestionLookup).length > 0 ||
      state.resolvingPermissionIds.length > 0 ||
      state.resolvingQuestionIds.length > 0
    );
  }

  function setTransientConversationState(
    sessionId: string,
    state: TransientConversationState | null
  ) {
    if (state && transientStateHasContent(state)) {
      transientConversationStates = { ...transientConversationStates, [sessionId]: state };
      return;
    }
    if (transientConversationStates[sessionId]) {
      const { [sessionId]: _drop, ...rest } = transientConversationStates;
      transientConversationStates = rest;
    }
  }

  function saveCurrentTransientConversationState(sessionId: string | null | undefined) {
    if (!sessionId) return;
    setTransientConversationState(sessionId, captureTransientConversationState());
  }

  function pendingSubmittedMessageKey(sessionId: string): string {
    return `${PENDING_SUBMITTED_MESSAGE_PREFIX}${sessionId}`;
  }

  function persistPendingSubmittedMessage(sessionId: string, item: TimelineItem) {
    if (typeof window === "undefined") return;
    try {
      window.localStorage.setItem(
        pendingSubmittedMessageKey(sessionId),
        JSON.stringify({ item, expiresAtMs: Date.now() + PENDING_SUBMITTED_MESSAGE_TTL_MS })
      );
    } catch {
      /* Best-effort recovery for reloads during turn start. */
    }
  }

  function clearPendingSubmittedMessage(sessionId: string, itemId?: string) {
    if (typeof window === "undefined") return;
    try {
      if (itemId) {
        const raw = window.localStorage.getItem(pendingSubmittedMessageKey(sessionId));
        const parsed = raw ? JSON.parse(raw) as { item?: { id?: string } } : null;
        if (parsed?.item?.id !== itemId) return;
      }
      window.localStorage.removeItem(pendingSubmittedMessageKey(sessionId));
    } catch {
      window.localStorage.removeItem(pendingSubmittedMessageKey(sessionId));
    }
  }

  function restorePendingSubmittedMessage(sessionId: string, timeline: TimelineItem[]): TimelineItem[] {
    if (typeof window === "undefined") return [];
    try {
      const raw = window.localStorage.getItem(pendingSubmittedMessageKey(sessionId));
      const parsed = raw ? JSON.parse(raw) as { item?: TimelineItem; expiresAtMs?: number } : null;
      const item = parsed?.item;
      if (!item || typeof parsed?.expiresAtMs !== "number" || parsed.expiresAtMs < Date.now()) {
        clearPendingSubmittedMessage(sessionId);
        return [];
      }
      const alreadyPersisted = timeline.some(
        (candidate) =>
          candidate.id === item.id ||
          (candidate.kind === "user" && item.kind === "user" && candidate.body === item.body)
      );
      if (alreadyPersisted) {
        clearPendingSubmittedMessage(sessionId, item.id);
        return [];
      }
      return [item];
    } catch {
      clearPendingSubmittedMessage(sessionId);
      return [];
    }
  }

  function cacheHiddenTurnStartError(sessionId: string, localUserId: string, detail: string) {
    clearPendingSubmittedMessage(sessionId, localUserId);
    const cached = transientConversationStates[sessionId] ?? emptyTransientConversationState();
    const baselineIds = { ...cached.submittedMessageBaselineIds };
    delete baselineIds[localUserId];
    setTransientConversationState(sessionId, {
      ...cached,
      submittedMessages: cached.submittedMessages.filter((item) => item.id !== localUserId),
      submittedMessageBaselineIds: baselineIds,
      liveStreamItems: appendCachedLiveItem(cached, {
        id: `live-error-turn-start-${localUserId}`,
        kind: "system",
        title: "Agent start failed",
        summary: detail,
        body: detail,
        meta: ["error", "turn-start-error"],
        status: "error"
      }),
      currentTurnId: null,
      cancelingTurnId: null,
      turnStartedAtMs: null,
      turnThinking: false,
      turnStatusHint: null
    });
  }

  function appendCachedLiveItem(
    state: TransientConversationState,
    item: TimelineItem
  ): TimelineItem[] {
    const existingIdx = state.liveStreamItems.findIndex((existing) => existing.id === item.id);
    if (existingIdx >= 0) {
      return [
        ...state.liveStreamItems.slice(0, existingIdx),
        item,
        ...state.liveStreamItems.slice(existingIdx + 1)
      ];
    }
    return [...state.liveStreamItems, item];
  }

  function withCachedTurnState(
    state: TransientConversationState,
    turnId: string,
    updates: Partial<TransientConversationState>
  ): TransientConversationState {
    return {
      ...state,
      ...updates,
      currentTurnId: turnId,
      cancelingTurnId: null,
      turnStartedAtMs: state.turnStartedAtMs ?? Date.now()
    };
  }

  function replaySafeCachedDelta(
    state: TransientConversationState,
    turnId: string,
    delta: string
  ): { delta: string; replayTextByTurn: Record<string, string> } {
    const replayText = `${state.replayTextByTurn[turnId] ?? ""}${delta}`;
    const replayTextByTurn = { ...state.replayTextByTurn, [turnId]: replayText };
    const currentItem = state.liveStreamItems.find((item) => item.id === streamingAssistantId(turnId));
    if (!currentItem || currentItem.kind !== "assistant") {
      return { delta, replayTextByTurn };
    }
    const current = currentItem.body;
    if (current.startsWith(replayText)) return { delta: "", replayTextByTurn };
    if (replayText.startsWith(current)) {
      return { delta: replayText.slice(current.length), replayTextByTurn };
    }
    return { delta, replayTextByTurn };
  }

  function upsertCachedStreamingAssistant(
    state: TransientConversationState,
    turnId: string,
    delta: string
  ): TimelineItem[] {
    const id = streamingAssistantId(turnId);
    const existingIdx = state.liveStreamItems.findIndex(
      (item) => item.id === id && item.kind === "assistant"
    );
    if (existingIdx >= 0) {
      const existing = state.liveStreamItems[existingIdx];
      if (existing.kind !== "assistant") return state.liveStreamItems;
      const body = existing.body + delta;
      return [
        ...state.liveStreamItems.slice(0, existingIdx),
        {
          ...existing,
          body,
          summary: body
        },
        ...state.liveStreamItems.slice(existingIdx + 1)
      ];
    }
    return appendCachedLiveItem(state, {
      id,
      kind: "assistant",
      title: "Assistant",
      summary: delta,
      body: delta,
      meta: []
    });
  }

  function cacheBackgroundTextDelta(
    sessionId: string,
    ev: Extract<SessionStreamEvent, { type: "text-delta" }>
  ) {
    const cached = transientConversationStates[sessionId] ?? emptyTransientConversationState();
    const replay = ev.replay
      ? replaySafeCachedDelta(cached, ev.turnId, ev.delta)
      : { delta: ev.delta, replayTextByTurn: cached.replayTextByTurn };
    setTransientConversationState(
      sessionId,
      withCachedTurnState(cached, ev.turnId, {
        liveStreamItems: replay.delta
          ? upsertCachedStreamingAssistant(cached, ev.turnId, replay.delta)
          : cached.liveStreamItems,
        replayTextByTurn: replay.replayTextByTurn,
        turnThinking: false,
        turnStatusHint: null
      })
    );
  }

  function cacheBackgroundToolCallsRequested(
    sessionId: string,
    ev: Extract<SessionStreamEvent, { type: "tool-calls-requested" }>
  ) {
    const cached = transientConversationStates[sessionId] ?? emptyTransientConversationState();
    let liveItems = cached.liveStreamItems;
    for (const req of ev.requests) {
      const id = liveToolId(ev.turnId, req.callId);
      if (liveItems.some((item) => item.id === id)) continue;
      liveItems = appendCachedLiveItem(
        { ...cached, liveStreamItems: liveItems },
        {
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
        }
      );
    }
    setTransientConversationState(
      sessionId,
      withCachedTurnState(cached, ev.turnId, {
        liveStreamItems: liveItems,
        turnThinking: false,
        turnStatusHint: "Running tools"
      })
    );
  }

  function cacheBackgroundToolInvocations(
    sessionId: string,
    ev: Extract<SessionStreamEvent, { type: "tool-invocations" }>
  ) {
    const cached = transientConversationStates[sessionId] ?? emptyTransientConversationState();
    let liveItems = cached.liveStreamItems;
    for (const inv of ev.invocations) {
      const id = liveToolId(ev.turnId, inv.callId);
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
        inputJson: safeParseJson(inv.input),
        metadata: inv.metadata
      };
      const existingIdx = liveItems.findIndex((item) => item.id === id);
      liveItems = existingIdx >= 0
        ? [
            ...liveItems.slice(0, existingIdx),
            payload,
            ...liveItems.slice(existingIdx + 1)
          ]
        : appendCachedLiveItem({ ...cached, liveStreamItems: liveItems }, payload);
    }
    setTransientConversationState(
      sessionId,
      withCachedTurnState(cached, ev.turnId, {
        liveStreamItems: liveItems,
        turnThinking: false,
        turnStatusHint: null
      })
    );
  }

  function lambdaGateSummary(
    ev: Extract<SessionStreamEvent, { type: "lambda-gate" }>
  ): string {
    if (ev.gateEvent === "host_call_admitted") {
      return `Gate admitted ${ev.hostTool ?? "host call"} for ${ev.concreteTool ?? ev.toolId}`;
    }
    if (ev.gateEvent === "host_call_committed") {
      return `Gate committed ${ev.hostTool ?? "host call"} through ${ev.concreteTool ?? ev.toolId}`;
    }
    if (ev.gateEvent === "gate_rejected") {
      return `Gate rejected ${ev.toolId}: ${ev.reason ?? "call did not satisfy the Verified Skill gate"}`;
    }
    return `Gate event: ${ev.gateEvent}`;
  }

  function gateJson(value: unknown): string | null {
    if (value === null || value === undefined) return null;
    try {
      return JSON.stringify(value);
    } catch {
      return String(value);
    }
  }

  function lambdaGateBody(
    ev: Extract<SessionStreamEvent, { type: "lambda-gate" }>
  ): string {
    const hostTool = ev.hostTool ?? "host call";
    const concreteTool = ev.concreteTool ?? ev.toolId;
    const hostArgs = gateJson(ev.hostArgs);
    const concreteInput = gateJson(ev.concreteInput);
    const registeredFacts = gateJson(ev.registeredFacts);
    const lines = ["Verified Skill Gate", `event: ${ev.gateEvent}`];
    if (ev.gateEvent === "gate_rejected") {
      lines.push("check: Compared the attempted tool call against the active LambdaHostCall gate.");
      if (ev.reason) lines.push(`reason: ${ev.reason}`);
      if (ev.retryTool) lines.push(`retry_tool: ${ev.retryTool}`);
      lines.push(
        "confirmation: Puffer rejected this call before committing the Lambda gate. Retry by opening LambdaHostCall with the formal host tool, host args, concrete tool, and exact concrete input."
      );
      return lines.join("\n");
    }
    if (ev.gateEvent === "host_call_committed") {
      lines.push(
        `check: Confirmed the concrete ${concreteTool} call matched the pending LambdaHostCall bridge for formal host tool ${hostTool}.`
      );
    } else {
      lines.push(
        `check: Verified LambdaHostCall may bind formal host tool ${hostTool} to concrete tool ${concreteTool}, and recorded the exact concrete input that must run next.`
      );
    }
    lines.push(`host_tool: ${hostTool}`);
    if (hostArgs) lines.push(`host_args: ${hostArgs}`);
    lines.push(`concrete_tool: ${concreteTool}`);
    if (concreteInput) lines.push(`concrete_input: ${concreteInput}`);
    if (registeredFacts) lines.push(`registered_facts: ${registeredFacts}`);
    lines.push(
      ev.gateEvent === "host_call_committed"
        ? "confirmation: Puffer observed the declared concrete tool succeed, then committed the Lambda gate and any registered facts."
        : "confirmation: Compare concrete_tool with the next activity row's tool name and concrete_input with that tool's input. Puffer only allows the next concrete call when both match exactly."
    );
    return lines.join("\n");
  }

  function cacheBackgroundLambdaGateEvent(
    sessionId: string,
    ev: Extract<SessionStreamEvent, { type: "lambda-gate" }>
  ) {
    const cached = transientConversationStates[sessionId] ?? emptyTransientConversationState();
    const id = `live-gate-${ev.turnId}-${ev.callId}-${ev.gateEvent}`;
    const payload: TimelineItem = {
      id,
      kind: "system",
      title: "Verified Skill Gate",
      summary: lambdaGateSummary(ev),
      body: lambdaGateBody(ev),
      meta: ["verified skill", ev.gateEvent],
      status: ev.gateEvent === "gate_rejected" ? "error" : "success",
      actor: ev.actor ?? null
    };
    const existingIdx = cached.liveStreamItems.findIndex((item) => item.id === id);
    const liveItems = existingIdx >= 0
      ? [
          ...cached.liveStreamItems.slice(0, existingIdx),
          payload,
          ...cached.liveStreamItems.slice(existingIdx + 1)
        ]
      : appendCachedLiveItem(cached, payload);
    setTransientConversationState(
      sessionId,
      withCachedTurnState(cached, ev.turnId, {
        liveStreamItems: liveItems,
        turnThinking: false,
        turnStatusHint: ev.gateEvent === "gate_rejected" ? "Gate rejected" : "Gate checked"
      })
    );
  }

  function cacheBackgroundPermissionRequest(
    sessionId: string,
    ev: Extract<SessionStreamEvent, { type: "permission-request" }>
  ) {
    const id = livePermissionId(ev.turnId, ev.requestId);
    const cached = transientConversationStates[sessionId] ?? emptyTransientConversationState();
    setTransientConversationState(sessionId, {
      ...cached,
      liveStreamItems: appendCachedLiveItem(cached, {
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
          choices: ["Approve once", "Always allow", "Deny"]
        },
        scopeLabel: null,
        choices: ["Approve once", "Always allow", "Deny"]
      }),
      turnPermissionLookup: {
        ...cached.turnPermissionLookup,
        [id]: { turnId: ev.turnId, requestId: ev.requestId }
      },
      currentTurnId: ev.turnId,
      cancelingTurnId: null,
      turnStartedAtMs: cached.turnStartedAtMs ?? Date.now(),
      turnThinking: false,
      turnStatusHint: "Awaiting approval"
    });
  }

  function cacheBackgroundUserQuestionRequest(
    sessionId: string,
    ev: Extract<SessionStreamEvent, { type: "user-question-request" }>
  ) {
    const id = liveQuestionId(ev.turnId, ev.requestId);
    const questions = normalizeUserQuestions(ev.questions);
    const cached = transientConversationStates[sessionId] ?? emptyTransientConversationState();
    setTransientConversationState(sessionId, {
      ...cached,
      liveStreamItems: appendCachedLiveItem(cached, {
        id,
        kind: "question",
        title: "Question",
        summary: questions.map((q) => q.question).join("\n"),
        body: "",
        meta: [],
        status: "pending",
        questions
      }),
      turnQuestionLookup: {
        ...cached.turnQuestionLookup,
        [id]: { turnId: ev.turnId, requestId: ev.requestId }
      },
      currentTurnId: ev.turnId,
      cancelingTurnId: null,
      turnStartedAtMs: cached.turnStartedAtMs ?? Date.now(),
      turnThinking: false,
      turnStatusHint: "Waiting for answer"
    });
  }

  function cacheBackgroundTurnComplete(
    sessionId: string,
    ev: Extract<SessionStreamEvent, { type: "turn-complete" }>
  ) {
    const cached = transientConversationStates[sessionId] ?? emptyTransientConversationState();
    const { [ev.turnId]: _dropReplay, ...replayTextByTurn } = cached.replayTextByTurn;
    const wasCancelingTurn = cached.cancelingTurnId === ev.turnId;
    const settledLiveItems = wasCancelingTurn
      ? withoutLiveItemsForTurn(cached.liveStreamItems, ev.turnId)
      : cached.liveStreamItems.filter(
          (item) => item.kind !== "permission" && item.kind !== "question"
        );
    setTransientConversationState(sessionId, {
      ...cached,
      liveStreamItems: wasCancelingTurn
        ? settledLiveItems
        : withCompletionAssistantFallback(
            settledLiveItems,
            ev.assistantText,
            ev.turnId
          ),
      replayTextByTurn,
      turnPermissionLookup: {},
      turnQuestionLookup: {},
      resolvingPermissionIds: [],
      resolvingQuestionIds: [],
      currentTurnId: null,
      cancelingTurnId: null,
      turnStartedAtMs: null,
      turnThinking: false,
      turnStatusHint: null
    });
  }

  function cacheBackgroundTurnError(
    sessionId: string,
    ev: Extract<SessionStreamEvent, { type: "turn-error" }>
  ) {
    const cached = transientConversationStates[sessionId] ?? emptyTransientConversationState();
    const detail = ev.error?.trim() || "Unknown agent error.";
    const { [ev.turnId]: _dropReplay, ...replayTextByTurn } = cached.replayTextByTurn;
    const wasCancelingTurn = cached.cancelingTurnId === ev.turnId;
    const settledLiveItems = wasCancelingTurn
      ? withoutLiveItemsForTurn(cached.liveStreamItems, ev.turnId)
      : cached.liveStreamItems.filter(
          (item) => item.kind !== "permission" && item.kind !== "question"
        );
    setTransientConversationState(sessionId, {
      ...cached,
      liveStreamItems: appendCachedLiveItem(
        { ...cached, liveStreamItems: settledLiveItems },
        {
          id: `live-error-turn-error-${ev.turnId}`,
          kind: "system",
          title: "Agent error",
          summary: detail,
          body: detail,
          meta: ["error", "turn-error"],
          status: "error"
        }
      ),
      replayTextByTurn,
      turnPermissionLookup: {},
      turnQuestionLookup: {},
      resolvingPermissionIds: [],
      resolvingQuestionIds: [],
      currentTurnId: null,
      cancelingTurnId: null,
      turnStartedAtMs: null,
      turnThinking: false,
      turnStatusHint: null
    });
  }

  function cacheBackgroundSessionEvent(sessionId: string, ev: SessionStreamEvent) {
    if (isTurnSettled(sessionId, ev.turnId)) return;
    switch (ev.type) {
      case "turn-start":
      case "thinking-delta":
      case "reflection-checkpoint":
      case "retry-attempt": {
        const cached = transientConversationStates[sessionId] ?? emptyTransientConversationState();
        setTransientConversationState(
          sessionId,
          withCachedTurnState(cached, ev.turnId, {
            turnThinking: true,
            turnStatusHint: ev.type === "retry-attempt"
              ? `Retrying ${ev.attempt}/${ev.maxAttempts}`
              : "Thinking"
          })
        );
        break;
      }
      case "text-delta":
        cacheBackgroundTextDelta(sessionId, ev);
        break;
      case "tool-calls-requested":
        cacheBackgroundToolCallsRequested(sessionId, ev);
        break;
      case "tool-invocations":
        cacheBackgroundToolInvocations(sessionId, ev);
        break;
      case "lambda-gate":
        cacheBackgroundLambdaGateEvent(sessionId, ev);
        break;
      case "permission-request":
        cacheBackgroundPermissionRequest(sessionId, ev);
        break;
      case "user-question-request":
        cacheBackgroundUserQuestionRequest(sessionId, ev);
        break;
      case "usage":
        break;
    }
  }

  function restoreTransientConversationState(sessionId: string) {
    const cached = transientConversationStates[sessionId];
    if (!cached) {
      resetLiveTurnState();
      return;
    }
    submittedMessages = cached.submittedMessages;
    submittedMessageBaselineIds = { ...cached.submittedMessageBaselineIds };
    liveStreamItems = cached.liveStreamItems;
    replayTextByTurn = { ...cached.replayTextByTurn };
    turnPermissionLookup = { ...cached.turnPermissionLookup };
    turnQuestionLookup = { ...cached.turnQuestionLookup };
    resolvingPermissionIds = [...cached.resolvingPermissionIds];
    resolvingQuestionIds = [...cached.resolvingQuestionIds];
    currentTurnId = cached.currentTurnId;
    cancelingTurnId = cached.cancelingTurnId;
    turnStartedAtMs = cached.turnStartedAtMs;
    turnThinking = cached.turnThinking;
    turnStatusHint = cached.turnStatusHint;
  }

  function removeCachedSubmittedMessage(sessionId: string, localUserId: string) {
    const cached = transientConversationStates[sessionId];
    if (!cached) return;
    const { [localUserId]: _drop, ...baselineIds } = cached.submittedMessageBaselineIds;
    setTransientConversationState(sessionId, {
      ...cached,
      submittedMessages: cached.submittedMessages.filter((item) => item.id !== localUserId),
      submittedMessageBaselineIds: baselineIds
    });
  }

  function markCachedTurnStarted(sessionId: string, turnId: string) {
    const cached = transientConversationStates[sessionId];
    if (!cached) return;
    setTransientConversationState(sessionId, {
      ...cached,
      currentTurnId: turnId,
      cancelingTurnId: null,
      turnStartedAtMs: cached.turnStartedAtMs ?? Date.now(),
      turnThinking: true,
      turnStatusHint: "Thinking"
    });
  }

  function hasTransientConversationState(sessionId: string): boolean {
    return (
      submitMessageInFlightFor(sessionId) ||
      submittedMessages.length > 0 ||
      liveStreamItems.length > 0 ||
      currentTurnId !== null ||
      turnStartedAtMs !== null ||
      Object.keys(turnPermissionLookup).length > 0 ||
      Object.keys(turnQuestionLookup).length > 0
    );
  }

  function hasTurnRuntimeState(): boolean {
    return (
      currentTurnId !== null ||
      cancelingTurnId !== null ||
      turnStartedAtMs !== null ||
      Object.keys(turnPermissionLookup).length > 0 ||
      Object.keys(turnQuestionLookup).length > 0
    );
  }

  function clearTurnRuntimeState(sessionId: string, turnId: string | null) {
    if (turnId) {
      rememberSettledTurn(sessionId, turnId);
      const { [turnId]: _drop, ...rest } = replayTextByTurn;
      replayTextByTurn = rest;
    }
    clearLiveSidebarAgentState(sessionId, turnId);
    currentTurnId = null;
    cancelingTurnId = null;
    turnStartedAtMs = null;
    turnThinking = false;
    turnStatusHint = null;
    turnPermissionLookup = {};
    turnQuestionLookup = {};
  }

  function clearCachedTurnRuntimeState(sessionId: string) {
    const cached = transientConversationStates[sessionId];
    if (!cached) return;
    const settledLiveItems = cached.liveStreamItems.filter(
      (item) => item.kind !== "permission" && item.kind !== "question"
    );
    setTransientConversationState(sessionId, {
      ...cached,
      liveStreamItems: settledLiveItems,
      replayTextByTurn: {},
      turnPermissionLookup: {},
      turnQuestionLookup: {},
      resolvingPermissionIds: [],
      resolvingQuestionIds: [],
      currentTurnId: null,
      cancelingTurnId: null,
      turnStartedAtMs: null,
      turnThinking: false,
      turnStatusHint: null
    });
  }

  function dismissPermissionId(scopedPermissionId: string) {
    if (dismissedPermissionIds.includes(scopedPermissionId)) return;
    dismissedPermissionIds = [...dismissedPermissionIds, scopedPermissionId].slice(-DISMISSED_IDS_CAP);
  }

  function dismissQuestionId(scopedQuestionId: string) {
    if (dismissedQuestionIds.includes(scopedQuestionId)) return;
    dismissedQuestionIds = [...dismissedQuestionIds, scopedQuestionId].slice(-DISMISSED_IDS_CAP);
  }

  function clearCachedResolvedPermission(
    sessionId: string | null,
    permissionId: string,
    turnId: string
  ) {
    if (!sessionId) return;
    const cached = transientConversationStates[sessionId];
    if (!cached) return;
    const { [permissionId]: _drop, ...nextLookup } = cached.turnPermissionLookup;
    setTransientConversationState(sessionId, {
      ...cached,
      liveStreamItems: cached.liveStreamItems.filter((item) => item.id !== permissionId),
      turnPermissionLookup: nextLookup,
      resolvingPermissionIds: cached.resolvingPermissionIds.filter(
        (id) => id !== `${sessionId}::${permissionId}`
      ),
      turnThinking: cached.currentTurnId === turnId ? false : cached.turnThinking,
      turnStatusHint: cached.currentTurnId === turnId ? "Running" : cached.turnStatusHint
    });
  }

  function clearCachedResolvedQuestion(
    sessionId: string | null,
    questionId: string,
    turnId: string
  ) {
    if (!sessionId) return;
    const cached = transientConversationStates[sessionId];
    if (!cached) return;
    const { [questionId]: _drop, ...nextLookup } = cached.turnQuestionLookup;
    setTransientConversationState(sessionId, {
      ...cached,
      liveStreamItems: cached.liveStreamItems.filter((item) => item.id !== questionId),
      turnQuestionLookup: nextLookup,
      resolvingQuestionIds: cached.resolvingQuestionIds.filter(
        (id) => id !== `${sessionId}::${questionId}`
      ),
      turnThinking: cached.currentTurnId === turnId ? false : cached.turnThinking,
      turnStatusHint: cached.currentTurnId === turnId ? "Running" : cached.turnStatusHint
    });
  }

  function clearSettledLoadedTurnState(
    sessionId: string,
    activityStatus: AgentActivityStatus,
    remainingSubmittedMessages: TimelineItem[],
    remainingLiveItems: TimelineItem[]
  ) {
    if (!hasTurnRuntimeState() || activityStatusIsActive(activityStatus)) return;
    const orphanedPendingStart =
      currentTurnId === null &&
      turnStartedAtMs !== null &&
      remainingLiveItems.length === 0 &&
      Object.keys(turnPermissionLookup).length === 0 &&
      Object.keys(turnQuestionLookup).length === 0;
    if (orphanedPendingStart) {
      clearTurnRuntimeState(sessionId, null);
      return;
    }
    if (remainingSubmittedMessages.length > 0 || remainingLiveItems.length > 0) return;
    clearTurnRuntimeState(sessionId, currentTurnId);
  }

  function clearCanceledLoadedTurnState(sessionId: string, activityStatus: AgentActivityStatus) {
    if (cancelingTurnId === null || activityStatusIsActive(activityStatus)) return;
    clearTurnRuntimeState(sessionId, currentTurnId);
    liveStreamItems = [];
    submittedMessages = [];
    submittedMessageBaselineIds = {};
  }

  async function openSession(session: SessionListItem, options: OpenSessionOptions = {}) {
    if (selectedSession?.id !== session.id) cancelRecapBlurTimer();
    const showLoading = options.showLoading ?? selectedSession?.id !== session.id;
    const sameSession = selectedSession?.id === session.id;
    const resetLiveState = options.resetLiveState ?? !sameSession;
    const loadGeneration = ++sessionLoadGeneration;
    if (showLoading) sessionLoading = true;
    if (resetLiveState && selectedSession?.id !== session.id) {
      saveCurrentTransientConversationState(selectedSession?.id);
      selectedSession = session;
      rememberFallbackSession(session);
      sessionDetail = null;
      rememberSession(session.id);
      restoreTransientConversationState(session.id);
    }
    try {
      const detail = await loadSessionDetailFromDaemon(session.id);
      if (loadGeneration !== sessionLoadGeneration) return;
      const preserveTransientState =
        resetLiveState &&
        selectedSession?.id === session.id &&
        hasTransientConversationState(session.id);
      const shouldResetLiveState = resetLiveState && !preserveTransientState;
      const timeline = shouldResetLiveState
        ? detail.timeline
        : reuseTransientMessageIds(detail.timeline, [...submittedMessages, ...liveStreamItems]);
      selectedSession = detail.session;
      rememberFallbackSession(detail.session);
      sessionDetail = { ...detail, timeline };
      rememberSession(detail.session.id);
      if (shouldResetLiveState) {
        // New session lands: drop any lingering live-stream items + local draft
        // so the composer feels fresh.
        resetLiveTurnState();
      } else {
        const remainingSubmittedMessages = submittedStillMissingFromPersisted(timeline, submittedMessages);
        const remainingLiveItems = stillMissingFromPersisted(timeline, liveStreamItems);
        const restoredPendingMessages = restorePendingSubmittedMessage(detail.session.id, timeline);
        submittedMessages = [
          ...remainingSubmittedMessages,
          ...restoredPendingMessages.filter(
            (pending) => !remainingSubmittedMessages.some((item) => item.id === pending.id)
          )
        ];
        liveStreamItems = remainingLiveItems;
        clearCanceledLoadedTurnState(detail.session.id, detail.session.activityStatus);
        if (resetLiveState) {
          clearSettledLoadedTurnState(
            detail.session.id,
            detail.session.activityStatus,
            remainingSubmittedMessages,
            remainingLiveItems
          );
        }
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
    newSessionError = null;
  }

  async function handleNewAgent(cwd: string, providerId?: string): Promise<boolean> {
    try {
      const created = await createSession(cwd || undefined, providerId);
      await openCreatedSession(created, providerId);
      statusMessage = `New ${created.providerId ?? providerId ?? "agent"} session in ${cwd || defaultWorkspaceCwd || "default workspace"}.`;
      newSessionError = null;
      return true;
    } catch (error) {
      const detail = errorText(error).replace(/^Error:\s*/, "");
      newSessionError = `Failed to create session: ${detail}`;
      statusMessage = newSessionError;
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
    cancelRecapBlurTimer();
    selectedSession = null;
    groups = [];
    groupsLoading = false;
    fallbackSessionsById = {};
    liveSidebarAgentsById = {};
    sessionDetail = null;
    openAgentSessionId = null;
    openProjectId = null;
    submittedMessages = [];
    submittedMessageBaselineIds = {};
    transientConversationStates = {};
    submitMessageInFlightSessionIds = [];
    dismissedPermissionIds = [];
    dismissedQuestionIds = [];
    resolvingPermissionIds = [];
    resolvingQuestionIds = [];
    desktopPinInFlightKeys = [];
    desktopPinInFlightStates = {};
    desktopPinQueuedStates = {};
    liveStreamItems = [];
    replayTextByTurn = {};
    turnPermissionLookup = {};
    turnQuestionLookup = {};
    currentTurnId = null;
    cancelingTurnId = null;
    turnStartedAtMs = null;
    turnThinking = false;
    turnStatusHint = null;
    settledTurnKeys = new Set();
    groupsRefreshGeneration += 1;
    sessionLoadGeneration += 1;
    sessionSubscriptionGeneration += 1;
    if (sessionEventUnlisten) {
      sessionEventUnlisten();
      sessionEventUnlisten = null;
    }
    clearLiveSidebarSessionSubscriptions();
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
    saveCurrentTransientConversationState(selectedSession?.id);
    tweaks = { ...tweaks, screen: id };
    openProjectId = null;
    openAgentSessionId = null;
  }

  function onOpenAgent(id: string) {
    const realTarget =
      groups.flatMap((g) => g.sessions).find((s) => s.id === id) ??
      fallbackSessionsById[id] ??
      liveSidebarAgentsById[id]?.session;
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
    saveCurrentTransientConversationState(selectedSession?.id);
    openAgentSessionId = null;
    clearRememberedSession();
  }

  function onOpenProject(id: string) {
    openProjectId = id;
    openAgentSessionId = null;
    tweaks = { ...tweaks, screen: "workspace" };
  }

  /** Fired by ConnectProjectModal once a clone+create has landed. Refreshes
   *  the workspace board and drills straight into the new session. */
  async function handleSessionReady(created: CreatedSessionResult) {
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
    await openCreatedSession(created, created.providerId);
  }

  async function openCreatedSession(
    created: CreatedSessionResult,
    requestedProviderId?: string
  ) {
    await refreshGroups();
    const newSession =
      groups.flatMap((g) => g.sessions).find((s) => s.id === created.sessionId) ?? null;
    if (newSession) {
      await openSession({
        ...newSession,
        providerId: created.providerId ?? requestedProviderId ?? newSession.providerId,
        modelId: created.modelId ?? newSession.modelId
      });
    } else {
      await openSession(sessionFallbackFromCreated(created, requestedProviderId));
    }
    openAgentSessionId = created.sessionId;
    tweaks = { ...tweaks, screen: "workspace" };
  }

  async function runWorkflowCommand(command: string): Promise<boolean> {
    const trimmed = command.trim();
    if (!trimmed) return false;
    try {
      if (!selectedSession) {
        const providerId = settingsSnapshot?.config.defaultProvider ?? undefined;
        const created = await createSession(defaultWorkspaceCwd || undefined, providerId);
        await openCreatedSession(created, providerId);
      }
      const started = await submitMessage(trimmed);
      if (started && selectedSession) {
        openAgentSessionId = selectedSession.id;
        tweaks = { ...tweaks, screen: "workspace" };
      }
      return started;
    } catch (error) {
      statusMessage = `Workflow command failed: ${errorText(error)}`;
      return false;
    }
  }

  function sessionFallbackFromCreated(
    created: CreatedSessionResult,
    requestedProviderId?: string
  ): SessionListItem {
    return {
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
      providerId: created.providerId ?? requestedProviderId ?? "codex",
      modelId: created.modelId ?? null
    };
  }

  function providerIsAuthenticated(providerId: string | null | undefined): boolean {
    if (!providerId) return true;
    const snapshot = settingsSnapshot;
    if (!providerIdCanRunAgent(providerId, snapshot?.providers ?? [])) return false;
    if (!snapshot) return true;
    const authenticatedProviderIds = snapshot.auth.map((auth) => auth.providerId);
    return snapshot.providers.some(
      (provider) =>
        providerIdInSet(providerId, [provider.id]) &&
        providerIsAvailableForAgent(provider, authenticatedProviderIds)
    );
  }

  function submitMessageInFlightFor(sessionId: string): boolean {
    return submitMessageInFlightSessionIds.includes(sessionId);
  }

  function setSubmitMessageInFlight(sessionId: string, inFlight: boolean) {
    if (inFlight) {
      submitMessageInFlightGuards.add(sessionId);
      if (!submitMessageInFlightSessionIds.includes(sessionId)) {
        submitMessageInFlightSessionIds = [...submitMessageInFlightSessionIds, sessionId];
      }
      return;
    }
    submitMessageInFlightGuards.delete(sessionId);
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
      submitMessageInFlightGuards.has(submitSessionId) ||
      submitMessageInFlightFor(submitSessionId) ||
      connectionState !== "open" ||
      turnStartedAtMs !== null ||
      currentTurnId !== null ||
      sessionAtSubmit.activityStatus === "running" ||
      sessionAtSubmit.activityStatus === "awaiting"
    ) {
      statusMessage = "Wait for the current turn to finish before sending another message.";
      return false;
    }
    const requestedProviderId =
      options.providerId ?? sessionAtSubmit.providerId ?? settingsSnapshot?.config.defaultProvider;
    if (
      requestedProviderId &&
      !providerIdCanRunAgent(requestedProviderId, settingsSnapshot?.providers ?? [])
    ) {
      const detail = `${requestedProviderId} cannot run agent sessions. Select an authenticated model provider.`;
      statusMessage = detail;
      appendAgentError("Provider unavailable", detail, "provider-agent");
      return false;
    }
    if (!providerIsAuthenticated(requestedProviderId)) {
      const detail = `Reconnect ${requestedProviderId} before continuing this session.`;
      statusMessage = detail;
      appendAgentError("Provider disconnected", detail, "provider-auth");
      return false;
    }
    setSubmitMessageInFlight(submitSessionId, true);
    const now = Date.now();
    const localUserId = `local-user-${now}`;
    submittedMessageBaselineIds = {
      ...submittedMessageBaselineIds,
      [localUserId]: (sessionDetail?.timeline ?? [])
        .map((item) => item.id)
        .filter((id) => id.length > 0)
    };
    const submittedMessage: TimelineItem = {
      id: localUserId,
      kind: "user",
      createdAtMs: now,
      title: "User",
      summary: message,
      body: message,
      meta: []
    };
    submittedMessages = [...submittedMessages, submittedMessage];
    persistPendingSubmittedMessage(submitSessionId, submittedMessage);
    turnStartedAtMs = now;
    turnThinking = true;
    turnStatusHint = "Thinking";
    setLiveSidebarAgentState(submitSessionId, "thinking", null, sessionAtSubmit);
    try {
      const turnId = await runAgentTurn(submitSessionId, message, options);
      const settledBeforeRpcReturned = isTurnSettled(submitSessionId, turnId);
      if (!settledBeforeRpcReturned) {
        setLiveSidebarAgentState(submitSessionId, "thinking", turnId, sessionAtSubmit);
      }
      if (selectedSession?.id !== submitSessionId) {
        if (!settledBeforeRpcReturned) {
          markCachedTurnStarted(submitSessionId, turnId);
        }
        return true;
      }
      if (settledBeforeRpcReturned) {
        clearPendingSubmittedMessage(submitSessionId, localUserId);
        currentTurnId = null;
        cancelingTurnId = null;
        turnStartedAtMs = null;
        turnThinking = false;
        turnStatusHint = null;
        return true;
      }
      currentTurnId = turnId;
      if (cancelingTurnId !== turnId) {
        cancelingTurnId = null;
      }
      forgetSettledTurn(submitSessionId, turnId);
      statusMessage = `Agent turn ${turnId.slice(0, 8)} started.`;
      return true;
    } catch (error) {
      const detail = errorText(error);
      if (isRecoverableTurnStartDisconnect(detail)) {
        turnThinking = false;
        turnStatusHint = "Waiting for reconnect";
        statusMessage = "Connection lost while starting the agent turn. Reconnect to sync the transcript.";
        return true;
      }
      clearLiveSidebarAgentState(submitSessionId, null);
      if (selectedSession?.id !== submitSessionId) {
        cacheHiddenTurnStartError(submitSessionId, localUserId, detail);
        return false;
      }
      clearPendingSubmittedMessage(submitSessionId, localUserId);
      submittedMessages = submittedMessages.filter((item) => item.id !== localUserId);
      const { [localUserId]: _drop, ...restBaselineIds } = submittedMessageBaselineIds;
      submittedMessageBaselineIds = restBaselineIds;
      currentTurnId = null;
      cancelingTurnId = null;
      turnStartedAtMs = null;
      turnThinking = false;
      turnStatusHint = null;
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
    if (n.includes("always")) return "allow_session";
    if (n.includes("session")) return "allow_session";
    if (n.includes("deny") || n.includes("never")) return "deny";
    return "allow_once";
  }

  async function resolvePermission(permissionId: string, choice: string) {
    const scopedPermissionId = activeSessionItemId(permissionId);
    if (resolvingPermissionIds.includes(scopedPermissionId)) return;
    const responseSessionId = selectedSession?.id ?? null;
    resolvingPermissionIds = [...resolvingPermissionIds, scopedPermissionId];
    try {
      const mapping = turnPermissionLookup[permissionId];
      if (mapping) {
        try {
          await resolveTurnPermission(mapping.turnId, mapping.requestId, mapPermissionAction(choice));
          dismissPermissionId(scopedPermissionId);
          if (selectedSession?.id !== responseSessionId) {
            clearCachedResolvedPermission(responseSessionId, permissionId, mapping.turnId);
            return;
          }
          statusMessage = `${choice} sent to agent.`;
          if (currentTurnId === mapping.turnId) {
            turnThinking = false;
            turnStatusHint = "Running";
          }
          const { [permissionId]: _drop, ...rest } = turnPermissionLookup;
          turnPermissionLookup = rest;
        } catch (error) {
          if (selectedSession?.id !== responseSessionId) return;
          const detail = errorText(error);
          statusMessage = `resolve_permission failed: ${detail}`;
          appendAgentError("Permission response failed", detail, "permission-error");
        }
      } else {
        dismissPermissionId(scopedPermissionId);
        statusMessage = `${choice} selected (no in-flight turn).`;
      }
    } finally {
      resolvingPermissionIds = resolvingPermissionIds.filter((id) => id !== scopedPermissionId);
    }
  }

  async function resolveUserQuestion(
    questionId: string,
    answers: Record<string, string | string[]>,
    annotations: Record<string, Record<string, string>> = {}
  ) {
    const scopedQuestionId = activeSessionItemId(questionId);
    if (resolvingQuestionIds.includes(scopedQuestionId)) return;
    const responseSessionId = selectedSession?.id ?? null;
    resolvingQuestionIds = [...resolvingQuestionIds, scopedQuestionId];
    try {
      const mapping = turnQuestionLookup[questionId];
      if (mapping) {
        try {
          await resolveTurnUserQuestion(mapping.turnId, mapping.requestId, answers, annotations);
          dismissQuestionId(scopedQuestionId);
          if (selectedSession?.id !== responseSessionId) {
            clearCachedResolvedQuestion(responseSessionId, questionId, mapping.turnId);
            return;
          }
          statusMessage = "Answer sent to agent.";
          if (currentTurnId === mapping.turnId) {
            turnThinking = false;
            turnStatusHint = "Running";
          }
          const { [questionId]: _drop, ...rest } = turnQuestionLookup;
          turnQuestionLookup = rest;
        } catch (error) {
          if (selectedSession?.id !== responseSessionId) return;
          const detail = errorText(error);
          statusMessage = `resolve_user_question failed: ${detail}`;
          appendAgentError("Question response failed", detail, "question-error");
        }
      } else {
        dismissQuestionId(scopedQuestionId);
        statusMessage = "Answer selected (no in-flight turn).";
      }
    } finally {
      resolvingQuestionIds = resolvingQuestionIds.filter((id) => id !== scopedQuestionId);
    }
  }

  async function cancelCurrentTurn() {
    const turnId = currentTurnId;
    if (!turnId || cancelingTurnId === turnId) return;
    cancelingTurnId = turnId;
    turnStatusHint = "Cancel requested";
    try {
      const result = await cancelTurn(turnId);
      if (!result.ok && currentTurnId === turnId) {
        if (selectedSession) markTurnSettled(selectedSession.id, turnId);
        liveStreamItems = withoutLiveItemsForTurn(liveStreamItems, turnId);
        cancelingTurnId = null;
        currentTurnId = null;
        turnStartedAtMs = null;
        turnThinking = false;
        turnStatusHint = null;
        statusMessage = `Turn ${turnId.slice(0, 8)} already finished.`;
        return;
      }
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

  function isRecoverableTurnStartDisconnect(detail: string): boolean {
    const normalized = detail.toLowerCase();
    return (
      normalized.includes("websocket closed") ||
      normalized.includes("websocket is not open")
    );
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

  function normalizedGateKey(value: string | null | undefined): string {
    return (value ?? "").toLowerCase().replace(/[^a-z0-9]/g, "");
  }

  function gateBodyField(item: TimelineItem, name: string): string | null {
    if (item.kind !== "system") return null;
    const target = normalizedGateKey(name);
    for (const line of item.body.split("\n")) {
      const idx = line.indexOf(":");
      if (idx < 0) continue;
      if (normalizedGateKey(line.slice(0, idx)) !== target) continue;
      const value = line.slice(idx + 1).trim();
      return value || null;
    }
    return null;
  }

  function transientGateSignature(item: TimelineItem): string | null {
    if (item.kind !== "system") return null;
    const isGate =
      item.title === "Verified Skill Gate" ||
      item.meta.includes("verified skill") ||
      item.body.trim().startsWith("Verified Skill Gate");
    if (!isGate) return null;
    const event = gateBodyField(item, "event") ??
      item.meta.find((value) => normalizedGateKey(value) !== "verifiedskill") ??
      null;
    const hostTool = gateBodyField(item, "host_tool") ?? gateBodyField(item, "hosttool");
    const concreteTool = gateBodyField(item, "concrete_tool") ?? gateBodyField(item, "concretetool");
    const reason = gateBodyField(item, "reason");
    if (!event && !hostTool && !concreteTool && !reason) return null;
    return stableJsonText({
      event: event ? normalizedGateKey(event) : null,
      hostTool,
      concreteTool,
      reason
    });
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
      const candidateIndex =
        candidates?.findIndex((candidate) => !wasPersistedBeforeSubmit(candidate, item.id)) ?? -1;
      const replacement =
        candidates && candidateIndex >= 0 ? candidates.splice(candidateIndex, 1)[0] : null;
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
    const gateSignature = transientGateSignature(pending);
    if (gateSignature) {
      return items.some(
        (item) =>
          !wasPersistedBeforeSubmit(pending.id, item.id) &&
          transientGateSignature(item) === gateSignature
      );
    }
    const body = timelineItemBody(pending).trim();
    if (!body) {
      const toolSignature = transientToolSignature(pending);
      if (toolSignature) {
        return items.some(
          (item) =>
            !wasPersistedBeforeSubmit(pending.id, item.id) &&
            transientToolSignature(item) === toolSignature
        );
      }
      return items.some(
        (item) =>
          !wasPersistedBeforeSubmit(pending.id, item.id) &&
          item.kind === pending.kind &&
          item.id === pending.id
      );
    }
    return items.some(
      (item) =>
        !wasPersistedBeforeSubmit(pending.id, item.id) &&
        item.kind === pending.kind &&
        ((item.id && item.id === pending.id) ||
          (timelineItemBody(item).trim() === body && transientTimestampsMatch(item, pending)))
    );
  }

  function wasPersistedBeforeSubmit(pendingId: string, persistedId: string): boolean {
    return submittedMessageBaselineIds[pendingId]?.includes(persistedId) ?? false;
  }

  function stillMissingFromPersisted(items: TimelineItem[], pending: TimelineItem[]): TimelineItem[] {
    return pending.filter((item) => !timelineHasTransientMatch(items, item));
  }

  function submittedStillMissingFromPersisted(
    items: TimelineItem[],
    pending: TimelineItem[]
  ): TimelineItem[] {
    const missing = stillMissingFromPersisted(items, pending);
    const missingIds = new Set(missing.map((item) => item.id));
    let nextBaselineIds = submittedMessageBaselineIds;
    for (const item of pending) {
      if (missingIds.has(item.id)) continue;
      if (nextBaselineIds === submittedMessageBaselineIds) nextBaselineIds = { ...submittedMessageBaselineIds };
      delete nextBaselineIds[item.id];
    }
    if (nextBaselineIds !== submittedMessageBaselineIds) submittedMessageBaselineIds = nextBaselineIds;
    return missing;
  }

  function withCompletionAssistantFallback(
    items: TimelineItem[],
    text: string,
    turnId: string
  ): TimelineItem[] {
    const trimmed = text.trim();
    if (!trimmed) return items;
    const streamingId = streamingAssistantId(turnId);
    const streamingIndex = items.findIndex((item) => item.kind === "assistant" && item.id === streamingId);
    if (streamingIndex >= 0) {
      const existing = items[streamingIndex];
      if (existing.kind !== "assistant" || existing.body.trim() === trimmed) return items;
      return [
        ...items.slice(0, streamingIndex),
        {
          ...existing,
          summary: trimmed,
          body: trimmed
        },
        ...items.slice(streamingIndex + 1)
      ];
    }
    if (timelineHasBody(items, "assistant", trimmed)) return items;
    return [
      ...items,
      {
        id: `live-complete-assistant-${turnId}`,
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
        submittedMessages = submittedStillMissingFromPersisted(persistedTimeline, submittedAtCompletion);
        return;
      }
      liveStreamItems = stillMissingFromPersisted(persistedTimeline, liveItemsAtCompletion);
      submittedMessages = submittedStillMissingFromPersisted(persistedTimeline, submittedAtCompletion);
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

  function liveItemBelongsToTurn(item: TimelineItem, turnId: string): boolean {
    return (
      item.id === streamingAssistantId(turnId) ||
      item.id === `live-complete-assistant-${turnId}` ||
      item.id === `live-error-turn-error-${turnId}` ||
      item.id.startsWith(`live-tool-${turnId}-`) ||
      item.id.startsWith(`live-gate-${turnId}-`) ||
      item.id.startsWith(`live-perm-${turnId}-`) ||
      item.id.startsWith(`live-question-${turnId}-`)
    );
  }

  function withoutLiveItemsForTurn(items: TimelineItem[], turnId: string): TimelineItem[] {
    return items.filter((item) => !liveItemBelongsToTurn(item, turnId));
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

  function turnKey(sessionId: string, turnId: string): string {
    return `${sessionId}\u0000${turnId}`;
  }

  function isTurnSettled(sessionId: string, turnId: string): boolean {
    return settledTurnKeys.has(turnKey(sessionId, turnId));
  }

  function rememberSettledTurn(sessionId: string, turnId: string) {
    settledTurnKeys.add(turnKey(sessionId, turnId));
    if (settledTurnKeys.size > SETTLED_TURN_KEYS_CAP) {
      const overflow = settledTurnKeys.size - SETTLED_TURN_KEYS_CAP;
      let removed = 0;
      for (const key of settledTurnKeys) {
        settledTurnKeys.delete(key);
        removed += 1;
        if (removed >= overflow) break;
      }
    }
  }

  function forgetSettledTurn(sessionId: string, turnId: string) {
    settledTurnKeys.delete(turnKey(sessionId, turnId));
  }

  function shouldIgnoreTurnEvent(sessionId: string, turnId: string): boolean {
    if (isTurnSettled(sessionId, turnId)) return true;
    return currentTurnId !== null && currentTurnId !== turnId;
  }

  function markTurnActive(sessionId: string, turnId: string) {
    if (currentTurnId !== null && currentTurnId !== turnId) {
      cancelingTurnId = null;
    }
    currentTurnId = turnId;
    forgetSettledTurn(sessionId, turnId);
  }

  function markTurnSettled(sessionId: string, turnId: string) {
    rememberSettledTurn(sessionId, turnId);
    const { [turnId]: _drop, ...rest } = replayTextByTurn;
    replayTextByTurn = rest;
    turnPermissionLookup = Object.fromEntries(
      Object.entries(turnPermissionLookup).filter(([, mapping]) => mapping.turnId !== turnId)
    );
    turnQuestionLookup = Object.fromEntries(
      Object.entries(turnQuestionLookup).filter(([, mapping]) => mapping.turnId !== turnId)
    );
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
    const selectedForEvent = selectedSession?.id === sid;
    if (isTurnSettled(sid, ev.turnId)) return;
    const ignoredForSelected = selectedForEvent && shouldIgnoreTurnEvent(sid, ev.turnId);
    if (!ignoredForSelected) applySidebarSessionEvent(sid, ev);
    if (ignoredForSelected) {
      if (ev.type === "turn-complete" || ev.type === "turn-error") {
        markTurnSettled(sid, ev.turnId);
      }
      return;
    }
    if (!selectedForEvent) {
      if (ev.type === "turn-complete") {
        rememberSettledTurn(sid, ev.turnId);
        cacheBackgroundTurnComplete(sid, ev);
        return;
      }
      if (ev.type === "turn-error") {
        rememberSettledTurn(sid, ev.turnId);
        cacheBackgroundTurnError(sid, ev);
        return;
      }
      cacheBackgroundSessionEvent(sid, ev);
      return;
    }
    switch (ev.type) {
      case "turn-start":
        markTurnActive(sid, ev.turnId);
        turnStartedAtMs = Date.now();
        turnThinking = true;
        turnStatusHint = "Thinking";
        if (!ev.replay) {
          const { [ev.turnId]: _drop, ...rest } = replayTextByTurn;
          replayTextByTurn = rest;
        }
        break;
      case "thinking-delta":
        markTurnActive(sid, ev.turnId);
        turnThinking = true;
        turnStatusHint = "Thinking";
        break;
      case "text-delta":
        markTurnActive(sid, ev.turnId);
        turnThinking = false;
        turnStatusHint = null;
        {
          const delta = ev.replay ? replaySafeDelta(ev.turnId, ev.delta) : ev.delta;
          if (delta) upsertStreamingAssistant(ev.turnId, delta);
        }
        break;
      case "tool-calls-requested":
        markTurnActive(sid, ev.turnId);
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
        markTurnActive(sid, ev.turnId);
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
            inputJson: safeParseJson(inv.input),
            metadata: inv.metadata
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
      case "lambda-gate":
        markTurnActive(sid, ev.turnId);
        turnThinking = false;
        turnStatusHint = ev.gateEvent === "gate_rejected" ? "Gate rejected" : "Gate checked";
        {
          const id = `live-gate-${ev.turnId}-${ev.callId}-${ev.gateEvent}`;
          const payload: TimelineItem = {
            id,
            kind: "system",
            title: "Verified Skill Gate",
            summary: lambdaGateSummary(ev),
            body: lambdaGateBody(ev),
            meta: ["verified skill", ev.gateEvent],
            status: ev.gateEvent === "gate_rejected" ? "error" : "success",
            actor: ev.actor ?? null
          };
          const existingIdx = liveStreamItems.findIndex((item) => item.id === id);
          if (existingIdx >= 0) {
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
      case "plan-updated":
        currentTurnId = ev.turnId;
        turnThinking = false;
        turnStatusHint = "Planning";
        break;
      case "plan-completed":
        currentTurnId = ev.turnId;
        turnThinking = false;
        turnStatusHint = "Plan ready";
        break;
      case "reflection-checkpoint":
        markTurnActive(sid, ev.turnId);
        turnThinking = true;
        turnStatusHint = "Thinking";
        break;
      case "retry-attempt":
        markTurnActive(sid, ev.turnId);
        turnThinking = true;
        turnStatusHint = `Retrying ${ev.attempt}/${ev.maxAttempts}`;
        break;
      case "usage":
        markTurnActive(sid, ev.turnId);
        break;
      case "permission-request": {
        markTurnActive(sid, ev.turnId);
        turnThinking = false;
        turnStatusHint = "Awaiting approval";
        const id = livePermissionId(ev.turnId, ev.requestId);
        const choices = ev.browser
          ? ["Approve once", "Always allow browser context", "Deny"]
          : ["Approve once", "Always allow", "Deny"];
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
            choices
          },
          scopeLabel: null,
          choices
        });
        turnPermissionLookup = {
          ...turnPermissionLookup,
          [id]: { turnId: ev.turnId, requestId: ev.requestId }
        };
        break;
      }
      case "user-question-request": {
        markTurnActive(sid, ev.turnId);
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
      case "turn-error": {
        const wasCancelingTurn = cancelingTurnId === ev.turnId;
        markTurnSettled(sid, ev.turnId);
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
          const liveItemsAtCompletion = wasCancelingTurn
            ? withoutLiveItemsForTurn(liveStreamItems, ev.turnId)
            : withCompletionAssistantFallback(
                liveStreamItems,
                completionText,
                ev.turnId
              );
          const submittedAtCompletion = submittedMessages;
          liveStreamItems = stillMissingFromPersisted(
            [...(sessionDetail?.timeline ?? []), ...submittedAtCompletion],
            liveItemsAtCompletion
          );
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
        type: item.type === "input" ? "input" as const : "choice" as const,
        multiSelect: item.multiSelect === true,
        searchable: item.searchable === true,
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
      }));
  }

  async function ensureSessionSubscription() {
    if (!selectedSession) {
      sessionSubscriptionGeneration += 1;
      if (sessionEventUnlisten) {
        sessionEventUnlisten();
        sessionEventUnlisten = null;
      }
      subscribedSessionId = null;
      return;
    }
    if (subscribedSessionId === selectedSession.id && sessionEventUnlisten) return;
    const generation = ++sessionSubscriptionGeneration;
    if (sessionEventUnlisten) {
      sessionEventUnlisten();
      sessionEventUnlisten = null;
    }
    const sid = selectedSession.id;
    subscribedSessionId = sid;
    const unlisten = await subscribeSessionEvents(sid, (ev) => handleSessionEvent(sid, ev));
    if (
      generation !== sessionSubscriptionGeneration ||
      selectedSession?.id !== sid ||
      subscribedSessionId !== sid
    ) {
      unlisten();
      return;
    }
    sessionEventUnlisten = unlisten;
  }

  $effect(() => {
    void ensureSessionSubscription();
  });

  $effect(() => {
    const targetIds = liveSidebarSessionSubscriptionTargets();
    void ensureLiveSidebarSessionSubscriptions(targetIds);
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
          />
        </div>
      </div>
    </div>
  {:else}
    <div class="pf-app-body">
      {#if tweaks.showSidebar}
        <Sidebar
          screen={openAgentSessionId || openProjectId ? null : tweaks.screen}
          collapsed={tweaks.collapsedSidebar}
          width={tweaks.sidebarWidth}
          onSelectScreen={onSelectScreen}
          agents={activeAgents}
          activeAgentId={openAgentSessionId}
          onOpenAgent={onOpenAgent}
          onToggleAgentPin={(id, pinned) => void toggleDesktopPin("agent", id, pinned)}
          onToggleCollapse={() => updateTweak("collapsedSidebar", !tweaks.collapsedSidebar)}
          onResize={(width) => updateTweak("sidebarWidth", width)}
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
                resolvingPermissionIds={resolvingPermissionIds}
                resolvingQuestionIds={resolvingQuestionIds}
                loading={sessionLoading}
                turnRunning={turnRunning}
                turnCancelable={currentTurnId !== null && cancelingTurnId !== currentTurnId}
                turnStartedAtMs={turnStartedAtMs}
                turnThinking={turnThinking}
                turnStatusHint={turnStatusHint}
                settingsSnapshot={settingsSnapshot}
                backendConnected={connectionState === "open"}
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
                onSessionReady={(created) => handleSessionReady(created)}
                onOpenWorkspacePicker={() => (showWorkspacePicker = true)}
                pinnedWorkspacePaths={desktopPins.pinnedWorkspacePaths}
                pinningWorkspacePaths={desktopPinInFlightKeys
                  .filter((key) => key.startsWith("workspace:"))
                  .map((key) => key.slice("workspace:".length))}
                onToggleWorkspacePin={(path, pinned) => void toggleDesktopPin("workspace", path, pinned)}
              />
            {/if}
          {:else if tweaks.screen === "workflows"}
            <Workflows onRunWorkflowCommand={runWorkflowCommand} />
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
    error={newSessionError}
    onClose={() => {
      if (!newSessionBusy) {
        newSessionCwd = null;
        newSessionError = null;
      }
    }}
    onCreate={async (providerId) => {
      if (!newSessionCwd || newSessionBusy) return;
      newSessionError = null;
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
      Lost connection to Puffer backend. Reconnecting…
    {:else}
      <span class="dot err"></span>
      Puffer backend disconnected.
      <button
        type="button"
        class="sc-btn"
        data-variant="outline"
        data-size="sm"
        aria-label="Reconnect backend"
        aria-busy={reconnectBusy}
        disabled={reconnectBusy}
        onclick={() => void reconnectBackend()}
      >{reconnectBusy ? "Reconnecting..." : "Reconnect"}</button>
      {#if reconnectError}
        <span class="connection-error">{reconnectError}</span>
      {/if}
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
    flex-wrap: wrap;
    align-items: center;
    justify-content: center;
    gap: 10px;
    max-width: min(760px, calc(100vw - 32px));
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
  .connection-banner .connection-error {
    max-width: 520px;
    overflow: hidden;
    color: color-mix(in oklab, oklch(0.62 0.22 25) 78%, var(--foreground));
    font-weight: 650;
    text-overflow: ellipsis;
    white-space: nowrap;
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

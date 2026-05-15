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
    type ConnectionState
  } from "./lib/api/daemonClient";
  import { sessionDisplayName, sessionDisplayTitle } from "./lib/sessionDisplay";
  import type { UnlistenFn } from "@tauri-apps/api/event";
  import type {
    DesktopPreferences,
    DesktopPinState,
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
  let dismissedPermissionIds = $state<string[]>([]);
  let dismissedQuestionIds = $state<string[]>([]);

  // Live turn state: items synthesized from streaming events while a turn is
  // running. When the turn finishes we reload the session detail so the real
  // persisted transcript replaces these placeholders.
  let currentTurnId = $state<string | null>(null);
  let turnStartedAtMs = $state<number | null>(null);
  let turnThinking = $state(false);
  let turnStatusHint = $state<string | null>(null);
  let liveStreamItems = $state<TimelineItem[]>([]);
  let turnPermissionLookup = $state<Record<string, { turnId: string; requestId: string }>>({});
  let turnQuestionLookup = $state<Record<string, { turnId: string; requestId: string }>>({});
  let sessionEventUnlisten: UnlistenFn | null = null;
  let subscribedSessionId: string | null = null;
  let connectionState = $state<ConnectionState>("idle");
  let sessionLoadGeneration = 0;
  let desktopPins = $state<DesktopPinState>({ pinnedAgentIds: [], pinnedWorkspacePaths: [] });

  let settingsSnapshot = $state<SettingsSnapshot | null>(null);
  let settingsLoading = $state(false);
  let authBusyProviderId = $state<string | null>(null);
  let authError = $state<string | null>(null);
  let externalCredentials = $state<ExternalCredential[]>([]);
  let importBusyKey = $state<string | null>(null);
  let actionBusy = $state(false);
  let remoteOperation = $state<RemoteOperation | null>(null);
  let remoteBusy = $state(false);
  let remotePassword = $state("");

  // Tauri is stateless: preferences live in Puffer's workspace config, not
  // here. We keep an in-memory copy to drive the Settings pane but never
  // persist it — relaunching the app re-reads from the daemon.
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
  let desktopPreferences = $state<DesktopPreferences>({ ...defaultDesktopPreferences });

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
  let combinedTimeline = $derived<TimelineItem[]>([
    ...(sessionDetail?.timeline ?? []),
    ...submittedMessages,
    ...liveStreamItems
  ]);
  let pendingPermissions = $derived<PermissionTimelineItem[]>(
    combinedTimeline.filter(
      (t): t is PermissionTimelineItem =>
        t.kind === "permission" && !dismissedPermissionIds.includes(t.id)
    )
  );
  let pendingQuestions = $derived<UserQuestionTimelineItem[]>(
    combinedTimeline.filter(
      (t): t is UserQuestionTimelineItem =>
        t.kind === "question" && t.status === "pending" && !dismissedQuestionIds.includes(t.id)
    )
  );
  let turnRunning = $derived(currentTurnId !== null || turnStartedAtMs !== null);

  function latestGroupMs(group: FolderGroup): number {
    return group.sessions.reduce((latest, session) => Math.max(latest, session.updatedAtMs), 0);
  }

  function pinnedIndex(values: string[], id: string): number {
    const index = values.indexOf(id);
    return index === -1 ? Number.MAX_SAFE_INTEGER : index;
  }

  let sortedGroups = $derived<FolderGroup[]>(
    groups.slice().sort((left, right) => {
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
    })
  );

  let realAgents = $derived<ActiveAgent[]>(
    sortedGroups
      .flatMap((g) =>
        g.sessions.map((s) => ({
        id: s.id,
        name: sessionDisplayName(s).slice(0, 24),
        title: sessionDisplayTitle(s),
        project: g.label,
        branch: "",
        state: "idle" as AgentState,
        updatedAtMs: s.updatedAtMs,
        pinned: desktopPins.pinnedAgentIds.includes(s.id)
      }))
    )
      .slice()
      .sort((left, right) =>
        pinnedIndex(desktopPins.pinnedAgentIds, left.id) - pinnedIndex(desktopPins.pinnedAgentIds, right.id)
        || right.updatedAtMs - left.updatedAtMs
        || left.project.localeCompare(right.project)
      )
  );

  let activeAgents = $derived<ActiveAgent[]>(realAgents);

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

  function armRecapBlurTimer() {
    if (recapBlurTimer != null) return;
    recapBlurTimer = setTimeout(() => {
      recapBlurTimer = null;
      if (!selectedSession) return;
      void submitMessage("/recap", {});
    }, RECAP_IDLE_MS);
  }

  function cancelRecapBlurTimer() {
    if (recapBlurTimer != null) {
      clearTimeout(recapBlurTimer);
      recapBlurTimer = null;
    }
  }

  onMount(() => {
    tweaks = loadTweaks();
    applyTweaksToDocument(tweaks);
    if (forceOnboarding) {
      onboarding = true;
    } else if (skipOnboarding) {
      onboarding = false;
    }
    window.addEventListener("blur", armRecapBlurTimer);
    window.addEventListener("focus", cancelRecapBlurTimer);
    void init();
    return () => {
      cancelRecapBlurTimer();
      window.removeEventListener("blur", armRecapBlurTimer);
      window.removeEventListener("focus", cancelRecapBlurTimer);
    };
  });

  $effect(() => {
    applyTweaksToDocument(tweaks);
    persistTweaks(tweaks); // Tweaks are renderer ergonomics, not workspace data.
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
        client.onConnectionChange((s) => {
          connectionState = s;
          // When we reconnect after a drop, refresh groups + re-open the
          // selected session so the UI catches up.
          if (s === "open" && !onboarding) {
            void refreshPins();
            void refreshGroups();
            if (selectedSession) void openSession(selectedSession);
          }
        });
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
        });
        client.on<DesktopPinState>("desktop:pins:changed", (pins) => {
          desktopPins = {
            pinnedAgentIds: Array.isArray(pins?.pinnedAgentIds) ? pins.pinnedAgentIds : [],
            pinnedWorkspacePaths: Array.isArray(pins?.pinnedWorkspacePaths) ? pins.pinnedWorkspacePaths : []
          };
        });
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
    settingsLoading = true;
    try {
      settingsSnapshot = await loadSettingsSnapshot(remoteConnection);
      if (forceOnboarding) {
        onboarding = true;
      } else if (skipOnboarding) {
        onboarding = false;
      } else {
        onboarding = (settingsSnapshot.auth?.length ?? 0) === 0;
      }
      // Re-scan ~/.claude / ~/.codex so the LoginView can offer one-click
      // imports for credentials the user already has on disk. Failure is
      // non-fatal — the manual API-key path still works.
      void listExternalCredentials()
        .then((found) => {
          externalCredentials = found;
        })
        .catch(() => {
          externalCredentials = [];
        });
      statusMessage = "Settings snapshot refreshed.";
    } catch (error) {
      statusMessage = String(error);
      if (skipOnboarding) onboarding = false;
    } finally {
      settingsLoading = false;
    }
  }

  async function handleImportExternal(providerId: string, source: "claude" | "codex") {
    importBusyKey = `${providerId}::${source}`;
    authError = null;
    try {
      settingsSnapshot = await importExternalCredential(providerId, source);
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

  async function handleOauthLogin(providerId: string) {
    authBusyProviderId = providerId;
    authError = null;
    try {
      settingsSnapshot = await loginWithOauth(providerId, remoteConnection);
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
    groupsLoading = true;
    try {
      groups = await listGroupedSessionsFromDaemon();
      statusMessage =
        groups.length === 0
          ? "No sessions in this workspace yet."
          : `${groups.length} project${groups.length === 1 ? "" : "s"} loaded.`;
    } catch (error) {
      statusMessage = String(error);
    } finally {
      groupsLoading = false;
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

  async function toggleDesktopPin(kind: "agent" | "workspace", id: string, pinned: boolean) {
    applyPin(kind, id, pinned);
    try {
      desktopPins = await setDesktopPin(kind, id, pinned);
      statusMessage = `${pinned ? "Pinned" : "Unpinned"} ${kind}.`;
    } catch (error) {
      applyPin(kind, id, !pinned);
      statusMessage = `Failed to update pin: ${error}`;
    }
  }

  type OpenSessionOptions = {
    showLoading?: boolean;
    resetLiveState?: boolean;
  };

  async function openSession(session: SessionListItem, options: OpenSessionOptions = {}) {
    const showLoading = options.showLoading ?? selectedSession?.id !== session.id;
    const resetLiveState = options.resetLiveState ?? true;
    const loadGeneration = ++sessionLoadGeneration;
    if (showLoading) sessionLoading = true;
    try {
      const detail = await loadSessionDetailFromDaemon(session.id);
      if (loadGeneration !== sessionLoadGeneration) return;
      selectedSession = detail.session;
      sessionDetail = detail;
      if (resetLiveState) {
        // New session lands: drop any lingering live-stream items + local draft
        // so the composer feels fresh.
        submittedMessages = [];
        liveStreamItems = [];
        turnPermissionLookup = {};
        turnQuestionLookup = {};
        currentTurnId = null;
        turnStartedAtMs = null;
        turnThinking = false;
        turnStatusHint = null;
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
        await openSession(newSession);
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
  }

  function resetDesktopPreferences() {
    desktopPreferences = { ...defaultDesktopPreferences };
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
    if (id !== "workspace") {
      openProjectId = null;
      openAgentSessionId = null;
    }
  }

  function onOpenAgent(id: string) {
    const realTarget = groups.flatMap((g) => g.sessions).find((s) => s.id === id);
    if (!realTarget) return;
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
    // Refresh the default workspace info in case we just connected to a
    // remote daemon — the workspace root changed.
    void loadDefaultWorkspace()
      .then((info) => {
        defaultWorkspaceCwd = info.cwd;
      })
      .catch(() => {});
    await refreshGroups();
    const session = groups.flatMap((g) => g.sessions).find((s) => s.id === sessionId);
    if (session) {
      await openSession(session);
    }
    openAgentSessionId = sessionId;
    tweaks = { ...tweaks, screen: "workspace" };
  }

  async function submitMessage(message: string, options: AgentTurnOptions = {}) {
    if (!selectedSession) {
      statusMessage = "Select a session to send a message.";
      return;
    }
    const now = Date.now();
    turnStartedAtMs = now;
    turnThinking = true;
    turnStatusHint = "Thinking";
    submittedMessages = [
      ...submittedMessages,
      {
        id: `local-user-${now}`,
        kind: "user",
        title: "User",
        summary: message,
        body: message,
        meta: []
      }
    ];
    try {
      const turnId = await runAgentTurn(selectedSession.id, message, options);
      currentTurnId = turnId;
      statusMessage = `Agent turn ${turnId.slice(0, 8)} started.`;
    } catch (error) {
      currentTurnId = null;
      turnStartedAtMs = null;
      turnThinking = false;
      turnStatusHint = null;
      const detail = errorText(error);
      statusMessage = `run_agent_turn failed: ${detail}`;
      appendAgentError("Agent start failed", detail, "turn-start-error");
    }
  }

  async function renameSelectedSession(title: string) {
    if (!selectedSession) return;
    const previous = selectedSession;
    try {
      const detail = await renameSession(selectedSession.id, title);
      selectedSession = detail.session;
      sessionDetail = detail;
      await refreshGroups();
      statusMessage = title.trim() ? "Session title updated." : "Session title reset.";
    } catch (error) {
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
    dismissedPermissionIds = [...dismissedPermissionIds, permissionId];
    const mapping = turnPermissionLookup[permissionId];
    if (mapping) {
      try {
        await resolveTurnPermission(mapping.turnId, mapping.requestId, mapPermissionAction(choice));
        statusMessage = `${choice} sent to agent.`;
      } catch (error) {
        const detail = errorText(error);
        statusMessage = `resolve_permission failed: ${detail}`;
        appendAgentError("Permission response failed", detail, "permission-error");
      }
      const { [permissionId]: _drop, ...rest } = turnPermissionLookup;
      turnPermissionLookup = rest;
    } else {
      statusMessage = `${choice} selected (no in-flight turn).`;
    }
  }

  async function resolveUserQuestion(
    questionId: string,
    answers: Record<string, string | string[]>,
    annotations: Record<string, Record<string, string>> = {}
  ) {
    dismissedQuestionIds = [...dismissedQuestionIds, questionId];
    const mapping = turnQuestionLookup[questionId];
    if (mapping) {
      try {
        await resolveTurnUserQuestion(mapping.turnId, mapping.requestId, answers, annotations);
        statusMessage = "Answer sent to agent.";
      } catch (error) {
        const detail = errorText(error);
        statusMessage = `resolve_user_question failed: ${detail}`;
        appendAgentError("Question response failed", detail, "question-error");
      }
      const { [questionId]: _drop, ...rest } = turnQuestionLookup;
      turnQuestionLookup = rest;
    } else {
      statusMessage = "Answer selected (no in-flight turn).";
    }
  }

  function appendLive(item: TimelineItem) {
    liveStreamItems = [...liveStreamItems, item];
  }

  function errorText(error: unknown): string {
    return error instanceof Error ? error.message : String(error);
  }

  function appendAgentError(title: string, body: string, code: string) {
    const trimmed = body.trim() || "Unknown error.";
    appendLive({
      id: `live-error-${code}-${Date.now()}`,
      kind: "system",
      title,
      summary: trimmed,
      body: trimmed,
      meta: ["error", code],
      status: "error"
    });
  }

  function upsertStreamingAssistant(delta: string) {
    const last = liveStreamItems[liveStreamItems.length - 1];
    if (last && last.kind === "assistant" && last.id.startsWith("live-stream-assistant")) {
      const updated = { ...last, body: last.body + delta, summary: last.body + delta };
      liveStreamItems = [...liveStreamItems.slice(0, -1), updated];
    } else {
      appendLive({
        id: `live-stream-assistant-${Date.now()}`,
        kind: "assistant",
        title: "Assistant",
        summary: delta,
        body: delta,
        meta: []
      });
    }
  }

  function handleSessionEvent(sid: string, ev: SessionStreamEvent) {
    if (!selectedSession || selectedSession.id !== sid) return;
    switch (ev.type) {
      case "turn-start":
        currentTurnId = ev.turnId;
        turnStartedAtMs = Date.now();
        turnThinking = true;
        turnStatusHint = "Thinking";
        liveStreamItems = [];
        break;
      case "thinking-delta":
        currentTurnId = ev.turnId;
        turnThinking = true;
        turnStatusHint = "Thinking";
        break;
      case "text-delta":
        currentTurnId = ev.turnId;
        turnThinking = false;
        turnStatusHint = null;
        upsertStreamingAssistant(ev.delta);
        break;
      case "tool-calls-requested":
        currentTurnId = ev.turnId;
        turnThinking = false;
        turnStatusHint = "Running tools";
        // Render an immediate pending card per requested call so the user
        // sees *what* the agent is doing before it finishes. The id is
        // `live-tool-<callId>` — we replace in place when `tool-invocations`
        // arrives with the matching callId.
        for (const req of ev.requests) {
          const id = `live-tool-${req.callId}`;
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
        currentTurnId = ev.turnId;
        turnThinking = false;
        turnStatusHint = null;
        for (const inv of ev.invocations) {
          const id = `live-tool-${inv.callId}`;
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
        currentTurnId = ev.turnId;
        turnThinking = true;
        turnStatusHint = "Thinking";
        break;
      case "retry-attempt":
        currentTurnId = ev.turnId;
        turnThinking = true;
        turnStatusHint = `Retrying ${ev.attempt}/${ev.maxAttempts}`;
        break;
      case "usage":
        currentTurnId = ev.turnId;
        break;
      case "permission-request": {
        currentTurnId = ev.turnId;
        turnThinking = false;
        turnStatusHint = "Awaiting approval";
        const id = `live-perm-${ev.requestId}`;
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
        currentTurnId = ev.turnId;
        turnThinking = false;
        turnStatusHint = "Waiting for answer";
        const id = `live-question-${ev.requestId}`;
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
        currentTurnId = null;
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
          const preservedErrorItems = liveStreamItems.filter(
            (item) => item.kind === "system" && item.meta.includes("error")
          );
          void openSession(sessionToRefresh, {
            showLoading: false,
            resetLiveState: false
          }).then(() => {
            // Preserve a turn-error placeholder so the user can still
            // read the failure after the persisted transcript reloads.
            liveStreamItems = preservedErrorItems;
            submittedMessages = [];
          });
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
          activeAgentId={selectedSession?.id ?? null}
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
                turnStartedAtMs={turnStartedAtMs}
                turnThinking={turnThinking}
                turnStatusHint={turnStatusHint}
                settingsSnapshot={settingsSnapshot}
                userDisplayName={tweaks.userName}
                onBack={onCloseAgent}
                onSubmitMessage={submitMessage}
                onResolvePermission={resolvePermission}
                onResolveUserQuestion={resolveUserQuestion}
                onCancelTurn={() => { if (currentTurnId) void cancelTurn(currentTurnId); }}
                onRenameTitle={renameSelectedSession}
              />
            {:else if openProjectId && sortedGroups.find((g) => g.id === openProjectId)}
              <ProjectDetail
                group={sortedGroups.find((g) => g.id === openProjectId)!}
                pinnedAgentIds={desktopPins.pinnedAgentIds}
                onBack={() => (openProjectId = null)}
                onOpenAgent={(id) => onOpenAgent(id)}
                onNewAgent={(cwd) => requestNewAgent(cwd)}
              />
            {:else}
              <Workspace
                groups={sortedGroups}
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
    onSwitched={async (hs) => {
      showWorkspacePicker = false;
      // Daemon has swapped — reload the default workspace + groups so the
      // UI reflects the new session store.
      defaultWorkspaceCwd = hs.workspaceRoot;
      selectedSession = null;
      sessionDetail = null;
      openAgentSessionId = null;
      openProjectId = null;
      submittedMessages = [];
      liveStreamItems = [];
      turnPermissionLookup = {};
      turnQuestionLookup = {};
      currentTurnId = null;
      turnStartedAtMs = null;
      turnThinking = false;
      turnStatusHint = null;
      await refreshPins();
      await refreshGroups();
      statusMessage = `Switched workspace to ${hs.workspaceRoot}.`;
    }}
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
      if (!newSessionCwd) return;
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

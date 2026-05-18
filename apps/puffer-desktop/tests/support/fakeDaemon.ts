import type { Page, WebSocketRoute } from "@playwright/test";

export const FAKE_DAEMON_URL = "ws://127.0.0.1:17777/ws";

type JsonRecord = Record<string, unknown>;

type DaemonRequest = {
  id: number;
  method: string;
  params: JsonRecord;
};

type Waiter = {
  method: string;
  predicate: (request: DaemonRequest) => boolean;
  resolve: (request: DaemonRequest) => void;
};

type TabSet = {
  activeTabId: string | null;
  tabs: JsonRecord[];
};

const ONE_PIXEL_PNG =
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAFgwJ/lzTnGQAAAABJRU5ErkJggg==";

const now = Date.now();

const session = {
  sessionId: "session-browser",
  displayName: "Browser regression",
  generatedTitle: null,
  title: "Browser regression",
  cwd: "/tmp/puffer",
  folderPath: "/tmp/puffer",
  updatedAtMs: now,
  createdAtMs: now - 60_000,
  eventCount: 3,
  slug: "browser-regression",
  tags: ["desktop", "browser"],
  note: "Automated desktop browser regression fixture",
  parentSessionId: null,
  providerId: "codex",
  modelId: "test-model"
};

function response(id: number, result: unknown): string {
  return JSON.stringify({ type: "response", id, ok: true, result });
}

function failure(id: number, error: string): string {
  return JSON.stringify({ type: "response", id, ok: false, error });
}

function browserTabInfo(tabId: string, url = "about:blank", active = true): JsonRecord {
  return {
    tabId,
    label: tabId === "tab-1" ? "New tab" : "Fixture tab",
    url,
    title: url === "about:blank" ? "" : "Fixture page",
    loading: false,
    connected: true,
    active,
    backendSessionId: `${session.sessionId}:browser:${tabId}`,
    createdAtMs: now,
    updatedAtMs: Date.now()
  };
}

function browserState(url = "about:blank"): JsonRecord {
  return {
    url,
    title: url === "about:blank" ? "" : "Fixture page",
    loading: false,
    width: 960,
    height: 720
  };
}

export class FakeDaemon {
  readonly requests: DaemonRequest[] = [];
  private readonly sockets = new Set<WebSocketRoute>();
  private readonly waiters: Waiter[] = [];
  private readonly browserTabs = new Map<string, TabSet>();
  private nextTab = 2;

  async install(page: Page): Promise<void> {
    await page.routeWebSocket(FAKE_DAEMON_URL, (socket) => {
      this.sockets.add(socket);
      socket.onMessage((message) => this.handleMessage(socket, String(message)));
      socket.onClose(() => {
        this.sockets.delete(socket);
      });
    });
  }

  async open(page: Page): Promise<void> {
    await page.goto(
      `/?skipOnboarding&corbinaBackend=${encodeURIComponent(FAKE_DAEMON_URL)}&corbinaToken=test`
    );
  }

  emit(event: string, payload: unknown): void {
    const message = JSON.stringify({ type: "event", event, payload });
    for (const socket of this.sockets) socket.send(message);
  }

  waitForRequest(
    method: string,
    predicate: (request: DaemonRequest) => boolean = () => true
  ): Promise<DaemonRequest> {
    const existing = this.requests.find((request) => request.method === method && predicate(request));
    if (existing) return Promise.resolve(existing);
    return new Promise((resolve) => {
      this.waiters.push({ method, predicate, resolve });
    });
  }

  private handleMessage(socket: WebSocketRoute, raw: string): void {
    let message: JsonRecord;
    try {
      message = JSON.parse(raw) as JsonRecord;
    } catch {
      return;
    }
    if (message.type !== "request" || typeof message.id !== "number") return;

    const request: DaemonRequest = {
      id: message.id,
      method: String(message.method ?? ""),
      params: typeof message.params === "object" && message.params !== null
        ? (message.params as JsonRecord)
        : {}
    };
    this.record(request);

    try {
      socket.send(response(request.id, this.dispatch(request)));
    } catch (error) {
      socket.send(failure(request.id, String(error)));
    }
  }

  private record(request: DaemonRequest): void {
    this.requests.push(request);
    for (let index = 0; index < this.waiters.length; index += 1) {
      const waiter = this.waiters[index];
      if (waiter.method === request.method && waiter.predicate(request)) {
        this.waiters.splice(index, 1);
        waiter.resolve(request);
        return;
      }
    }
  }

  private dispatch(request: DaemonRequest): unknown {
    switch (request.method) {
      case "default_workspace":
        return { cwd: "/tmp/puffer", workspaceRoot: "/tmp/puffer" };
      case "load_settings_snapshot":
        return this.settingsSnapshot();
      case "list_external_credentials":
        return [];
      case "load_desktop_pins":
        return { pinnedAgentIds: [], pinnedWorkspacePaths: [] };
      case "list_grouped_sessions":
        return [
          {
            folderId: "/tmp/puffer",
            folderLabel: "puffer",
            folderPath: "/tmp/puffer",
            sessionCount: 1,
            sessions: [session]
          }
        ];
      case "load_session_detail":
        return this.sessionDetail();
      case "browser_agent":
        return this.browserAgent(request.params);
      case "browser_open":
        return this.openBrowser(request.params);
      case "browser_navigate":
        return this.navigateBrowser(request.params);
      case "browser_reload":
      case "browser_history":
      case "browser_resize":
      case "browser_input":
        return {};
      case "browser_cursor":
        return { cursor: "text" };
      case "browser_copy_selection":
        return { text: "selected fixture text", copiedFrom: String(request.params.sessionId ?? "") };
      case "browser_close":
        return {};
      case "browser_recording":
        return { frames: [] };
      default:
        throw new Error(`Unhandled fake daemon method: ${request.method}`);
    }
  }

  private settingsSnapshot(): JsonRecord {
    return {
      workspaceRoot: "/tmp/puffer",
      workspaceConfigFile: "/tmp/puffer/.puffer/config.json",
      userConfigFile: "/tmp/home/.puffer/config.json",
      authStoreFile: "/tmp/puffer/.puffer/auth.json",
      builtinResourcesDir: "/tmp/puffer/resources",
      config: {
        appName: "Puffer Code",
        defaultProvider: "codex",
        defaultModel: "test-model",
        openaiBaseUrl: null,
        theme: "system",
        mascotId: "puffer",
        mascotDisplayName: "Puffer",
        mascotEnabled: true,
        uiNoAltScreen: false,
        uiTmuxGoldenMode: false
      },
      resources: {
        providers: 1,
        tools: 1,
        agents: 0,
        prompts: 0,
        hooks: 0,
        skills: 0,
        mascots: 1,
        plugins: 0,
        mcpServers: 1,
        ides: 0
      },
      sessions: { totalSessions: 1, folderGroups: 1 },
      auth: [
        {
          providerId: "codex",
          kind: "oauth",
          email: "tester@example.com",
          expiresAtMs: null,
          scopes: [],
          planType: "test",
          organizationName: null
        }
      ],
      providers: [
        {
          id: "codex",
          displayName: "Codex",
          baseUrl: "",
          defaultApi: "responses",
          modelCount: 1,
          authModes: ["oauth"],
          sourceKind: "test",
          sourcePath: null
        }
      ]
    };
  }

  private sessionDetail(): JsonRecord {
    return {
      ...session,
      timeline: [
        {
          kind: "user_message",
          id: "msg-user",
          text: "Open the browser tab.",
          createdAtMs: now - 30_000
        },
        {
          kind: "assistant_message",
          id: "msg-assistant",
          text: "Ready to exercise the managed browser.",
          createdAtMs: now - 20_000
        }
      ],
      latestDiff: null,
      diffHistory: [],
      repoStatus: {
        sessionId: session.sessionId,
        cwd: session.cwd,
        repoRoot: session.folderPath,
        branch: "codex/desktop-gui-e2e-fixes",
        headSha: "abcdef0",
        isClean: true,
        statusLines: [],
        hasGh: false,
        ghAuthenticated: false,
        canCreatePullRequest: false,
        canMergePullRequest: false,
        createPullRequestReason: "gh unavailable in tests",
        mergePullRequestReason: "gh unavailable in tests",
        openPullRequest: null,
        warnings: []
      },
      agentDiff: { files: [], entries: [] },
      divergence: { agentOnly: [], gitOnly: [], agentTotal: 0, gitTotal: 0 }
    };
  }

  private browserAgent(params: JsonRecord): unknown {
    const action = String(params.action ?? "list");
    const sessionId = String(params.sessionId ?? session.sessionId);
    if (action === "list") return this.tabState(sessionId);
    if (action === "focus") {
      const tabId = String(params.tabId ?? "tab-1");
      const set = this.tabSet(sessionId);
      set.activeTabId = tabId;
      this.refreshActiveFlags(set);
      return set.tabs.find((tab) => tab.tabId === tabId) ?? browserTabInfo(tabId);
    }
    if (action === "close") {
      const tabId = String(params.tabId ?? "tab-1");
      const set = this.tabSet(sessionId);
      set.tabs = set.tabs.filter((tab) => tab.tabId !== tabId);
      set.activeTabId = (set.tabs[0]?.tabId as string | undefined) ?? null;
      this.refreshActiveFlags(set);
      return this.tabState(sessionId);
    }
    if (action === "open") {
      const set = this.tabSet(sessionId);
      if (typeof params.tabId !== "string" && set.tabs.length > 0) {
        return set.tabs.find((tab) => tab.active === true) ?? set.tabs[0];
      }
      const tabId = typeof params.tabId === "string" ? params.tabId : `t${this.nextTab++}`;
      return this.upsertTab(sessionId, browserTabInfo(tabId, String(params.url ?? "about:blank")));
    }
    throw new Error(`Unhandled browser_agent action: ${action}`);
  }

  private openBrowser(params: JsonRecord): unknown {
    const sessionId = String(params.sessionId ?? "");
    const url = String(params.url ?? "about:blank");
    this.recordBrowserOpen(sessionId, url);
    queueMicrotask(() => {
      this.emit(`browser:${sessionId}:frame`, {
        frameId: "frame-1",
        mimeType: "image/png",
        encoding: "base64",
        data: ONE_PIXEL_PNG,
        width: 960,
        height: 720
      });
    });
    return browserState(url);
  }

  private tabSet(sessionId: string): TabSet {
    const existing = this.browserTabs.get(sessionId);
    if (existing) return existing;
    const created: TabSet = { activeTabId: null, tabs: [] };
    this.browserTabs.set(sessionId, created);
    return created;
  }

  private tabState(sessionId: string): JsonRecord {
    const set = this.tabSet(sessionId);
    return { activeTabId: set.activeTabId, tabs: set.tabs };
  }

  private upsertTab(sessionId: string, tab: JsonRecord): JsonRecord {
    const set = this.tabSet(sessionId);
    set.tabs = [...set.tabs.filter((item) => item.tabId !== tab.tabId), tab];
    set.activeTabId = String(tab.tabId);
    this.refreshActiveFlags(set);
    return tab;
  }

  private refreshActiveFlags(set: TabSet): void {
    set.tabs = set.tabs.map((tab) => ({
      ...tab,
      active: tab.tabId === set.activeTabId
    }));
  }

  private recordBrowserOpen(backendSessionId: string, url: string): void {
    const marker = ":browser:";
    const markerIndex = backendSessionId.indexOf(marker);
    if (markerIndex === -1) return;
    const rootSessionId = backendSessionId.slice(0, markerIndex);
    const tabId = backendSessionId.slice(markerIndex + marker.length);
    if (!rootSessionId || !tabId) return;
    this.upsertTab(rootSessionId, browserTabInfo(tabId, url));
  }

  private navigateBrowser(params: JsonRecord): unknown {
    const sessionId = String(params.sessionId ?? "");
    const rawUrl = String(params.url ?? "about:blank");
    const url = rawUrl.includes("://") || rawUrl === "about:blank" ? rawUrl : `https://${rawUrl}`;
    queueMicrotask(() => {
      this.emit(`browser:${sessionId}:state`, browserState(url));
    });
    return {};
  }
}

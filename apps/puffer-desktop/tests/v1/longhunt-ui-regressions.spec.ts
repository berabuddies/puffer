import { expect, type Page, test, type WebSocketRoute } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";

type RequestMessage = {
  id: string | number;
  method: string;
  params?: Record<string, unknown>;
};

const ONE_PIXEL_PNG =
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAFgwJ/lzTnGQAAAABJRU5ErkJggg==";

const baseTime = Date.now();
const strictDaemonUrl = "ws://127.0.0.1:18881/ws";

const openAiProvider = {
  id: "openai",
  displayName: "OpenAI",
  baseUrl: "",
  defaultApi: "openai-responses",
  modelCount: 1,
  authModes: ["oauth", "api_key"],
  sourceKind: "test",
  sourcePath: null
};

const anthropicProvider = {
  id: "anthropic",
  displayName: "Anthropic",
  baseUrl: "",
  defaultApi: "anthropic-messages",
  modelCount: 1,
  authModes: ["api_key"],
  sourceKind: "test",
  sourcePath: null
};

async function openRegressionAgent(page: Page): Promise<void> {
  await page
    .locator(".pf-sidebar-agents-list")
    .getByRole("button", { name: /^Browser regression\b/ })
    .click();
}

async function openBrowserPane(page: Page, daemon: FakeDaemon): Promise<void> {
  await daemon.install(page);
  await daemon.open(page);
  await openRegressionAgent(page);
  await page.locator(".pf-agent-tabs").getByRole("button", { name: "Browser", exact: true }).click();
  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-browser:browser:tab-1"
  );
}

async function openProviderSettings(page: Page): Promise<void> {
  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Providers" }).click();
  await expect(page.getByRole("heading", { name: "Providers" })).toBeVisible();
}

function responseFrame(id: string | number, result: unknown): string {
  return JSON.stringify({ type: "response", id, ok: true, result });
}

function eventFrame(event: string, payload: unknown): string {
  return JSON.stringify({ type: "event", event, payload });
}

class StrictSubscriptionDaemon {
  readonly requests: RequestMessage[] = [];
  readonly sockets = new Set<WebSocketRoute>();
  readonly subscriptions = new Map<WebSocketRoute, Set<string>>();
  rejectConnections = false;
  nextPty = 1;
  ptys: Record<string, unknown>[] = [];
  activePtyId: string | null = null;

  async install(page: Page): Promise<void> {
    const expected = new URL(strictDaemonUrl);
    await page.routeWebSocket((url) => {
      return !this.rejectConnections && url.origin === expected.origin && url.pathname === expected.pathname;
    }, (socket) => {
      this.sockets.add(socket);
      this.subscriptions.set(socket, new Set());
      socket.onMessage((message) => this.handle(socket, String(message)));
      socket.onClose(() => {
        this.sockets.delete(socket);
        this.subscriptions.delete(socket);
      });
    });
  }

  async open(page: Page): Promise<void> {
    await page.goto(`/?skipOnboarding=1&corbinaBackend=${encodeURIComponent(strictDaemonUrl)}&corbinaToken=test`);
  }

  async disconnectAll(): Promise<void> {
    const sockets = [...this.sockets];
    await Promise.all(sockets.map((socket) => socket.close({ code: 1011, reason: "test reconnect" }).catch(() => undefined)));
    for (const socket of sockets) {
      this.sockets.delete(socket);
      this.subscriptions.delete(socket);
    }
  }

  emit(event: string, payload: unknown): void {
    for (const socket of this.sockets) {
      if (this.subscriptions.get(socket)?.has(event)) {
        socket.send(eventFrame(event, payload));
      }
    }
  }

  waitFor(method: string, predicate: (request: RequestMessage) => boolean = () => true): Promise<RequestMessage> {
    const existing = this.requests.find((request) => request.method === method && predicate(request));
    if (existing) return Promise.resolve(existing);
    return new Promise((resolve) => {
      const check = setInterval(() => {
        const request = this.requests.find((item) => item.method === method && predicate(item));
        if (request) {
          clearInterval(check);
          resolve(request);
        }
      }, 10);
    });
  }

  private handle(socket: WebSocketRoute, raw: string): void {
    const message = JSON.parse(raw) as RequestMessage;
    const params = message.params ?? {};
    this.requests.push(message);
    switch (message.method) {
      case "subscribe_event":
        this.subscriptions.get(socket)?.add(String(params.event ?? ""));
        socket.send(responseFrame(message.id, {}));
        return;
      case "unsubscribe_event":
        this.subscriptions.get(socket)?.delete(String(params.event ?? ""));
        socket.send(responseFrame(message.id, {}));
        return;
      case "default_workspace":
        socket.send(responseFrame(message.id, { cwd: "/tmp/puffer", workspaceRoot: "/tmp/puffer" }));
        return;
      case "load_settings_snapshot":
        socket.send(responseFrame(message.id, this.settings()));
        return;
      case "load_desktop_pins":
        socket.send(responseFrame(message.id, { pinnedAgentIds: [], pinnedWorkspacePaths: [] }));
        return;
      case "list_grouped_sessions":
        socket.send(responseFrame(message.id, [{
          folderId: "/tmp/puffer",
          folderLabel: "puffer",
          folderPath: "/tmp/puffer",
          sessionCount: 1,
          sessions: [this.session()]
        }]));
        return;
      case "load_session_detail":
        socket.send(responseFrame(message.id, {
          session: this.session(),
          timeline: [],
          latestDiff: null,
          diffHistory: [],
          repoStatus: null,
          agentDiff: { files: [], entries: [] },
          divergence: { agentOnly: [], gitOnly: [], agentTotal: 0, gitTotal: 0 }
        }));
        return;
      case "pty_list":
        socket.send(responseFrame(message.id, {
          initialized: this.ptys.length > 0,
          activePtyId: this.activePtyId,
          tabs: this.ptys
        }));
        return;
      case "pty_open":
        this.openPty(socket, message.id, params);
        return;
      case "pty_focus":
        this.activePtyId = String(params.ptyId ?? "");
        this.ptys = this.ptys.map((tab) => ({ ...tab, active: tab.ptyId === this.activePtyId }));
        socket.send(responseFrame(message.id, {}));
        return;
      case "pty_close":
        this.ptys = this.ptys.filter((tab) => tab.ptyId !== params.ptyId);
        this.activePtyId = (this.ptys[0]?.ptyId as string | null) ?? null;
        socket.send(responseFrame(message.id, {}));
        return;
      case "pty_resize":
      case "pty_write":
      case "pty_replay":
        socket.send(responseFrame(message.id, message.method === "pty_replay" ? { chunks: [] } : {}));
        return;
      default:
        socket.send(JSON.stringify({ type: "response", id: message.id, ok: false, error: `unhandled ${message.method}` }));
    }
  }

  private openPty(socket: WebSocketRoute, id: string | number, params: Record<string, unknown>): void {
    const ptyId = `pty-${this.nextPty++}`;
    this.activePtyId = ptyId;
    this.ptys = this.ptys.map((tab) => ({ ...tab, active: false }));
    this.ptys.push({
      ptyId,
      sessionId: String(params.sessionId ?? "session-terminal-subscription"),
      title: String(params.title ?? `Terminal ${this.nextPty - 1}`),
      cwd: String(params.cwd ?? "/tmp/puffer"),
      cols: Number(params.cols ?? 80),
      rows: Number(params.rows ?? 24),
      createdAtMs: Date.now(),
      active: true
    });
    socket.send(responseFrame(id, { ptyId }));
  }

  private session(): Record<string, unknown> {
    return {
      sessionId: "session-terminal-subscription",
      displayName: "Terminal subscription",
      generatedTitle: null,
      title: "Terminal subscription",
      cwd: "/tmp/puffer",
      folderPath: "/tmp/puffer",
      updatedAtMs: baseTime,
      createdAtMs: baseTime - 60_000,
      eventCount: 0,
      activityStatus: "idle",
      slug: "terminal-subscription",
      tags: [],
      note: null,
      parentSessionId: null,
      providerId: "codex",
      modelId: "test-model"
    };
  }

  private settings(): Record<string, unknown> {
    return {
      workspaceRoot: "/tmp/puffer",
      workspaceConfigFile: "/tmp/puffer/.puffer/config.json",
      userConfigFile: "/tmp/home/.puffer/config.json",
      authStoreFile: "/tmp/puffer/.puffer/auth.json",
      builtinResourcesDir: "/tmp/puffer/resources",
      config: { appName: "Puffer Code", defaultProvider: "codex", defaultModel: "test-model", theme: "system" },
      resources: { providers: 1, tools: 1, agents: 0, prompts: 0, hooks: 0, skills: 0, mascots: 1, plugins: 0, mcpServers: 0, ides: 0 },
      sessions: { totalSessions: 1, folderGroups: 1 },
      auth: [{ providerId: "codex", kind: "oauth", email: "tester@example.com", expiresAtMs: null, scopes: [], planType: "test", organizationName: null }],
      providers: [{ id: "codex", displayName: "Codex", baseUrl: "", defaultApi: "responses", modelCount: 1, authModes: ["oauth"], sourceKind: "test", sourcePath: null }]
    };
  }
}

test("browser state errors leave the address bar editable", async ({ page }) => {
  const daemon = new FakeDaemon();
  await openBrowserPane(page, daemon);

  const address = page.getByLabel("URL");
  await address.fill("https://state-error.example.test");
  await address.press("Enter");
  await daemon.waitForRequest("browser_navigate");
  daemon.emit("browser:session-browser:browser:tab-1:state", {
    url: "about:blank",
    title: "",
    loading: false,
    error: "navigation failed: net::ERR_NAME_NOT_RESOLVED",
    popOut: false
  });

  await expect(address).toBeEnabled();
  await address.fill("https://recovered.example.test");
  await expect(address).toHaveValue("https://recovered.example.test");
});

test("stale recording frames for unknown tabs do not steal input focus", async ({ page }) => {
  const daemon = new FakeDaemon();
  await openBrowserPane(page, daemon);

  daemon.emit("browser:session-browser:recording", {
    frameId: "stale-recording-frame",
    backendSessionId: "session-browser:browser:tab-stale",
    rootSessionId: "session-browser",
    tabId: "tab-stale",
    url: "https://stale-recording.example.test",
    title: "Stale recording tab",
    mimeType: "image/png",
    encoding: "base64",
    data: ONE_PIXEL_PNG,
    width: 960,
    height: 720,
    recordedAtMs: Date.now() - 60_000
  });

  await expect(page.locator(".pf-browser-tab.active")).toContainText("New tab");
  await expect(page.getByLabel("URL")).toHaveValue("about:blank");
  await page.locator(".pf-browser-canvas").click();
  await page.keyboard.type("abc123");
  await daemon.waitForRequest("browser_input", (request) =>
    request.params.sessionId === "session-browser:browser:tab-1"
  );
});

test("stale tab-list pushes do not drop successful open and close actions", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.delayResponse(
    "browser_agent",
    (request) => request.params.action === "open" && request.params.tabId === "tab-2",
    120
  );
  await openBrowserPane(page, daemon);

  await page.getByRole("button", { name: "New tab" }).click();
  await daemon.waitForRequest("browser_agent", (request) => request.params.action === "open");
  daemon.emit("browser:session-browser:tabs", {
    activeTabId: "tab-1",
    tabs: [
      {
        tabId: "tab-1",
        label: "New tab",
        url: "about:blank",
        title: "",
        loading: false,
        connected: true,
        backendSessionId: "session-browser:browser:tab-1",
        updatedAtMs: Date.now()
      }
    ]
  });
  await expect(page.locator(".pf-browser-tab")).toHaveCount(2);

  daemon.delayResponse(
    "browser_agent",
    (request) => request.params.action === "close" && request.params.tabId === "tab-2",
    120
  );
  await page.getByRole("button", { name: /^Close tab 2:/ }).click();
  await daemon.waitForRequest("browser_agent", (request) => request.params.action === "close");
  daemon.emit("browser:session-browser:tabs", {
    activeTabId: "tab-1",
    tabs: [
      {
        tabId: "tab-1",
        label: "New tab",
        url: "about:blank",
        title: "",
        loading: false,
        connected: true,
        backendSessionId: "session-browser:browser:tab-1",
        updatedAtMs: Date.now()
      },
      {
        tabId: "tab-2",
        label: "New tab",
        url: "about:blank",
        title: "",
        loading: false,
        connected: true,
        backendSessionId: "session-browser:browser:tab-2",
        updatedAtMs: Date.now() - 1_000
      }
    ]
  });
  await expect(page.locator(".pf-browser-tab")).toHaveCount(1);
});

test("duplicate question text preserves each visible answer", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  daemon.emit("session:session-browser:event", {
    type: "user-question-request",
    turnId: "turn-question-duplicate-submit",
    requestId: "question-duplicate-submit",
    questions: [
      {
        header: "Source",
        question: "Which path should I use?",
        options: [
          { label: "src", description: "Use the source directory." },
          { label: "tests", description: "Use the test directory." }
        ]
      },
      {
        header: "Destination",
        question: "Which path should I use?",
        options: [
          { label: "docs", description: "Use documentation." },
          { label: "examples", description: "Use examples." }
        ]
      }
    ]
  });

  const blocks = page.locator(".pf-question-block");
  await blocks.nth(0).locator(".pf-question-option").filter({ hasText: "src" }).click();
  await blocks.nth(1).locator(".pf-question-option").filter({ hasText: "examples" }).click();
  await page.getByRole("button", { name: "Send answer" }).click();

  const request = await daemon.waitForRequest("resolve_user_question");
  expect(JSON.stringify(request.params.answers)).toContain("src");
  expect(JSON.stringify(request.params.answers)).toContain("examples");
});

test("workspace turn completion clears active running state before transcript reload", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-workspace-complete-before-stream",
        displayName: "Workspace complete before stream",
        title: "Workspace complete before stream",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: /Workspace complete before stream/ }).first().click();
  const composer = page.locator(".pf-composer textarea");
  await composer.fill("complete from workspace event");
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await daemon.waitForRequest("run_agent_turn");
  await expect(page.getByRole("button", { name: "Stop turn" })).toBeVisible();

  daemon.delayResponse("load_session_detail", () => true, 400);
  daemon.emit("workspace:sessions:changed", {
    sessionId: "session-workspace-complete-before-stream",
    reason: "turn_complete"
  });

  await expect(page.getByRole("button", { name: "Stop turn" })).toHaveCount(0);
  await expect(composer).toBeEnabled();
});

test("stop disables pending permission approval controls", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.delayResponse("cancel_turn", () => true, 800);
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await page.locator(".pf-composer textarea").fill("run a tool and wait");
  await page.getByRole("button", { name: "Send" }).click();
  await daemon.waitForRequest("run_agent_turn");

  daemon.emit("session:session-browser:event", {
    type: "permission-request",
    turnId: "turn-session-browser",
    requestId: "perm-1",
    toolId: "bash",
    summary: "Run rm -rf /tmp/nope",
    reason: "Needs shell access"
  });
  const allowOnce = page.getByRole("button", { name: "Allow once" });
  await expect(allowOnce).toBeEnabled();

  await page.getByRole("button", { name: "Stop turn" }).click();
  await daemon.waitForRequest("cancel_turn");
  await expect(allowOnce).toBeDisabled();
});

test("file save success preserves edits typed while save is in flight", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.delayResponse("write_file", (request) => request.params.path === "/tmp/puffer/src/main.rs", 500);
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await page.locator(".pf-agent-tabs").getByRole("button", { name: "Files", exact: true }).click();
  const editor = page.getByLabel("Edit file contents");
  const savedDraft = "fn main() {\n    println!(\"first save\");\n}\n";
  const laterDraft = "fn main() {\n    println!(\"first save\");\n    println!(\"typed during save\");\n}\n";
  await editor.fill(savedDraft);
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await daemon.waitForRequest("write_file");
  await editor.fill(laterDraft);
  await expect(editor).toHaveValue(laterDraft);
  await page.waitForTimeout(700);
  await expect(editor).toHaveValue(laterDraft);
  await expect(page.locator(".file-tab.active .dirty-dot")).toBeVisible();
});

test("settings provider credential success stays in provider settings", async ({ page }) => {
  const daemon = new FakeDaemon({
    auth: [],
    providers: [openAiProvider, anthropicProvider],
    externalCredentials: [
      {
        providerId: "anthropic",
        source: "claude",
        sourcePath: "/home/tester/.claude/.credentials.json",
        kind: "api_key"
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page, { allowUnauthenticatedWorkspace: true });
  await openProviderSettings(page);

  const openAiCard = page.locator(".provider-card").filter({ hasText: "OpenAI" });
  await openAiCard.getByLabel("API key for OpenAI").fill("sk-openai-longhunt");
  await openAiCard.getByRole("button", { name: "Connect", exact: true }).click();
  await daemon.waitForRequest("login_with_api_key", (request) => request.params.providerId === "openai");
  await expect(page.getByRole("heading", { name: "Providers" })).toBeVisible();

  const anthropicCard = page.locator(".provider-card").filter({ hasText: "Anthropic" });
  await anthropicCard.getByRole("button", { name: /Use credentials from/ }).click();
  await daemon.waitForRequest("import_external_credential", (request) => request.params.providerId === "anthropic");
  await expect(page.getByRole("heading", { name: "Providers" })).toBeVisible();
});

test("terminal PTY data subscription is restored after websocket reconnect", async ({ page }) => {
  const daemon = new StrictSubscriptionDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: /Terminal subscription/ }).first().click();
  await page.locator(".pf-agent-tabs").getByRole("button", { name: "Terminal", exact: true }).click();
  await daemon.waitFor("pty_open");
  await daemon.waitFor("subscribe_event", (request) => request.params?.event === "pty:pty-1:data");

  daemon.emit("pty:pty-1:data", { seq: 1, data: Buffer.from("before reconnect\n", "utf8").toString("base64") });
  await expect(page.locator(".xterm-rows")).toContainText("before reconnect");

  await daemon.disconnectAll();
  await page.locator(".pf-terminal-host").click();
  await page.keyboard.type("x");
  await daemon.waitFor("pty_write");
  await daemon.waitFor("subscribe_event", (request) => request.params?.event === "pty:pty-1:data");

  daemon.emit("pty:pty-1:data", { seq: 2, data: Buffer.from("after reconnect\n", "utf8").toString("base64") });
  await expect(page.locator(".xterm-rows")).toContainText("after reconnect");
});

test("in-flight permission responses stay disabled across session round trips", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-permission-roundtrip-a",
        displayName: "Permission roundtrip A",
        title: "Permission roundtrip A",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0
      },
      {
        sessionId: "session-permission-roundtrip-b",
        displayName: "Permission roundtrip B",
        title: "Permission roundtrip B",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        eventCount: 0
      }
    ]
  });
  daemon.delayResponse("resolve_permission", () => true, 500);
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: /Permission roundtrip A/ }).first().click();
  daemon.emit("session:session-permission-roundtrip-a:event", {
    type: "permission-request",
    turnId: "turn-permission-roundtrip",
    requestId: "perm-roundtrip",
    toolId: "bash",
    summary: "Run duplicate-sensitive command",
    reason: "Needs one approval only."
  });
  await page.getByRole("button", { name: "Allow once" }).click();
  await daemon.waitForRequest("resolve_permission");

  await page.getByRole("button", { name: /Permission roundtrip B/ }).first().click();
  await page.getByRole("button", { name: /Permission roundtrip A/ }).first().click();
  await expect(page.getByRole("button", { name: "Allow once" })).toBeDisabled();
  await page.waitForTimeout(80);
  expect(daemon.requests.filter((request) => request.method === "resolve_permission")).toHaveLength(1);
});

test("in-flight question responses stay disabled across session round trips", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-question-roundtrip-a",
        displayName: "Question roundtrip A",
        title: "Question roundtrip A",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0
      },
      {
        sessionId: "session-question-roundtrip-b",
        displayName: "Question roundtrip B",
        title: "Question roundtrip B",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        eventCount: 0
      }
    ]
  });
  daemon.delayResponse("resolve_user_question", () => true, 500);
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: /Question roundtrip A/ }).first().click();
  daemon.emit("session:session-question-roundtrip-a:event", {
    type: "user-question-request",
    turnId: "turn-question-roundtrip",
    requestId: "question-roundtrip",
    questions: [
      {
        header: "Path",
        question: "Which path?",
        options: [{ label: "src", description: "Source" }]
      }
    ]
  });
  await page.locator(".pf-question-option").filter({ hasText: "src" }).click();
  await page.getByRole("button", { name: "Send answer" }).click();
  await daemon.waitForRequest("resolve_user_question");

  await page.getByRole("button", { name: /Question roundtrip B/ }).first().click();
  await page.getByRole("button", { name: /Question roundtrip A/ }).first().click();
  await expect(page.getByRole("button", { name: "Send answer" })).toBeDisabled();
  await page.waitForTimeout(80);
  expect(daemon.requests.filter((request) => request.method === "resolve_user_question")).toHaveLength(1);
});

test("hidden turn-start failures are shown when returning to the session", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-hidden-start-fail-a",
        displayName: "Hidden start fail A",
        title: "Hidden start fail A",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0
      },
      {
        sessionId: "session-hidden-start-fail-b",
        displayName: "Hidden start fail B",
        title: "Hidden start fail B",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        eventCount: 0
      }
    ]
  });
  daemon.delayFailure(
    "run_agent_turn",
    (request) => request.params.sessionId === "session-hidden-start-fail-a",
    "queued turn rejected",
    120
  );
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: /Hidden start fail A/ }).first().click();
  await page.locator(".pf-composer textarea").fill("fail while hidden");
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await daemon.waitForRequest("run_agent_turn");
  await page.getByRole("button", { name: /Hidden start fail B/ }).first().click();
  await page.waitForTimeout(180);
  await page.getByRole("button", { name: /Hidden start fail A/ }).first().click();

  await expect(page.getByText("Agent start failed")).toBeVisible();
  await expect(page.getByText("queued turn rejected")).toBeVisible();
});

test("submitted prompt survives reload while turn start is pending", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-reload-pending-prompt",
        displayName: "Reload pending prompt",
        title: "Reload pending prompt",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0
      }
    ]
  });
  daemon.delayResponse("run_agent_turn", () => true, 1_000);
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: /Reload pending prompt/ }).first().click();
  await page.locator(".pf-composer textarea").fill("reload should keep this prompt");
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await daemon.waitForRequest("run_agent_turn");

  await page.reload({ waitUntil: "domcontentloaded" });
  await page.getByRole("button", { name: /Reload pending prompt/ }).first().click();
  await expect(page.getByText("reload should keep this prompt")).toBeVisible();
});

test("browser Ctrl+L releases the remote Control modifier", async ({ page }) => {
  const daemon = new FakeDaemon();
  await openBrowserPane(page, daemon);

  await page.locator(".pf-browser-canvas").click();
  const before = daemon.requests.length;
  await page.keyboard.press("Control+L");
  await expect(page.getByLabel("URL")).toBeFocused();
  const inputs = daemon.requests
    .slice(before)
    .filter((request) => request.method === "browser_input")
    .map((request) => request.params.event as Record<string, unknown>);
  expect(inputs.some((event) => event.eventType === "keyUp" && event.key === "Control")).toBe(true);
});

test("pending credential import disables default model save", async ({ page }) => {
  const daemon = new FakeDaemon({
    auth: [],
    providers: [openAiProvider, anthropicProvider],
    externalCredentials: [
      {
        providerId: "anthropic",
        source: "claude",
        sourcePath: "/home/tester/.claude/.credentials.json",
        kind: "api_key"
      }
    ]
  });
  daemon.delayResponse("import_external_credential", () => true, 500);
  await daemon.install(page);
  await daemon.open(page, { allowUnauthenticatedWorkspace: true });
  await openProviderSettings(page);

  const anthropicCard = page.locator(".provider-card").filter({ hasText: "Anthropic" });
  await anthropicCard.getByRole("button", { name: /Use credentials from/ }).click();
  await daemon.waitForRequest("import_external_credential");
  await expect(page.getByRole("button", { name: "Save default" })).toBeDisabled();
});

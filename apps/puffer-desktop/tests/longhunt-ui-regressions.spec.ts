import { expect, type Page, test } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";

const ONE_PIXEL_PNG =
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAFgwJ/lzTnGQAAAABJRU5ErkJggg==";

const baseTime = Date.now();

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

import { expect, type Page, test } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";

async function openRegressionAgent(page: Page): Promise<void> {
  await page
    .locator(".pf-sidebar-agents-list")
    .getByRole("button", { name: /^Browser regression\b/ })
    .click();
}

async function openAgentPanel(page: Page, name: "Browser" | "Files"): Promise<void> {
  await page.locator(".pf-agent-tabs").getByRole("button", { name, exact: true }).click();
}

function browserTab(tabId: string, url = `https://${tabId}.example`, connected = true): Record<string, unknown> {
  return {
    tabId,
    label: `Fuzz ${tabId}`,
    url,
    title: `Fuzz ${tabId}`,
    loading: false,
    connected,
    active: false,
    backendSessionId: `session-browser:browser:${tabId}`,
    createdAtMs: Date.now(),
    updatedAtMs: Date.now()
  };
}

function browserTabForSession(
  sessionId: string,
  tabId: string,
  url = `https://${tabId}.example`,
  connected = true
): Record<string, unknown> {
  return {
    ...browserTab(tabId, url, connected),
    backendSessionId: `${sessionId}:browser:${tabId}`
  };
}

const ONE_PIXEL_PNG =
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAFgwJ/lzTnGQAAAABJRU5ErkJggg==";

async function pasteText(page: Page, text: string): Promise<void> {
  await page.evaluate((value) => {
    const data = new DataTransfer();
    data.setData("text/plain", value);
    const canvas = document.querySelector(".pf-browser-canvas");
    canvas?.dispatchEvent(new ClipboardEvent("paste", { bubbles: true, cancelable: true, clipboardData: data }));
  }, text);
}

function invalidBrowserSessionRequests(daemon: FakeDaemon): string[] {
  return daemon.requests
    .filter((request) => request.method.startsWith("browser_"))
    .map((request) => String(request.params.sessionId ?? ""))
    .filter((sessionId) =>
      sessionId.endsWith(":browser:") ||
      sessionId.includes(":browser:missing") ||
      sessionId.includes(":browser:undefined")
    );
}

test("opens the Browser tab against a mocked desktop daemon", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openAgentPanel(page, "Browser");

  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-browser:browser:tab-1"
  );

  await expect(page.getByLabel("URL")).toHaveValue("about:blank");
  await expect(page.locator(".pf-browser-status")).toHaveText("Connected");
  await expect(page.locator(".pf-browser-canvas")).toBeVisible();
  await expect(page.locator(".pf-browser-error")).toHaveCount(0);
});

test("Browser tab event refreshes a connected blank canvas", async ({ page }) => {
  const daemon = new FakeDaemon({
    emitBrowserOpenFrame: false,
    emitBrowserResizeFrame: true
  });
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openAgentPanel(page, "Browser");
  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-browser:browser:tab-1"
  );

  daemon.emit("browser:session-browser:tabs", {
    activeTabId: "tab-1",
    tabs: [{ ...browserTab("tab-1", "https://agent.example"), active: true }]
  });

  await daemon.waitForRequest("browser_resize", (request) =>
    request.params.sessionId === "session-browser:browser:tab-1"
  );
  await expect.poll(async () =>
    page.locator(".pf-browser-canvas").evaluate((node) => (node as HTMLCanvasElement).width)
  ).toBe(960);
});

test("Browser panel coalesces rapid screencast frames to the newest frame", async ({ page }) => {
  const daemon = new FakeDaemon({
    emitBrowserOpenFrame: false,
    emitBrowserResizeFrame: false
  });
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openAgentPanel(page, "Browser");
  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-browser:browser:tab-1"
  );

  for (let index = 0; index < 20; index += 1) {
    daemon.emit("browser:session-browser:browser:tab-1:frame", {
      frameId: `rapid-${index}`,
      mimeType: "image/png",
      encoding: "base64",
      data: ONE_PIXEL_PNG,
      width: 100 + index,
      height: 200 + index
    });
  }

  await expect.poll(async () =>
    page.locator(".pf-browser-canvas").evaluate((node) => (node as HTMLCanvasElement).width)
  ).toBe(119);
  await expect.poll(async () =>
    page.locator(".pf-browser-canvas").evaluate((node) => (node as HTMLCanvasElement).height)
  ).toBe(219);
});

test("Browser panel hydrates a connected agent tab from recorded frames", async ({ page }) => {
  const daemon = new FakeDaemon({
    emitBrowserOpenFrame: false,
    emitBrowserResizeFrame: false
  });
  daemon.setBrowserTabs("session-browser", {
    activeTabId: "tab-1",
    tabs: [{ ...browserTab("tab-1", "https://agent.example"), active: true }]
  });
  daemon.setBrowserRecording("session-browser", [
    {
      frameId: "recorded-agent-frame",
      backendSessionId: "session-browser:browser:tab-1",
      rootSessionId: "session-browser",
      tabId: "tab-1",
      url: "https://agent.example",
      title: "Agent page",
      mimeType: "image/png",
      encoding: "base64",
      data: ONE_PIXEL_PNG,
      width: 333,
      height: 222,
      recordedAtMs: Date.now()
    }
  ]);
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openAgentPanel(page, "Browser");

  await daemon.waitForRequest("browser_agent", (request) =>
    request.params.action === "list" && request.params.sessionId === "session-browser"
  );
  await daemon.waitForRequest("browser_resize", (request) =>
    request.params.sessionId === "session-browser:browser:tab-1"
  );
  await expect(page.getByLabel("URL")).toHaveValue("https://agent.example");
  await expect.poll(async () =>
    page.locator(".pf-browser-canvas").evaluate((node) => (node as HTMLCanvasElement).width)
  ).toBe(333);
  await expect.poll(async () =>
    page.locator(".pf-browser-canvas").evaluate((node) => (node as HTMLCanvasElement).height)
  ).toBe(222);
});

test("Browser panel restores a recorded agent tab when daemon tab state is empty", async ({ page }) => {
  const daemon = new FakeDaemon({
    emitBrowserOpenFrame: false,
    emitBrowserResizeFrame: false
  });
  daemon.setBrowserRecording("session-browser", [
    {
      frameId: "recorded-only-frame",
      backendSessionId: "session-browser:browser:t1",
      rootSessionId: "session-browser",
      tabId: "t1",
      url: "https://recorded.example",
      title: "Recorded page",
      mimeType: "image/png",
      encoding: "base64",
      data: ONE_PIXEL_PNG,
      width: 321,
      height: 210,
      recordedAtMs: Date.now()
    }
  ]);
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openAgentPanel(page, "Browser");

  await daemon.waitForRequest("browser_recording", (request) =>
    request.params.sessionId === "session-browser"
  );
  await expect(page.getByLabel("URL")).toHaveValue("https://recorded.example", { timeout: 1000 });
  await expect.poll(async () =>
    page.locator(".pf-browser-canvas").evaluate((node) => (node as HTMLCanvasElement).width)
  ).toBe(321);
});

test("Browser panel prefers recorded agent frames over stale saved tabs", async ({ page }) => {
  const daemon = new FakeDaemon({
    emitBrowserOpenFrame: false,
    emitBrowserResizeFrame: false
  });
  daemon.setBrowserRecording("session-browser", [
    {
      frameId: "recorded-over-stale-frame",
      backendSessionId: "session-browser:browser:agent-tab",
      rootSessionId: "session-browser",
      tabId: "agent-tab",
      url: "https://agent-recorded.example/results",
      title: "Agent recorded results",
      mimeType: "image/png",
      encoding: "base64",
      data: ONE_PIXEL_PNG,
      width: 418,
      height: 260,
      recordedAtMs: Date.now()
    }
  ]);
  await daemon.install(page);
  await page.addInitScript(() => {
    window.localStorage.setItem(
      "puffer-browser-tabs:session-browser",
      JSON.stringify({
        tabs: [
          {
            id: "stale-tab",
            label: "Stale tab",
            url: "https://stale-saved.example",
            title: "Stale saved tab",
            favicon: ""
          }
        ]
      })
    );
  });
  await daemon.open(page);

  await openRegressionAgent(page);
  await openAgentPanel(page, "Browser");

  await expect.poll(
    () => daemon.requests.some((request) =>
      request.method === "browser_recording" &&
      request.params.sessionId === "session-browser"
    ),
    { timeout: 1000 }
  ).toBe(true);
  await expect(page.getByLabel("URL")).toHaveValue("https://agent-recorded.example/results");
  await expect(page.getByRole("tab", { name: /Agent recorded results/ })).toHaveAttribute(
    "aria-selected",
    "true"
  );
  await expect.poll(async () =>
    page.locator(".pf-browser-canvas").evaluate((node) => (node as HTMLCanvasElement).width)
  ).toBe(418);
});

test("Browser panel adopts a newly recorded agent tab while already open", async ({ page }) => {
  const daemon = new FakeDaemon({
    emitBrowserOpenFrame: false,
    emitBrowserResizeFrame: false
  });
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openAgentPanel(page, "Browser");
  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-browser:browser:tab-1"
  );

  daemon.emit("browser:session-browser:recording", {
    frameId: "agent-created-tab-frame",
    backendSessionId: "session-browser:browser:agent-tab",
    rootSessionId: "session-browser",
    tabId: "agent-tab",
    url: "https://agent-open.example/results",
    title: "Agent opened tab",
    mimeType: "image/png",
    encoding: "base64",
    data: ONE_PIXEL_PNG,
    width: 444,
    height: 333,
    recordedAtMs: Date.now()
  });

  await expect(page.getByRole("tab", { name: /Agent opened tab/ })).toHaveAttribute(
    "aria-selected",
    "true"
  );
  await expect(page.getByLabel("URL")).toHaveValue("https://agent-open.example/results");
  await expect.poll(async () =>
    page.locator(".pf-browser-canvas").evaluate((node) => (node as HTMLCanvasElement).width)
  ).toBe(444);
});

test("Browser controls target daemon-provided backend tab ids", async ({ page }) => {
  const backendSessionId = "browser-worker-opaque-tab-1";
  const daemon = new FakeDaemon({
    emitBrowserOpenFrame: false,
    emitBrowserResizeFrame: false
  });
  daemon.setBrowserTabs("session-browser", {
    activeTabId: "tab-1",
    tabs: [
      {
        ...browserTab("tab-1", "https://agent.example"),
        active: true,
        backendSessionId
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openAgentPanel(page, "Browser");

  await daemon.waitForRequest("browser_resize", (request) =>
    request.params.sessionId === backendSessionId
  );

  const toolbar = page.locator(".pf-browser-toolbar");
  await toolbar.getByRole("button", { name: "Reload" }).click();
  await daemon.waitForRequest("browser_reload", (request) =>
    request.params.sessionId === backendSessionId
  );

  await toolbar.getByRole("button", { name: "Back" }).click();
  await daemon.waitForRequest("browser_history", (request) =>
    request.params.sessionId === backendSessionId && request.params.direction === "back"
  );

  await page.getByLabel("URL").fill("https://example.com/search");
  await page.keyboard.press("Enter");
  await daemon.waitForRequest("browser_navigate", (request) =>
    request.params.sessionId === backendSessionId &&
    request.params.url === "https://example.com/search"
  );

  await page.locator(".pf-browser-canvas").dispatchEvent("pointerdown", {
    clientX: 20,
    clientY: 20,
    pointerId: 5,
    button: 0,
    buttons: 1,
    pointerType: "mouse"
  });
  await daemon.waitForRequest("browser_input", (request) => {
    const event = request.params.event as Record<string, unknown> | undefined;
    return request.params.sessionId === backendSessionId &&
      event?.kind === "mouse" &&
      event.eventType === "mousePressed";
  });
});

test("Browser panel follows agent Browser recording updates for the active tab", async ({ page }) => {
  const daemon = new FakeDaemon({
    emitBrowserResizeFrame: false
  });
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openAgentPanel(page, "Browser");
  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-browser:browser:tab-1"
  );
  await expect.poll(async () =>
    page.locator(".pf-browser-canvas").evaluate((node) => (node as HTMLCanvasElement).width)
  ).toBe(960);

  daemon.emit("browser:session-browser:tabs", {
    activeTabId: "tab-1",
    tabs: [{ ...browserTab("tab-1", "https://google.com"), active: true }]
  });
  daemon.emit("browser:session-browser:recording", {
    frameId: "agent-open-google-frame",
    backendSessionId: "session-browser:browser:tab-1",
    rootSessionId: "session-browser",
    tabId: "tab-1",
    url: "https://google.com",
    title: "Google",
    mimeType: "image/png",
    encoding: "base64",
    data: ONE_PIXEL_PNG,
    width: 444,
    height: 333,
    recordedAtMs: Date.now()
  });

  await expect(page.getByLabel("URL")).toHaveValue("https://google.com");
  await expect.poll(async () =>
    page.locator(".pf-browser-canvas").evaluate((node) => (node as HTMLCanvasElement).width)
  ).toBe(444);
  await expect.poll(async () =>
    page.locator(".pf-browser-canvas").evaluate((node) => (node as HTMLCanvasElement).height)
  ).toBe(333);
});

test("sends Browser tab navigation through the daemon bridge", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openAgentPanel(page, "Browser");
  await daemon.waitForRequest("browser_open");

  await page.getByLabel("URL").fill("example.com");
  await page.getByLabel("URL").press("Enter");

  const request = await daemon.waitForRequest("browser_navigate");
  expect(request.params).toMatchObject({
    sessionId: "session-browser:browser:tab-1",
    url: "example.com"
  });
  await expect(page.getByLabel("URL")).toHaveValue("https://example.com");
});

test("Browser toolbar ignores repeated commands while a request is in flight", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openAgentPanel(page, "Browser");
  await daemon.waitForRequest("browser_open");

  daemon.delayResponse(
    "browser_navigate",
    (request) => request.params.sessionId === "session-browser:browser:tab-1",
    250
  );
  await page.getByLabel("URL").fill("example.com");
  await page.locator(".pf-browser-toolbar").evaluate((form) => {
    form.dispatchEvent(new SubmitEvent("submit", { bubbles: true, cancelable: true }));
    form.dispatchEvent(new SubmitEvent("submit", { bubbles: true, cancelable: true }));
  });

  await daemon.waitForRequest("browser_navigate");
  await expect(page.getByLabel("URL")).toBeDisabled();
  await expect.poll(() =>
    daemon.requests.filter((request) => request.method === "browser_navigate").length
  ).toBe(1);
  await expect(page.getByLabel("URL")).toBeEnabled();

  daemon.delayResponse(
    "browser_reload",
    (request) => request.params.sessionId === "session-browser:browser:tab-1",
    250
  );
  const reload = page.locator(".pf-browser-toolbar").getByRole("button", { name: "Reload" });
  await reload.evaluate((button) => {
    (button as HTMLButtonElement).click();
    (button as HTMLButtonElement).click();
  });

  await daemon.waitForRequest("browser_reload");
  await expect(reload).toBeDisabled();
  await expect.poll(() =>
    daemon.requests.filter((request) => request.method === "browser_reload").length
  ).toBe(1);
  await expect(reload).toBeEnabled();
});

test("late Browser navigation failures stay scoped to the submitted tab", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openAgentPanel(page, "Browser");
  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-browser:browser:tab-1"
  );
  await page.getByRole("button", { name: "New tab" }).click();
  await daemon.waitForRequest("browser_agent", (candidate) =>
    candidate.params.action === "open" && candidate.params.tabId === "tab-2"
  );
  await expect(page.locator(".pf-browser-tab")).toHaveCount(2);

  await page.locator(".pf-browser-tab").nth(0).click();
  await expect(page.getByLabel("URL")).toHaveValue("about:blank");
  daemon.delayFailure(
    "browser_navigate",
    (request) => request.params.sessionId === "session-browser:browser:tab-1",
    "navigation failed after tab switch",
    120
  );
  await page.getByLabel("URL").fill("broken.example");
  await page.getByLabel("URL").press("Enter");
  await daemon.waitForRequest("browser_navigate", (request) =>
    request.params.sessionId === "session-browser:browser:tab-1"
  );

  await page.locator(".pf-browser-tab").nth(1).click();
  await expect(page.getByLabel("URL")).toHaveValue("about:blank");
  await page.waitForTimeout(170);

  await expect(page.getByLabel("URL")).toHaveValue("about:blank");
  await expect(page.locator(".pf-browser-error")).toHaveCount(0);
});

test("renders Browser devtools events from the daemon stream", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openAgentPanel(page, "Browser");
  await daemon.waitForRequest("browser_open");

  await page.getByRole("button", { name: "DevTools" }).click();
  daemon.emit("browser:session-browser:browser:tab-1:devtools", {
    kind: "console",
    level: "log",
    text: "hello from browser fixture"
  });

  await expect(page.getByText("hello from browser fixture")).toBeVisible();
});

test("Browser devtools controls expose selected state", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openAgentPanel(page, "Browser");
  await daemon.waitForRequest("browser_open");

  const devtoolsToggle = page.getByRole("button", { name: "DevTools" });
  await expect(devtoolsToggle).toHaveAttribute("aria-pressed", "false");

  await devtoolsToggle.click();
  await expect(devtoolsToggle).toHaveAttribute("aria-pressed", "true");

  const consoleView = page.getByRole("button", { name: "Console" });
  const networkView = page.getByRole("button", { name: "Network" });
  await expect(consoleView).toHaveAttribute("aria-pressed", "true");
  await expect(networkView).toHaveAttribute("aria-pressed", "false");

  await networkView.click();
  await expect(consoleView).toHaveAttribute("aria-pressed", "false");
  await expect(networkView).toHaveAttribute("aria-pressed", "true");
});

test("late Browser devtools events do not leak into a switched agent", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-alpha-browser",
        displayName: "Alpha browser",
        title: "Alpha browser",
        cwd: "/tmp/puffer-alpha",
        folderPath: "/tmp/puffer-alpha",
        updatedAtMs: Date.now(),
        createdAtMs: Date.now() - 60_000,
        timeline: []
      },
      {
        sessionId: "session-beta-browser",
        displayName: "Beta browser",
        title: "Beta browser",
        cwd: "/tmp/puffer-beta",
        folderPath: "/tmp/puffer-beta",
        updatedAtMs: Date.now() - 1_000,
        createdAtMs: Date.now() - 120_000,
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await page
    .locator(".pf-sidebar-agents-list")
    .getByRole("button", { name: /^Alpha browser\b/ })
    .click();
  await openAgentPanel(page, "Browser");
  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-alpha-browser:browser:tab-1"
  );
  await page.getByRole("button", { name: "DevTools" }).click();

  await page
    .locator(".pf-sidebar-agents-list")
    .getByRole("button", { name: /^Beta browser\b/ })
    .click();
  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-beta-browser:browser:tab-1"
  );

  daemon.emit("browser:session-alpha-browser:browser:tab-1:devtools", {
    kind: "console",
    level: "log",
    text: "late alpha console event"
  });
  daemon.emit("browser:session-beta-browser:browser:tab-1:devtools", {
    kind: "console",
    level: "log",
    text: "current beta console event"
  });

  await expect(page.getByText("current beta console event")).toBeVisible();
  await expect(page.getByText("late alpha console event")).toHaveCount(0);
});

test("Browser state errors disable controls and stop canvas input", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openAgentPanel(page, "Browser");
  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-browser:browser:tab-1"
  );

  daemon.emit("browser:session-browser:browser:tab-1:state", {
    url: "about:blank",
    title: "",
    loading: false,
    error: "cdp socket closed"
  });

  await expect(page.locator(".pf-browser-status")).toHaveText("Chrome error");
  await expect(page.getByLabel("URL")).toBeDisabled();

  const before = daemon.requests.length;
  await page.locator(".pf-browser-canvas").click({ position: { x: 20, y: 20 } });
  await page.waitForTimeout(50);

  expect(
    daemon.requests.slice(before).filter((request) => request.method === "browser_input")
  ).toHaveLength(0);
});

test("dispatches printable Browser keyboard input as key events", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openAgentPanel(page, "Browser");
  await daemon.waitForRequest("browser_open");

  await page.locator(".pf-browser-canvas").click({ position: { x: 20, y: 20 } });
  await page.keyboard.press("a");

  const keyDown = await daemon.waitForRequest("browser_input", (request) => {
    const event = request.params.event as Record<string, unknown> | undefined;
    return event?.kind === "key" && event.eventType === "keyDown" && event.key === "a";
  });
  expect(keyDown.params.event).toMatchObject({
    kind: "key",
    eventType: "keyDown",
    key: "a",
    code: "KeyA",
    text: "a"
  });

  await daemon.waitForRequest("browser_input", (request) => {
    const event = request.params.event as Record<string, unknown> | undefined;
    return event?.kind === "key" && event.eventType === "keyUp" && event.key === "a";
  });
  const textInsertions = daemon.requests.filter((request) => {
    const event = request.params.event as Record<string, unknown> | undefined;
    return request.method === "browser_input" && event?.kind === "text" && event.text === "a";
  });
  expect(textInsertions).toHaveLength(0);
});

test("dispatches Browser Enter and Backspace as non-text key events", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openAgentPanel(page, "Browser");
  await daemon.waitForRequest("browser_open");

  await page.locator(".pf-browser-canvas").click({ position: { x: 20, y: 20 } });
  await page.keyboard.press("Enter");
  await page.keyboard.press("Backspace");

  const enter = await daemon.waitForRequest("browser_input", (request) => {
    const event = request.params.event as Record<string, unknown> | undefined;
    return event?.kind === "key" && event.eventType === "rawKeyDown" && event.key === "Enter";
  });
  expect(enter.params.event).toMatchObject({
    kind: "key",
    eventType: "rawKeyDown",
    key: "Enter",
    code: "Enter"
  });
  expect(enter.params.event).not.toHaveProperty("text");

  const backspace = await daemon.waitForRequest("browser_input", (request) => {
    const event = request.params.event as Record<string, unknown> | undefined;
    return event?.kind === "key" && event.eventType === "rawKeyDown" && event.key === "Backspace";
  });
  expect(backspace.params.event).toMatchObject({
    kind: "key",
    eventType: "rawKeyDown",
    key: "Backspace",
    code: "Backspace"
  });
  expect(backspace.params.event).not.toHaveProperty("text");
});

test("Browser canvas reload shortcut calls daemon reload", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openAgentPanel(page, "Browser");
  await daemon.waitForRequest("browser_open");

  await page.locator(".pf-browser-canvas").focus();
  await page.keyboard.press("Control+R");

  await expect.poll(() =>
    daemon.requests.some((request) => request.method === "browser_reload")
  ).toBe(true);
  const reload = daemon.requests.find((request) => request.method === "browser_reload");
  expect(reload?.params).toMatchObject({
    sessionId: "session-browser:browser:tab-1"
  });
  const forwardedShortcut = daemon.requests.filter((request) => {
    const event = request.params.event as Record<string, unknown> | undefined;
    return request.method === "browser_input" && event?.key === "r";
  });
  expect(forwardedShortcut).toHaveLength(0);
});

test("Browser canvas location shortcut focuses the URL field", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openAgentPanel(page, "Browser");
  await daemon.waitForRequest("browser_open");

  await page.locator(".pf-browser-canvas").focus();
  await page.keyboard.press("Control+L");

  await expect(page.getByLabel("URL")).toBeFocused();
  const forwardedShortcut = daemon.requests.filter((request) => {
    const event = request.params.event as Record<string, unknown> | undefined;
    return request.method === "browser_input" && event?.key === "l";
  });
  expect(forwardedShortcut).toHaveLength(0);
});

test("Browser canvas keeps global find shortcuts while focused", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openAgentPanel(page, "Browser");
  await daemon.waitForRequest("browser_open");

  const canvas = page.locator(".pf-browser-canvas");
  await canvas.focus();
  await expect(canvas).toBeFocused();
  await page.keyboard.press("Control+F");

  await expect(page.getByRole("search", { name: "Find in agent view" })).toHaveCount(0);
  await expect(canvas).toBeFocused();
});

test("Browser canvas close shortcut closes the active tab", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openAgentPanel(page, "Browser");
  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-browser:browser:tab-1"
  );

  await page.locator(".pf-browser-canvas").focus();
  await page.keyboard.press("Control+W");

  await expect.poll(() =>
    daemon.requests.some((request) =>
      request.method === "browser_agent" && request.params.action === "close"
    )
  ).toBe(true);
  const close = daemon.requests.find((request) =>
    request.method === "browser_agent" && request.params.action === "close"
  );
  expect(close?.params).toMatchObject({
    sessionId: "session-browser",
    tabId: "tab-1"
  });
  const forwardedShortcut = daemon.requests.filter((request) => {
    const event = request.params.event as Record<string, unknown> | undefined;
    return request.method === "browser_input" && event?.key === "w";
  });
  expect(forwardedShortcut).toHaveLength(0);
});

test("new Browser tab button creates a distinct daemon tab", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openAgentPanel(page, "Browser");
  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-browser:browser:tab-1"
  );

  await page.getByRole("button", { name: "New tab" }).click();
  const request = await daemon.waitForRequest("browser_agent", (candidate) =>
    candidate.params.action === "open" && candidate.params.tabId === "tab-2"
  );

  expect(request.params).toMatchObject({
    action: "open",
    sessionId: "session-browser",
    tabId: "tab-2",
    activate: true
  });
  await expect(page.locator(".pf-browser-tab")).toHaveCount(2);
});

test("rapid Browser new-tab clicks allocate unique tab ids", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.delayResponse(
    "browser_agent",
    (request) => request.params.action === "open" && typeof request.params.tabId === "string",
    120
  );
  daemon.delayResponse(
    "browser_agent",
    (request) => request.params.action === "open" && typeof request.params.tabId === "string",
    120
  );
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openAgentPanel(page, "Browser");
  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-browser:browser:tab-1"
  );

  const newTab = page.getByRole("button", { name: "New tab" });
  await newTab.click();
  await newTab.click();

  await expect.poll(() =>
    daemon.requests
      .filter((request) => request.method === "browser_agent" && request.params.action === "open")
      .map((request) => request.params.tabId)
  ).toEqual(["tab-2", "tab-3"]);
  await expect(page.locator(".pf-browser-tab")).toHaveCount(3);
});

test("stale empty Browser tab lists do not cancel a pending new tab", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.delayResponse(
    "browser_agent",
    (request) => request.params.action === "open" && request.params.tabId === "tab-2",
    120
  );
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openAgentPanel(page, "Browser");
  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-browser:browser:tab-1"
  );

  await page.getByRole("button", { name: "New tab" }).click();
  await daemon.waitForRequest("browser_agent", (request) =>
    request.params.action === "open" && request.params.tabId === "tab-2"
  );
  daemon.emit("browser:session-browser:tabs", { activeTabId: null, tabs: [] });
  await expect(page.locator(".pf-browser-tab")).toHaveCount(1);

  await page.waitForTimeout(170);
  await expect(page.locator(".pf-browser-tab")).toHaveCount(2);
  await expect(page.locator(".pf-browser-status")).toHaveText("Connected");
});

test("Browser tab close controls name the exact tab they target", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openAgentPanel(page, "Browser");
  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-browser:browser:tab-1"
  );
  await page.getByRole("button", { name: "New tab" }).click();
  await daemon.waitForRequest("browser_agent", (candidate) =>
    candidate.params.action === "open" && candidate.params.tabId === "tab-2"
  );

  await expect(page.getByRole("button", { name: "Close tab", exact: true })).toHaveCount(0);
  const closeControls = page.getByRole("button", { name: /^Close tab \d+:/ });
  await expect(closeControls.first()).toHaveJSProperty("tagName", "BUTTON");
  await page.getByRole("button", { name: "Close tab 2: blank page" }).click();
  await expect(page.locator(".pf-browser-tab")).toHaveCount(1);
});

test("Browser tab close failure keeps the tab retryable", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openAgentPanel(page, "Browser");
  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-browser:browser:tab-1"
  );
  await page.getByRole("button", { name: "New tab" }).click();
  await daemon.waitForRequest("browser_agent", (candidate) =>
    candidate.params.action === "open" && candidate.params.tabId === "tab-2"
  );
  await expect(page.locator(".pf-browser-tab")).toHaveCount(2);

  daemon.failNext("browser_agent", "browser close channel closed");
  await page.getByRole("button", { name: "Close tab" }).nth(1).click();

  const request = await daemon.waitForRequest("browser_agent", (candidate) =>
    candidate.params.action === "close" && candidate.params.tabId === "tab-2"
  );
  expect(request.params.sessionId).toBe("session-browser");
  await expect(page.locator(".pf-browser-tab")).toHaveCount(2);
  await expect(page.getByText(/browser close channel closed/)).toBeVisible();
});

test("Browser tab list event can clear stale tabs", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openAgentPanel(page, "Browser");
  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-browser:browser:tab-1"
  );
  await expect(page.locator(".pf-browser-tab")).toHaveCount(1);
  await page.getByRole("button", { name: "DevTools" }).click();
  await expect(page.locator(".pf-browser-devtools")).toBeVisible();

  daemon.emit("browser:session-browser:tabs", { activeTabId: null, tabs: [] });

  await expect(page.locator(".pf-browser-tab")).toHaveCount(0);
  await expect(page.locator(".pf-browser-status")).toHaveText("No pages");
  await expect(page.locator(".pf-browser-devtools")).toHaveCount(0);
});

test("Browser paste does not send input after tabs are cleared", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openAgentPanel(page, "Browser");
  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-browser:browser:tab-1"
  );

  daemon.emit("browser:session-browser:tabs", { activeTabId: null, tabs: [] });
  await expect(page.locator(".pf-browser-status")).toHaveText("No pages");

  await page.evaluate(() => {
    const data = new DataTransfer();
    data.setData("text/plain", "orphan paste");
    const canvas = document.querySelector(".pf-browser-canvas");
    canvas?.dispatchEvent(new ClipboardEvent("paste", { bubbles: true, cancelable: true, clipboardData: data }));
  });
  await page.waitForTimeout(20);

  expect(daemon.requests.filter((request) => request.method === "browser_input")).toHaveLength(0);
});

test("Browser cursor probe does not run after tabs are cleared", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openAgentPanel(page, "Browser");
  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-browser:browser:tab-1"
  );

  await page.locator(".pf-browser-canvas").dispatchEvent("pointermove", {
    clientX: 20,
    clientY: 20,
    pointerId: 1,
    button: -1,
    buttons: 0,
    pointerType: "mouse"
  });
  daemon.emit("browser:session-browser:tabs", { activeTabId: null, tabs: [] });
  await page.waitForTimeout(90);

  expect(daemon.requests.filter((request) => request.method === "browser_cursor")).toHaveLength(0);
});

test("Browser cleared tabs do not paint late frame images", async ({ page }) => {
  await page.addInitScript(() => {
    const pendingImages: Array<{ onload: (() => void) | null }> = [];
    const originalGetContext = HTMLCanvasElement.prototype.getContext;
    HTMLCanvasElement.prototype.getContext = function patchedGetContext(
      this: HTMLCanvasElement,
      contextId: string,
      options?: CanvasRenderingContext2DSettings
    ) {
      const context = originalGetContext.call(this, contextId, options);
      if (contextId !== "2d" || !context) return context;
      const record = context as CanvasRenderingContext2D & { __pufferDrawWrapped?: boolean };
      if (!record.__pufferDrawWrapped) {
        record.__pufferDrawWrapped = true;
        record.drawImage = (() => {
          window.__pufferBrowserDrawCalls = (window.__pufferBrowserDrawCalls ?? 0) + 1;
        }) as typeof record.drawImage;
      }
      return context;
    } as typeof HTMLCanvasElement.prototype.getContext;
    class DelayedImage {
      onload: (() => void) | null = null;

      set src(_value: string) {
        pendingImages.push(this);
      }
    }
    window.__pufferBrowserDrawCalls = 0;
    window.__flushPufferBrowserImages = () => {
      while (pendingImages.length > 0) {
        pendingImages.shift()?.onload?.();
      }
    };
    Object.defineProperty(window, "Image", { value: DelayedImage });
  });

  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openAgentPanel(page, "Browser");
  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-browser:browser:tab-1"
  );

  daemon.emit("browser:session-browser:browser:tab-1:frame", {
    frameId: "late-frame",
    mimeType: "image/png",
    encoding: "base64",
    data: "unused",
    width: 2,
    height: 2
  });
  daemon.emit("browser:session-browser:tabs", { activeTabId: null, tabs: [] });
  await expect(page.locator(".pf-browser-status")).toHaveText("No pages");

  await page.evaluate(() => window.__flushPufferBrowserImages?.());

  const drawCalls = await page.evaluate(() => window.__pufferBrowserDrawCalls ?? 0);
  expect(drawCalls).toBe(0);
});

test("Browser pointer state resets when tabs clear mid-drag", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openAgentPanel(page, "Browser");
  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-browser:browser:tab-1"
  );

  const canvas = page.locator(".pf-browser-canvas");
  await canvas.dispatchEvent("pointerdown", {
    clientX: 20,
    clientY: 20,
    pointerId: 7,
    button: 0,
    buttons: 1,
    pointerType: "mouse"
  });
  await daemon.waitForRequest("browser_input", (request) => {
    const event = request.params.event as Record<string, unknown> | undefined;
    return event?.kind === "mouse" && event.eventType === "mousePressed";
  });

  daemon.emit("browser:session-browser:tabs", { activeTabId: null, tabs: [] });
  await expect(page.locator(".pf-browser-status")).toHaveText("No pages");
  await page.evaluate(() => {
    window.dispatchEvent(new PointerEvent("pointerup", {
      clientX: 22,
      clientY: 22,
      pointerId: 7,
      button: 0,
      buttons: 0,
      pointerType: "mouse"
    }));
  });

  const tab = browserTab("tab-1", "https://restored.example");
  daemon.emit("browser:session-browser:tabs", { activeTabId: "tab-1", tabs: [{ ...tab, active: true }] });
  await daemon.waitForRequest("browser_resize", (request) =>
    request.params.sessionId === "session-browser:browser:tab-1"
  );
  const previousRequestCount = daemon.requests.length;

  await canvas.dispatchEvent("pointermove", {
    clientX: 42,
    clientY: 42,
    pointerId: 7,
    button: -1,
    buttons: 0,
    pointerType: "mouse"
  });
  await page.waitForTimeout(90);

  const newRequests = daemon.requests.slice(previousRequestCount);
  expect(newRequests.some((request) =>
    request.method === "browser_cursor" &&
    request.params.sessionId === "session-browser:browser:tab-1"
  )).toBe(true);
  const staleDragMoves = newRequests.filter((request) => {
    const event = request.params.event as Record<string, unknown> | undefined;
    return request.method === "browser_input" &&
      event?.kind === "mouse" &&
      event.eventType === "mouseMoved" &&
      event.buttons === 1;
  });
  expect(staleDragMoves).toHaveLength(0);
});

test("Browser pointer release does not move to newly active tab mid drag", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openAgentPanel(page, "Browser");
  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-browser:browser:tab-1"
  );

  const canvas = page.locator(".pf-browser-canvas");
  await canvas.dispatchEvent("pointerdown", {
    clientX: 20,
    clientY: 20,
    pointerId: 9,
    button: 0,
    buttons: 1,
    pointerType: "mouse"
  });
  await daemon.waitForRequest("browser_input", (request) => {
    const event = request.params.event as Record<string, unknown> | undefined;
    return request.params.sessionId === "session-browser:browser:tab-1" &&
      event?.kind === "mouse" &&
      event.eventType === "mousePressed";
  });

  const previousRequestCount = daemon.requests.length;
  daemon.emit("browser:session-browser:tabs", {
    activeTabId: "tab-2",
    tabs: [
      { ...browserTab("tab-1", "https://first.example"), active: false },
      { ...browserTab("tab-2", "https://second.example"), active: true }
    ]
  });
  await daemon.waitForRequest("browser_resize", (request) =>
    request.params.sessionId === "session-browser:browser:tab-2"
  );

  await page.evaluate(() => {
    window.dispatchEvent(new PointerEvent("pointerup", {
      clientX: 26,
      clientY: 26,
      pointerId: 9,
      button: 0,
      buttons: 0,
      pointerType: "mouse"
    }));
  });

  const newRequests = daemon.requests.slice(previousRequestCount);
  const releasedIntoSecondTab = newRequests.filter((request) => {
    const event = request.params.event as Record<string, unknown> | undefined;
    return request.method === "browser_input" &&
      request.params.sessionId === "session-browser:browser:tab-2" &&
      event?.kind === "mouse" &&
      event.eventType === "mouseReleased";
  });
  expect(releasedIntoSecondTab).toHaveLength(0);
});

test("late Browser mouse input failures do not leak into a switched agent", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-alpha-input-fail",
        displayName: "Alpha input fail",
        title: "Alpha input fail",
        cwd: "/tmp/puffer-alpha",
        folderPath: "/tmp/puffer-alpha",
        updatedAtMs: Date.now(),
        createdAtMs: Date.now() - 60_000,
        timeline: []
      },
      {
        sessionId: "session-beta-input-fail",
        displayName: "Beta input fail",
        title: "Beta input fail",
        cwd: "/tmp/puffer-beta",
        folderPath: "/tmp/puffer-beta",
        updatedAtMs: Date.now() - 1_000,
        createdAtMs: Date.now() - 120_000,
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await page
    .locator(".pf-sidebar-agents-list")
    .getByRole("button", { name: /^Alpha input fail\b/ })
    .click();
  await openAgentPanel(page, "Browser");
  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-alpha-input-fail:browser:tab-1"
  );
  daemon.delayFailure(
    "browser_input",
    (request) => {
      const event = request.params.event as Record<string, unknown> | undefined;
      return request.params.sessionId === "session-alpha-input-fail:browser:tab-1" &&
        event?.kind === "mouse" &&
        event.eventType === "mousePressed";
    },
    "mouse input failed after agent switch",
    160
  );

  await page.locator(".pf-browser-canvas").dispatchEvent("pointerdown", {
    clientX: 20,
    clientY: 20,
    pointerId: 13,
    button: 0,
    buttons: 1,
    pointerType: "mouse"
  });
  await daemon.waitForRequest("browser_input", (request) => {
    const event = request.params.event as Record<string, unknown> | undefined;
    return request.params.sessionId === "session-alpha-input-fail:browser:tab-1" &&
      event?.kind === "mouse" &&
      event.eventType === "mousePressed";
  });

  await page
    .locator(".pf-sidebar-agents-list")
    .getByRole("button", { name: /^Beta input fail\b/ })
    .click();
  await daemon.waitForRequest("browser_agent", (request) =>
    request.params.action === "list" &&
    request.params.sessionId === "session-beta-input-fail"
  );
  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-beta-input-fail:browser:tab-1"
  );
  await expect(page.locator(".pf-browser-status")).toHaveText("Connected");

  await page.waitForTimeout(220);
  await expect(page.locator(".pf-browser-error")).toHaveCount(0);
  await expect(page.locator(".pf-browser-status")).toHaveText("Connected");
});

test("late Browser resize failures do not leak into a switched agent", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-alpha-resize-fail",
        displayName: "Alpha resize fail",
        title: "Alpha resize fail",
        cwd: "/tmp/puffer-alpha",
        folderPath: "/tmp/puffer-alpha",
        updatedAtMs: Date.now(),
        createdAtMs: Date.now() - 60_000,
        timeline: []
      },
      {
        sessionId: "session-beta-resize-fail",
        displayName: "Beta resize fail",
        title: "Beta resize fail",
        cwd: "/tmp/puffer-beta",
        folderPath: "/tmp/puffer-beta",
        updatedAtMs: Date.now() - 1_000,
        createdAtMs: Date.now() - 120_000,
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await page
    .locator(".pf-sidebar-agents-list")
    .getByRole("button", { name: /^Alpha resize fail\b/ })
    .click();
  await openAgentPanel(page, "Browser");
  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-alpha-resize-fail:browser:tab-1"
  );
  await expect(page.locator(".pf-browser-status")).toHaveText("Connected");
  daemon.delayFailure(
    "browser_resize",
    (request) => request.params.sessionId === "session-alpha-resize-fail:browser:tab-1",
    "resize failed after agent switch",
    160
  );

  await page.locator(".pf-browser-viewport").evaluate((node) => {
    const viewport = node as HTMLElement;
    viewport.style.height = "260px";
  });
  await daemon.waitForRequest("browser_resize", (request) =>
    request.params.sessionId === "session-alpha-resize-fail:browser:tab-1"
  );

  await page
    .locator(".pf-sidebar-agents-list")
    .getByRole("button", { name: /^Beta resize fail\b/ })
    .click();
  await daemon.waitForRequest("browser_agent", (request) =>
    request.params.action === "list" &&
    request.params.sessionId === "session-beta-resize-fail"
  );
  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-beta-resize-fail:browser:tab-1"
  );
  await expect(page.locator(".pf-browser-status")).toHaveText("Connected");

  await page.waitForTimeout(220);
  await expect(page.locator(".pf-browser-error")).toHaveCount(0);
  await expect(page.locator(".pf-browser-status")).toHaveText("Connected");
});

test("late Browser copy failures do not leak into a switched agent", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-alpha-copy-fail",
        displayName: "Alpha copy fail",
        title: "Alpha copy fail",
        cwd: "/tmp/puffer-alpha",
        folderPath: "/tmp/puffer-alpha",
        updatedAtMs: Date.now(),
        createdAtMs: Date.now() - 60_000,
        timeline: []
      },
      {
        sessionId: "session-beta-copy-fail",
        displayName: "Beta copy fail",
        title: "Beta copy fail",
        cwd: "/tmp/puffer-beta",
        folderPath: "/tmp/puffer-beta",
        updatedAtMs: Date.now() - 1_000,
        createdAtMs: Date.now() - 120_000,
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await page
    .locator(".pf-sidebar-agents-list")
    .getByRole("button", { name: /^Alpha copy fail\b/ })
    .click();
  await openAgentPanel(page, "Browser");
  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-alpha-copy-fail:browser:tab-1"
  );
  daemon.delayFailure(
    "browser_copy_selection",
    (request) => request.params.sessionId === "session-alpha-copy-fail:browser:tab-1",
    "copy failed after agent switch",
    160
  );

  await page.locator(".pf-browser-canvas").focus();
  await page.keyboard.press("Control+C");
  await daemon.waitForRequest("browser_copy_selection", (request) =>
    request.params.sessionId === "session-alpha-copy-fail:browser:tab-1"
  );

  await page
    .locator(".pf-sidebar-agents-list")
    .getByRole("button", { name: /^Beta copy fail\b/ })
    .click();
  await daemon.waitForRequest("browser_agent", (request) =>
    request.params.action === "list" &&
    request.params.sessionId === "session-beta-copy-fail"
  );
  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-beta-copy-fail:browser:tab-1"
  );
  await expect(page.locator(".pf-browser-status")).toHaveText("Connected");

  await page.waitForTimeout(220);
  await expect(page.locator(".pf-browser-error")).toHaveCount(0);
  await expect(page.locator(".pf-browser-status")).toHaveText("Connected");
});

test("Browser fuzz click storm keeps daemon session ids valid", async ({ page }) => {
  const daemon = new FakeDaemon();
  const consoleErrors: string[] = [];
  page.on("pageerror", (error) => consoleErrors.push(error.message));
  page.on("console", (message) => {
    if (message.type() === "error" && !message.text().startsWith("Failed to load resource:")) {
      consoleErrors.push(message.text());
    }
  });
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openAgentPanel(page, "Browser");
  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-browser:browser:tab-1"
  );

  const canvas = page.locator(".pf-browser-canvas");
  for (let step = 0; step < 24; step += 1) {
    const mode = step % 8;
    if (mode === 0) {
      await canvas.click({ position: { x: 18 + step, y: 20 } });
    } else if (mode === 1) {
      await canvas.dispatchEvent("pointermove", {
        clientX: 30 + step,
        clientY: 32,
        pointerId: 1,
        button: -1,
        buttons: 0,
        pointerType: "mouse"
      });
    } else if (mode === 2) {
      await page.keyboard.press("a");
    } else if (mode === 3) {
      await pasteText(page, `paste-${step}`);
    } else if (mode === 4) {
      daemon.emit("browser:session-browser:tabs", { activeTabId: null, tabs: [] });
      await expect(page.locator(".pf-browser-status")).toHaveText("No pages");
    } else if (mode === 5) {
      const tab = browserTab("tab-1", "https://restored.example");
      daemon.emit("browser:session-browser:tabs", { activeTabId: "tab-1", tabs: [{ ...tab, active: true }] });
      await expect(page.getByLabel("URL")).toHaveValue("https://restored.example");
    } else if (mode === 6) {
      await page.getByRole("button", { name: "New tab" }).click();
      await daemon.waitForRequest("browser_agent", (request) =>
        request.params.action === "open" && typeof request.params.tabId === "string"
      );
    } else {
      const tab = browserTab("tab-1", "https://stable.example");
      daemon.emit("browser:session-browser:tabs", { activeTabId: "missing-tab", tabs: [{ ...tab, active: true }] });
      await canvas.dispatchEvent("pointermove", {
        clientX: 44,
        clientY: 44,
        pointerId: 1,
        button: -1,
        buttons: 0,
        pointerType: "mouse"
      });
    }
    await page.waitForTimeout(8);
  }
  await page.waitForTimeout(90);

  expect(invalidBrowserSessionRequests(daemon)).toEqual([]);
  await expect(page.locator(".pf-browser-error")).toHaveCount(0);
  expect(consoleErrors).toEqual([]);
});

test("Browser pane resets daemon tabs when switching agents", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-alpha-browser",
        displayName: "Alpha browser",
        title: "Alpha browser",
        cwd: "/tmp/puffer-alpha",
        folderPath: "/tmp/puffer-alpha",
        updatedAtMs: Date.now(),
        createdAtMs: Date.now() - 60_000,
        timeline: []
      },
      {
        sessionId: "session-beta-browser",
        displayName: "Beta browser",
        title: "Beta browser",
        cwd: "/tmp/puffer-beta",
        folderPath: "/tmp/puffer-beta",
        updatedAtMs: Date.now() - 1_000,
        createdAtMs: Date.now() - 120_000,
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await page
    .locator(".pf-sidebar-agents-list")
    .getByRole("button", { name: /^Alpha browser\b/ })
    .click();
  await openAgentPanel(page, "Browser");
  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-alpha-browser:browser:tab-1"
  );

  daemon.emit("browser:session-alpha-browser:tabs", {
    activeTabId: "tab-alpha",
    tabs: [
      {
        ...browserTabForSession(
          "session-alpha-browser",
          "tab-alpha",
          "https://alpha-only.example"
        ),
        active: true
      }
    ]
  });
  await expect(page.getByLabel("URL")).toHaveValue("https://alpha-only.example");

  await page
    .locator(".pf-sidebar-agents-list")
    .getByRole("button", { name: /^Beta browser\b/ })
    .click();
  await daemon.waitForRequest("browser_agent", (request) =>
    request.params.action === "list" &&
    request.params.sessionId === "session-beta-browser"
  );
  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-beta-browser:browser:tab-1"
  );
  await expect(page.getByLabel("URL")).toHaveValue("about:blank");

  daemon.emit("browser:session-alpha-browser:tabs", {
    activeTabId: "tab-late-alpha",
    tabs: [
      {
        ...browserTabForSession(
          "session-alpha-browser",
          "tab-late-alpha",
          "https://late-alpha.example"
        ),
        active: true
      }
    ]
  });
  await page.waitForTimeout(80);
  await expect(page.getByLabel("URL")).toHaveValue("about:blank");

  await page.getByLabel("URL").fill("beta.example");
  await page.getByLabel("URL").press("Enter");
  await daemon.waitForRequest("browser_navigate", (request) =>
    request.params.sessionId === "session-beta-browser:browser:tab-1" &&
    request.params.url === "beta.example"
  );
});

test("late Browser tab focus failures do not leak into a switched agent", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-alpha-focus",
        displayName: "Alpha focus",
        title: "Alpha focus",
        cwd: "/tmp/puffer-alpha",
        folderPath: "/tmp/puffer-alpha",
        updatedAtMs: Date.now(),
        createdAtMs: Date.now() - 60_000,
        timeline: []
      },
      {
        sessionId: "session-beta-focus",
        displayName: "Beta focus",
        title: "Beta focus",
        cwd: "/tmp/puffer-beta",
        folderPath: "/tmp/puffer-beta",
        updatedAtMs: Date.now() - 1_000,
        createdAtMs: Date.now() - 120_000,
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await page
    .locator(".pf-sidebar-agents-list")
    .getByRole("button", { name: /^Alpha focus\b/ })
    .click();
  await openAgentPanel(page, "Browser");
  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-alpha-focus:browser:tab-1"
  );
  await page.getByRole("button", { name: "New tab" }).click();
  await daemon.waitForRequest("browser_agent", (request) =>
    request.params.action === "open" &&
    request.params.sessionId === "session-alpha-focus" &&
    request.params.tabId === "tab-2"
  );
  await expect(page.locator(".pf-browser-tab")).toHaveCount(2);

  daemon.delayFailure(
    "browser_agent",
    (request) =>
      request.params.action === "focus" &&
      request.params.sessionId === "session-alpha-focus" &&
      request.params.tabId === "tab-1",
    "focus failed after agent switch",
    160
  );
  await page.locator(".pf-browser-tab").nth(0).click();
  await daemon.waitForRequest("browser_agent", (request) =>
    request.params.action === "focus" &&
    request.params.sessionId === "session-alpha-focus" &&
    request.params.tabId === "tab-1"
  );

  await page
    .locator(".pf-sidebar-agents-list")
    .getByRole("button", { name: /^Beta focus\b/ })
    .click();
  await daemon.waitForRequest("browser_agent", (request) =>
    request.params.action === "list" &&
    request.params.sessionId === "session-beta-focus"
  );
  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-beta-focus:browser:tab-1"
  );
  await expect(page.locator(".pf-browser-status")).toHaveText("Connected");

  await page.waitForTimeout(220);
  await expect(page.locator(".pf-browser-error")).toHaveCount(0);
  await expect(page.locator(".pf-browser-status")).toHaveText("Connected");
});

test("late Browser new-tab failures do not leak into a switched agent", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-alpha-newtab-fail",
        displayName: "Alpha newtab fail",
        title: "Alpha newtab fail",
        cwd: "/tmp/puffer-alpha",
        folderPath: "/tmp/puffer-alpha",
        updatedAtMs: Date.now(),
        createdAtMs: Date.now() - 60_000,
        timeline: []
      },
      {
        sessionId: "session-beta-newtab-fail",
        displayName: "Beta newtab fail",
        title: "Beta newtab fail",
        cwd: "/tmp/puffer-beta",
        folderPath: "/tmp/puffer-beta",
        updatedAtMs: Date.now() - 1_000,
        createdAtMs: Date.now() - 120_000,
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await page
    .locator(".pf-sidebar-agents-list")
    .getByRole("button", { name: /^Alpha newtab fail\b/ })
    .click();
  await openAgentPanel(page, "Browser");
  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-alpha-newtab-fail:browser:tab-1"
  );
  daemon.delayFailure(
    "browser_agent",
    (request) =>
      request.params.action === "open" &&
      request.params.sessionId === "session-alpha-newtab-fail" &&
      request.params.tabId === "tab-2",
    "new tab failed after agent switch",
    160
  );
  await page.getByRole("button", { name: "New tab" }).click();
  await daemon.waitForRequest("browser_agent", (request) =>
    request.params.action === "open" &&
    request.params.sessionId === "session-alpha-newtab-fail" &&
    request.params.tabId === "tab-2"
  );

  await page
    .locator(".pf-sidebar-agents-list")
    .getByRole("button", { name: /^Beta newtab fail\b/ })
    .click();
  await daemon.waitForRequest("browser_agent", (request) =>
    request.params.action === "list" &&
    request.params.sessionId === "session-beta-newtab-fail"
  );
  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-beta-newtab-fail:browser:tab-1"
  );
  await expect(page.locator(".pf-browser-status")).toHaveText("Connected");

  await page.waitForTimeout(220);
  await expect(page.locator(".pf-browser-error")).toHaveCount(0);
  await expect(page.locator(".pf-browser-status")).toHaveText("Connected");
});

test("late Browser reload failures do not leak into a switched agent", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-alpha-reload-fail",
        displayName: "Alpha reload fail",
        title: "Alpha reload fail",
        cwd: "/tmp/puffer-alpha",
        folderPath: "/tmp/puffer-alpha",
        updatedAtMs: Date.now(),
        createdAtMs: Date.now() - 60_000,
        timeline: []
      },
      {
        sessionId: "session-beta-reload-fail",
        displayName: "Beta reload fail",
        title: "Beta reload fail",
        cwd: "/tmp/puffer-beta",
        folderPath: "/tmp/puffer-beta",
        updatedAtMs: Date.now() - 1_000,
        createdAtMs: Date.now() - 120_000,
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await page
    .locator(".pf-sidebar-agents-list")
    .getByRole("button", { name: /^Alpha reload fail\b/ })
    .click();
  await openAgentPanel(page, "Browser");
  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-alpha-reload-fail:browser:tab-1"
  );
  daemon.delayFailure(
    "browser_reload",
    (request) => request.params.sessionId === "session-alpha-reload-fail:browser:tab-1",
    "reload failed after agent switch",
    160
  );
  await page.locator(".pf-browser-toolbar").getByRole("button", { name: "Reload" }).click();
  await daemon.waitForRequest("browser_reload", (request) =>
    request.params.sessionId === "session-alpha-reload-fail:browser:tab-1"
  );

  await page
    .locator(".pf-sidebar-agents-list")
    .getByRole("button", { name: /^Beta reload fail\b/ })
    .click();
  await daemon.waitForRequest("browser_agent", (request) =>
    request.params.action === "list" &&
    request.params.sessionId === "session-beta-reload-fail"
  );
  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-beta-reload-fail:browser:tab-1"
  );
  await expect(page.locator(".pf-browser-status")).toHaveText("Connected");

  await page.waitForTimeout(220);
  await expect(page.locator(".pf-browser-error")).toHaveCount(0);
  await expect(page.locator(".pf-browser-status")).toHaveText("Connected");
});

test("late Browser close responses do not overwrite a switched agent", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-alpha-close",
        displayName: "Alpha close",
        title: "Alpha close",
        cwd: "/tmp/puffer-alpha",
        folderPath: "/tmp/puffer-alpha",
        updatedAtMs: Date.now(),
        createdAtMs: Date.now() - 60_000,
        timeline: []
      },
      {
        sessionId: "session-beta-close",
        displayName: "Beta close",
        title: "Beta close",
        cwd: "/tmp/puffer-beta",
        folderPath: "/tmp/puffer-beta",
        updatedAtMs: Date.now() - 1_000,
        createdAtMs: Date.now() - 120_000,
        timeline: []
      }
    ]
  });
  daemon.delayResponse(
    "browser_agent",
    (request) =>
      request.params.action === "close" &&
      request.params.sessionId === "session-alpha-close",
    120
  );
  await daemon.install(page);
  await daemon.open(page);

  await page
    .locator(".pf-sidebar-agents-list")
    .getByRole("button", { name: /^Alpha close\b/ })
    .click();
  await openAgentPanel(page, "Browser");
  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-alpha-close:browser:tab-1"
  );
  await expect(page.locator(".pf-browser-status")).toHaveText("Connected");

  await page.getByRole("button", { name: "Close tab" }).click();
  await daemon.waitForRequest("browser_agent", (request) =>
    request.params.action === "close" &&
    request.params.sessionId === "session-alpha-close"
  );

  await page
    .locator(".pf-sidebar-agents-list")
    .getByRole("button", { name: /^Beta close\b/ })
    .click();
  await daemon.waitForRequest("browser_agent", (request) =>
    request.params.action === "list" &&
    request.params.sessionId === "session-beta-close"
  );
  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-beta-close:browser:tab-1"
  );
  await expect(page.locator(".pf-browser-status")).toHaveText("Connected");
  await expect(page.locator(".pf-browser-tab")).toHaveCount(1);
  await expect(page.getByRole("button", { name: "Close tab" })).toBeEnabled();

  await page.waitForTimeout(170);
  await expect(page.locator(".pf-browser-status")).toHaveText("Connected");
  await expect(page.locator(".pf-browser-tab")).toHaveCount(1);
});

test("streamed assistant text stays visible when completion reload is stale", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await expect(page.getByText("Ready to exercise the managed browser.")).toBeVisible();

  daemon.emit("session:session-browser:event", { type: "turn-start", turnId: "turn-flash" });
  daemon.emit("session:session-browser:event", {
    type: "text-delta",
    turnId: "turn-flash",
    delta: "Streaming answer should not flash"
  });
  await expect(page.getByText("Streaming answer should not flash")).toBeVisible();

  const previousRequestCount = daemon.requests.length;
  daemon.emit("session:session-browser:event", {
    type: "turn-complete",
    turnId: "turn-flash",
    assistantText: "Streaming answer should not flash"
  });
  await daemon.waitForRequest("load_session_detail", (request) =>
    daemon.requests.indexOf(request) >= previousRequestCount
  );
  await page.waitForTimeout(50);

  await expect(page.getByText("Streaming answer should not flash")).toBeVisible();
});

test("completion assistant text appears when no delta was streamed", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await expect(page.getByText("Ready to exercise the managed browser.")).toBeVisible();

  const previousRequestCount = daemon.requests.length;
  daemon.emit("session:session-browser:event", { type: "turn-start", turnId: "turn-final-only" });
  daemon.emit("session:session-browser:event", {
    type: "turn-complete",
    turnId: "turn-final-only",
    assistantText: "Final answer arrived only at completion"
  });
  await daemon.waitForRequest("load_session_detail", (request) =>
    daemon.requests.indexOf(request) >= previousRequestCount
  );
  await page.waitForTimeout(50);

  await expect(page.getByText("Final answer arrived only at completion")).toBeVisible();
});

test("Browser tab list event reconnects when active tab changes", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openAgentPanel(page, "Browser");
  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-browser:browser:tab-1"
  );

  daemon.emit("browser:session-browser:tabs", {
    activeTabId: "tab-2",
    tabs: [
      {
        tabId: "tab-1",
        label: "New tab",
        url: "about:blank",
        title: "",
        loading: false,
        connected: true,
        active: false,
        backendSessionId: "session-browser:browser:tab-1",
        createdAtMs: Date.now(),
        updatedAtMs: Date.now()
      },
      {
        tabId: "tab-2",
        label: "Remote tab",
        url: "https://example.com",
        title: "Remote tab",
        loading: false,
        connected: true,
        active: true,
        backendSessionId: "session-browser:browser:tab-2",
        createdAtMs: Date.now(),
        updatedAtMs: Date.now()
      }
    ]
  });

  await daemon.waitForRequest("browser_resize", (request) =>
    request.params.sessionId === "session-browser:browser:tab-2"
  );
  await expect(page.getByLabel("URL")).toHaveValue("https://example.com");
});

test("Browser tab list ignores active ids missing from the tab set", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openAgentPanel(page, "Browser");
  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-browser:browser:tab-1"
  );
  const previousRequestCount = daemon.requests.length;

  daemon.emit("browser:session-browser:tabs", {
    activeTabId: "missing-tab",
    tabs: [
      {
        tabId: "tab-1",
        label: "Stable tab",
        url: "https://example.com",
        title: "Stable tab",
        loading: false,
        connected: true,
        active: true,
        backendSessionId: "session-browser:browser:tab-1",
        createdAtMs: Date.now(),
        updatedAtMs: Date.now()
      }
    ]
  });
  await page.waitForTimeout(20);

  const newRequests = daemon.requests.slice(previousRequestCount);
  expect(newRequests.map((request) => request.params.sessionId)).not.toContain("session-browser:browser:missing-tab");
  await expect(page.getByLabel("URL")).toHaveValue("https://example.com");
});

test("Browser tab list event reopens disconnected active tab", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openAgentPanel(page, "Browser");
  const firstOpen = await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-browser:browser:tab-1"
  );

  daemon.emit("browser:session-browser:tabs", {
    activeTabId: "tab-1",
    tabs: [
      {
        tabId: "tab-1",
        label: "Recovered tab",
        url: "https://example.com",
        title: "Recovered tab",
        loading: false,
        connected: false,
        active: true,
        backendSessionId: "session-browser:browser:tab-1",
        createdAtMs: Date.now(),
        updatedAtMs: Date.now()
      }
    ]
  });

  const reopen = await daemon.waitForRequest("browser_open", (request) =>
    request.id !== firstOpen.id && request.params.sessionId === "session-browser:browser:tab-1"
  );
  expect(reopen.params).toMatchObject({
    sessionId: "session-browser:browser:tab-1",
    url: "https://example.com"
  });
});

test("Browser navigation controls are disabled while reconnecting but address stays editable", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openAgentPanel(page, "Browser");
  const firstOpen = await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-browser:browser:tab-1"
  );

  daemon.delayResponse(
    "browser_open",
    (request) => request.id !== firstOpen.id && request.params.sessionId === "session-browser:browser:tab-1",
    1000
  );
  daemon.emit("browser:session-browser:tabs", {
    activeTabId: "tab-1",
    tabs: [
      {
        tabId: "tab-1",
        label: "Recovered tab",
        url: "https://example.com",
        title: "Recovered tab",
        loading: false,
        connected: false,
        active: true,
        backendSessionId: "session-browser:browser:tab-1",
        createdAtMs: Date.now(),
        updatedAtMs: Date.now()
      }
    ]
  });

  await expect(page.locator(".pf-browser-status")).toHaveText("Disconnected");
  const toolbar = page.locator(".pf-browser-toolbar");
  await expect(toolbar.getByRole("button", { name: "Back" })).toBeDisabled({ timeout: 250 });
  await expect(toolbar.getByRole("button", { name: "Forward" })).toBeDisabled({ timeout: 250 });
  await expect(toolbar.getByRole("button", { name: "Reload" })).toBeDisabled({ timeout: 250 });
  await expect(page.getByLabel("URL")).toBeEnabled({ timeout: 250 });
});

test("late Browser open responses do not overwrite the active tab", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await page.addInitScript(() => {
    window.localStorage.setItem(
      "puffer-browser-tabs:session-browser",
      JSON.stringify({
        tabs: [
          {
            id: "tab-1",
            label: "Slow tab",
            url: "https://slow.example",
            title: "Slow tab",
            favicon: ""
          },
          {
            id: "tab-2",
            label: "Fast tab",
            url: "https://fast.example",
            title: "Fast tab",
            favicon: ""
          }
        ]
      })
    );
  });
  daemon.delayResponse(
    "browser_open",
    (request) => request.params.sessionId === "session-browser:browser:tab-1",
    120
  );
  await daemon.open(page);

  await openRegressionAgent(page);
  await openAgentPanel(page, "Browser");
  await daemon.waitForRequest(
    "browser_open",
    (request) => request.params.sessionId === "session-browser:browser:tab-1"
  );

  await page.getByRole("tab", { name: /Fast tab/ }).click();
  await daemon.waitForRequest(
    "browser_open",
    (request) => request.params.sessionId === "session-browser:browser:tab-2"
  );
  await expect(page.getByLabel("URL")).toHaveValue("https://fast.example");

  await page.waitForTimeout(160);
  await expect(page.getByLabel("URL")).toHaveValue("https://fast.example");
});

test("Files tab close controls are native buttons", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openAgentPanel(page, "Files");

  await expect(page.getByRole("tab", { name: /main\.rs/ })).toBeVisible();
  await expect(page.getByRole("tab", { name: /lib\.rs/ })).toBeVisible();

  const closeControls = page.getByRole("button", { name: /Close .*\.rs/ });
  await expect(closeControls.first()).toHaveJSProperty("tagName", "BUTTON");
  await page.getByRole("button", { name: "Close src/lib.rs" }).click();
  await expect(page.getByRole("tab", { name: /lib\.rs/ })).toHaveCount(0);
});

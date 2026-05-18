import { expect, test } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";

test("opens the Browser tab against a mocked desktop daemon", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: /Browser regression/ }).click();
  await page.getByRole("button", { name: "Browser" }).click();

  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-browser:browser:tab-1"
  );

  await expect(page.getByLabel("URL")).toHaveValue("about:blank");
  await expect(page.locator(".pf-browser-status")).toHaveText("Connected");
  await expect(page.locator(".pf-browser-canvas")).toBeVisible();
  await expect(page.locator(".pf-browser-error")).toHaveCount(0);
});

test("sends Browser tab navigation through the daemon bridge", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: /Browser regression/ }).click();
  await page.getByRole("button", { name: "Browser" }).click();
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

test("renders Browser devtools events from the daemon stream", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: /Browser regression/ }).click();
  await page.getByRole("button", { name: "Browser" }).click();
  await daemon.waitForRequest("browser_open");

  await page.getByRole("button", { name: "DevTools" }).click();
  daemon.emit("browser:session-browser:browser:tab-1:devtools", {
    kind: "console",
    level: "log",
    text: "hello from browser fixture"
  });

  await expect(page.getByText("hello from browser fixture")).toBeVisible();
});

test("dispatches printable Browser keyboard input as key events", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: /Browser regression/ }).click();
  await page.getByRole("button", { name: "Browser" }).click();
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

test("new Browser tab button creates a distinct daemon tab", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: /Browser regression/ }).click();
  await page.getByRole("button", { name: "Browser" }).click();
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

test("Browser tab close control is a native button", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: /Browser regression/ }).click();
  await page.getByRole("button", { name: "Browser" }).click();
  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-browser:browser:tab-1"
  );
  await page.getByRole("button", { name: "New tab" }).click();
  await daemon.waitForRequest("browser_agent", (candidate) =>
    candidate.params.action === "open" && candidate.params.tabId === "tab-2"
  );

  const closeControls = page.getByRole("button", { name: "Close tab" });
  await expect(closeControls.first()).toHaveJSProperty("tagName", "BUTTON");
  await closeControls.nth(1).click();
  await expect(page.locator(".pf-browser-tab")).toHaveCount(1);
});

test("Browser tab list event can clear stale tabs", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: /Browser regression/ }).click();
  await page.getByRole("button", { name: "Browser" }).click();
  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-browser:browser:tab-1"
  );
  await expect(page.locator(".pf-browser-tab")).toHaveCount(1);

  daemon.emit("browser:session-browser:tabs", { activeTabId: null, tabs: [] });

  await expect(page.locator(".pf-browser-tab")).toHaveCount(0);
  await expect(page.locator(".pf-browser-status")).toHaveText("No pages");
});

test("Browser tab list event reconnects when active tab changes", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: /Browser regression/ }).click();
  await page.getByRole("button", { name: "Browser" }).click();
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

test("Browser tab list event reopens disconnected active tab", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: /Browser regression/ }).click();
  await page.getByRole("button", { name: "Browser" }).click();
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

test("Files tab close controls are native buttons", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: /Browser regression/ }).click();
  await page.getByRole("button", { name: "Files" }).click();

  await expect(page.getByRole("tab", { name: /main\.rs/ })).toBeVisible();
  await expect(page.getByRole("tab", { name: /lib\.rs/ })).toBeVisible();

  const closeControls = page.getByRole("button", { name: /Close .*\.rs/ });
  await expect(closeControls.first()).toHaveJSProperty("tagName", "BUTTON");
  await closeControls.nth(1).click();
  await expect(page.getByRole("tab", { name: /lib\.rs/ })).toHaveCount(0);
});

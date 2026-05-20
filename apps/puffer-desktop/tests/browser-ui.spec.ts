import { expect, type Page, test } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";

async function openBrowserAgent(page: Page): Promise<void> {
  await page.locator(".pf-sidebar-agents-list").getByRole("button", { name: /^Browser regression\b/ }).click();
}

async function openBrowserPane(page: Page, daemon: FakeDaemon): Promise<void> {
  const tabs = page.locator(".pf-agent-tabs");
  await tabs.getByRole("button", { name: "Browser", exact: true }).click();
  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-browser:browser:tab-1"
  );
}

test("Address bar preserves user input when a background state event arrives", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openBrowserAgent(page);
  await openBrowserPane(page, daemon);

  const addressBar = page.locator(".pf-browser-address");
  await expect(addressBar).toBeVisible();

  // Focus the address bar and type a partial URL
  await addressBar.click();
  await addressBar.fill("https://example.com/my-page");

  // Simulate a background state event (e.g., agent navigated the page)
  daemon.emit("browser:session-browser:browser:tab-1:state", {
    url: "https://redirected.example.com/other",
    title: "Redirected page",
    loading: false,
    width: 960,
    height: 720
  });

  // Wait a tick for the event to propagate
  await page.waitForTimeout(50);

  // The address bar should still show the user's typed URL, not the background event's URL
  await expect(addressBar).toHaveValue("https://example.com/my-page");
});

test("Address bar updates after user submits a URL", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openBrowserAgent(page);
  await openBrowserPane(page, daemon);

  const addressBar = page.locator(".pf-browser-address");
  await expect(addressBar).toBeVisible();

  // Type a URL and submit
  await addressBar.click();
  await addressBar.fill("https://example.com/submitted");
  await addressBar.press("Enter");

  // Wait for the navigate request
  await daemon.waitForRequest("browser_navigate", (request) =>
    request.params.url === "https://example.com/submitted"
  );

  // After submit, the address bar should be blurred so state events can update it
  // Simulate the state event from the navigation completing with a redirect
  daemon.emit("browser:session-browser:browser:tab-1:state", {
    url: "https://example.com/submitted/final",
    title: "Final page",
    loading: false,
    width: 960,
    height: 720
  });

  await page.waitForTimeout(50);

  // The address bar should now show the final URL since it was blurred after submit
  await expect(addressBar).toHaveValue("https://example.com/submitted/final");
});

test("Address bar updates when switching tabs even if previously focused", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openBrowserAgent(page);
  await openBrowserPane(page, daemon);

  const addressBar = page.locator(".pf-browser-address");
  await expect(addressBar).toBeVisible();

  // Focus the address bar and type something
  await addressBar.click();
  await addressBar.fill("https://partial-typing.example.com");

  // Open a new tab — clicking the "+" button should blur the address bar
  await page.locator(".pf-browser-tab-add").click();
  await daemon.waitForRequest("browser_agent", (request) =>
    request.params.action === "open" && request.params.tabId === "tab-2"
  );

  // After opening a new tab, the address bar should show the new tab's URL
  await expect(addressBar).toHaveValue("about:blank");
});

test("Status bar shows loading state on reload", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openBrowserAgent(page);
  await openBrowserPane(page, daemon);

  const statusBar = page.locator(".pf-browser-status");
  await expect(statusBar).toContainText("Connected");

  // Click the reload button
  await page.locator("button[title='Reload']").click();
  await daemon.waitForRequest("browser_reload");

  // The status bar should show "Loading"
  await expect(statusBar).toContainText("Loading");
});

test("Status bar shows loading state on back/forward navigation", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openBrowserAgent(page);
  await openBrowserPane(page, daemon);

  const statusBar = page.locator(".pf-browser-status");
  await expect(statusBar).toContainText("Connected");

  // Click the back button
  await page.locator("button[title='Back']").click();
  await daemon.waitForRequest("browser_history");

  // The status bar should show "Loading"
  await expect(statusBar).toContainText("Loading");
});

test("Stale Browser tab list does not clear reload loading feedback", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openBrowserAgent(page);
  await openBrowserPane(page, daemon);

  const statusBar = page.locator(".pf-browser-status");
  await expect(statusBar).toContainText("Connected");

  await page.locator("button[title='Reload']").click();
  await daemon.waitForRequest("browser_reload");
  await expect(statusBar).toContainText("Loading");

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
        active: true,
        backendSessionId: "session-browser:browser:tab-1"
      }
    ]
  });

  await expect(statusBar).toContainText("Loading");
});

test("New browser tab ignores repeated clicks while open is in flight", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openBrowserAgent(page);
  await openBrowserPane(page, daemon);

  daemon.delayResponse(
    "browser_agent",
    (request) => request.params.action === "open" && request.params.tabId === "tab-2",
    500
  );

  const addTab = page.getByRole("button", { name: "New tab" });
  await addTab.click();
  await addTab.click({ force: true });

  await page.waitForTimeout(50);

  const opens = daemon.requests.filter(
    (request) => request.method === "browser_agent" && request.params.action === "open"
  );
  expect(opens).toHaveLength(1);
  await expect(addTab).toBeDisabled();
});

test("Reload loading state recovers when no browser state event follows", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openBrowserAgent(page);
  await openBrowserPane(page, daemon);

  const statusBar = page.locator(".pf-browser-status");
  await expect(statusBar).toContainText("Connected");

  await page.locator("button[title='Reload']").click();
  await daemon.waitForRequest("browser_reload");
  await expect(statusBar).toContainText("Loading");

  await expect(statusBar).toContainText("Connected", { timeout: 2_000 });
});

test("Address bar re-enables and loading clears after failed navigation", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openBrowserAgent(page);
  await openBrowserPane(page, daemon);

  const addressBar = page.locator(".pf-browser-address");
  await expect(addressBar).toBeEnabled();

  daemon.failNext("browser_navigate", "navigation channel closed");
  await addressBar.fill("https://fails.example.com");
  await addressBar.press("Enter");

  await daemon.waitForRequest("browser_navigate", (request) =>
    request.params.url === "https://fails.example.com"
  );
  await expect(addressBar).toBeEnabled();
  await expect(page.locator(".pf-browser-status")).toContainText("Chrome error");
  await expect(page.locator(".pf-browser-status")).not.toHaveClass(/loading/);
  await expect(addressBar).toHaveValue("https://fails.example.com");
});

test("Address bar can reopen a disconnected tab with the typed URL", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openBrowserAgent(page);
  await openBrowserPane(page, daemon);

  daemon.emit("browser:session-browser:tabs", {
    activeTabId: "tab-1",
    tabs: [
      {
        tabId: "tab-1",
        label: "Recovered tab",
        url: "https://old.example.com",
        title: "Recovered tab",
        loading: false,
        connected: false,
        active: true,
        backendSessionId: "session-browser:browser:tab-1"
      }
    ]
  });
  await daemon.waitForRequest("browser_open", (request) =>
    request.params.sessionId === "session-browser:browser:tab-1" &&
    request.params.url === "https://old.example.com"
  );

  const addressBar = page.locator(".pf-browser-address");
  await expect(addressBar).toBeEnabled();
  await addressBar.fill("https://reopen.example.com");
  await addressBar.press("Enter");

  await daemon.waitForRequest("browser_navigate", (request) =>
    request.params.sessionId === "session-browser:browser:tab-1" &&
    request.params.url === "https://reopen.example.com"
  );
});

test("Stale empty tab list is ignored while a new tab is opening", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.delayResponse(
    "browser_agent",
    (request) => request.params.action === "open" && request.params.tabId === "tab-2",
    220
  );
  await daemon.install(page);
  await daemon.open(page);

  await openBrowserAgent(page);
  await openBrowserPane(page, daemon);

  await page.getByRole("button", { name: "New tab" }).click();
  await daemon.waitForRequest("browser_agent", (request) =>
    request.params.action === "open" && request.params.tabId === "tab-2"
  );
  daemon.emit("browser:session-browser:tabs", { activeTabId: null, tabs: [] });

  await expect(page.locator(".pf-browser-tab")).toHaveCount(1);
  await page.waitForTimeout(260);
  await expect(page.locator(".pf-browser-tab")).toHaveCount(2);
  await expect(page.getByLabel("URL")).toBeEnabled();
});

test("Stale tab URL events do not overwrite the active address bar", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openBrowserAgent(page);
  await openBrowserPane(page, daemon);

  const addressBar = page.locator(".pf-browser-address");
  await addressBar.fill("https://current.example.test");
  await addressBar.press("Enter");
  await daemon.waitForRequest("browser_navigate", (request) =>
    request.params.url === "https://current.example.test"
  );
  await expect(addressBar).toHaveValue("https://current.example.test");

  daemon.emit("browser:session-browser:tabs", {
    activeTabId: "tab-1",
    tabs: [
      {
        tabId: "tab-1",
        label: "Old tab",
        url: "https://old.example.test",
        title: "Old tab",
        loading: false,
        connected: true,
        active: true,
        backendSessionId: "session-browser:browser:tab-1",
        createdAtMs: 1,
        updatedAtMs: 1
      }
    ]
  });

  await expect(addressBar).toHaveValue("https://current.example.test");
});

test("Duplicate browser navigate submits are ignored while the URL is pending", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.delayResponse(
    "browser_navigate",
    (request) => request.params.url === "https://dedupe.example.test",
    300
  );
  await daemon.install(page);
  await daemon.open(page);

  await openBrowserAgent(page);
  await openBrowserPane(page, daemon);

  await page.locator(".pf-browser-address").fill("https://dedupe.example.test");
  await page.locator(".pf-browser-toolbar").evaluate((form) => {
    (form as HTMLFormElement).requestSubmit();
    (form as HTMLFormElement).requestSubmit();
  });

  await daemon.waitForRequest("browser_navigate", (request) =>
    request.params.url === "https://dedupe.example.test"
  );
  await page.waitForTimeout(50);
  expect(
    daemon.requests.filter(
      (request) =>
        request.method === "browser_navigate" &&
        request.params.url === "https://dedupe.example.test"
    )
  ).toHaveLength(1);
});

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

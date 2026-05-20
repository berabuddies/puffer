import { readFile } from "node:fs/promises";
import { expect, test } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";

test("Tauri mac shell exposes a drag-only titlebar without duplicate branding", async ({ page }) => {
  const daemon = new FakeDaemon();
  await page.addInitScript(() => {
    Object.defineProperty(window.navigator, "userAgent", {
      get: () => "Mozilla/5.0 (Macintosh; Intel Mac OS X 15_5) AppleWebKit/537.36"
    });
    (globalThis as unknown as { isTauri?: boolean }).isTauri = true;
  });
  await daemon.install(page);
  await daemon.open(page);

  await expect(page.locator("html")).toHaveClass(/is-tauri-mac/);

  const titlebar = page.locator(".pf-titlebar");
  await expect(titlebar).toBeVisible();
  const titlebarBox = await titlebar.boundingBox();
  expect(titlebarBox?.height).toBeGreaterThanOrEqual(44);
  await expect(titlebar).toHaveAttribute("data-tauri-drag-region", "");
  await expect(titlebar.locator(".pf-brand-logo")).toHaveCount(0);
  await expect(titlebar.locator(".pf-titlebar-drag-fill")).toHaveAttribute(
    "data-tauri-drag-region",
    ""
  );

  const sidebarLogo = page.locator(".pf-sidebar .pf-brand-logo").first();
  await expect(page.locator(".pf-sidebar .pf-brand-logo")).toHaveCount(1);
  const logoBox = await sidebarLogo.boundingBox();
  expect(logoBox).not.toBeNull();
  const trafficLightSafeRect = { left: 0, top: 0, right: 88, bottom: 44 };
  const overlapsTrafficLights =
    logoBox!.left < trafficLightSafeRect.right &&
    logoBox!.left + logoBox!.width > trafficLightSafeRect.left &&
    logoBox!.top < trafficLightSafeRect.bottom &&
    logoBox!.top + logoBox!.height > trafficLightSafeRect.top;
  expect(overlapsTrafficLights).toBe(false);

  expect(await page.locator(".pf-sidebar-collapse").evaluate((node) =>
    node.hasAttribute("data-tauri-drag-region")
  )).toBe(false);
});

test("Tauri capability permits data drag regions to move the native window", async () => {
  const raw = await readFile("src-tauri/capabilities/default.json", "utf8");
  const capability = JSON.parse(raw) as { permissions?: string[] };
  expect(capability.permissions).toContain("core:window:allow-start-dragging");
});

test("Playwright does not reuse stale Vite servers in Codex automation", async () => {
  const raw = await readFile("playwright.config.ts", "utf8");
  expect(raw).toContain("process.env.CODEX_CI");
  expect(raw).toContain("reuseExistingServer: shouldReuseExistingServer");
});

test("desktop minimum width keeps primary navigation visible", async ({ page }) => {
  const daemon = new FakeDaemon();
  await page.setViewportSize({ width: 720, height: 480 });
  await daemon.install(page);
  await daemon.open(page);

  const sidebar = page.locator(".pf-sidebar");
  await expect(sidebar).toBeVisible();
  await expect(sidebar.getByRole("button", { name: "Project" })).toBeVisible();
  await expect(sidebar.getByRole("button", { name: "Pipelines" })).toBeVisible();
  await expect(sidebar.getByRole("button", { name: "Deployments" })).toBeVisible();
  await expect(sidebar.getByRole("button", { name: "Settings" })).toBeVisible();
});

test("sidebar primary navigation exposes the current page", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  const sidebar = page.locator(".pf-sidebar");
  const project = sidebar.getByRole("button", { name: "Project" });
  const pipelines = sidebar.getByRole("button", { name: "Pipelines" });
  const settings = sidebar.getByRole("button", { name: "Settings" });

  await expect(project).toHaveAttribute("aria-current", "page");
  await expect(pipelines).not.toHaveAttribute("aria-current", "page");

  await pipelines.click();
  await expect(project).not.toHaveAttribute("aria-current", "page");
  await expect(pipelines).toHaveAttribute("aria-current", "page");

  await settings.click();
  await expect(pipelines).not.toHaveAttribute("aria-current", "page");
  await expect(settings).toHaveAttribute("aria-current", "page");
});

test("desktop user-visible copy uses Puffer branding", async () => {
  const userFacingFiles = [
    "src/App.svelte",
    "src/lib/screens/agent/BrowserPane.svelte",
    "src/lib/screens/agent/FilesPane.svelte",
    "src/lib/screens/agent/TerminalPane.svelte",
    "src/lib/screens/workspace/ConnectProjectModal.svelte"
  ];

  for (const file of userFacingFiles) {
    const source = await readFile(file, "utf8");
    expect(source, file).not.toContain("Corbina");
  }
});

test("sidebar width can be resized and persists as a local shell tweak", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  const sidebar = page.locator(".pf-sidebar");
  const resizer = page.locator(".pf-sidebar-resizer");
  await expect(resizer).toBeVisible();
  await expect(page.getByRole("button", { name: "Adjust navigation size" })).toHaveCount(1);

  const initialBox = await sidebar.boundingBox();
  const handleBox = await resizer.boundingBox();
  expect(initialBox).not.toBeNull();
  expect(handleBox).not.toBeNull();

  await page.mouse.move(handleBox!.x + handleBox!.width / 2, handleBox!.y + 120);
  await page.mouse.down();
  await page.mouse.move(handleBox!.x + handleBox!.width / 2 + 96, handleBox!.y + 120);
  await page.mouse.up();

  await expect
    .poll(async () => Math.round((await sidebar.boundingBox())?.width ?? 0))
    .toBeGreaterThan(Math.round(initialBox!.width + 72));
  const storedWidth = await page.evaluate(() => {
    const raw = window.localStorage.getItem("puffer-desktop:tweaks");
    return raw ? JSON.parse(raw).sidebarWidth : null;
  });
  expect(storedWidth).toBeGreaterThan(initialBox!.width + 72);
});

test("sidebar can open the deployments screen", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  const sidebar = page.locator(".pf-sidebar");
  await sidebar.getByRole("button", { name: "Deployments" }).click();

  await expect(page.locator(".pf-dep")).toBeVisible();
  await expect(page.getByText(/environments/)).toBeVisible();
  await expect(page.getByRole("button", { name: /New deployment/ })).toBeVisible();
});

test("deployment search filters environments and resets from Escape", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Deployments" }).click();
  await page.locator(".pf-dep-top-right").getByRole("button", { name: "Search" }).click();

  const search = page.getByRole("searchbox", { name: "Search deployments" });
  await expect(search).toBeFocused();
  await search.fill("cloudflare");

  const rows = page.locator(".pf-dep-row");
  await expect(rows).toHaveCount(1);
  await expect(rows.first()).toContainText("edge-cdn");
  await expect(page.locator(".pf-dep-detail-name")).toContainText("edge-cdn");

  await search.fill("no-match");
  await expect(rows).toHaveCount(0);
  await expect(page.getByText("No deployments match")).toBeVisible();

  await search.press("Escape");
  await expect(page.getByRole("searchbox", { name: "Search deployments" })).toHaveCount(0);
  await expect(page.locator(".pf-dep-row")).toHaveCount(6);
  await expect(page.locator(".pf-dep-row").filter({ hasText: "stripe-api · production" })).toBeVisible();
});

test("deployment provider sync button reports progress and completion", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Deployments" }).click();
  const syncButton = page.locator(".pf-dep-top-right").getByRole("button", { name: "Sync providers" });
  const syncStatus = page.locator(".pf-dep-sync-status");

  await expect(syncStatus).toHaveCount(0);
  await syncButton.click();

  await expect(syncButton).toBeDisabled();
  await expect(syncButton).toHaveAttribute("aria-busy", "true");
  await expect(syncStatus).toHaveAttribute("role", "status");
  await expect(syncStatus).toContainText("Syncing providers...");
  await expect(syncStatus).toContainText("Providers synced: 6 environments across 4 providers refreshed.");
  await expect(syncButton).toBeEnabled();
  await expect(syncButton).toHaveAttribute("aria-busy", "false");

  const statusBox = await syncStatus.boundingBox();
  const topbarBox = await page.locator(".pf-dep-top").boundingBox();
  expect(statusBox).not.toBeNull();
  expect(topbarBox).not.toBeNull();
  expect(statusBox!.height).toBeLessThanOrEqual(topbarBox!.height);
});

test("deployment new deployment button creates a local draft", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Deployments" }).click();
  const newDeployment = page.locator(".pf-dep-top-right").getByRole("button", { name: "New deployment" });

  await newDeployment.click();
  let dialog = page.getByRole("dialog", { name: "New deployment" });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByLabel("Service name")).toBeFocused();
  await expect(dialog.getByRole("button", { name: "Create deployment" })).toBeDisabled();

  await dialog.press("Escape");
  await expect(dialog).toHaveCount(0);
  await expect(page.locator(".pf-dep-row")).toHaveCount(6);

  await newDeployment.click();
  dialog = page.getByRole("dialog", { name: "New deployment" });
  await dialog.getByLabel("Service name").fill("checkout-worker");
  await dialog.getByLabel("Provider").selectOption("fly");
  await dialog.getByLabel("Environment").selectOption("preview");
  await dialog.getByLabel("Branch").fill("feature/checkouts");
  await expect(dialog.getByText("Draft will appear as checkout-worker · preview.")).toBeVisible();
  await dialog.getByRole("button", { name: "Create deployment" }).click();

  await expect(dialog).toHaveCount(0);
  await expect(newDeployment).toBeFocused();
  await expect(page.locator(".pf-dep-top-title")).toContainText("7 environments");
  const draft = page.locator(".pf-dep-row").filter({ hasText: "checkout-worker · preview" });
  await expect(draft).toBeVisible();
  await expect(draft).toContainText("Fly.io Machines");
  await expect(page.locator(".pf-dep-detail-name")).toContainText("checkout-worker · preview");
  await expect(page.locator(".pf-dep-detail-name")).toContainText(/deploying/i);
  await expect(page.locator(".pf-dep-detail-sub")).toContainText("checkout-worker-preview.puffer.app");
  await expect(page.getByRole("tab", { name: "Deploys" })).toHaveAttribute("aria-selected", "true");
});

test("deployment redeploy controls insert a live deploy history item", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Deployments" }).click();
  const detailHeader = page.locator(".pf-dep-detail-head");
  const redeploy = detailHeader.getByRole("button", { name: "Redeploy" });

  await redeploy.click();

  await expect(page.getByRole("tab", { name: "Deploys" })).toHaveAttribute("aria-selected", "true");
  await expect(redeploy).toBeDisabled();
  await expect(redeploy).toHaveAttribute("aria-busy", "true");
  await expect(detailHeader.getByRole("status")).toContainText("Redeploying stripe-api · production from main.");
  const firstRun = page.locator(".pf-dep-history-row").first();
  await expect(firstRun).toContainText("manual-1430");
  await expect(firstRun).toContainText("Otter");
  await expect(firstRun).toContainText(/deploying/i);

  await expect(detailHeader.getByRole("status")).toContainText("Redeploy complete for stripe-api · production.");
  await expect(redeploy).toBeEnabled();
  await expect(redeploy).toHaveAttribute("aria-busy", "false");
  await expect(firstRun).toContainText(/healthy/i);
  await expect(firstRun).toContainText("0m 12s");

  const trigger = page.getByRole("button", { name: "Trigger deploy" });
  await trigger.click();
  await expect(trigger).toBeDisabled();
  await expect(page.locator(".pf-dep-history-row").first()).toContainText("manual-1431");
});

test("deployment detail tabs expose selected state", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  const sidebar = page.locator(".pf-sidebar");
  await sidebar.getByRole("button", { name: "Deployments" }).click();

  const tabs = page.locator(".pf-dep-tabs");
  await expect(tabs).toHaveAttribute("role", "tablist");
  const askTab = tabs.getByRole("tab", { name: "Ask Puffer" });
  const secretsTab = tabs.getByRole("tab", { name: "Secrets" });
  await expect(askTab).toHaveAttribute("aria-selected", "true");
  await expect(secretsTab).toHaveAttribute("aria-selected", "false");

  await secretsTab.click();
  await expect(askTab).toHaveAttribute("aria-selected", "false");
  await expect(secretsTab).toHaveAttribute("aria-selected", "true");
  await expect(page.getByRole("heading", { name: "Secrets & env" })).toBeVisible();

  await secretsTab.press("ArrowRight");
  const providersTab = tabs.getByRole("tab", { name: "Providers" });
  await expect(providersTab).toHaveAttribute("aria-selected", "true");
  await expect(page.getByRole("heading", { name: "Providers & integrations" })).toBeVisible();

  await providersTab.press("Home");
  await expect(askTab).toHaveAttribute("aria-selected", "true");
});

test("deployment Ask Puffer composer sends prompts from button and Enter", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Deployments" }).click();
  const composer = page.locator(".pf-dep-ask-composer");
  const thread = page.locator(".pf-dep-ask-thread");
  const textbox = composer.getByRole("textbox", { name: "Ask Puffer" });

  await textbox.fill("Check failed deploys");
  await composer.getByRole("button", { name: "Send" }).click();

  await expect(textbox).toHaveValue("");
  await expect(thread.locator('.pf-msg[data-role="user"] .pf-msg-text').filter({ hasText: "Check failed deploys" })).toHaveCount(1);
  await expect(thread).toContainText("I queued an investigation for stripe-api · production: Check failed deploys.");

  await textbox.fill("Summarize logs");
  await textbox.press("Enter");

  await expect(textbox).toHaveValue("");
  await expect(thread.locator('.pf-msg[data-role="user"] .pf-msg-text').filter({ hasText: "Summarize logs" })).toHaveCount(1);
  await expect(thread).toContainText("I queued an investigation for stripe-api · production: Summarize logs.");

  await page.locator(".pf-dep-row").filter({ hasText: "puffer-web · production" }).click();
  await expect(thread).not.toContainText("Check failed deploys");
  await expect(thread).not.toContainText("Summarize logs");
  await expect(textbox).toHaveValue("");
});

test("deployment secret reveal controls target one key and toggle their state", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  const sidebar = page.locator(".pf-sidebar");
  await sidebar.getByRole("button", { name: "Deployments" }).click();
  await page.getByRole("tab", { name: "Secrets" }).click();

  const row = page.locator(".pf-dep-secrets-row").filter({ hasText: "DATABASE_URL" });
  await expect(row).toContainText("••••••••••••••");
  await expect(page.getByRole("button", { name: "Reveal", exact: true })).toHaveCount(0);

  const revealDatabaseUrl = page.getByRole("button", { name: "Reveal DATABASE_URL", exact: true });
  await expect(revealDatabaseUrl).toHaveCount(1);
  await revealDatabaseUrl.click();

  await expect(row).toContainText(/postgres:\/\/.*db\.puffer\.app\/prod/);
  const hideDatabaseUrl = page.getByRole("button", { name: "Hide DATABASE_URL", exact: true });
  await expect(hideDatabaseUrl).toHaveAttribute("aria-pressed", "true");
  await hideDatabaseUrl.click();

  await expect(row).toContainText("••••••••••••••");
  await expect(page.getByRole("button", { name: "Reveal DATABASE_URL", exact: true })).toHaveAttribute(
    "aria-pressed",
    "false"
  );
});

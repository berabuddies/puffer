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

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

import { expect, test } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";

test("Tauri mac shell exposes a drag titlebar without traffic-light overlap", async ({ page }) => {
  const daemon = new FakeDaemon();
  await page.addInitScript(() => {
    Object.defineProperty(window.navigator, "userAgent", {
      get: () => "Mozilla/5.0 (Macintosh; Intel Mac OS X 15_5) AppleWebKit/537.36"
    });
    const win = window as unknown as {
      __TAURI__?: unknown;
      __TAURI_INTERNALS__?: unknown;
    };
    win.__TAURI__ = {};
    win.__TAURI_INTERNALS__ = {};
  });
  await daemon.install(page);
  await daemon.open(page);

  await expect(page.locator("html")).toHaveClass(/is-tauri-mac/);

  const titlebar = page.locator(".pf-titlebar");
  await expect(titlebar).toBeVisible();
  const titlebarBox = await titlebar.boundingBox();
  expect(titlebarBox?.height).toBeGreaterThanOrEqual(38);
  await expect(titlebar).toHaveAttribute("data-tauri-drag-region", "");

  const sidebarLogo = page.locator(".pf-sidebar .pf-brand-logo").first();
  const logoBox = await sidebarLogo.boundingBox();
  expect(logoBox).not.toBeNull();
  const trafficLightSafeRect = { left: 0, top: 0, right: 84, bottom: 36 };
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

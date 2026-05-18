import { expect, type Page, test } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";

async function openRegressionAgent(page: Page): Promise<void> {
  await page
    .locator(".pf-sidebar-agents-list")
    .getByRole("button", { name: /^Browser regression\b/ })
    .click();
}

async function openFilesPanel(page: Page): Promise<void> {
  await page.locator(".pf-agent-tabs").getByRole("button", { name: "Files", exact: true }).click();
}

test("Files tab close button works from the keyboard", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openFilesPanel(page);

  const libTab = page.getByRole("tab", { name: /lib\.rs/ });
  await expect(libTab).toBeVisible();

  await libTab.getByRole("button", { name: "Close lib.rs" }).focus();
  await page.keyboard.press("Enter");

  await expect(page.getByRole("tab", { name: /lib\.rs/ })).toHaveCount(0);
});

test("New agent modal closes with Escape", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "New agent in puffer" }).click();
  const dialog = page.getByRole("dialog", { name: "New agent" });
  await expect(dialog).toBeVisible();

  await page.keyboard.press("Escape");

  await expect(dialog).toHaveCount(0);
});

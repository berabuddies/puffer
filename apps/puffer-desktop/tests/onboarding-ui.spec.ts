import { expect, type Page, test } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";

async function openForcedOnboarding(page: Page): Promise<void> {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page, { forceOnboarding: true, skipOnboarding: false });
}

test("onboarding repo Continue enters the workspace", async ({ page }) => {
  await openForcedOnboarding(page);

  await expect(
    page.getByRole("heading", { name: "Choose the repos Puffer can see" })
  ).toBeVisible();
  await page.getByRole("button", { name: /Continue/ }).click();

  await expect(page.getByRole("button", { name: "New agent in puffer" })).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Choose the repos Puffer can see" })
  ).toHaveCount(0);
});

test("onboarding repo Skip enters the workspace", async ({ page }) => {
  await openForcedOnboarding(page);

  await expect(
    page.getByRole("heading", { name: "Choose the repos Puffer can see" })
  ).toBeVisible();
  await page.getByRole("button", { name: "Skip for now" }).click();

  await expect(page.getByRole("button", { name: "New agent in puffer" })).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Choose the repos Puffer can see" })
  ).toHaveCount(0);
});

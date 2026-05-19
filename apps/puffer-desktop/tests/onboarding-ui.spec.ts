import { expect, type Page, test } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";

async function openForcedOnboarding(page: Page): Promise<void> {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page, { forceOnboarding: true, skipOnboarding: false });
}

test("onboarding Continue enters the workspace", async ({ page }) => {
  await openForcedOnboarding(page);

  await expect(
    page.getByRole("heading", { name: "Workspace is ready" })
  ).toBeVisible();
  await page.getByRole("button", { name: /Continue/ }).click();

  await expect(page.getByRole("button", { name: "New agent in puffer" })).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Workspace is ready" })
  ).toHaveCount(0);
});

test("onboarding does not show fake repository choices", async ({ page }) => {
  await openForcedOnboarding(page);

  await expect(page.getByRole("heading", { name: "Workspace is ready" })).toBeVisible();
  await expect(page.getByText("puffer-web")).toHaveCount(0);
  await expect(page.getByText("stripe-api")).toHaveCount(0);
});

test("skip flag does not bypass provider login when auth is empty", async ({ page }) => {
  const daemon = new FakeDaemon({ auth: [] });
  await daemon.install(page);
  await page.addInitScript(() => {
    window.localStorage.setItem("puffer-desktop:skip-onboarding", "1");
  });
  await daemon.open(page);

  await expect(page.getByLabel("API key for Anthropic")).toBeVisible();
  await expect(page.getByRole("button", { name: "New agent in puffer" })).toHaveCount(0);
});

test("skip flag does not bypass provider login with only non-agent auth", async ({ page }) => {
  const daemon = new FakeDaemon({
    auth: [
      {
        providerId: "github",
        kind: "oauth",
        email: "tester@example.com",
        expiresAtMs: null,
        scopes: [],
        planType: "test",
        organizationName: null
      }
    ],
    providers: [
      {
        id: "github",
        displayName: "GitHub",
        baseUrl: "",
        defaultApi: "oauth",
        modelCount: 0,
        authModes: ["oauth"],
        sourceKind: "test",
        sourcePath: null
      },
      {
        id: "anthropic",
        displayName: "Anthropic",
        baseUrl: "",
        defaultApi: "anthropic-messages",
        modelCount: 1,
        authModes: ["api_key"],
        sourceKind: "test",
        sourcePath: null
      }
    ]
  });
  await daemon.install(page);
  await page.addInitScript(() => {
    window.localStorage.setItem("puffer-desktop:skip-onboarding", "1");
  });
  await daemon.open(page);

  await expect(page.getByLabel("API key for Anthropic")).toBeVisible();
  await expect(page.getByText("Workspace is ready")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "New agent in puffer" })).toHaveCount(0);
});

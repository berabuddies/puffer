import { expect, test } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";

const WORLDROUTER_PROVIDER = {
  id: "worldrouter",
  displayName: "WorldRouter",
  baseUrl: "https://inference-api.worldrouter.ai",
  defaultApi: "openai-completions",
  modelCount: 2,
  authModes: ["api_key", "oauth"],
  sourceKind: "builtin",
  sourcePath: "resources/providers/worldrouter.yaml"
};

const BUILTIN_PROVIDERS = [
  {
    id: "openai",
    displayName: "OpenAI",
    baseUrl: "https://api.openai.com",
    defaultApi: "openai-responses",
    modelCount: 1,
    authModes: ["oauth", "api_key"],
    sourceKind: "builtin",
    sourcePath: "resources/providers/openai.yaml"
  },
  {
    id: "anthropic",
    displayName: "Anthropic",
    baseUrl: "https://api.anthropic.com",
    defaultApi: "anthropic-messages",
    modelCount: 1,
    authModes: ["api_key"],
    sourceKind: "builtin",
    sourcePath: "resources/providers/anthropic.yaml"
  },
  {
    id: "puffer",
    displayName: "Puffer",
    baseUrl: "local-cli://puffer",
    defaultApi: "cli",
    modelCount: 1,
    authModes: ["native", "api_key"],
    sourceKind: "builtin",
    sourcePath: "resources/providers/puffer.yaml"
  },
  WORLDROUTER_PROVIDER
];

test("settings provider page shows WorldRouter card with both auth modes", async ({ page }) => {
  const daemon = new FakeDaemon({
    auth: [],
    providers: [WORLDROUTER_PROVIDER]
  });
  await daemon.install(page);
  await daemon.open(page, { allowUnauthenticatedWorkspace: true });

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Providers" }).click();

  const card = page.locator(".provider-card").filter({ hasText: "WorldRouter" });
  await expect(card).toBeVisible();
  await expect(card.getByRole("heading", { name: "WorldRouter" })).toBeVisible();

  // Both auth modes should be offered (OAuth button + API key input).
  await expect(card.getByRole("button", { name: "Connect with OAuth" })).toBeVisible();
  await expect(card.getByLabel("API key for WorldRouter")).toBeVisible();
});

test("new agent modal lists WorldRouter alongside the other builtin providers", async ({ page }) => {
  const daemon = new FakeDaemon({
    auth: [
      {
        providerId: "openai",
        kind: "oauth",
        email: "tester@example.com",
        expiresAtMs: null,
        scopes: [],
        planType: "test",
        organizationName: null
      },
      {
        providerId: "anthropic",
        kind: "api_key",
        email: null,
        expiresAtMs: null,
        scopes: [],
        planType: null,
        organizationName: null
      },
      {
        providerId: "worldrouter",
        kind: "oauth",
        email: null,
        expiresAtMs: null,
        scopes: [],
        planType: "WorldRouter OAuth",
        organizationName: null
      }
    ],
    providers: BUILTIN_PROVIDERS
  });
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: /^New agent in / }).first().click();
  const dialog = page.getByRole("dialog", { name: "New agent" });
  await expect(dialog).toBeVisible();

  const radioGroup = dialog.locator(".pf-provider-choice");
  await expect(radioGroup.getByRole("radio", { name: /OpenAI/ })).toBeVisible();
  await expect(radioGroup.getByRole("radio", { name: /Anthropic/ })).toBeVisible();
  await expect(radioGroup.getByRole("radio", { name: /Puffer/ })).toBeVisible();
  await expect(radioGroup.getByRole("radio", { name: /WorldRouter/ })).toBeVisible();
  await expect(radioGroup.locator('input[type="radio"]')).toHaveCount(4);

  const worldrouterLabel = radioGroup.locator("label").filter({ hasText: "WorldRouter" });
  await expect(worldrouterLabel.locator(".meta")).toHaveText("WorldRouter inference");
});

test("selecting WorldRouter in the new agent modal toggles the radio without error", async ({ page }) => {
  const daemon = new FakeDaemon({
    auth: [
      {
        providerId: "openai",
        kind: "oauth",
        email: "tester@example.com",
        expiresAtMs: null,
        scopes: [],
        planType: "test",
        organizationName: null
      },
      {
        providerId: "anthropic",
        kind: "api_key",
        email: null,
        expiresAtMs: null,
        scopes: [],
        planType: null,
        organizationName: null
      },
      {
        providerId: "worldrouter",
        kind: "oauth",
        email: null,
        expiresAtMs: null,
        scopes: [],
        planType: "WorldRouter OAuth",
        organizationName: null
      }
    ],
    providers: BUILTIN_PROVIDERS
  });
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: /^New agent in / }).first().click();
  const dialog = page.getByRole("dialog", { name: "New agent" });
  await expect(dialog).toBeVisible();

  const radioGroup = dialog.locator(".pf-provider-choice");
  const worldrouterLabel = radioGroup.locator("label").filter({ hasText: "WorldRouter" });
  const worldrouterRadio = worldrouterLabel.locator('input[type="radio"]');

  await worldrouterLabel.click();

  await expect(worldrouterRadio).toBeChecked();
  await expect(worldrouterLabel).toHaveAttribute("data-active", "true");

  // No error banner should be raised by simply picking the provider.
  await expect(dialog.getByRole("alert")).toHaveCount(0);
  // Start agent should still be enabled (we don't click it: would spawn real session).
  await expect(dialog.getByRole("button", { name: "Start agent" })).toBeEnabled();
});

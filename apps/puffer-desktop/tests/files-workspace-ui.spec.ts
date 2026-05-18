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

test("new agent provider choice is used for the first turn", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "New agent in puffer" }).click();
  const dialog = page.getByRole("dialog", { name: "New agent" });
  await expect(dialog).toBeVisible();
  await dialog.getByRole("radio", { name: /Anthropic/ }).click();
  await dialog.getByRole("button", { name: "Start agent" }).click();

  const createRequest = await daemon.waitForRequest("create_session");
  expect(createRequest.params).toMatchObject({
    providerId: "anthropic"
  });

  await expect(page.locator(".pf-composer textarea")).toBeVisible();
  await page.locator(".pf-composer textarea").fill("Hello from Anthropic");
  await page.getByRole("button", { name: "Send" }).click();

  const turnRequest = await daemon.waitForRequest(
    "run_agent_turn",
    (request) => request.params.message === "Hello from Anthropic"
  );
  expect(turnRequest.params).toMatchObject({
    providerId: "anthropic",
    modelId: "test-model"
  });
});

test("empty workspace can start a new agent in the default workspace", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await daemon.install(page);
  await daemon.open(page);

  await expect(page.getByRole("heading", { name: "No sessions yet" })).toBeVisible();
  await page.getByRole("button", { name: "New agent in default workspace" }).click();

  const dialog = page.getByRole("dialog", { name: "New agent" });
  await expect(dialog).toBeVisible();
  await dialog.getByRole("button", { name: "Start agent" }).click();

  const createRequest = await daemon.waitForRequest("create_session");
  expect(createRequest.params).toMatchObject({
    cwd: "/tmp/puffer",
    providerId: "codex"
  });
});

test("connect project provider choice includes Anthropic", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Connect project" }).click();
  const dialog = page.getByRole("dialog", { name: "Connect project" });
  await expect(dialog).toBeVisible();

  await expect(dialog.getByRole("radio", { name: "Anthropic" })).toBeVisible();
  await dialog.getByRole("radio", { name: "Anthropic" }).click();
  await dialog.getByLabel("Directory").fill("/tmp/puffer-new-project");
  await dialog.getByRole("button", { name: "Start agent" }).click();

  const createRequest = await daemon.waitForRequest("create_session");
  expect(createRequest.params).toMatchObject({
    cwd: "/tmp/puffer-new-project",
    providerId: "anthropic"
  });
});

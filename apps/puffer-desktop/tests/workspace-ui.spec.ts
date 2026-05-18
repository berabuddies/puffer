import { expect, test } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";

const baseTime = Date.now();

test("workspace search filters projects and agents", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-alpha",
        displayName: "Alpha planner",
        title: "Alpha planner",
        cwd: "/tmp/puffer-alpha",
        folderPath: "/tmp/puffer-alpha",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 2
      },
      {
        sessionId: "session-beta",
        displayName: "Beta browser audit",
        title: "Beta browser audit",
        cwd: "/tmp/puffer-beta",
        folderPath: "/tmp/puffer-beta",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        eventCount: 4
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  const workspace = page.locator(".pf-pw-list");
  await expect(workspace.getByText("puffer-alpha")).toBeVisible();
  await expect(workspace.getByText("puffer-beta")).toBeVisible();

  await page.getByLabel("Search workspace").fill("beta browser");
  await expect(workspace.getByText("Beta browser audit")).toBeVisible();
  await expect(workspace.getByText("puffer-beta")).toBeVisible();
  await expect(workspace.getByText("Alpha planner")).toHaveCount(0);
  await expect(workspace.getByText("puffer-alpha")).toHaveCount(0);

  await page.getByLabel("Search workspace").fill("missing session");
  await expect(workspace.getByText("No workspace results")).toBeVisible();
  await page.getByRole("button", { name: "Clear search" }).click();
  await expect(workspace.getByText("Alpha planner")).toBeVisible();
  await expect(workspace.getByText("Beta browser audit")).toBeVisible();
});

test("project memory edit control is disabled until file editing is wired", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-pw-project").getByRole("button", { name: "Details" }).click();
  await page.getByRole("button", { name: /Memory/ }).click();

  const memoryDetail = page.locator(".pf-pmem-detail");
  await expect(memoryDetail.getByRole("button", { name: "Edit" })).toBeDisabled();
});

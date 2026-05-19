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

test("workspace search includes older sessions beyond the first page", async ({ page }) => {
  const sessions = Array.from({ length: 7 }, (_, index) => ({
    sessionId: `session-history-${index}`,
    displayName: index === 6 ? "Deep history session" : `Recent session ${index + 1}`,
    title: index === 6 ? "Deep history session" : `Recent session ${index + 1}`,
    cwd: "/tmp/puffer-history",
    folderPath: "/tmp/puffer-history",
    updatedAtMs: baseTime - index * 1_000,
    createdAtMs: baseTime - 60_000 - index * 1_000,
    eventCount: index === 6 ? 12 : 1
  }));
  const daemon = new FakeDaemon({ sessions });
  await daemon.install(page);
  await daemon.open(page);

  const project = page.locator(".pf-pw-project").filter({ hasText: "puffer-history" });
  await expect(project).toContainText("7 sessions");
  await expect(project.getByText("Deep history session")).toBeVisible();

  await page.getByLabel("Search workspace").fill("deep history");
  await expect(project.getByText("Deep history session")).toBeVisible();
  await project.getByRole("button", { name: /Deep history session/ }).click();
  await expect(page.locator(".pf-composer textarea")).toBeVisible();
});

test("project memory includes older sessions beyond the first page", async ({ page }) => {
  const sessions = Array.from({ length: 7 }, (_, index) => ({
    sessionId: `session-memory-${index}`,
    displayName: index === 6 ? "Deep memory session" : `Memory session ${index + 1}`,
    title: index === 6 ? "Deep memory session" : `Memory session ${index + 1}`,
    cwd: "/tmp/puffer-memory",
    folderPath: "/tmp/puffer-memory",
    updatedAtMs: baseTime - index * 1_000,
    createdAtMs: baseTime - 60_000 - index * 1_000,
    eventCount: index === 6 ? 9 : 1
  }));
  const daemon = new FakeDaemon({ sessions });
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-pw-project").filter({ hasText: "puffer-memory" })
    .getByRole("button", { name: "Details" })
    .click();
  await page.locator(".pf-fpb-tab").filter({ hasText: "Memory" }).click();

  const memoryPanel = page.locator(".pf-pmem");
  await expect(memoryPanel.getByText("session-7.md")).toBeVisible();
  await memoryPanel.getByRole("button", { name: /session-7\.md/ }).click();
  await expect(page.locator(".pf-pmem-title")).toHaveText("Deep memory session");
});

test("sidebar Workspace returns from agent detail to the workspace board", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page
    .locator(".pf-sidebar-agents-list")
    .getByRole("button", { name: /Browser regression/ })
    .click();
  await expect(page.locator(".pf-agent-detail")).toBeVisible();

  await page.locator(".pf-sidebar").getByRole("button", { name: "Workspace" }).click();

  await expect(page.locator(".pf-pw-list")).toBeVisible();
  await expect(page.locator(".pf-agent-detail")).toHaveCount(0);
  await expect(page.locator(".pf-pw-project").filter({ hasText: "puffer" })).toBeVisible();
});

test("workspace board renders daemon session activity states", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-running",
        displayName: "Running checkout fix",
        title: "Running checkout fix",
        cwd: "/tmp/puffer-active",
        folderPath: "/tmp/puffer-active",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 3,
        activityStatus: "running"
      },
      {
        sessionId: "session-awaiting",
        displayName: "Awaiting deploy approval",
        title: "Awaiting deploy approval",
        cwd: "/tmp/puffer-active",
        folderPath: "/tmp/puffer-active",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        eventCount: 5,
        activityStatus: "awaiting"
      },
      {
        sessionId: "session-idle",
        displayName: "Idle docs followup",
        title: "Idle docs followup",
        cwd: "/tmp/puffer-active",
        folderPath: "/tmp/puffer-active",
        updatedAtMs: baseTime - 2_000,
        createdAtMs: baseTime - 180_000,
        eventCount: 2,
        activityStatus: "idle"
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  const project = page.locator(".pf-pw-project").filter({ hasText: "puffer-active" });
  await expect(project).toContainText("2 active");

  await expect(
    page.locator(".pf-sidebar-agent-row").filter({ hasText: "Running checkout fix" })
  ).toContainText("running");
  await expect(
    page.locator(".pf-sidebar-agent-row").filter({ hasText: "Awaiting deploy approval" })
  ).toContainText("awaiting");

  await project.getByRole("button", { name: "Details" }).click();
  const runningColumn = page.locator(".pf-fpb-col").filter({ hasText: "Running" });
  await expect(runningColumn.getByText("Running checkout fix")).toBeVisible();
  await expect(runningColumn.getByText("Awaiting deploy approval")).toBeVisible();

  const queuedColumn = page.locator(".pf-fpb-col").filter({ hasText: "Queued" });
  await expect(queuedColumn.getByText("Idle docs followup")).toBeVisible();
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

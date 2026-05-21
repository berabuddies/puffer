import { expect, test } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";

const baseTime = Date.now();

test("PLUS-121: active agents are grouped by project and collapse on header click", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-alpha-1",
        displayName: "Alpha planner",
        title: "Alpha planner",
        cwd: "/tmp/alpha",
        folderPath: "/tmp/alpha",
        updatedAtMs: baseTime - 60_000,
        createdAtMs: baseTime - 120_000,
        eventCount: 1,
        activityStatus: "running"
      },
      {
        sessionId: "session-alpha-2",
        displayName: "Alpha tests",
        title: "Alpha tests",
        cwd: "/tmp/alpha",
        folderPath: "/tmp/alpha",
        updatedAtMs: baseTime - 600_000,
        createdAtMs: baseTime - 1_200_000,
        eventCount: 2,
        activityStatus: "idle"
      },
      {
        sessionId: "session-beta",
        displayName: "Beta deploy",
        title: "Beta deploy",
        cwd: "/tmp/beta",
        folderPath: "/tmp/beta",
        updatedAtMs: baseTime - 3_600_000,
        createdAtMs: baseTime - 7_200_000,
        eventCount: 3,
        activityStatus: "idle"
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  const sidebar = page.locator(".pf-sidebar-agents");

  const alphaGroup = sidebar.locator('.pf-sidebar-project-group').filter({ hasText: "alpha" });
  const betaGroup = sidebar.locator('.pf-sidebar-project-group').filter({ hasText: "beta" });
  await expect(alphaGroup).toHaveCount(1);
  await expect(betaGroup).toHaveCount(1);

  await expect(alphaGroup.locator(".pf-sidebar-agent-row")).toHaveCount(2);
  await expect(betaGroup.locator(".pf-sidebar-agent-row")).toHaveCount(1);

  await expect(alphaGroup.getByText("Alpha planner")).toBeVisible();
  await expect(alphaGroup.getByText("Alpha tests")).toBeVisible();
  await expect(betaGroup.getByText("Beta deploy")).toBeVisible();

  const alphaHeader = alphaGroup.locator(".pf-sidebar-project-header");
  await alphaHeader.click();
  await expect(alphaGroup.locator(".pf-sidebar-agent-row")).toHaveCount(0);
  await expect(alphaHeader).toHaveAttribute("aria-expanded", "false");

  await alphaHeader.click();
  await expect(alphaGroup.locator(".pf-sidebar-agent-row")).toHaveCount(2);
  await expect(alphaHeader).toHaveAttribute("aria-expanded", "true");
});

test("PLUS-121: active agent project stays collapsed after manual toggle", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-alpha-active",
        displayName: "Alpha active",
        title: "Alpha active",
        cwd: "/tmp/alpha",
        folderPath: "/tmp/alpha",
        updatedAtMs: baseTime - 60_000,
        createdAtMs: baseTime - 120_000,
        eventCount: 1,
        activityStatus: "running"
      },
      {
        sessionId: "session-alpha-sidecar",
        displayName: "Alpha sidecar",
        title: "Alpha sidecar",
        cwd: "/tmp/alpha",
        folderPath: "/tmp/alpha",
        updatedAtMs: baseTime - 600_000,
        createdAtMs: baseTime - 1_200_000,
        eventCount: 2,
        activityStatus: "idle"
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  const alphaGroup = page
    .locator(".pf-sidebar-agents .pf-sidebar-project-group")
    .filter({ hasText: "alpha" });
  const alphaHeader = alphaGroup.locator(".pf-sidebar-project-header");
  await alphaGroup.getByRole("button", { name: /^Alpha active\b/ }).click();
  await expect(page.locator(".pf-agent-detail")).toBeVisible();

  await alphaHeader.click();
  await expect(alphaHeader).toHaveAttribute("aria-expanded", "false");
  await expect(alphaGroup.locator(".pf-sidebar-agent-row")).toHaveCount(0);
  await page.waitForTimeout(50);
  await expect(alphaHeader).toHaveAttribute("aria-expanded", "false");
});

test("PLUS-121: child sessions are not listed as top-level active agents", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-parent-agent",
        displayName: "Parent planner",
        title: "Parent planner",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime - 60_000,
        createdAtMs: baseTime - 120_000,
        eventCount: 1,
        activityStatus: "running"
      },
      {
        sessionId: "session-child-explorer",
        displayName: "Child explorer",
        title: "Child explorer",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime - 10_000,
        createdAtMs: baseTime - 90_000,
        eventCount: 1,
        activityStatus: "running",
        parentSessionId: "session-parent-agent"
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  const sidebar = page.locator(".pf-sidebar-agents");
  const pufferGroup = sidebar.locator(".pf-sidebar-project-group").filter({ hasText: "puffer" });
  await expect(pufferGroup).toHaveCount(1);
  await expect(pufferGroup.getByText("Parent planner")).toBeVisible();
  await expect(sidebar.getByText("Child explorer")).toHaveCount(0);
  await expect(pufferGroup.locator(".pf-sidebar-agent-row")).toHaveCount(1);
});

test("PLUS-121: review agents keep review state in the active sidebar", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-review-agent",
        displayName: "Review ready",
        title: "Review ready",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 120_000,
        eventCount: 3,
        activityStatus: "review"
      },
      {
        sessionId: "session-running-agent",
        displayName: "Running build",
        title: "Running build",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime - 60_000,
        createdAtMs: baseTime - 180_000,
        eventCount: 2,
        activityStatus: "running"
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  const sidebar = page.locator(".pf-sidebar-agents");
  const reviewRow = sidebar.locator(".pf-sidebar-agent-row").filter({ hasText: "Review ready" });
  await expect(reviewRow.locator('.state[data-state="review"]')).toHaveText("review");

  await sidebar.getByLabel("Filter by state").selectOption("review");
  await expect(sidebar.locator(".pf-sidebar-label .count")).toHaveText("1");
  await expect(sidebar.getByText("Review ready")).toBeVisible();
  await expect(sidebar.getByText("Running build")).toHaveCount(0);
});

test("PLUS-121: collapsed active projects do not leak across same-name folders", async ({
  page
}) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-other-puffer",
        displayName: "Other puffer planner",
        title: "Other puffer planner",
        cwd: "/tmp/other/puffer",
        folderPath: "/tmp/other/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        activityStatus: "running"
      }
    ]
  });
  await daemon.install(page);
  await page.addInitScript(() => {
    window.localStorage.setItem("puffer.sidebar.collapsedProjects", JSON.stringify(["puffer"]));
  });
  await daemon.open(page);

  const group = page
    .locator(".pf-sidebar-agents .pf-sidebar-project-group")
    .filter({ hasText: "puffer" });
  const header = group.locator(".pf-sidebar-project-header");
  await expect(header).toHaveAttribute("aria-expanded", "true");
  await expect(group.getByText("Other puffer planner")).toBeVisible();
});

test("PLUS-121: persisted active project collapse is not auto-expanded on restore", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-puffer-restored",
        displayName: "Restored active",
        title: "Restored active",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime - 60_000,
        createdAtMs: baseTime - 120_000,
        eventCount: 1,
        activityStatus: "running"
      }
    ]
  });
  await daemon.install(page);
  await page.addInitScript(() => {
    window.localStorage.setItem(
      "puffer.sidebar.collapsedProjects",
      JSON.stringify(["/tmp/puffer"])
    );
    window.localStorage.setItem(
      "puffer-desktop:preferences",
      JSON.stringify({ rememberSession: true })
    );
    window.localStorage.setItem(
      "puffer-desktop:remembered-session",
      JSON.stringify({ workspaceRoot: "/tmp/puffer", sessionId: "session-puffer-restored" })
    );
  });
  await daemon.open(page);
  await daemon.waitForRequest(
    "load_session_detail",
    (request) => request.params.sessionId === "session-puffer-restored"
  );

  await expect(page.locator(".pf-agent-detail")).toBeVisible();
  const group = page
    .locator(".pf-sidebar-agents .pf-sidebar-project-group")
    .filter({ hasText: "puffer" });
  const header = group.locator(".pf-sidebar-project-header");
  await expect(header).toHaveAttribute("aria-expanded", "false");
  await expect(group.locator(".pf-sidebar-agent-row")).toHaveCount(0);
});

test("PLUS-121: active agents disambiguate projects with the same folder name", async ({
  page
}) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-team-a-puffer",
        displayName: "Team A planner",
        title: "Team A planner",
        cwd: "/tmp/team-a/puffer",
        folderPath: "/tmp/team-a/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        activityStatus: "running"
      },
      {
        sessionId: "session-team-b-puffer",
        displayName: "Team B planner",
        title: "Team B planner",
        cwd: "/tmp/team-b/puffer",
        folderPath: "/tmp/team-b/puffer",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        eventCount: 1,
        activityStatus: "running"
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  const sidebar = page.locator(".pf-sidebar-agents");
  const groups = sidebar.locator(".pf-sidebar-project-group");
  await expect(groups).toHaveCount(2);
  await expect(groups.filter({ hasText: "/tmp/team-a/puffer" })).toHaveCount(1);
  await expect(groups.filter({ hasText: "/tmp/team-b/puffer" })).toHaveCount(1);
  await expect(groups.first().locator(".pf-sidebar-agent-row")).toHaveCount(1);
  await expect(groups.nth(1).locator(".pf-sidebar-agent-row")).toHaveCount(1);
});

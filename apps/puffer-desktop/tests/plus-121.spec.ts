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

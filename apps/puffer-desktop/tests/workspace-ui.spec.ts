import { expect, test } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";

const baseTime = Date.now();

test("workspace picker ignores duplicate local switch submits while restart is in flight", async ({
  page
}) => {
  const daemon = new FakeDaemon();
  await page.addInitScript((daemonUrl) => {
    const win = window as unknown as {
      __TAURI__?: unknown;
      __TAURI_INTERNALS__?: {
        invoke?: (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;
      };
      __workspacePickerInvokeCalls?: Array<{ cmd: string; args: Record<string, unknown> }>;
    };
    win.__workspacePickerInvokeCalls = [];
    win.__TAURI__ = {};
    win.__TAURI_INTERNALS__ = {
      invoke: async (cmd: string, args: Record<string, unknown> = {}) => {
        win.__workspacePickerInvokeCalls?.push({ cmd, args });
        if (cmd !== "restart_local_daemon") throw new Error(`unexpected invoke: ${cmd}`);
        await new Promise((resolve) => setTimeout(resolve, 500));
        return {
          url: daemonUrl,
          token: "test",
          protocolVersion: "2025-01-01",
          workspaceRoot: String(args.cwd ?? "/tmp/puffer-next")
        };
      }
    };
  }, daemon.url);

  await daemon.install(page);
  await daemon.open(page);

  await page.getByTitle("Switch workspace").click();
  const dialog = page.getByRole("dialog", { name: "Switch workspace" });
  await dialog.getByRole("tab", { name: /Local/ }).click();
  await dialog.getByLabel("Workspace directory").fill("/tmp/puffer-next");
  await dialog.getByRole("button", { name: "Switch local workspace" }).evaluate((button) => {
    (button as HTMLButtonElement).click();
    (button as HTMLButtonElement).click();
  });

  await page.waitForTimeout(50);
  const calls = await page.evaluate(() => {
    const win = window as unknown as {
      __workspacePickerInvokeCalls?: Array<{ cmd: string; args: Record<string, unknown> }>;
    };
    return (win.__workspacePickerInvokeCalls ?? []).filter(
      (call) => call.cmd === "restart_local_daemon"
    );
  });
  expect(calls).toHaveLength(1);
  expect(calls[0].args.cwd).toBe("/tmp/puffer-next");
});

test("workspace picker clears local errors when switching modes", async ({ page }) => {
  const daemon = new FakeDaemon();
  await page.addInitScript(() => {
    const win = window as unknown as {
      __TAURI__?: unknown;
      __TAURI_INTERNALS__?: {
        invoke?: (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;
      };
    };
    win.__TAURI__ = {};
    win.__TAURI_INTERNALS__ = {
      invoke: async (cmd: string, args: Record<string, unknown> = {}) => {
        if (cmd !== "restart_local_daemon") throw new Error(`unexpected invoke: ${cmd}`);
        throw new Error(`cannot start ${String(args.cwd ?? "")}`);
      }
    };
  });

  await daemon.install(page);
  await daemon.open(page);

  await page.getByTitle("Switch workspace").click();
  const dialog = page.getByRole("dialog", { name: "Switch workspace" });
  await dialog.getByRole("tab", { name: /Local/ }).click();
  await dialog.getByLabel("Workspace directory").fill("/tmp/broken-workspace");
  await dialog.getByRole("button", { name: "Switch local workspace" }).click();

  const staleError = dialog.locator(".pf-modal-status", {
    hasText: "cannot start /tmp/broken-workspace"
  });
  await expect(staleError).toBeVisible();

  await dialog.getByRole("tab", { name: /Remote/ }).click();

  await expect(staleError).toHaveCount(0);
  await expect(dialog.getByLabel("SSH target")).toBeVisible();
});

test("agent pin ignores duplicate clicks while the pin save is in flight", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.delayResponse("set_desktop_pin", () => true, 500);
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: /Browser regression/ }).first().click();
  const agentRow = page.locator(".pf-sidebar-agent-row").filter({ hasText: "Browser regression" });
  await expect(agentRow).toBeVisible();
  await agentRow.getByRole("button", { name: "Pin agent" }).evaluate((button) => {
    (button as HTMLButtonElement).click();
    (button as HTMLButtonElement).click();
  });

  const request = await daemon.waitForRequest("set_desktop_pin");
  await expect(agentRow.getByRole("button", { name: "Unpin agent" })).toBeDisabled();
  expect(request.params).toMatchObject({
    kind: "agent",
    id: "session-browser",
    pinned: true
  });
  await page.waitForTimeout(50);
  expect(daemon.requests.filter((request) => request.method === "set_desktop_pin")).toHaveLength(1);
});

test("workspace pin control is disabled while the pin save is in flight", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.delayResponse("set_desktop_pin", () => true, 500);
  await daemon.install(page);
  await daemon.open(page);

  const project = page.locator(".pf-pw-project").filter({ hasText: "puffer" });
  const pinWorkspace = project.getByRole("button", { name: "Pin workspace" });
  await expect(pinWorkspace).toBeEnabled();

  await pinWorkspace.click();
  await daemon.waitForRequest("set_desktop_pin");

  await expect(project.getByRole("button", { name: "Unpin workspace" })).toBeDisabled();
});

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

test("empty workspace search shows only the search empty state", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await daemon.install(page);
  await daemon.open(page);

  await expect(page.getByRole("heading", { name: "No sessions yet" })).toBeVisible();

  await page.getByLabel("Search workspace").fill("missing session");

  await expect(page.getByRole("heading", { name: "No workspace results" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "No sessions yet" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "New agent in default workspace" })).toHaveCount(0);

  await page.getByRole("button", { name: "Clear search" }).click();
  await expect(page.getByRole("heading", { name: "No sessions yet" })).toBeVisible();
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

test("session history keeps older sessions available after starting a new agent", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-old-history",
        displayName: "Old browser plan",
        title: "Old browser plan",
        cwd: "/tmp/puffer-history",
        folderPath: "/tmp/puffer-history",
        updatedAtMs: baseTime - 120_000,
        createdAtMs: baseTime - 240_000,
        eventCount: 2,
        timeline: [
          {
            kind: "user_message",
            id: "old-user",
            text: "Keep this older session available.",
            createdAtMs: baseTime - 200_000
          },
          {
            kind: "assistant_message",
            id: "old-assistant",
            text: "Older transcript persisted.",
            createdAtMs: baseTime - 190_000
          }
        ]
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await expect(page.getByRole("region", { name: "Session history" })).toContainText("Old browser plan");

  await page
    .locator(".pf-pw-project")
    .filter({ hasText: "puffer-history" })
    .getByRole("button", { name: "New agent" })
    .click();
  await page.getByRole("button", { name: /Start agent/ }).click();

  await expect(page.locator(".pf-agent-detail")).toBeVisible();
  await page.getByRole("button", { name: "Back" }).click();

  const history = page.getByRole("region", { name: "Session history" });
  await expect(history).toContainText("Old browser plan");
  await expect(history).toContainText("New Session");
  await history.getByRole("button", { name: /Old browser plan/ }).click();

  await expect(page.locator(".pf-agent-detail")).toBeVisible();
  await expect(page.getByText("Older transcript persisted.")).toBeVisible();
});

test("late workspace refresh does not hide a newly created session", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-stale-history",
        displayName: "Stale history base",
        title: "Stale history base",
        cwd: "/tmp/puffer-stale-history",
        folderPath: "/tmp/puffer-stale-history",
        updatedAtMs: baseTime - 120_000,
        createdAtMs: baseTime - 240_000,
        eventCount: 1
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  const history = page.getByRole("region", { name: "Session history" });
  await expect(history).toContainText("Stale history base");

  daemon.delayResponse("list_grouped_sessions", () => true, 900);
  daemon.emit("workspace:sessions:changed", { reason: "manual-refresh" });
  await page.waitForTimeout(25);

  await page
    .locator(".pf-pw-project")
    .filter({ hasText: "puffer-stale-history" })
    .getByRole("button", { name: "New agent" })
    .click();
  await page.getByRole("button", { name: /Start agent/ }).click();

  await expect(page.locator(".pf-agent-detail")).toBeVisible();
  await page.getByRole("button", { name: "Back" }).click();
  await expect(history).toContainText("New Session");

  await page.waitForTimeout(1_000);
  await expect(history).toContainText("Stale history base");
  await expect(history).toContainText("New Session");
});

test("active agents includes an opened session before grouped history catches up", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-stale-active",
        displayName: "Stale active base",
        title: "Stale active base",
        cwd: "/tmp/puffer-stale-active",
        folderPath: "/tmp/puffer-stale-active",
        updatedAtMs: baseTime - 120_000,
        createdAtMs: baseTime - 240_000,
        eventCount: 1
      }
    ]
  });
  daemon.setGroupedSessionFilter(
    (metadata) => !String(metadata.sessionId ?? "").startsWith("session-created-")
  );
  await daemon.install(page);
  await daemon.open(page);

  await page
    .locator(".pf-pw-project")
    .filter({ hasText: "puffer-stale-active" })
    .getByRole("button", { name: "New agent" })
    .click();
  await page.getByRole("button", { name: /Start agent/ }).click();

  await expect(page.locator(".pf-agent-detail")).toBeVisible();
  const activeList = page.locator(".pf-sidebar-agents-list");
  await expect(activeList.locator(".pf-sidebar-agent-row").filter({ hasText: "New Session" })).toBeVisible();
  await expect(activeList.getByText("No agents match")).toHaveCount(0);

  await page.getByRole("button", { name: "Back" }).click();
  const history = page.getByRole("region", { name: "Session history" });
  await expect(history).toContainText("New Session");

  await page
    .locator(".pf-pw-project")
    .filter({ hasText: "puffer-stale-active" })
    .getByRole("button", { name: "Details" })
    .click();
  const projectDetail = page.locator(".pf-fpb");
  await expect(projectDetail).toContainText("New Session");
  await projectDetail.getByRole("button", { name: /New Session/ }).click();
  await expect(page.locator(".pf-agent-detail")).toBeVisible();
  await page.getByRole("button", { name: "Back" }).click();
  await expect(projectDetail).toBeVisible();
  await page.getByRole("button", { name: "Back" }).click();

  await history.getByRole("button", { name: /New Session/ }).click();
  await expect(page.locator(".pf-agent-detail")).toBeVisible();
});

test("create project opens created session before grouped history catches up", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-connect-base",
        displayName: "Connect base",
        title: "Connect base",
        cwd: "/tmp/puffer-connect-base",
        folderPath: "/tmp/puffer-connect-base",
        updatedAtMs: baseTime - 120_000,
        createdAtMs: baseTime - 240_000,
        eventCount: 1
      }
    ]
  });
  daemon.setGroupedSessionFilter(
    (metadata) => !String(metadata.sessionId ?? "").startsWith("session-created-")
  );
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Create Project" }).click();
  const dialog = page.getByRole("dialog", { name: "Create Project" });
  await expect(dialog).toBeVisible();
  await dialog.locator("#pf-local-dest").fill("/tmp/new-puffer-project");
  await dialog.getByRole("button", { name: "Create" }).click();

  await daemon.waitForRequest("create_session");

  await expect(page.locator(".pf-agent-detail .primary-title")).toContainText("New Session");
  await expect(page.locator(".pf-composer textarea")).toBeEnabled();
  const activeList = page.locator(".pf-sidebar-agents-list");
  await expect(activeList.locator(".pf-sidebar-agent-row").filter({ hasText: "New Session" })).toBeVisible();
  await expect(activeList.getByText("No agents match")).toHaveCount(0);
});

test("active agents can reopen a live session before grouped history catches up", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-live-fallback-base",
        displayName: "Live fallback base",
        title: "Live fallback base",
        cwd: "/tmp/puffer-live-fallback",
        folderPath: "/tmp/puffer-live-fallback",
        updatedAtMs: baseTime - 120_000,
        createdAtMs: baseTime - 240_000,
        eventCount: 1
      }
    ]
  });
  daemon.setGroupedSessionFilter(
    (metadata) => !String(metadata.sessionId ?? "").startsWith("session-created-")
  );
  await daemon.install(page);
  await daemon.open(page);

  await page
    .locator(".pf-pw-project")
    .filter({ hasText: "puffer-live-fallback" })
    .getByRole("button", { name: "New agent" })
    .click();
  await page.getByRole("button", { name: /Start agent/ }).click();
  await expect(page.locator(".pf-agent-detail .primary-title")).toContainText("New Session");

  await page.locator(".pf-composer textarea").fill("Keep this fallback session live");
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-created-2" &&
      request.params.message === "Keep this fallback session live"
  );

  const activeList = page.locator(".pf-sidebar-agents-list");
  await activeList.getByRole("button", { name: /Live fallback base/ }).click();
  await expect(page.locator(".pf-agent-detail .primary-title")).toContainText("Live fallback base");

  const liveFallbackRow = activeList.locator(".pf-sidebar-agent-row").filter({ hasText: "New Session" });
  await expect(liveFallbackRow).toBeVisible();
  await expect(liveFallbackRow.locator(".state")).toContainText("thinking");
  await liveFallbackRow.getByRole("button", { name: /New Session/ }).click();
  await expect(page.locator(".pf-agent-detail .primary-title")).toContainText("New Session");
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

  await page.locator(".pf-sidebar").getByRole("button", { name: "Project" }).click();

  await expect(page.locator(".pf-pw-list")).toBeVisible();
  await expect(page.locator(".pf-agent-detail")).toHaveCount(0);
  await expect(page.locator('.pf-sidebar-agent-row[data-active="true"]')).toHaveCount(0);
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

  await project.getByRole("button", { name: "Details" }).click();
  const runningColumn = page.locator(".pf-fpb-col").filter({ hasText: "Running" });
  await expect(runningColumn.getByText("Running checkout fix")).toBeVisible();
  await expect(runningColumn.getByText("Awaiting deploy approval")).toBeVisible();

  const queuedColumn = page.locator(".pf-fpb-col").filter({ hasText: "Queued" });
  await expect(queuedColumn.getByText("Idle docs followup")).toBeVisible();
});

test("running daemon sessions keep the composer from starting another turn", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-running-composer",
        displayName: "Running composer guard",
        title: "Running composer guard",
        cwd: "/tmp/puffer-active",
        folderPath: "/tmp/puffer-active",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 3,
        activityStatus: "running",
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await page
    .locator(".pf-sidebar-agent-row")
    .filter({ hasText: "Running composer guard" })
    .click();
  await expect(page.locator(".pf-agent-detail")).toBeVisible();
  await expect(page.locator(".pf-composer textarea")).toBeDisabled();
  await expect(page.getByRole("button", { name: "Send", exact: true })).toBeDisabled();
  await page.keyboard.press("Enter");

  expect(daemon.requests.filter((request) => request.method === "run_agent_turn")).toHaveLength(0);
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

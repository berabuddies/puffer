import { expect, type Locator, type Page, test } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";

const baseTime = Date.now();

async function expectFocusInside(dialog: Locator): Promise<void> {
  await expect.poll(() =>
    dialog.evaluate((node) => node.contains(document.activeElement))
  ).toBe(true);
}

async function expectTabFocusTrapped(page: Page, dialog: Locator, count: number): Promise<void> {
  for (let index = 0; index < count; index += 1) {
    await page.keyboard.press("Tab");
    await expectFocusInside(dialog);
  }
}

test("workspace picker modal receives and traps keyboard focus", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByTitle("Switch workspace").click();
  const dialog = page.getByRole("dialog", { name: "Switch workspace" });
  await expect(dialog).toBeVisible();

  await expectFocusInside(dialog);
  await expectTabFocusTrapped(page, dialog, 12);
  await page.keyboard.press("Shift+Tab");
  await expectFocusInside(dialog);
});

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

test("workspace switch clears live agents from the previous daemon", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-old-workspace-live",
        displayName: "Old workspace live agent",
        title: "Old workspace live agent",
        cwd: "/tmp/puffer-old",
        folderPath: "/tmp/puffer-old",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });
  await page.addInitScript((daemonUrl) => {
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
        await new Promise((resolve) => setTimeout(resolve, 400));
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

  await page
    .locator(".pf-sidebar-agent-row")
    .filter({ hasText: "Old workspace live agent" })
    .getByRole("button", { name: /Old workspace live agent/ })
    .click();
  await page.locator(".pf-composer textarea").fill("Keep the old workspace busy");
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-old-workspace-live" &&
      request.params.message === "Keep the old workspace busy"
  );
  await expect(
    page.locator(".pf-sidebar-agent-row").filter({ hasText: "Old workspace live agent" })
      .locator('.state[data-state="thinking"]')
  ).toContainText("thinking");

  await page.getByRole("button", { name: "Back" }).click();
  await page.getByTitle("Switch workspace").click();
  const dialog = page.getByRole("dialog", { name: "Switch workspace" });
  await dialog.getByRole("tab", { name: /Local/ }).click();
  await dialog.getByLabel("Workspace directory").fill("/tmp/puffer-next");
  await dialog.getByRole("button", { name: "Switch local workspace" }).click();
  await page.waitForTimeout(50);
  daemon.setWorkspaceRoot("/tmp/puffer-next");
  daemon.setGroupedSessionFilter(() => false);

  await expect(dialog).toHaveCount(0);
  await expect(page.getByRole("region", { name: "Session history" })).toHaveCount(0);
  await expect(
    page.locator(".pf-sidebar-agent-row").filter({ hasText: "Old workspace live agent" })
  ).toHaveCount(0);
  await expect(page.locator(".pf-sidebar-empty")).toContainText("No agents match");
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

test("agent pin accepts a confirmed opposite toggle before the save response returns", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.delayResponse("set_desktop_pin", () => true, 500);
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: /Browser regression/ }).first().click();
  const agentRow = page.locator(".pf-sidebar-agent-row").filter({ hasText: "Browser regression" });
  await expect(agentRow).toBeVisible();
  await agentRow.getByRole("button", { name: "Pin agent" }).click();

  const request = await daemon.waitForRequest("set_desktop_pin");
  await expect(agentRow.getByRole("button", { name: "Unpin agent" })).toBeEnabled();
  expect(request.params).toMatchObject({
    kind: "agent",
    id: "session-browser",
    pinned: true
  });

  await agentRow.getByRole("button", { name: "Unpin agent" }).click();
  const unpin = await daemon.waitForRequest(
    "set_desktop_pin",
    (request) => request.params.pinned === false
  );
  expect(unpin.params).toMatchObject({
    kind: "agent",
    id: "session-browser",
    pinned: false
  });
});

test("workspace pin control is re-enabled by the confirming pin event", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.delayResponse("set_desktop_pin", () => true, 500);
  await daemon.install(page);
  await daemon.open(page);

  const project = page.locator(".pf-pw-project").filter({ hasText: "puffer" });
  const pinWorkspace = project.getByRole("button", { name: "Pin workspace" });
  await expect(pinWorkspace).toBeEnabled();

  await pinWorkspace.click();
  await daemon.waitForRequest("set_desktop_pin");

  await expect(project.getByRole("button", { name: "Unpin workspace" })).toBeEnabled();
});

test("workspace pin save stays guarded after opening an agent", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-pin-alpha",
        displayName: "Alpha pin source",
        title: "Alpha pin source",
        cwd: "/tmp/puffer-pin",
        folderPath: "/tmp/puffer-pin",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1
      },
      {
        sessionId: "session-pin-beta",
        displayName: "Beta pin target",
        title: "Beta pin target",
        cwd: "/tmp/puffer-pin",
        folderPath: "/tmp/puffer-pin",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        eventCount: 1
      }
    ]
  });
  daemon.delayResponse("set_desktop_pin", () => true, 2_000);
  await daemon.install(page);
  await daemon.open(page);

  const project = page.locator(".pf-pw-project").filter({ hasText: "puffer-pin" });
  await project.getByRole("button", { name: "Pin workspace" }).click();
  await daemon.waitForRequest("set_desktop_pin");

  await project.getByRole("button", { name: /Beta pin target/ }).click();
  await expect(page.locator(".pf-agent-detail")).toBeVisible();
  await page.getByRole("button", { name: "Back" }).click();

  await expect(project.getByRole("button", { name: "Unpin workspace" })).toBeEnabled();
  expect(daemon.requests.filter((request) => request.method === "set_desktop_pin")).toHaveLength(1);
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

test("workspace search includes session notes in history results", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-note-history",
        displayName: "Workspace note target",
        title: "Workspace note target",
        cwd: "/tmp/puffer-note-search",
        folderPath: "/tmp/puffer-note-search",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        note: "Manual approval before browser replay"
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await page.getByLabel("Search workspace").fill("manual approval");

  const project = page.locator(".pf-pw-project").filter({ hasText: "puffer-note-search" });
  await expect(project.getByText("Workspace note target")).toBeVisible();
  await expect(
    page.getByLabel("Session history").getByRole("button", { name: /Workspace note target/ })
  ).toBeVisible();
});

test("workspace project rows collapse and expand their session list", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-collapse-alpha",
        displayName: "Collapse alpha",
        title: "Collapse alpha",
        cwd: "/tmp/puffer-collapse",
        folderPath: "/tmp/puffer-collapse",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 2
      },
      {
        sessionId: "session-collapse-beta",
        displayName: "Collapse beta",
        title: "Collapse beta",
        cwd: "/tmp/puffer-collapse",
        folderPath: "/tmp/puffer-collapse",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        eventCount: 3
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  const project = page.locator(".pf-pw-project").filter({ hasText: "puffer-collapse" });
  await expect(project.getByText("Collapse alpha")).toBeVisible();
  await expect(project.getByText("Collapse beta")).toBeVisible();

  const collapse = project.getByRole("button", { name: "Collapse puffer-collapse" });
  await collapse.click();
  const expand = project.getByRole("button", { name: "Expand puffer-collapse" });
  await expect(expand).toHaveAttribute("aria-expanded", "false");
  await expect(project.getByText("Collapse alpha")).toHaveCount(0);
  await expect(project.getByText("Collapse beta")).toHaveCount(0);
  await expect(project).toContainText("2 sessions");

  await expand.click();
  await expect(project.getByRole("button", { name: "Collapse puffer-collapse" })).toHaveAttribute(
    "aria-expanded",
    "true"
  );
  await expect(project.getByText("Collapse alpha")).toBeVisible();
  await expect(project.getByText("Collapse beta")).toBeVisible();
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

test("workspace board does not list child sessions as top-level history", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-parent-workspace",
        displayName: "Parent workspace agent",
        title: "Parent workspace agent",
        cwd: "/tmp/puffer-subagents",
        folderPath: "/tmp/puffer-subagents",
        updatedAtMs: baseTime - 60_000,
        createdAtMs: baseTime - 120_000,
        eventCount: 2,
        activityStatus: "running"
      },
      {
        sessionId: "session-child-workspace",
        displayName: "Child workspace agent",
        title: "Child workspace agent",
        cwd: "/tmp/puffer-subagents",
        folderPath: "/tmp/puffer-subagents",
        updatedAtMs: baseTime - 10_000,
        createdAtMs: baseTime - 90_000,
        eventCount: 1,
        activityStatus: "running",
        parentSessionId: "session-parent-workspace"
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  const history = page.locator(".pf-pw-history");
  const project = page.locator(".pf-pw-project").filter({ hasText: "puffer-subagents" });
  await expect(history.getByText("Parent workspace agent")).toBeVisible();
  await expect(history.getByText("Child workspace agent")).toHaveCount(0);
  await expect(project.getByText("Parent workspace agent")).toBeVisible();
  await expect(project.getByText("Child workspace agent")).toHaveCount(0);
  await expect(project).toContainText("1 active");

  await project.getByRole("button", { name: "Details" }).click();
  const projectDetail = page.locator(".pf-fpb");
  await expect(projectDetail.getByText("Parent workspace agent")).toBeVisible();
  await expect(projectDetail.getByText("Child workspace agent")).toHaveCount(0);
  await expect(projectDetail.getByText("1 agents")).toBeVisible();
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

test("close session returns to workspace without removing history", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-close-history",
        displayName: "Closable history session",
        title: "Closable history session",
        cwd: "/tmp/puffer-close-history",
        folderPath: "/tmp/puffer-close-history",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "user_message",
            id: "close-user",
            text: "This should survive closing the detail view.",
            createdAtMs: baseTime - 50_000
          }
        ]
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  const history = page.getByRole("region", { name: "Session history" });
  await history.getByRole("button", { name: /Closable history session/ }).click();
  await expect(page.locator(".pf-agent-detail .primary-title")).toContainText("Closable history session");

  await page.getByRole("button", { name: "Close session" }).click();

  await expect(page.locator(".pf-agent-detail")).toHaveCount(0);
  await expect(page.locator(".pf-pw-list")).toBeVisible();
  await expect(history).toContainText("Closable history session");

  await history.getByRole("button", { name: /Closable history session/ }).click();
  await expect(page.getByText("This should survive closing the detail view.")).toBeVisible();
});

test("close session clears remembered restore target without removing history", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-close-remembered",
        displayName: "Remembered closable session",
        title: "Remembered closable session",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "user_message",
            id: "remembered-close-user",
            text: "This remembered session should stay in history.",
            createdAtMs: baseTime - 50_000
          }
        ]
      }
    ]
  });
  await daemon.install(page);
  await page.addInitScript(() => {
    window.localStorage.setItem(
      "puffer-desktop:preferences",
      JSON.stringify({ rememberSession: true })
    );
  });
  await daemon.open(page);

  const history = page.getByRole("region", { name: "Session history" });
  await history.getByRole("button", { name: /Remembered closable session/ }).click();
  await expect(page.locator(".pf-agent-detail .primary-title")).toContainText(
    "Remembered closable session"
  );
  await expect
    .poll(() => page.evaluate(() => window.localStorage.getItem("puffer-desktop:remembered-session")))
    .toContain("session-close-remembered");

  await page.getByRole("button", { name: "Close session" }).click();

  await expect(page.locator(".pf-agent-detail")).toHaveCount(0);
  await expect(
    page.evaluate(() => window.localStorage.getItem("puffer-desktop:remembered-session"))
  ).resolves.toBeNull();

  await page.reload();
  await expect(page.locator(".pf-agent-detail")).toHaveCount(0);
  await expect(page.locator(".pf-pw-list")).toBeVisible();
  await expect(page.getByRole("region", { name: "Session history" })).toContainText(
    "Remembered closable session"
  );
});

test("closed remembered session stays closed after backend reconnect", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-close-reconnect",
        displayName: "Reconnect closed session",
        title: "Reconnect closed session",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "user_message",
            id: "reconnect-close-user",
            text: "This closed session should not be remembered again.",
            createdAtMs: baseTime - 50_000
          }
        ]
      }
    ]
  });
  await daemon.install(page);
  await page.addInitScript(() => {
    window.localStorage.setItem(
      "puffer-desktop:preferences",
      JSON.stringify({ rememberSession: true })
    );
  });
  await daemon.open(page);

  const history = page.getByRole("region", { name: "Session history" });
  await history.getByRole("button", { name: /Reconnect closed session/ }).click();
  await expect(page.locator(".pf-agent-detail .primary-title")).toContainText(
    "Reconnect closed session"
  );
  await expect
    .poll(() =>
      page.evaluate(() => window.localStorage.getItem("puffer-desktop:remembered-session"))
    )
    .toContain("session-close-reconnect");

  await page.getByRole("button", { name: "Close session" }).click();
  await expect(page.locator(".pf-agent-detail")).toHaveCount(0);
  await expect(
    page.evaluate(() => window.localStorage.getItem("puffer-desktop:remembered-session"))
  ).resolves.toBeNull();

  const settingsLoadsBefore = daemon.requests.filter(
    (request) => request.method === "load_settings_snapshot"
  ).length;
  const detailLoadsBefore = daemon.requests.filter(
    (request) => request.method === "load_session_detail"
  ).length;
  await daemon.dropConnections();
  const banner = page.locator(".connection-banner");
  await expect(banner).toContainText("Puffer backend disconnected.");
  daemon.allowConnections();
  await banner.getByRole("button", { name: "Reconnect backend" }).click();
  await expect.poll(() =>
    daemon.requests.filter((request) => request.method === "load_settings_snapshot").length
  ).toBeGreaterThan(settingsLoadsBefore);
  await expect(page.locator(".connection-banner")).toHaveCount(0);
  await page.waitForTimeout(150);

  await expect(page.locator(".pf-agent-detail")).toHaveCount(0);
  expect(daemon.requests.filter((request) => request.method === "load_session_detail")).toHaveLength(
    detailLoadsBefore
  );
  await expect(
    page.evaluate(() => window.localStorage.getItem("puffer-desktop:remembered-session"))
  ).resolves.toBeNull();
  await expect(history).toContainText("Reconnect closed session");
});

test("narrow workspace reconnect clears banner and preserves session navigation", async ({
  page
}) => {
  await page.setViewportSize({ width: 420, height: 820 });
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-narrow-reconnect",
        displayName: "Narrow reconnect",
        title: "Narrow reconnect",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  const history = page.getByRole("region", { name: "Session history" });
  await history.getByRole("button", { name: /Narrow reconnect/ }).click();
  await expect(page.locator(".pf-agent-detail .primary-title")).toContainText("Narrow reconnect");

  await daemon.dropConnections();
  const banner = page.locator(".connection-banner");
  await expect(banner).toContainText("Puffer backend disconnected.");
  daemon.allowConnections();
  await banner.getByRole("button", { name: "Reconnect backend" }).click();
  await expect(page.locator(".connection-banner")).toHaveCount(0);

  await page.getByRole("button", { name: "Back" }).first().click();
  await expect(history.getByRole("button", { name: /Narrow reconnect/ })).toBeVisible();
  await history.getByRole("button", { name: /Narrow reconnect/ }).click();
  await expect(page.locator(".pf-agent-detail .primary-title")).toContainText("Narrow reconnect");
});

test("closed remembered session ignores stale workspace update events", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-close-stale-event",
        displayName: "Stale closed session",
        title: "Stale closed session",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "user_message",
            id: "stale-close-user",
            text: "This closed session should ignore stale updates.",
            createdAtMs: baseTime - 50_000
          }
        ]
      }
    ]
  });
  await daemon.install(page);
  await page.addInitScript(() => {
    window.localStorage.setItem(
      "puffer-desktop:preferences",
      JSON.stringify({ rememberSession: true })
    );
  });
  await daemon.open(page);

  const history = page.getByRole("region", { name: "Session history" });
  await history.getByRole("button", { name: /Stale closed session/ }).click();
  await expect(page.locator(".pf-agent-detail .primary-title")).toContainText(
    "Stale closed session"
  );
  await expect
    .poll(() =>
      page.evaluate(() => window.localStorage.getItem("puffer-desktop:remembered-session"))
    )
    .toContain("session-close-stale-event");

  await page.getByRole("button", { name: "Close session" }).click();
  await expect(page.locator(".pf-agent-detail")).toHaveCount(0);
  await expect(
    page.evaluate(() => window.localStorage.getItem("puffer-desktop:remembered-session"))
  ).resolves.toBeNull();
  const detailLoadsBefore = daemon.requests.filter(
    (request) => request.method === "load_session_detail"
  ).length;

  daemon.emit("workspace:sessions:changed", {
    sessionId: "session-close-stale-event",
    reason: "generated_title"
  });
  await page.waitForTimeout(150);

  await expect(page.locator(".pf-agent-detail")).toHaveCount(0);
  expect(daemon.requests.filter((request) => request.method === "load_session_detail")).toHaveLength(
    detailLoadsBefore
  );
  await expect(
    page.evaluate(() => window.localStorage.getItem("puffer-desktop:remembered-session"))
  ).resolves.toBeNull();

  await page.reload();
  await expect(page.locator(".pf-agent-detail")).toHaveCount(0);
  await expect(page.locator(".pf-pw-list")).toBeVisible();
  await expect(history).toContainText("Stale closed session");
});

test("legacy remembered session without workspace root is ignored", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [],
    workspaceRoot: "/tmp/puffer-current-workspace"
  });
  await daemon.install(page);
  await page.addInitScript(() => {
    window.localStorage.setItem(
      "puffer-desktop:remembered-session",
      JSON.stringify({ sessionId: "session-legacy-rootless" })
    );
    window.localStorage.setItem(
      "puffer-desktop:preferences",
      JSON.stringify({ rememberSession: true })
    );
  });
  await daemon.open(page);

  await page.waitForTimeout(300);
  await expect(page.locator(".pf-agent-detail")).toHaveCount(0);
  expect(
    daemon.requests.filter(
      (request) =>
        request.method === "load_session_detail" &&
        request.params.sessionId === "session-legacy-rootless"
    )
  ).toHaveLength(0);
  await expect(
    page.evaluate(() => window.localStorage.getItem("puffer-desktop:remembered-session"))
  ).resolves.toBeNull();
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

test("session history keeps opened sessions after a later stale grouped refresh", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-history-alpha",
        displayName: "Alpha stale history",
        title: "Alpha stale history",
        cwd: "/tmp/puffer-vanishing-history",
        folderPath: "/tmp/puffer-vanishing-history",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "user_message",
            id: "alpha-user",
            text: "Keep alpha in history.",
            createdAtMs: baseTime - 50_000
          }
        ]
      },
      {
        sessionId: "session-history-beta",
        displayName: "Beta stable history",
        title: "Beta stable history",
        cwd: "/tmp/puffer-vanishing-history",
        folderPath: "/tmp/puffer-vanishing-history",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        eventCount: 1
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  const history = page.getByRole("region", { name: "Session history" });
  await history.getByRole("button", { name: /Alpha stale history/ }).click();
  await expect(page.locator(".pf-agent-detail .primary-title")).toContainText("Alpha stale history");
  await page.getByRole("button", { name: "Back" }).click();

  daemon.setGroupedSessionFilter(
    (metadata) => String(metadata.sessionId ?? "") !== "session-history-alpha"
  );
  const previousRefreshes = daemon.requests.filter(
    (request) => request.method === "list_grouped_sessions"
  ).length;
  daemon.emit("workspace:sessions:changed", { reason: "stale-alpha-drop" });
  await daemon.waitForRequest(
    "list_grouped_sessions",
    (request) =>
      daemon.requests.filter((candidate) => candidate.method === "list_grouped_sessions")
        .indexOf(request) >= previousRefreshes
  );

  await expect(history).toContainText("Alpha stale history");
  await history.getByRole("button", { name: /Beta stable history/ }).click();
  await expect(page.locator(".pf-agent-detail .primary-title")).toContainText("Beta stable history");
  await page.getByRole("button", { name: "Back" }).click();

  await expect(history).toContainText("Alpha stale history");
  await expect(history).toContainText("Beta stable history");
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

test("active agents keeps a fallback-created session after opening older history", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-stale-active-old",
        displayName: "Old active history",
        title: "Old active history",
        cwd: "/tmp/puffer-stale-active-old",
        folderPath: "/tmp/puffer-stale-active-old",
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
    .filter({ hasText: "puffer-stale-active-old" })
    .getByRole("button", { name: "New agent" })
    .click();
  await page.getByRole("button", { name: /Start agent/ }).click();

  await expect(page.locator(".pf-agent-detail .primary-title")).toContainText("New Session");
  await page.getByRole("button", { name: "Back" }).click();
  await page.getByRole("region", { name: "Session history" })
    .getByRole("button", { name: /Old active history/ })
    .click();
  await expect(page.locator(".pf-agent-detail .primary-title")).toContainText("Old active history");

  const activeList = page.locator(".pf-sidebar-agents-list");
  const createdRow = activeList.locator(".pf-sidebar-agent-row").filter({ hasText: "New Session" });
  await expect(createdRow).toBeVisible();
  await expect(activeList.getByText("No agents match")).toHaveCount(0);

  await createdRow.getByRole("button", { name: /New Session/ }).click();
  await expect(page.locator(".pf-agent-detail .primary-title")).toContainText("New Session");
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

test("sidebar keeps full session titles for resizable space", async ({ page }) => {
  const longTitle = "Long running browser investigation that should not be pre-truncated";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-long-sidebar-title",
        displayName: longTitle,
        title: longTitle,
        cwd: "/tmp/puffer-active",
        folderPath: "/tmp/puffer-active",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        activityStatus: "idle"
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  const title = page
    .locator(".pf-sidebar-agent-row")
    .filter({ hasText: longTitle })
    .locator(".title");
  await expect(title).toHaveText(longTitle);
});

test("agent detail header uses available space before title ellipsis", async ({ page }) => {
  await page.setViewportSize({ width: 1500, height: 900 });
  const longTitle =
    "Long running browser investigation that should not be squeezed before the header runs out of room";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-long-detail-title",
        displayName: longTitle,
        title: longTitle,
        cwd: "/tmp/puffer-active",
        folderPath: "/tmp/puffer-active",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        activityStatus: "idle"
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await page
    .locator(".pf-sidebar-agent-row")
    .filter({ hasText: longTitle })
    .getByRole("button", { name: new RegExp(`^${longTitle}`) })
    .click();

  const title = page.locator(".pf-agent-detail .primary-title");
  await expect(title).toHaveText(longTitle);
  await expect(title).toHaveAttribute("title", longTitle);

  const width = await title.evaluate((node) => {
    const identity = node.closest(".pf-agent-identity") as HTMLElement | null;
    return identity?.getBoundingClientRect().width ?? 0;
  });
  expect(width).toBeGreaterThan(600);
});

test("workspace agent cards keep full session titles for responsive ellipsis", async ({ page }) => {
  const longTitle =
    "Long workspace browser investigation title that should remain complete in the DOM and only ellipsize visually when space runs out";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-long-project-card-title",
        displayName: longTitle,
        title: longTitle,
        cwd: "/tmp/puffer-long-card",
        folderPath: "/tmp/puffer-long-card",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        activityStatus: "running"
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  const project = page.locator(".pf-pw-project").filter({ hasText: "puffer-long-card" });
  const agent = project.locator(".pf-pw-agent");

  await expect(agent.locator(".title")).toHaveText(longTitle);
  await expect(agent).toHaveAttribute("type", "button");
  await expect(agent).toHaveAttribute("title", new RegExp(`^${longTitle} - Running -`));
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

test("project memory file can be loaded and edited", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.seedFile(
    "/tmp/puffer/.puffer/memory/project.md",
    "Initial project memory body.\n\nKeep the browser regression notes close."
  );
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-pw-project").getByRole("button", { name: "Details" }).click();
  await page.getByRole("button", { name: /Memory/ }).click();

  const memoryDetail = page.locator(".pf-pmem-detail");
  await expect(memoryDetail.getByText("Initial project memory body.")).toBeVisible();
  await expect(memoryDetail.getByRole("button", { name: "Edit" })).toBeEnabled();

  await memoryDetail.getByRole("button", { name: "Edit" }).click();
  await memoryDetail.getByLabel("Memory file content").fill("Updated memory from the UI.");
  await memoryDetail.getByRole("button", { name: "Save" }).click();

  await expect(memoryDetail.getByText("Updated memory from the UI.")).toBeVisible();
  const writes = daemon.requests.filter((request) => request.method === "write_file");
  expect(writes.at(-1)?.params).toMatchObject({
    path: "/tmp/puffer/.puffer/memory/project.md",
    content: "Updated memory from the UI."
  });
});

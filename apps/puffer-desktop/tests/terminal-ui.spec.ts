import { expect, test } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";

const baseTime = Date.now();

test("Terminal pane restores PTYs when switching sessions", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-alpha",
        displayName: "Alpha terminal",
        title: "Alpha terminal",
        cwd: "/tmp/puffer-alpha",
        folderPath: "/tmp/puffer-alpha",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        timeline: []
      },
      {
        sessionId: "session-beta",
        displayName: "Beta terminal",
        title: "Beta terminal",
        cwd: "/tmp/puffer-beta",
        folderPath: "/tmp/puffer-beta",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: /Alpha terminal/ }).first().click();
  await page.locator(".pf-agent-tabs").getByRole("button", { name: "Terminal", exact: true }).click();

  await daemon.waitForRequest("pty_open", (request) =>
    request.params.sessionId === "session-alpha" &&
    request.params.cwd === "/tmp/puffer-alpha"
  );
  await expect(page.getByRole("tab", { name: /Terminal 1/ })).toBeVisible();

  await page.getByRole("button", { name: /Beta terminal/ }).first().click();

  await daemon.waitForRequest("pty_list", (request) =>
    request.params.sessionId === "session-beta"
  );
  await daemon.waitForRequest("pty_open", (request) =>
    request.params.sessionId === "session-beta" &&
    request.params.cwd === "/tmp/puffer-beta"
  );
});

test("Terminal decodes UTF-8 PTY output frames", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: /Browser regression/ }).first().click();
  await page.locator(".pf-agent-tabs").getByRole("button", { name: "Terminal", exact: true }).click();
  await daemon.waitForRequest("pty_open", (request) => request.params.sessionId === "session-browser");
  await daemon.waitForRequest("pty_replay", (request) => request.params.ptyId === "pty-1");

  daemon.emit("pty:pty-1:data", {
    seq: 1,
    data: Buffer.from("hello 你好\n", "utf8").toString("base64")
  });

  await expect(page.locator(".xterm-rows")).toContainText("hello 你好");
  await expect(page.locator(".xterm-rows")).not.toContainText("ä½");
});

test("Terminal decodes UTF-8 split across PTY output frames", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: /Browser regression/ }).first().click();
  await page.locator(".pf-agent-tabs").getByRole("button", { name: "Terminal", exact: true }).click();
  await daemon.waitForRequest("pty_open", (request) => request.params.sessionId === "session-browser");
  await daemon.waitForRequest("pty_replay", (request) => request.params.ptyId === "pty-1");

  const bytes = Buffer.from("split 你好\n", "utf8");
  daemon.emit("pty:pty-1:data", {
    seq: 1,
    data: bytes.subarray(0, 8).toString("base64")
  });
  daemon.emit("pty:pty-1:data", {
    seq: 2,
    data: bytes.subarray(8).toString("base64")
  });

  await expect(page.locator(".xterm-rows")).toContainText("split 你好");
  await expect(page.locator(".xterm-rows")).not.toContainText("�");
});

test("late Terminal focus does not reattach a switched session", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-alpha-focus-terminal",
        displayName: "Alpha focus terminal",
        title: "Alpha focus terminal",
        cwd: "/tmp/puffer-alpha",
        folderPath: "/tmp/puffer-alpha",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        timeline: []
      },
      {
        sessionId: "session-beta-focus-terminal",
        displayName: "Beta focus terminal",
        title: "Beta focus terminal",
        cwd: "/tmp/puffer-beta",
        folderPath: "/tmp/puffer-beta",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        timeline: []
      }
    ]
  });
  daemon.delayResponse("pty_focus", (request) => request.params.ptyId === "pty-1", 220);
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: /Alpha focus terminal/ }).first().click();
  await page
    .locator(".pf-agent-tabs")
    .getByRole("button", { name: "Terminal", exact: true })
    .click();
  await daemon.waitForRequest("pty_open", (request) =>
    request.params.sessionId === "session-alpha-focus-terminal"
  );
  await daemon.waitForRequest("pty_focus", (request) => request.params.ptyId === "pty-1");

  await page.getByRole("button", { name: /Beta focus terminal/ }).first().click();
  await daemon.waitForRequest("pty_open", (request) =>
    request.params.sessionId === "session-beta-focus-terminal"
  );
  await daemon.waitForRequest("pty_focus", (request) => request.params.ptyId === "pty-2");
  await expect(page.locator(".pf-terminal-host")).toBeVisible();

  await page.waitForTimeout(260);
  await page.locator(".pf-terminal-host").click();
  await page.keyboard.type("b");

  await daemon.waitForRequest("pty_write", (request) => request.params.ptyId === "pty-2");
  expect(
    daemon.requests.filter((request) =>
      request.method === "pty_write" && request.params.ptyId === "pty-1"
    )
  ).toHaveLength(0);
});

test("late Terminal close failures do not leak into a switched session", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-alpha-close-terminal",
        displayName: "Alpha close terminal",
        title: "Alpha close terminal",
        cwd: "/tmp/puffer-alpha",
        folderPath: "/tmp/puffer-alpha",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        timeline: []
      },
      {
        sessionId: "session-beta-close-terminal",
        displayName: "Beta close terminal",
        title: "Beta close terminal",
        cwd: "/tmp/puffer-beta",
        folderPath: "/tmp/puffer-beta",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: /Alpha close terminal/ }).first().click();
  await page.locator(".pf-agent-tabs").getByRole("button", { name: "Terminal", exact: true }).click();
  await daemon.waitForRequest("pty_open", (request) =>
    request.params.sessionId === "session-alpha-close-terminal"
  );
  await expect(page.getByRole("tab", { name: /Terminal 1/ })).toBeVisible();

  daemon.delayFailure(
    "pty_close",
    (request) => request.params.ptyId === "pty-1",
    "alpha terminal close failed after session switch",
    180
  );
  await page.getByRole("button", { name: "Close Terminal 1" }).click();
  await daemon.waitForRequest("pty_close", (request) => request.params.ptyId === "pty-1");

  await page.getByRole("button", { name: /Beta close terminal/ }).first().click();
  await daemon.waitForRequest("pty_open", (request) =>
    request.params.sessionId === "session-beta-close-terminal"
  );
  await expect(page.getByRole("tab", { name: /Terminal 1/ })).toBeVisible();
  await expect(page.locator(".pf-terminal-host")).toBeVisible();

  await page.waitForTimeout(240);
  await expect(page.getByText("Terminal failed")).toHaveCount(0);
  await expect(page.getByText(/alpha terminal close failed/)).toHaveCount(0);
  await expect(page.locator(".pf-terminal-host")).toBeVisible();
});

test("Terminal input keeps global find shortcuts while focused", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: /Browser regression/ }).first().click();
  await page.locator(".pf-agent-tabs").getByRole("button", { name: "Terminal", exact: true }).click();
  await daemon.waitForRequest("pty_open");

  const terminalHost = page.locator(".pf-terminal-host");
  await expect(terminalHost).toBeVisible();
  await terminalHost.click();
  await page.keyboard.press("Control+F");

  await expect(page.getByRole("search", { name: "Find in agent view" })).toHaveCount(0);
});

test("clicking the active Terminal tab does not reattach the PTY", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: /Browser regression/ }).first().click();
  await page.locator(".pf-agent-tabs").getByRole("button", { name: "Terminal", exact: true }).click();
  await daemon.waitForRequest("pty_open", (request) => request.params.sessionId === "session-browser");
  await daemon.waitForRequest("pty_focus", (request) => request.params.ptyId === "pty-1");
  await daemon.waitForRequest("pty_replay", (request) => request.params.ptyId === "pty-1");

  const focusCount = daemon.requests.filter(
    (request) => request.method === "pty_focus" && request.params.ptyId === "pty-1"
  ).length;
  const replayCount = daemon.requests.filter(
    (request) => request.method === "pty_replay" && request.params.ptyId === "pty-1"
  ).length;

  const activeTab = page.getByRole("tab", { name: /Terminal 1/ });
  await activeTab.click();
  await activeTab.click();
  await page.waitForTimeout(50);

  expect(
    daemon.requests.filter(
      (request) => request.method === "pty_focus" && request.params.ptyId === "pty-1"
    )
  ).toHaveLength(focusCount);
  expect(
    daemon.requests.filter(
      (request) => request.method === "pty_replay" && request.params.ptyId === "pty-1"
    )
  ).toHaveLength(replayCount);
});

test("Terminal new tab ignores repeated clicks while create is in flight", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: /Browser regression/ }).first().click();
  await page.locator(".pf-agent-tabs").getByRole("button", { name: "Terminal", exact: true }).click();
  await daemon.waitForRequest("pty_open", (request) => request.params.sessionId === "session-browser");
  await expect(page.getByRole("tab", { name: /Terminal 1/ })).toBeVisible();

  const openedBefore = daemon.requests.filter((request) => request.method === "pty_open").length;
  daemon.delayResponse(
    "pty_open",
    (request) =>
      request.params.sessionId === "session-browser" &&
      request.params.title === "Terminal 2",
    500
  );
  await page.getByRole("button", { name: "New terminal" }).evaluate((button) => {
    (button as HTMLButtonElement).click();
    (button as HTMLButtonElement).click();
  });

  const request = await daemon.waitForRequest(
    "pty_open",
    (request) =>
      request.params.sessionId === "session-browser" &&
      request.params.title === "Terminal 2"
  );
  expect(request.params.cwd).toBe("/tmp/puffer");
  await page.waitForTimeout(50);
  expect(daemon.requests.filter((request) => request.method === "pty_open")).toHaveLength(
    openedBefore + 1
  );
});

test("stale Terminal resize observers do not resize a previous PTY", async ({ page }) => {
  await page.addInitScript(() => {
    type ResizeCallback = ResizeObserverCallback;
    const callbacks: ResizeCallback[] = [];
    class ManualResizeObserver {
      constructor(callback: ResizeCallback) {
        callbacks.push(callback);
      }

      observe() {}
      unobserve() {}
      disconnect() {}
    }

    const target = window as unknown as {
      ResizeObserver: typeof ResizeObserver;
      __triggerTerminalResizeObserver: (index: number) => void;
    };
    target.ResizeObserver = ManualResizeObserver as unknown as typeof ResizeObserver;
    target.__triggerTerminalResizeObserver = (index: number) => {
      callbacks[index]?.([], {} as ResizeObserver);
    };
  });

  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: /Browser regression/ }).first().click();
  await page.locator(".pf-agent-tabs").getByRole("button", { name: "Terminal", exact: true }).click();
  await daemon.waitForRequest("pty_open", (request) => request.params.title === "Terminal 1");
  await daemon.waitForRequest("pty_resize", (request) => request.params.ptyId === "pty-1");
  const firstPtyResizeCount = daemon.requests.filter((request) =>
    request.method === "pty_resize" && request.params.ptyId === "pty-1"
  ).length;

  await page.getByRole("button", { name: "New terminal" }).click();
  await daemon.waitForRequest("pty_open", (request) => request.params.title === "Terminal 2");
  await daemon.waitForRequest("pty_resize", (request) => request.params.ptyId === "pty-2");

  await page.evaluate(() => {
    (window as unknown as { __triggerTerminalResizeObserver: (index: number) => void })
      .__triggerTerminalResizeObserver(0);
  });
  await page.waitForTimeout(50);

  expect(
    daemon.requests.filter((request) =>
      request.method === "pty_resize" && request.params.ptyId === "pty-1"
    )
  ).toHaveLength(firstPtyResizeCount);
});

test("Terminal close ignores repeated clicks while close is in flight", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: /Browser regression/ }).first().click();
  await page.locator(".pf-agent-tabs").getByRole("button", { name: "Terminal", exact: true }).click();
  await daemon.waitForRequest("pty_open", (request) => request.params.sessionId === "session-browser");
  await expect(page.getByRole("tab", { name: /Terminal 1/ })).toBeVisible();

  daemon.delayResponse("pty_close", () => true, 500);
  await page.getByRole("button", { name: "Close Terminal 1" }).evaluate((button) => {
    (button as HTMLButtonElement).click();
    (button as HTMLButtonElement).click();
  });

  const request = await daemon.waitForRequest("pty_close");
  expect(request.params.ptyId).toBe("pty-1");
  await expect(page.getByRole("button", { name: "Close Terminal 1" })).toBeDisabled();
  await page.waitForTimeout(50);
  expect(daemon.requests.filter((request) => request.method === "pty_close")).toHaveLength(1);
});

test("Terminal input is ignored while the active PTY is closing", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: /Browser regression/ }).first().click();
  await page.locator(".pf-agent-tabs").getByRole("button", { name: "Terminal", exact: true }).click();
  await daemon.waitForRequest("pty_open", (request) => request.params.sessionId === "session-browser");
  await expect(page.getByRole("tab", { name: /Terminal 1/ })).toBeVisible();

  daemon.delayResponse("pty_close", () => true, 500);
  await page.getByRole("button", { name: "Close Terminal 1" }).click();
  await daemon.waitForRequest("pty_close", (request) => request.params.ptyId === "pty-1");

  await page.locator(".pf-terminal-host").click();
  await page.keyboard.type("x");
  await page.waitForTimeout(80);

  expect(
    daemon.requests.filter((request) => request.method === "pty_write")
  ).toHaveLength(0);
});

test("Terminal tab close controls include tab titles", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: /Browser regression/ }).first().click();
  await page.locator(".pf-agent-tabs").getByRole("button", { name: "Terminal", exact: true }).click();
  await daemon.waitForRequest("pty_open", (request) => request.params.title === "Terminal 1");

  await page.getByRole("button", { name: "New terminal" }).click();
  await daemon.waitForRequest("pty_open", (request) => request.params.title === "Terminal 2");

  await expect(page.getByRole("button", { name: "Close Terminal 1" })).toHaveAttribute(
    "title",
    "Close Terminal 1"
  );
  await expect(page.getByRole("button", { name: "Close Terminal 2" })).toHaveAttribute(
    "title",
    "Close Terminal 2"
  );
});

test("Terminal close failure keeps the tab retryable", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: /Browser regression/ }).first().click();
  await page.locator(".pf-agent-tabs").getByRole("button", { name: "Terminal", exact: true }).click();
  await daemon.waitForRequest("pty_open", (request) => request.params.sessionId === "session-browser");
  await expect(page.getByRole("tab", { name: /Terminal 1/ })).toBeVisible();

  daemon.failNext("pty_close", "pty close channel closed");
  await page.getByRole("button", { name: "Close Terminal 1" }).click();

  const request = await daemon.waitForRequest("pty_close");
  expect(request.params.ptyId).toBe("pty-1");
  await expect(page.getByRole("tab", { name: /Terminal 1/ })).toBeVisible();
  await expect(page.getByText("Terminal failed")).toBeVisible();
  await expect(page.getByText(/pty close channel closed/)).toBeVisible();
});

test("Terminal inactive close failure keeps the active terminal usable", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: /Browser regression/ }).first().click();
  await page.locator(".pf-agent-tabs").getByRole("button", { name: "Terminal", exact: true }).click();
  await daemon.waitForRequest("pty_open", (request) => request.params.title === "Terminal 1");
  await expect(page.getByRole("tab", { name: /Terminal 1/ })).toBeVisible();

  await page.getByRole("button", { name: "New terminal" }).click();
  await daemon.waitForRequest("pty_open", (request) => request.params.title === "Terminal 2");
  await expect(page.getByRole("tab", { name: /Terminal 2/ })).toHaveAttribute("aria-selected", "true");
  await expect(page.locator(".pf-terminal-host")).toBeVisible();

  daemon.failNext("pty_close", "inactive close channel closed");
  await page.getByRole("button", { name: "Close Terminal 1" }).click();

  const request = await daemon.waitForRequest("pty_close");
  expect(request.params.ptyId).toBe("pty-1");
  await expect(page.getByRole("tab", { name: /Terminal 1/ })).toBeVisible();
  await expect(page.getByRole("tab", { name: /Terminal 2/ })).toHaveAttribute("aria-selected", "true");
  await expect(page.locator(".pf-terminal-host")).toBeVisible();
  await expect(page.getByText("Terminal failed")).toHaveCount(0);
  await expect(page.getByText(/inactive close channel closed/)).toBeVisible();
});

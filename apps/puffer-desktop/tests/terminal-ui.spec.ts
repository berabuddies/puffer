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
  await page.waitForTimeout(50);
  expect(daemon.requests.filter((request) => request.method === "pty_close")).toHaveLength(1);
});

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

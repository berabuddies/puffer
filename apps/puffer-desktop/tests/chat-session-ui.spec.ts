import { expect, test } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";

const baseTime = Date.now();

test("turn completion reload does not leak live chat into a newly selected session", async ({
  page
}) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-alpha",
        displayName: "Alpha session",
        title: "Alpha session",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "alpha-seed",
            text: "Alpha seed",
            createdAtMs: baseTime - 30_000
          }
        ]
      },
      {
        sessionId: "session-beta",
        displayName: "Beta session",
        title: "Beta session",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "beta-seed",
            text: "Beta seed",
            createdAtMs: baseTime - 90_000
          }
        ]
      }
    ]
  });

  await daemon.install(page);
  await daemon.open(page);

  await expect(page.getByRole("button", { name: /Alpha session/ }).first()).toBeVisible();
  await page.getByRole("button", { name: /Alpha session/ }).first().click();
  await expect(page.getByText("Alpha seed")).toBeVisible();

  await page.locator(".pf-composer textarea").fill("Race from alpha");
  await page.getByRole("button", { name: "Send" }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-alpha" &&
      request.params.message === "Race from alpha"
  );

  daemon.delayResponse(
    "load_session_detail",
    (request) => request.params.sessionId === "session-alpha",
    500
  );
  daemon.emit("session:session-alpha:event", {
    type: "turn-complete",
    turnId: "turn-session-alpha",
    assistantText: "Alpha completion should stay with alpha"
  });

  await page.getByRole("button", { name: /Beta session/ }).first().click();
  await expect(page.getByText("Beta seed")).toBeVisible();

  await page.waitForTimeout(650);
  await expect(page.getByText("Beta seed")).toBeVisible();
  await expect(page.getByText("Alpha completion should stay with alpha")).toHaveCount(0);
  await expect(page.getByText("Race from alpha")).toHaveCount(0);
});

test("resolved transcript permissions do not reappear as pending approvals", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-resolved-permission",
        displayName: "Resolved permission",
        title: "Resolved permission",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 3,
        timeline: [
          {
            kind: "user_message",
            id: "perm-user",
            text: "Run the command.",
            createdAtMs: baseTime - 50_000
          },
          {
            kind: "permission_dialog",
            id: "perm-allowed",
            toolId: "bash",
            state: "allowed",
            summary: "bash was allowed",
            reason: "User approved this earlier.",
            inputText: "echo ok",
            createdAtMs: baseTime - 45_000
          },
          {
            kind: "assistant_message",
            id: "perm-assistant",
            text: "The command finished.",
            createdAtMs: baseTime - 40_000
          }
        ]
      }
    ]
  });

  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: /Resolved permission/ }).first().click();
  await expect(page.getByText("The command finished.")).toBeVisible();
  await expect(page.getByText("Approval needed")).toHaveCount(0);
  await expect(page.locator(".pf-agent-status-pill")).toHaveAttribute("data-status", "idle");
});

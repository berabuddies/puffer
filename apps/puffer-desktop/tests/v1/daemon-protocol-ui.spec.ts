import { expect, type Page, test } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";

async function openRegressionAgent(page: Page): Promise<void> {
  await page
    .locator(".pf-sidebar-agents-list")
    .getByRole("button", { name: /^Browser regression\b/ })
    .click();
}

test("desktop client speaks the real daemon WebSocket protocol", async ({ page }) => {
  const daemon = new FakeDaemon({ protocol: "real" });
  await daemon.install(page);
  await daemon.open(page);

  await expect.poll(() => daemon.socketUrls.length).toBeGreaterThan(0);
  expect(daemon.socketUrls[0]).toContain("token=test");

  await openRegressionAgent(page);
  await page.locator(".pf-composer textarea").fill("Use real daemon framing");
  await page.getByRole("button", { name: "Send" }).click();

  const turnRequest = await daemon.waitForRequest(
    "run_agent_turn",
    (request) => request.params.message === "Use real daemon framing"
  );
  expect(typeof turnRequest.id).toBe("string");
  expect(turnRequest.params).toMatchObject({
    sessionId: "session-browser",
    providerId: "codex",
    modelId: "test-model"
  });

  daemon.setSessionTimeline("session-browser", [
    {
      kind: "user_message",
      id: "real-user",
      text: "Use real daemon framing",
      createdAtMs: Date.now() - 1000
    },
    {
      kind: "assistant_message",
      id: "real-assistant",
      text: "Real daemon completion arrived.",
      createdAtMs: Date.now()
    }
  ]);
  daemon.emit("session:session-browser:event", {
    type: "turn-complete",
    turnId: "turn-session-browser",
    assistantText: "Real daemon completion arrived."
  });

  await expect(page.getByText("Real daemon completion arrived.")).toBeVisible();
});

test("backend reconnect button reports failed retries", async ({ page }) => {
  const daemon = new FakeDaemon({ protocol: "real" });
  await daemon.install(page);
  await daemon.open(page);

  await expect.poll(() => daemon.socketUrls.length).toBeGreaterThan(0);
  await daemon.dropConnections();

  const banner = page.locator(".connection-banner");
  await expect(banner).toContainText("Puffer backend disconnected.");

  await banner.getByRole("button", { name: "Reconnect backend" }).click();
  await expect(banner).toContainText("Reconnect failed:");
  await expect(banner).toContainText("Unable to connect to Puffer daemon");

  daemon.allowConnections();
  await banner.getByRole("button", { name: "Reconnect backend" }).click();

  await expect.poll(() => daemon.socketUrls.length).toBeGreaterThan(1);
  await expect(page.locator(".connection-banner")).toHaveCount(0);
});

import { expect, type Page, test } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";

// UI-behavior rows of the agentenv/monorepo#767 test matrix for the unified
// outbound gate. Each ConnectorActionDraft tool call renders an approval card
// (ToolCard.svelte -> connector-draft render) that must gate the send behind an
// explicit Approve, support Cancel, and route rejection sentinels to the right
// terminal / warning pill instead of a generic error.
//
// Drafts are surfaced as live tool invocations on an in-flight turn, matching
// the proven inline `.pf-tool` streaming pattern used elsewhere in
// chat-session-ui.spec.ts (turn-start + tool-invocations, turn kept active so
// the card stays inline and is not rolled up under the "Agent activity" group).

const baseTime = Date.now();

type DraftFields = {
  draftId: string;
  version?: number;
  recipient?: string;
  recipientSource?: "stamped" | "model";
  message?: string;
  status?: string;
};

function draftInvocation(fields: DraftFields): Record<string, unknown> {
  const draft = {
    id: fields.draftId,
    version: fields.version ?? 1,
    status: fields.status ?? "draft_ready",
    connectorSlug: "telegram-login",
    connectionSlug: "telegram-user",
    action: "send_message",
    recipient: fields.recipient ?? "@alice",
    recipientSource: fields.recipientSource ?? "model",
    message: fields.message ?? "Deploy is finished — shipping now."
  };
  return {
    callId: `call-${fields.draftId}`,
    toolId: "ConnectorActionDraft",
    input: JSON.stringify({ action: "send_message" }),
    output: JSON.stringify({ draft }),
    success: true
  };
}

function daemonWithSession(sessionId: string): FakeDaemon {
  return new FakeDaemon({
    sessions: [
      {
        sessionId,
        displayName: sessionId,
        title: sessionId,
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });
}

async function openSession(page: Page, name: RegExp): Promise<void> {
  await page.getByRole("button", { name }).first().click();
}

/** Start a live turn and stream the given draft invocations as inline cards. */
function streamDrafts(
  daemon: FakeDaemon,
  sessionId: string,
  turnId: string,
  invocations: Record<string, unknown>[]
): void {
  daemon.emit(`session:${sessionId}:event`, { type: "turn-start", turnId });
  daemon.emit(`session:${sessionId}:event`, {
    type: "tool-invocations",
    turnId,
    invocations
  });
}

function executeRequests(daemon: FakeDaemon) {
  return daemon.requests.filter((request) => request.method === "outbound_action_execute");
}

test("agent 会话直发: draft renders an approval card and only sends on approve", async ({
  page
}) => {
  const sessionId = "session-outbound-send";
  const daemon = daemonWithSession(sessionId);
  await daemon.install(page);
  await daemon.open(page);
  await openSession(page, /session-outbound-send/);

  streamDrafts(daemon, sessionId, "turn-send", [
    draftInvocation({
      draftId: "oa-send-1",
      version: 3,
      recipientSource: "model",
      message: "Deploy is finished — shipping now."
    })
  ]);

  const card = page.locator(".pf-connector-draft");
  await expect(card).toBeVisible();
  await expect(card).toHaveAttribute("data-state", "idle");
  // Model-chosen recipient carries the destructive-styled badge.
  await expect(card.locator(".pf-connector-draft-source")).toHaveText("model-chosen");
  await expect(card.locator("blockquote")).toContainText("Deploy is finished");

  const send = card.locator(".pf-connector-draft-send");
  const cancel = card.locator(".pf-connector-draft-cancel");
  await expect(send).toBeEnabled();
  await expect(send).toContainText("Approve and send");
  await expect(cancel).toBeEnabled();

  // Nothing is sent until the user approves.
  expect(executeRequests(daemon)).toHaveLength(0);

  const executePromise = daemon.waitForRequest(
    "outbound_action_execute",
    (request) => request.params.action_id === "oa-send-1"
  );
  await send.click();
  const executeRequest = await executePromise;

  expect(executeRequest.params.action_id).toBe("oa-send-1");
  expect(executeRequest.params.version).toBe(3);
  expect(executeRequest.params.approved_message).toBe("Deploy is finished — shipping now.");
  expect(typeof executeRequest.params.client_request_id).toBe("string");

  await expect(card).toHaveAttribute("data-state", "sent");
  await expect(send).toContainText("Sent");
  await expect(send).toBeDisabled();
  // Exactly one execute call for the single approval.
  expect(executeRequests(daemon)).toHaveLength(1);
});

test("草稿取消: cancel calls outbound_action_cancel and pins the cancelled pill", async ({
  page
}) => {
  const sessionId = "session-outbound-cancel";
  const daemon = daemonWithSession(sessionId);
  await daemon.install(page);
  await daemon.open(page);
  await openSession(page, /session-outbound-cancel/);

  streamDrafts(daemon, sessionId, "turn-cancel", [
    draftInvocation({ draftId: "oa-cancel-1", version: 2, message: "Cancel me." })
  ]);

  const card = page.locator(".pf-connector-draft");
  await expect(card).toBeVisible();

  const send = card.locator(".pf-connector-draft-send");
  const cancel = card.locator(".pf-connector-draft-cancel");

  const cancelPromise = daemon.waitForRequest(
    "outbound_action_cancel",
    (request) => request.params.action_id === "oa-cancel-1"
  );
  await cancel.click();
  const cancelRequest = await cancelPromise;
  expect(cancelRequest.params.action_id).toBe("oa-cancel-1");
  expect(cancelRequest.params.version).toBe(2);

  await expect(card).toHaveAttribute("data-state", "cancelled");
  await expect(send).toContainText("Cancelled");
  // Neither action is clickable after cancellation.
  await expect(send).toBeDisabled();
  await expect(cancel).toBeDisabled();

  // A subsequent status poll returning "cancelled" must keep it terminal — the
  // fake daemon now reports cancelled for this action, so no resurrection UI.
  await page.waitForTimeout(200);
  await expect(card).toHaveAttribute("data-state", "cancelled");
  await expect(send).toBeDisabled();
  expect(executeRequests(daemon)).toHaveLength(0);
});

test("取消后重新要求: a second distinct draft renders fresh while the first stays cancelled", async ({
  page
}) => {
  const sessionId = "session-outbound-redraft";
  const daemon = daemonWithSession(sessionId);
  await daemon.install(page);
  await daemon.open(page);
  await openSession(page, /session-outbound-redraft/);

  // Two distinct drafts (different draftIds) mounted in the same conversation.
  streamDrafts(daemon, sessionId, "turn-redraft", [
    draftInvocation({ draftId: "oa-redraft-1", recipient: "@first", message: "First draft." }),
    draftInvocation({ draftId: "oa-redraft-2", recipient: "@second", message: "Second draft." })
  ]);

  const firstCard = page.locator(".pf-connector-draft").filter({ hasText: "First draft." });
  const secondCard = page.locator(".pf-connector-draft").filter({ hasText: "Second draft." });
  await expect(firstCard).toBeVisible();
  await expect(secondCard).toBeVisible();

  // Cancel the first draft only.
  const cancelPromise = daemon.waitForRequest(
    "outbound_action_cancel",
    (request) => request.params.action_id === "oa-redraft-1"
  );
  await firstCard.locator(".pf-connector-draft-cancel").click();
  await cancelPromise;

  await expect(firstCard).toHaveAttribute("data-state", "cancelled");
  await expect(firstCard.locator(".pf-connector-draft-send")).toBeDisabled();

  // The second card is independent and stays a fresh, approvable draft.
  await expect(secondCard).toHaveAttribute("data-state", "idle");
  await expect(secondCard.locator(".pf-connector-draft-send")).toBeEnabled();
  await expect(secondCard.locator(".pf-connector-draft-send")).toContainText("Approve and send");
});

test("error sentinel: outbound_action_expired routes to the Expired terminal pill", async ({
  page
}) => {
  const sessionId = "session-outbound-expired";
  const daemon = daemonWithSession(sessionId);
  await daemon.install(page);
  await daemon.open(page);
  await openSession(page, /session-outbound-expired/);

  streamDrafts(daemon, sessionId, "turn-expired", [
    draftInvocation({ draftId: "oa-expired-1", message: "Too late." })
  ]);

  const card = page.locator(".pf-connector-draft");
  await expect(card).toBeVisible();

  // Execute rejects with the expiry sentinel.
  daemon.failNext("outbound_action_execute", "outbound_action_expired");
  await card.locator(".pf-connector-draft-send").click();

  await expect(card).toHaveAttribute("data-state", "expired");
  await expect(card.locator(".pf-connector-draft-send")).toContainText("Expired");
  await expect(card.locator(".pf-connector-draft-send")).toBeDisabled();
  // Terminal expiry is not surfaced as a generic red error string.
  await expect(card.locator(".pf-connector-draft-error")).toHaveCount(0);
});

test("uncertain status: card shows the uncertain warning and is not left idle", async ({
  page
}) => {
  const sessionId = "session-outbound-uncertain";
  const daemon = daemonWithSession(sessionId);
  // Persisted status reads back as uncertain, so the mount-time status refresh
  // keeps the card in the warning state rather than re-enabling a clean idle
  // approve button.
  daemon.seedOutboundAction("oa-uncertain-1", { status: "uncertain", version: 1 });
  await daemon.install(page);
  await daemon.open(page);
  await openSession(page, /session-outbound-uncertain/);

  streamDrafts(daemon, sessionId, "turn-uncertain", [
    draftInvocation({ draftId: "oa-uncertain-1", status: "uncertain", message: "Maybe sent." })
  ]);

  const card = page.locator(".pf-connector-draft");
  await expect(card).toBeVisible();
  await expect(card.locator(".pf-connector-draft-error")).toContainText(
    "Send status is uncertain"
  );
  // Not left in the pristine idle state that would imply a safe re-send.
  await expect(card).toHaveAttribute("data-state", "uncertain");
  // Approve is blocked until the duplicate risk is resolved; cancel stays
  // available (the server allows cancelling an uncertain action).
  await expect(card.locator(".pf-connector-draft-send")).toBeDisabled();
  await expect(card.locator(".pf-connector-draft-cancel")).toBeEnabled();
});

test("stamped recipient renders without the model-chosen badge", async ({ page }) => {
  const sessionId = "session-outbound-stamped";
  const daemon = daemonWithSession(sessionId);
  await daemon.install(page);
  await daemon.open(page);
  await openSession(page, /session-outbound-stamped/);

  streamDrafts(daemon, sessionId, "turn-stamped", [
    draftInvocation({
      draftId: "oa-stamped-1",
      recipient: "@stamped",
      recipientSource: "stamped",
      message: "Stamped recipient."
    })
  ]);

  const card = page.locator(".pf-connector-draft");
  await expect(card).toBeVisible();
  await expect(card.locator(".pf-connector-draft-recipient")).toContainText("@stamped");
  await expect(card.locator(".pf-connector-draft-source")).toHaveCount(0);
});

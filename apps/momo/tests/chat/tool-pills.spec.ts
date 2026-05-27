/**
 * Task 4 — tool call pills.
 *
 * When the agent emits `tool-calls-requested`, a `.tool-call-pill` renders
 * in `data-status="running"`. When the matching `tool-invocations` event
 * lands, the pill flips to `data-status="success"` or `"failed"` keyed by
 * `callId` (so DOM identity is preserved across the status transition —
 * tests use `locateToolPill({ callId })` to observe the same node).
 *
 * Unmapped tool ids (the only kind today — `toolLabels.ts` ships empty)
 * fall through to debug-flag-controlled copy. These tests run under
 * `vite dev`, so `SHOW_RAW_AGENT_ACTIVITY` is true and the pill reads
 * `Calling: <toolId>`.
 *
 * Edge cases covered:
 *   - synth (invocation without prior request)
 *   - callId reuse across turns (two pills coexist)
 *   - multiple tools in one request batch
 *   - empty arrays are no-ops
 *   - tool pill is inserted BEFORE the assistant bubble for that turn
 */
import { expect, test, type Page } from "@playwright/test";
import { FakeDaemon } from "../support/fakeDaemon";
import { bootOnboarded } from "../support/bootHelpers";
import {
  emitTurnStart,
  emitTextDelta,
  emitToolRequest,
  emitToolInvocation,
  emitTurnComplete
} from "../support/chatEmit";
import { locateAssistantBubble, locateToolPill } from "../support/chatLocators";

/**
 * Boilerplate: type a prompt into the home composer, wait for the
 * synthesised sessionId, kick off a turn-start. Local to this spec —
 * mirrors the helper in thinking.spec.ts.
 */
async function bootAndStartTurn(
  page: Page,
  daemon: FakeDaemon,
  prompt = "use a tool"
): Promise<{ sessionId: string; turnId: string }> {
  await page.getByLabel("Message").fill(prompt);
  await page.getByLabel("Message").press("Enter");
  await daemon.waitForRequest("create_session");
  const sessionId = "session-created-1";
  await daemon.waitForRequest("run_agent_turn");
  const turnId = `turn-${sessionId}`;
  emitTurnStart(daemon, { sessionId, turnId });
  return { sessionId, turnId };
}

test("tool-calls-requested renders a pending pill", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);
  const { sessionId, turnId } = await bootAndStartTurn(page, daemon);

  emitToolRequest(daemon, {
    sessionId,
    turnId,
    callId: "c-running-1",
    toolId: "wallet.balance"
  });

  await expect(locateToolPill(page, { status: "running" })).toBeVisible();
});

test("DEV label fallback shows 'Calling: <toolId>' for unmapped tools", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);
  const { sessionId, turnId } = await bootAndStartTurn(page, daemon);

  emitToolRequest(daemon, {
    sessionId,
    turnId,
    callId: "c-dev-1",
    toolId: "unknown.tool"
  });

  const pill = locateToolPill(page, { callId: "c-dev-1" });
  // The label lives inside the wrapped `ToolBlock`'s `.tool-block__label`.
  await expect(pill).toContainText("Calling: unknown.tool");
});

test("tool-invocations flips running → success (DOM identity preserved)", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);
  const { sessionId, turnId } = await bootAndStartTurn(page, daemon);

  emitToolRequest(daemon, {
    sessionId,
    turnId,
    callId: "c-flip-1",
    toolId: "wallet.send"
  });

  // Same locator across the status flip — keys on callId so the assertion
  // observes the same DOM node as it transitions from running to success.
  const pill = locateToolPill(page, { callId: "c-flip-1" });
  await expect(pill).toHaveAttribute("data-status", "running");

  emitToolInvocation(daemon, {
    sessionId,
    turnId,
    callId: "c-flip-1",
    toolId: "wallet.send",
    success: true,
    output: { ok: true }
  });

  await expect(pill).toHaveAttribute("data-status", "success");
});

test("failed pill renders red border", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);
  const { sessionId, turnId } = await bootAndStartTurn(page, daemon);

  emitToolRequest(daemon, {
    sessionId,
    turnId,
    callId: "c-fail-1",
    toolId: "wallet.send"
  });
  emitToolInvocation(daemon, {
    sessionId,
    turnId,
    callId: "c-fail-1",
    toolId: "wallet.send",
    success: false,
    output: { error: "rejected" }
  });

  const pill = locateToolPill(page, { callId: "c-fail-1", status: "failed" });
  await expect(pill).toBeVisible();
  // The `data-status="failed"` rule cascades to the inner `.tool-block`'s
  // border-color via `:global(.tool-block)`. Observing border-color is more
  // brittle than the data-status attribute, so anchor on the attribute
  // (load-bearing) and the inner border-color (visual cue) together.
  const borderColor = await pill
    .locator(".tool-block")
    .evaluate((el) => getComputedStyle(el).borderColor);
  // `var(--color-danger, #c0392b)` resolves to rgb(192, 57, 43) when the
  // variable is unset (no theme value defined in V2 today).
  expect(borderColor).toMatch(/rgb\(192,\s*57,\s*43\)/);
});

test("callId reuse across turns: both pills coexist", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);
  const { sessionId, turnId: turn1 } = await bootAndStartTurn(page, daemon);

  // Turn 1 — first pill with callId "c1".
  emitToolRequest(daemon, {
    sessionId,
    turnId: turn1,
    callId: "c1",
    toolId: "tool.alpha"
  });
  emitToolInvocation(daemon, {
    sessionId,
    turnId: turn1,
    callId: "c1",
    toolId: "tool.alpha",
    success: true
  });
  emitTurnComplete(daemon, { sessionId, turnId: turn1 });

  // First pill must be observable and at terminal state before we drive
  // turn 2 so the test's intent stays clear (no concurrent flicker).
  await expect(locateToolPill(page, { toolId: "tool.alpha" })).toHaveAttribute(
    "data-status",
    "success"
  );

  // Turn 2 — REUSE the same callId "c1" but a different toolId.
  await page.getByLabel("Message").fill("do another thing");
  await page.getByLabel("Message").press("Enter");
  await daemon.waitForRequest(
    "run_agent_turn",
    (req) =>
      req.params.sessionId === sessionId && req.params.message === "do another thing"
  );
  const turn2 = `turn-${sessionId}-2`;
  emitTurnStart(daemon, { sessionId, turnId: turn2 });
  emitToolRequest(daemon, {
    sessionId,
    turnId: turn2,
    callId: "c1",
    toolId: "tool.beta"
  });

  // Both pills must coexist — the store de-dupes by (callId, turnId), so a
  // reused callId across two turns produces TWO pills, matching v1's
  // behaviour (apps/puffer-desktop/tests/chat-session-ui.spec.ts:1112).
  const allPills = locateToolPill(page);
  await expect(allPills).toHaveCount(2);
});

test("invocation without prior request synthesizes a terminal pill", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);
  const { sessionId, turnId } = await bootAndStartTurn(page, daemon);

  // Skip the request — emit only the invocation.
  emitToolInvocation(daemon, {
    sessionId,
    turnId,
    callId: "c-synth-1",
    toolId: "tool.synth",
    success: true,
    output: { synthesized: true }
  });

  const pill = locateToolPill(page, { callId: "c-synth-1" });
  await expect(pill).toBeVisible();
  await expect(pill).toHaveAttribute("data-status", "success");
});

test("multiple tools in one request batch render independently", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);
  const { sessionId, turnId } = await bootAndStartTurn(page, daemon);

  // Two requests in a single batch — emit raw so the test owns the shape.
  daemon.emit(`session:${sessionId}:event`, {
    type: "tool-calls-requested",
    turnId,
    requests: [
      { callId: "batch-a", toolId: "tool.a", input: {} },
      { callId: "batch-b", toolId: "tool.b", input: {} }
    ]
  });

  await expect(locateToolPill(page, { callId: "batch-a" })).toHaveAttribute(
    "data-status",
    "running"
  );
  await expect(locateToolPill(page, { callId: "batch-b" })).toHaveAttribute(
    "data-status",
    "running"
  );

  // Resolve A as success, B as failure — independent updates.
  daemon.emit(`session:${sessionId}:event`, {
    type: "tool-invocations",
    turnId,
    invocations: [
      { callId: "batch-a", toolId: "tool.a", success: true, output: {} },
      { callId: "batch-b", toolId: "tool.b", success: false, output: {} }
    ]
  });

  await expect(locateToolPill(page, { callId: "batch-a" })).toHaveAttribute(
    "data-status",
    "success"
  );
  await expect(locateToolPill(page, { callId: "batch-b" })).toHaveAttribute(
    "data-status",
    "failed"
  );
});

test("tool pill renders BEFORE the assistant final answer for the same turn", async ({
  page
}) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);
  const { sessionId, turnId } = await bootAndStartTurn(page, daemon);

  emitToolRequest(daemon, {
    sessionId,
    turnId,
    callId: "c-order-1",
    toolId: "tool.x"
  });
  emitToolInvocation(daemon, {
    sessionId,
    turnId,
    callId: "c-order-1",
    toolId: "tool.x",
    success: true
  });
  emitTextDelta(daemon, { sessionId, turnId, delta: "Final answer" });
  emitTurnComplete(daemon, { sessionId, turnId });

  const pill = locateToolPill(page, { callId: "c-order-1" });
  const bubble = locateAssistantBubble(page);
  // Wait for both to be in the DOM and visible.
  await expect(pill).toBeVisible();
  await expect(bubble).toContainText("Final answer");

  const pillBox = await pill.boundingBox();
  const bubbleBox = await bubble.boundingBox();
  expect(pillBox).not.toBeNull();
  expect(bubbleBox).not.toBeNull();
  // Vertical flex layout — the pill's top should sit above the bubble's
  // top by at least a few px. (Bounding boxes are absolute viewport coords.)
  expect(pillBox!.y).toBeLessThan(bubbleBox!.y);
});

test("empty requests / invocations arrays are no-ops", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);
  const { sessionId, turnId } = await bootAndStartTurn(page, daemon);

  // Baseline: zero pills before either event.
  await expect(locateToolPill(page)).toHaveCount(0);

  daemon.emit(`session:${sessionId}:event`, {
    type: "tool-calls-requested",
    turnId,
    requests: []
  });
  daemon.emit(`session:${sessionId}:event`, {
    type: "tool-invocations",
    turnId,
    invocations: []
  });

  // Give the renderer a beat to commit any (incorrect) work.
  await page.waitForTimeout(50);
  await expect(locateToolPill(page)).toHaveCount(0);
});

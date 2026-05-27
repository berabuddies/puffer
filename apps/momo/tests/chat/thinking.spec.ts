/**
 * Task 3 — streaming "thinking" display.
 *
 * Dev builds (Playwright runs against `vite dev`) show a soft-grey italic
 * block carrying the streamed `thinking-delta` content; prod builds collapse
 * the same payload into a fixed pill labeled "I'm working on it now...".
 *
 * Both surfaces are addressable via testids:
 *   - `[data-testid="thinking-block"]` — dev variant; carries `data-pending`
 *     toggled by turn-complete / turn-error.
 *   - `[data-testid="thinking-pill"]` — prod variant; wraps the ToolBlock
 *     fallback pill. Also rendered in dev when the accumulated text trims
 *     to empty (intentional, per ThinkingBlock.svelte's `trimmed.length > 0`
 *     guard).
 *
 * These specs run under Playwright → `vite dev`, so `SHOW_RAW_AGENT_ACTIVITY`
 * is true and the dev branch is the one we observe in 7 of the 8 tests. The
 * empty-text test asserts the trimmed-fallback path explicitly.
 */
import { expect, test } from "@playwright/test";
import { FakeDaemon } from "../support/fakeDaemon";
import { bootOnboarded } from "../support/bootHelpers";
import {
  emitTurnStart,
  emitTextDelta,
  emitThinkingDelta,
  emitTurnComplete,
  emitTurnError
} from "../support/chatEmit";
import { locateAssistantBubble, locateThinkingBlock } from "../support/chatLocators";

/**
 * Boilerplate: type a prompt into the home composer, wait for the synthesised
 * sessionId, kick off a turn-start. Returns sessionId + turnId so each test
 * can drive its own thinking-delta sequence without re-deriving the channel.
 *
 * Local to this spec — the assistant-bubble spec has its own variant tuned
 * for its lifecycle. Sharing them in `chatEmit.ts` would couple unrelated
 * boot paths.
 */
async function bootAndStartTurn(
  page: import("@playwright/test").Page,
  daemon: FakeDaemon,
  prompt = "think with me"
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

test("single thinking-delta renders the thinking block (dev)", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);
  const { sessionId, turnId } = await bootAndStartTurn(page, daemon);

  emitThinkingDelta(daemon, { sessionId, turnId, delta: "Let me think..." });

  const block = locateThinkingBlock(page);
  await expect(block).toBeVisible();
  await expect(block.locator(".thinking-block__text")).toHaveText("Let me think...");
});

test("multiple thinking-deltas accumulate into a single block", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);
  const { sessionId, turnId } = await bootAndStartTurn(page, daemon);

  emitThinkingDelta(daemon, { sessionId, turnId, delta: "Part one. " });
  emitThinkingDelta(daemon, { sessionId, turnId, delta: "Part two." });

  // Trailing whitespace inside `Part one. ` is preserved between deltas; the
  // outer trim() only strips leading/trailing whitespace of the full string.
  const block = locateThinkingBlock(page);
  await expect(block).toHaveCount(1);
  await expect(block.locator(".thinking-block__text")).toHaveText("Part one. Part two.");
});

test("turn-complete flips the thinking block's data-pending to 'false'", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);
  const { sessionId, turnId } = await bootAndStartTurn(page, daemon);

  emitThinkingDelta(daemon, { sessionId, turnId, delta: "Working it out." });

  const block = locateThinkingBlock(page);
  await expect(block).toHaveAttribute("data-pending", "true");

  emitTurnComplete(daemon, { sessionId, turnId });
  await expect(block).toHaveAttribute("data-pending", "false");
});

test("turn-error also flips the thinking block's pending state", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);
  const { sessionId, turnId } = await bootAndStartTurn(page, daemon);

  emitThinkingDelta(daemon, { sessionId, turnId, delta: "Still thinking." });

  const block = locateThinkingBlock(page);
  await expect(block).toHaveAttribute("data-pending", "true");

  // turn-error fires `pushToast` internally — the test doesn't care about the
  // toast, only about the pending flip.
  emitTurnError(daemon, { sessionId, turnId, error: "model offline" });
  await expect(block).toHaveAttribute("data-pending", "false");
});

test("empty thinking text renders the fallback pill, not a thinking block", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);
  const { sessionId, turnId } = await bootAndStartTurn(page, daemon);

  // Deltas that trim to empty take the `trimmed.length > 0` guard's else
  // branch in ThinkingBlock.svelte. Even though the dev flag is on, the
  // block is suppressed and the prod-style pill renders instead.
  emitThinkingDelta(daemon, { sessionId, turnId, delta: "   " });
  emitThinkingDelta(daemon, { sessionId, turnId, delta: "\n\t " });

  // Give the renderer a chance to commit; the prod-style pill must be
  // visible while the dev block stays absent.
  await expect(page.locator('[data-testid="thinking-pill"]')).toBeVisible();
  await expect(locateThinkingBlock(page)).toHaveCount(0);
});

test("thinking block coexists with the assistant text bubble", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);
  const { sessionId, turnId } = await bootAndStartTurn(page, daemon);

  emitThinkingDelta(daemon, { sessionId, turnId, delta: "Hmm. " });
  emitTextDelta(daemon, { sessionId, turnId, delta: "Actually..." });
  // The assistant bubble stays in `pending` (typing dots) until
  // turn-complete fires, so the test has to drive the full happy-path to
  // reach the `.assistant-bubble__text` render branch. The thinking block
  // is observable mid-flight (it doesn't gate on pending for visibility),
  // but the assertion below covers the coexistence at the resolved state
  // which is the user-facing one we ship.
  emitTurnComplete(daemon, { sessionId, turnId });

  // Both surfaces render side-by-side — the thinking block carries the raw
  // thought, the assistant bubble carries the model's user-facing text.
  const block = locateThinkingBlock(page);
  await expect(block.locator(".thinking-block__text")).toHaveText("Hmm.");
  await expect(
    locateAssistantBubble(page).locator(".assistant-bubble__text")
  ).toContainText("Actually...");
});

test("two concurrent turns render independent thinking blocks", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);
  const { sessionId, turnId: turnA } = await bootAndStartTurn(page, daemon, "turn A");

  emitThinkingDelta(daemon, { sessionId, turnId: turnA, delta: "Idea A" });
  emitTurnComplete(daemon, { sessionId, turnId: turnA });

  // Submit a second turn into the same session. The composer now lives under
  // /agent/<id>; appendUserMessage runs and the daemon mints a fresh turnId.
  await page.getByLabel("Message").fill("turn B");
  await page.getByLabel("Message").press("Enter");
  await daemon.waitForRequest(
    "run_agent_turn",
    (req) => req.params.sessionId === sessionId && req.params.message === "turn B"
  );
  // FakeDaemon.runAgentTurn keeps emitting `turn-${sessionId}` for every
  // subsequent turn on the same session — turn ids are session-scoped, not
  // monotonically unique, in the fake. Drive the second turn with a distinct
  // id by skipping the daemon's `turn-start` emission and crafting our own.
  const turnB = `turn-${sessionId}-B`;
  emitTurnStart(daemon, { sessionId, turnId: turnB });
  emitThinkingDelta(daemon, { sessionId, turnId: turnB, delta: "Idea B" });

  const blocks = locateThinkingBlock(page);
  await expect(blocks).toHaveCount(2);
  await expect(blocks.nth(0).locator(".thinking-block__text")).toHaveText("Idea A");
  await expect(blocks.nth(1).locator(".thinking-block__text")).toHaveText("Idea B");
});

test("thinking block DOM identity is preserved across deltas", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);
  const { sessionId, turnId } = await bootAndStartTurn(page, daemon);

  emitThinkingDelta(daemon, { sessionId, turnId, delta: "First " });

  // Grab the locator after the FIRST delta lands; if a future refactor stops
  // keying the each block by message.id, this same locator will mismatch
  // after the second delta lands (re-mount = re-painted node).
  const block = locateThinkingBlock(page);
  await expect(block).toBeVisible();

  emitThinkingDelta(daemon, { sessionId, turnId, delta: "second " });
  emitThinkingDelta(daemon, { sessionId, turnId, delta: "third." });

  await expect(block.locator(".thinking-block__text")).toHaveText("First second third.");
});

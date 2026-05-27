/**
 * Assistant bubble tests — happy-path migration plus planned coverage.
 *
 * Already migrated (lives in this file as a real test):
 *   - "home composer → create_session → run_agent_turn → streamed assistant text"
 *     (was apps/momo/tests/chat.spec.ts, test 1)
 *
 * Planned (Task 1 + streaming):
 *   - Markdown bold / italic / code / links render
 *   - URL click opens via tauri-plugin-opener (openUrl)
 *   - Timestamp renders alongside the bubble
 *   - Error variant (assistant bubble flagged with error)
 *   - DOM identity preserved across delta → complete phases
 *   - Partial Markdown during streaming doesn't crash
 */
import { expect, test } from "@playwright/test";
import { FakeDaemon } from "../support/fakeDaemon";
import { bootOnboarded } from "../support/bootHelpers";
import { emitTurnStart, emitTextDelta, emitTurnComplete } from "../support/chatEmit";
import { locateAssistantBubble } from "../support/chatLocators";

// V2 happy path: typing into the home composer creates a puffer-backed session
// over WebSocket, fires a turn, and streams an assistant bubble in via deltas.
test("home composer → create_session → run_agent_turn → streamed assistant text", async ({
  page
}) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);

  const prompt = "Hello from V2";
  await page.getByLabel("Message").fill(prompt);
  await page.getByLabel("Message").press("Enter");

  // FakeDaemon.createSession synthesizes `session-created-${size+1}`; mirror
  // it here so we can address the session in the run_agent_turn predicate
  // and event channel below.
  await daemon.waitForRequest("create_session");
  const sessionId = "session-created-1";

  await daemon.waitForRequest(
    "run_agent_turn",
    (req) => req.params.sessionId === sessionId && req.params.message === prompt
  );

  await expect(page).toHaveURL(new RegExp(`#/agent/${sessionId}$`));

  // FakeDaemon.runAgentTurn synthesizes turnId as `turn-${sessionId}`; the
  // chat store binds the pending bubble to that exact id in fireTurn's .then.
  // Reusing it here keeps the bubble binding intact instead of forking a
  // second pending bubble via the fallback branch.
  const turnId = `turn-${sessionId}`;
  emitTurnStart(daemon, { sessionId, turnId });
  emitTextDelta(daemon, { sessionId, turnId, delta: "Hello" });
  emitTextDelta(daemon, { sessionId, turnId, delta: " world" });
  // Intentionally omit `assistantText` from turn-complete so the resolved
  // bubble has to rely on the accumulated deltas. Including it would let
  // the turn-complete handler overwrite whatever the delta accumulator did,
  // masking accumulator bugs (verified via mutation test 2026-05-25).
  emitTurnComplete(daemon, { sessionId, turnId });

  // Resolved bubble renders `.assistant-bubble__text`; pending bubble renders
  // the `.typing` dots (Agent.svelte:201-209). Asserting the resolved
  // element implicitly asserts pending is gone.
  const resolved = locateAssistantBubble(page).locator(".assistant-bubble__text", {
    hasText: "Hello world"
  });
  await expect(resolved).toBeVisible();
  await expect(page.locator(".typing")).toHaveCount(0);
});

test.fixme(
  "assistant-bubble: Markdown / URL click / timestamp / error variant — see file header",
  async () => {}
);

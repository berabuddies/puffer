import { expect, test, type Page } from "@playwright/test";
import { FakeDaemon } from "../v1/support/fakeDaemon";

async function bootOnboarded(page: Page, daemon: FakeDaemon): Promise<void> {
  await page.addInitScript(() => {
    try {
      window.localStorage.setItem("puffer.onboarded", "true");
    } catch {
      /* private mode — auth.svelte.ts treats absence as not-onboarded; tests that
         care will fail loudly. */
    }
  });
  await daemon.install(page);
  await page.goto("/#/home");
  await expect(page).toHaveURL(/#\/home$/);
}

// V2 happy path: typing into the home composer creates a puffer-backed session
// over WebSocket, fires a turn, and streams an assistant bubble in via deltas.
test("home composer → create_session → run_agent_turn → streamed assistant text", async ({ page }) => {
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
  const channel = `session:${sessionId}:event`;
  daemon.emit(channel, { type: "turn-start", turnId });
  daemon.emit(channel, { type: "text-delta", turnId, delta: "Hello" });
  daemon.emit(channel, { type: "text-delta", turnId, delta: " world" });
  // Intentionally omit `assistantText` from turn-complete so the resolved
  // bubble has to rely on the accumulated deltas. Including it would let
  // the turn-complete handler overwrite whatever the delta accumulator did,
  // masking accumulator bugs (verified via mutation test 2026-05-25).
  daemon.emit(channel, { type: "turn-complete", turnId });

  // Resolved bubble renders `.assistant-bubble__text`; pending bubble renders
  // the `.typing` dots (Agent.svelte:201-209). Asserting the resolved
  // element implicitly asserts pending is gone.
  const resolved = page.locator(".assistant-bubble__text", { hasText: "Hello world" });
  await expect(resolved).toBeVisible();
  await expect(page.locator(".typing")).toHaveCount(0);
});

// Regression for the Sidebar "+ new chat" Promise-stringification bug:
// startNewChat used to template-literal the Promise into the URL, producing
// `/agent/[object Promise]`. The async-then refactor must navigate to the
// resolved sessionId.
test('sidebar "+" New chat in Work navigates to a real session id, not [object Promise]', async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);

  await page.getByLabel("New chat in Work").click();
  await daemon.waitForRequest("create_session");

  // Wait for the navigation to land. Hash routing means the URL contains
  // `#/agent/<id>`.
  await page.waitForURL(/#\/agent\//);
  const url = page.url();

  expect(url).not.toContain("Promise");
  expect(url).not.toContain("%5B");
  expect(url).not.toContain("[");
  // Accept FakeDaemon's `session-created-N` shape as well as real UUIDs —
  // both are alphanumeric-plus-hyphen and prove the Promise wasn't stringified.
  expect(url).toMatch(/#\/agent\/[a-z0-9-]+$/i);
});

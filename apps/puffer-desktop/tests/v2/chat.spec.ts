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

// Regression: a desktop restart kept the session list (server-backed metadata)
// but opened sessions blank because the V2 chat store only carried live deltas
// and never replayed the persisted timeline. ensureSession() now calls
// load_session_detail and prepends user_message/assistant_message items.
test("existing session with historical timeline renders past messages on /agent/<id> open", async ({
  page
}) => {
  const baseTime = Date.now();
  const sessionId = "session-history-1";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId,
        displayName: "Restart survivor",
        title: "Restart survivor",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 3,
        timeline: [
          {
            kind: "user_message",
            id: "hist-user-1",
            text: "remind me what we were doing",
            createdAtMs: baseTime - 45_000
          },
          {
            kind: "assistant_message",
            id: "hist-assistant-1",
            text: "we were drafting the launch checklist",
            createdAtMs: baseTime - 40_000
          },
          // A kind V2 doesn't render yet — must be skipped silently, not crash.
          {
            kind: "system_message",
            id: "hist-system-1",
            text: "session renamed",
            createdAtMs: baseTime - 39_000
          }
        ]
      }
    ]
  });
  await bootOnboarded(page, daemon);

  // Navigate directly to /agent/<seededSessionId>; mimics what happens after
  // a webview reload when the sidebar restores the active session.
  await page.goto(`/#/agent/${sessionId}`);
  await daemon.waitForRequest(
    "load_session_detail",
    (req) => req.params.sessionId === sessionId
  );

  // User bubble renders inside `.bubble`; assistant text inside
  // `.assistant-bubble__text` (Agent.svelte:99-117).
  const userBubble = page.locator(".bubble", { hasText: "remind me what we were doing" });
  const assistantBubble = page.locator(".assistant-bubble__text", {
    hasText: "we were drafting the launch checklist"
  });
  await expect(userBubble).toBeVisible();
  await expect(assistantBubble).toBeVisible();

  // system_message was in the timeline; it must not have surfaced as a bubble.
  await expect(page.getByText("session renamed")).toHaveCount(0);
});

// Hydration UX: while load_session_detail is in flight the thread shows a
// "Loading conversation…" affordance; once the response lands the loader is
// removed and historical bubbles render.
test("shows a loading indicator while load_session_detail is in flight, then resolves to messages", async ({
  page
}) => {
  const baseTime = Date.now();
  const sessionId = "session-loading-1";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId,
        displayName: "Slow loader",
        title: "Slow loader",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "hist-assistant-late",
            text: "loaded after delay",
            createdAtMs: baseTime - 10_000
          }
        ]
      }
    ]
  });
  await bootOnboarded(page, daemon);

  daemon.delayResponse(
    "load_session_detail",
    (req) => req.params.sessionId === sessionId,
    800
  );

  await page.goto(`/#/agent/${sessionId}`);

  const loader = page.getByTestId("hydration-loading");
  await expect(loader).toBeVisible({ timeout: 200 });
  await expect(loader).toContainText("Loading conversation…");

  const resolved = page.locator(".assistant-bubble__text", { hasText: "loaded after delay" });
  await expect(resolved).toBeVisible();
  await expect(loader).toHaveCount(0);
});

// Hydration error UX: when load_session_detail rejects the thread shows an
// inline failure card + a Retry button. Clicking Retry re-runs the call and,
// once it succeeds, historical bubbles render.
test("shows error state with retry when load_session_detail fails", async ({ page }) => {
  const baseTime = Date.now();
  const sessionId = "session-error-1";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId,
        displayName: "Error first",
        title: "Error first",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "hist-after-retry",
            text: "history after retry",
            createdAtMs: baseTime - 10_000
          }
        ]
      }
    ]
  });
  await bootOnboarded(page, daemon);

  // First call fails synchronously; retry will then hit the normal path.
  daemon.failNext("load_session_detail", "transient backend hiccup");

  await page.goto(`/#/agent/${sessionId}`);

  const errorPanel = page.getByTestId("hydration-error");
  await expect(errorPanel).toBeVisible();
  await expect(errorPanel).toContainText("Failed to load history");

  const retry = errorPanel.getByRole("button", { name: "Retry" });
  await expect(retry).toBeVisible();

  await retry.click();
  const resolved = page.locator(".assistant-bubble__text", { hasText: "history after retry" });
  await expect(resolved).toBeVisible();
  await expect(errorPanel).toHaveCount(0);
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

/**
 * Session lifecycle tests — cold hydration, loading state, error+retry.
 *
 * Already migrated (live tests in this file):
 *   - "existing session with historical timeline renders past messages
 *     on /agent/<id> open"  (was chat.spec.ts, test 2)
 *   - "shows a loading indicator while load_session_detail is in flight,
 *     then resolves to messages"  (was test 3)
 *   - "shows error state with retry when load_session_detail fails"
 *     (was test 4)
 *
 * Planned:
 *   - hydration mid-flow doesn't lose live deltas — call
 *     `hydrateMidFlow(page, daemon, id)` while a turn is streaming,
 *     assert the in-flight bubble is preserved
 *   - session creation race: two create_session in quick succession don't
 *     both navigate (v1 ref :936 persisted prompt during pending turn)
 *   - "kinds V2 doesn't render" — system_message etc. are skipped silently
 */
import { expect, test } from "@playwright/test";
import { FakeDaemon } from "../support/fakeDaemon";
import { bootOnboarded } from "../support/bootHelpers";
import { makeSession } from "../support/sessionFixtures";
import { delayRpc } from "../support/chatTiming";
import { locateUserBubble, locateAssistantBubble } from "../support/chatLocators";

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
      makeSession({
        id: sessionId,
        title: "Restart survivor",
        cwd: "/tmp/puffer",
        baseTime,
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
      })
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
  const userBubble = locateUserBubble(page, { text: "remind me what we were doing" });
  const assistantBubble = locateAssistantBubble(page).locator(".assistant-bubble__text", {
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
      makeSession({
        id: sessionId,
        title: "Slow loader",
        cwd: "/tmp/puffer",
        baseTime,
        timeline: [
          {
            kind: "assistant_message",
            id: "hist-assistant-late",
            text: "loaded after delay",
            createdAtMs: baseTime - 10_000
          }
        ]
      })
    ]
  });
  await bootOnboarded(page, daemon);

  delayRpc(
    daemon,
    "load_session_detail",
    800,
    (req) => req.params.sessionId === sessionId
  );

  await page.goto(`/#/agent/${sessionId}`);

  const loader = page.getByTestId("hydration-loading");
  await expect(loader).toBeVisible({ timeout: 200 });
  await expect(loader).toContainText("Loading conversation…");

  const resolved = locateAssistantBubble(page).locator(".assistant-bubble__text", {
    hasText: "loaded after delay"
  });
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
      makeSession({
        id: sessionId,
        title: "Error first",
        cwd: "/tmp/puffer",
        baseTime,
        timeline: [
          {
            kind: "assistant_message",
            id: "hist-after-retry",
            text: "history after retry",
            createdAtMs: baseTime - 10_000
          }
        ]
      })
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
  const resolved = locateAssistantBubble(page).locator(".assistant-bubble__text", {
    hasText: "history after retry"
  });
  await expect(resolved).toBeVisible();
  await expect(errorPanel).toHaveCount(0);
});

test.fixme(
  "session-lifecycle: hydration mid-flow + creation race — see file header",
  async () => {}
);

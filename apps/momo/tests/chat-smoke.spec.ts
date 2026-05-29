/**
 * Chat smoke test. Holds tiny, harness-wide regressions that aren't a
 * good fit for a per-feature spec in `chat/`, plus the one end-to-end
 * "send a message → see the assistant reply" path through the new
 * daemon-backed `ConversationView` surface (Task 5).
 *
 * The deeper flows — permission / question / cancel / replay e2e — live in
 * `chat/` and are owned by Task 8. This file only guards that the Agent page
 * renders a streamed assistant message at all after the chat-controller wiring.
 */
import { expect, test } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";
import { bootOnboarded } from "./support/bootHelpers";
import { deferRpc } from "./support/chatTiming";
import { emitTextDelta, emitTurnComplete } from "./support/chatEmit";

// End-to-end happy path through the migrated daemon chat surface:
//   shell Composer → create_session → run_agent_turn{turnId}
//     → emit text-delta + turn-complete
//     → ConversationView renders the assistant text.
//
// Boots via `bootOnboarded` (which installs the FakeDaemon ws route and lands
// on /home). The frontend's daemonClient parses both the legacy and "real"
// daemon envelopes, so the default protocol round-trips fine here; the
// fakeDaemon dispatcher already answers create_session / run_agent_turn and
// re-broadcasts `session:<id>:event` frames to every open socket.
test("home composer → run_agent_turn → ConversationView renders the assistant reply", async ({
  page
}) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);

  const prompt = "What's my luck today?";
  await page.getByLabel("Message").fill(prompt);
  await page.getByLabel("Message").press("Enter");

  // FakeDaemon.createSession mints `session-created-${size+1}`; with no seeded
  // sessions the first new chat is `session-created-1`.
  await daemon.waitForRequest("create_session");
  const sessionId = "session-created-1";

  // The turn carries the typed prompt and targets the freshly-created session.
  await daemon.waitForRequest(
    "run_agent_turn",
    (req) => req.params.sessionId === sessionId && req.params.message === prompt
  );

  // Navigation lands on the new session's agent route.
  await expect(page).toHaveURL(new RegExp(`#/agent/${sessionId}$`));

  // FakeDaemon.runAgentTurn synthesizes turnId as `turn-${sessionId}`; the
  // controller binds the live stream to that exact id, so reuse it for the
  // event channel below.
  const turnId = `turn-${sessionId}`;
  emitTextDelta(daemon, { sessionId, turnId, delta: "You're " });
  emitTextDelta(daemon, { sessionId, turnId, delta: "in luck!" });
  // Omit `assistantText` so the resolved message has to come from the
  // accumulated deltas (guards the delta accumulator, not the completion
  // fallback).
  emitTurnComplete(daemon, { sessionId, turnId });

  // ConversationView renders an agent row (`.pf-msg[data-role="agent"]`) whose
  // body text comes through MessageBody as a paragraph inside `.pf-msg-text`.
  const assistant = page
    .locator('.pf-msg[data-role="agent"] .pf-msg-text')
    .filter({ hasText: "You're in luck!" });
  await expect(assistant).toBeVisible();

  // The user's prompt renders as a user row in the same thread.
  await expect(
    page.locator('.pf-msg[data-role="user"] .pf-msg-text').filter({ hasText: prompt })
  ).toBeVisible();
});

// Streaming jitter fix (MessageBody rAF throttle): the assistant MessageBody
// now coalesces its markdown re-parse to one-per-frame instead of per-token.
// The visible *jitter* can't be asserted in a unit test, but the throttle's
// hard correctness guarantee can: a rapid burst of deltas (whose markdown
// markers flip between "half-finished" and "closed" frame-to-frame) must still
// land the COMPLETE final text — no dropped tail, and the closed markdown wins.
test("streaming: a rapid delta burst still renders the complete final markdown (no dropped tail)", async ({
  page
}) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);

  const prompt = "stream me a burst";
  await page.getByLabel("Message").fill(prompt);
  await page.getByLabel("Message").press("Enter");

  await daemon.waitForRequest("create_session");
  const sessionId = "session-created-1";
  await daemon.waitForRequest("run_agent_turn");
  await expect(page).toHaveURL(new RegExp(`#/agent/${sessionId}$`));

  const turnId = `turn-${sessionId}`;
  // Tokenize a markdown sentence into many tiny deltas. Mid-stream, the unclosed
  // `**bold**` / `` `code` `` markers make the parser flip block/inline kinds on
  // almost every frame — exactly the case the throttle is meant to absorb.
  const full = "This is **bold** and `code` and the *end*.";
  for (const ch of full) {
    emitTextDelta(daemon, { sessionId, turnId, delta: ch });
  }
  // Omit assistantText so the final text MUST come from the accumulated deltas
  // (guards that the throttle flushed the tail, not the completion fallback).
  emitTurnComplete(daemon, { sessionId, turnId });

  const bubble = page
    .locator('.pf-msg[data-role="agent"] .pf-msg-text')
    .filter({ hasText: "This is bold and code and the end." });
  await expect(bubble).toBeVisible();
  // The closed markdown won the race: `**bold**` rendered as <strong>, not as
  // literal asterisks, and `` `code` `` rendered as an inline <code> element.
  await expect(bubble.locator(".strong").filter({ hasText: "bold" })).toBeVisible();
  await expect(bubble.locator("code").filter({ hasText: "code" })).toBeVisible();
  // No leftover literal markdown markers from a mid-stream parse leaking through.
  await expect(bubble).not.toContainText("**");
  await expect(bubble).not.toContainText("`");
});

// Regression for the `state_unsafe_mutation` crash on DIRECT navigation to an
// existing session (sidebar click / Home task card / page reload). Unlike the
// create-flow above — which pre-seeds the controller state via
// `createSessionFromText` before navigating — landing straight on
// `/agent/<id>` makes Agent.svelte the first thing to touch the session state.
// When the controller was created inside a `$derived` (T5 v1), `createController`
// → `ensureState` mutated `chatStates` inside a tracked read scope and Svelte 5
// threw `state_unsafe_mutation` (fatal in WebKit/Tauri), blanking the page.
// Seeding the controller in a `$effect` instead is the fix; this guards it by
// failing on ANY uncaught pageerror while asserting the seeded timeline renders.
test("direct goto /agent/<id> hydrates the timeline without a pageerror", async ({ page }) => {
  const pageErrors: Error[] = [];
  page.on("pageerror", (error) => pageErrors.push(error));

  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "seeded-1",
        timeline: [
          { kind: "user_message", id: "u-1", text: "ping from history", createdAtMs: 1 },
          { kind: "assistant_message", id: "a-1", text: "pong from history", createdAtMs: 2 }
        ]
      }
    ]
  });
  await bootOnboarded(page, daemon);

  // Hard-navigate straight to the session — no create-flow, no pre-seeded
  // controller state. This is the path that crashed before the fix.
  await page.goto("/#/agent/seeded-1");
  await expect(page).toHaveURL(/#\/agent\/seeded-1$/);

  // ConversationView must hydrate and render the persisted transcript.
  await expect(
    page.locator('.pf-msg[data-role="user"] .pf-msg-text').filter({ hasText: "ping from history" })
  ).toBeVisible();
  await expect(
    page
      .locator('.pf-msg[data-role="agent"] .pf-msg-text')
      .filter({ hasText: "pong from history" })
  ).toBeVisible();

  // No uncaught error (the state_unsafe_mutation regression would surface here).
  expect(pageErrors, pageErrors.map((e) => e.message).join("\n")).toHaveLength(0);
});

// Regression for the Sidebar "+ new chat" Promise-stringification bug:
// startNewChat used to template-literal the Promise into the URL, producing
// `/agent/[object Promise]`. The async-then refactor must navigate to the
// resolved sessionId.
test('sidebar "+" New chat navigates to a real session id, not [object Promise]', async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);

  await page.getByLabel("New chat").click();
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

// Harness self-test: prove `deferRpc` actually pins an RPC and `resolve()`
// releases it. Race tests in chat/{stop-button,answer-form,...}.spec.ts
// depend on this — if this regresses, those suites will hang.
test("harness: deferRpc pins create_session until resolve() is called", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);

  // Pin the next create_session response. The webview will fire the RPC but
  // not get a result back until we call pending.resolve().
  const pending = deferRpc(daemon, "create_session");

  await page.getByLabel("New chat").click();
  // The request reaches the daemon synchronously, but we did NOT resolve yet.
  await daemon.waitForRequest("create_session");
  // URL stays on /home — navigation only fires after the sessionId resolves.
  await page.waitForTimeout(200);
  expect(page.url()).not.toMatch(/#\/agent\//);

  // Release the pin. The default dispatcher path runs (deferRpc with no
  // explicit value falls through to the normal create_session result), and
  // navigation should complete.
  pending.resolve();
  await page.waitForURL(/#\/agent\//, { timeout: 5_000 });
});

/**
 * Task 2 — Stop button.
 *
 * The Composer normally renders a Send button; once a turn is in flight the
 * chat store flips `runningTurnBySessionId[sessionId]` and the Composer
 * swaps in a red Stop button. Clicking Stop fires `cancel_turn` against the
 * current turnId; the daemon's authoritative `turn-complete` / `turn-error`
 * clears state and the Send button returns.
 *
 * Race coverage stays close to `daemon.emit(channel, payload)` and
 * `deferRpc` per `tests/support/README.md` — sugar would hide the gaps these
 * tests exist to lock down.
 */
import { expect, test } from "@playwright/test";
import { FakeDaemon } from "../support/fakeDaemon";
import { bootOnboarded } from "../support/bootHelpers";
import {
  emitTurnStart,
  emitTurnComplete,
  emitTurnError
} from "../support/chatEmit";

// Default state: an unopened composer has only a Send button. No Stop in
// the DOM — the swap is conditional on a running turn.
test("composer shows Send by default and no Stop button", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);

  await expect(page.getByLabel("Send")).toBeVisible();
  await expect(page.locator("[data-stop]")).toHaveCount(0);
});

// Stop button appears when `turn-start` lands. Send button is gone — the
// swap is exclusive (one button slot, two roles).
test("turn-start swaps Send for Stop in the composer", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);

  await page.getByLabel("Message").fill("kick off a turn");
  await page.getByLabel("Message").press("Enter");

  await daemon.waitForRequest("create_session");
  const sessionId = "session-created-1";
  const turnId = `turn-${sessionId}`;
  await daemon.waitForRequest(
    "run_agent_turn",
    (req) => req.params.sessionId === sessionId
  );

  // Wait for navigation so the Composer mounts under /agent/<id> with the
  // `running` prop wired up.
  await expect(page).toHaveURL(new RegExp(`#/agent/${sessionId}$`));

  emitTurnStart(daemon, { sessionId, turnId });

  await expect(page.locator("[data-stop]")).toBeVisible();
  await expect(page.getByLabel("Send")).toHaveCount(0);

  // Tidy up so the page doesn't hang on a still-running turn.
  emitTurnComplete(daemon, { sessionId, turnId });
});

// The Stop click is wired to `agent.cancelTurn(turnId)`. The cancel must
// carry the active turnId — predicate-matching catches off-by-one slips
// where a stale turnId leaked from a previous turn.
test("clicking Stop fires cancel_turn with the active turnId", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);

  await page.getByLabel("Message").fill("long running");
  await page.getByLabel("Message").press("Enter");

  await daemon.waitForRequest("create_session");
  const sessionId = "session-created-1";
  const turnId = `turn-${sessionId}`;
  await daemon.waitForRequest(
    "run_agent_turn",
    (req) => req.params.sessionId === sessionId
  );
  await expect(page).toHaveURL(new RegExp(`#/agent/${sessionId}$`));

  emitTurnStart(daemon, { sessionId, turnId });
  await expect(page.locator("[data-stop]")).toBeVisible();

  await page.locator("[data-stop]").click();
  await daemon.waitForRequest(
    "cancel_turn",
    (req) => req.params.turnId === turnId
  );

  // Daemon's authoritative cleanup arrives next — Send returns.
  emitTurnComplete(daemon, { sessionId, turnId });
  await expect(page.getByLabel("Send")).toBeVisible();
  await expect(page.locator("[data-stop]")).toHaveCount(0);
});

// `turn-complete` is the canonical end-of-turn signal. The composer must
// flip back even if the user never clicked Stop.
test("turn-complete clears Stop and restores Send", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);

  await page.getByLabel("Message").fill("auto-complete");
  await page.getByLabel("Message").press("Enter");

  await daemon.waitForRequest("create_session");
  const sessionId = "session-created-1";
  const turnId = `turn-${sessionId}`;
  await daemon.waitForRequest("run_agent_turn", (req) => req.params.sessionId === sessionId);
  await expect(page).toHaveURL(new RegExp(`#/agent/${sessionId}$`));

  emitTurnStart(daemon, { sessionId, turnId });
  await expect(page.locator("[data-stop]")).toBeVisible();

  emitTurnComplete(daemon, { sessionId, turnId });
  await expect(page.locator("[data-stop]")).toHaveCount(0);
  await expect(page.getByLabel("Send")).toBeVisible();
});

// `turn-error` is the other terminal signal. Same UI flip as turn-complete —
// the cleanup is symmetric and identical mid-flow regardless of outcome.
test("turn-error clears Stop and restores Send", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);

  await page.getByLabel("Message").fill("doomed turn");
  await page.getByLabel("Message").press("Enter");

  await daemon.waitForRequest("create_session");
  const sessionId = "session-created-1";
  const turnId = `turn-${sessionId}`;
  await daemon.waitForRequest("run_agent_turn", (req) => req.params.sessionId === sessionId);
  await expect(page).toHaveURL(new RegExp(`#/agent/${sessionId}$`));

  emitTurnStart(daemon, { sessionId, turnId });
  await expect(page.locator("[data-stop]")).toBeVisible();

  emitTurnError(daemon, { sessionId, turnId, error: "model exploded" });
  await expect(page.locator("[data-stop]")).toHaveCount(0);
  await expect(page.getByLabel("Send")).toBeVisible();
});

// v1 :1253 race: even after the user pressed Send, Stop must NOT appear
// until run_agent_turn returns a turnId. We pin the RPC with `deferRpc`
// BEFORE clicking Enter so the deferral catches the very first matching
// request; until we release the pin the chat store has no turnId and
// `runningTurnBySessionId[sessionId]` stays empty → Composer stays in Send
// mode (or, while the textarea is empty after submit, Send is disabled —
// either way no `[data-stop]` is in the DOM).
test("Stop does not appear until run_agent_turn resolves", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);

  // Pin BEFORE submit so the deferral intercepts the first run_agent_turn.
  // No predicate yet — we don't know the sessionId until create_session
  // resolves. The deferRpc is one-shot, so it'll capture exactly the next
  // run_agent_turn that matches the (default true) predicate.
  const pendingTurn = daemon.deferRpc("run_agent_turn");

  await page.getByLabel("Message").fill("pin me");
  await page.getByLabel("Message").press("Enter");

  await daemon.waitForRequest("create_session");
  const sessionId = "session-created-1";
  const turnId = `turn-${sessionId}`;
  await daemon.waitForRequest("run_agent_turn", (req) => req.params.sessionId === sessionId);
  await expect(page).toHaveURL(new RegExp(`#/agent/${sessionId}$`));

  // Daemon has seen the request but is sitting on it. The chat store has
  // no turnId for sessionId yet — Composer's `running` derives to false
  // and there's no [data-stop] in the DOM.
  await page.waitForTimeout(200);
  await expect(page.locator("[data-stop]")).toHaveCount(0);

  // Release the deferral. fakeDaemon's normal dispatcher fills in
  // `turnId: turn-${sessionId}` and the chat store's .then handler sets
  // `runningTurnBySessionId[sessionId] = turnId`. The Composer should
  // immediately flip to Stop on this state write — turn-start is the
  // event-side correlate but isn't required for the swap.
  pendingTurn.resolve();
  await expect(page.locator("[data-stop]")).toBeVisible();

  emitTurnStart(daemon, { sessionId, turnId });
  emitTurnComplete(daemon, { sessionId, turnId });
});

// `failNext` rejects cancel_turn at the protocol layer. The chat store
// surfaces a toast — local UI state stays Stop until `turn-complete` /
// `turn-error` from the daemon, per the cancelRunningTurn doc-comment.
test("cancel_turn RPC failure surfaces a toast", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);

  await page.getByLabel("Message").fill("cancel will fail");
  await page.getByLabel("Message").press("Enter");
  await daemon.waitForRequest("create_session");
  const sessionId = "session-created-1";
  const turnId = `turn-${sessionId}`;
  await daemon.waitForRequest("run_agent_turn", (req) => req.params.sessionId === sessionId);
  await expect(page).toHaveURL(new RegExp(`#/agent/${sessionId}$`));

  emitTurnStart(daemon, { sessionId, turnId });
  await expect(page.locator("[data-stop]")).toBeVisible();

  daemon.failNext("cancel_turn", "no such turn");
  await page.locator("[data-stop]").click();

  // Toast renderer is `.toast` w/ kind-classed modifier (Toast.svelte:23-27).
  // FakeDaemon stringifies the queued failure via `String(error)` in its
  // dispatch catch block (fakeDaemon.ts:1224), which prepends "Error: " to
  // the message — the production WS host doesn't do this, but we match the
  // fixture's behaviour so the test stays honest about what arrives at
  // `wsClient`.
  const errorToast = page
    .locator(".toast")
    .filter({ hasText: "Cancel failed:" })
    .filter({ hasText: "no such turn" });
  await expect(errorToast).toBeVisible({ timeout: 2500 });

  // Stop is still visible because the daemon never cleared the turn.
  await expect(page.locator("[data-stop]")).toBeVisible();

  emitTurnComplete(daemon, { sessionId, turnId });
});

// Two sequential turns must not leak state. After turn A completes, the
// composer must drop into Send-only — clicking the (no-longer-there) Stop
// must NOT fire a cancel for A's turnId. We verify the negative by
// asserting Stop is gone, then firing turn B and confirming Stop fires
// cancel_turn with the in-flight turnId.
//
// FakeDaemon limitation: `runAgentTurn` always returns `turn-${sessionId}`
// regardless of how many turns ran (see fakeDaemon.ts:1573). So turn A and
// turn B share an id at the wire level. The state-leak property we lock
// down is therefore "after turn-complete, the chat store forgets the id" —
// which the deletion in handleSessionEvent makes truthy.
test("two sequential turns: Stop on each cancels the active turn only", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);

  // Turn A.
  await page.getByLabel("Message").fill("turn A");
  await page.getByLabel("Message").press("Enter");
  await daemon.waitForRequest("create_session");
  const sessionId = "session-created-1";
  await daemon.waitForRequest("run_agent_turn", (req) => req.params.sessionId === sessionId);
  await expect(page).toHaveURL(new RegExp(`#/agent/${sessionId}$`));

  const turnIdA = `turn-${sessionId}`;
  emitTurnStart(daemon, { sessionId, turnId: turnIdA });
  await expect(page.locator("[data-stop]")).toBeVisible();
  emitTurnComplete(daemon, { sessionId, turnId: turnIdA });

  // After turn A completes, Stop is gone. Critically: there's no stale
  // running-turn entry for sessionId — the next user action gets a fresh
  // Send button, not a stale Stop. (If state leaked, Stop would persist.)
  await expect(page.locator("[data-stop]")).toHaveCount(0);
  await expect(page.getByLabel("Send")).toBeVisible();

  // Turn B — append in the same session.
  await page.getByLabel("Message").fill("turn B");
  await page.getByLabel("Message").press("Enter");
  await daemon.waitForRequest(
    "run_agent_turn",
    (req) => req.params.sessionId === sessionId && req.params.message === "turn B"
  );
  const turnIdB = `turn-${sessionId}`; // fakeDaemon recycles the id
  emitTurnStart(daemon, { sessionId, turnId: turnIdB });
  await expect(page.locator("[data-stop]")).toBeVisible();

  await page.locator("[data-stop]").click();
  await daemon.waitForRequest(
    "cancel_turn",
    (req) => req.params.turnId === turnIdB
  );

  // Backstop: only one cancel_turn ever landed (turn A produced none
  // because no Stop click happened after its turn-complete).
  const cancelCount = daemon.requests.filter((r) => r.method === "cancel_turn").length;
  expect(cancelCount).toBe(1);

  emitTurnComplete(daemon, { sessionId, turnId: turnIdB });
});

// Cross-session isolation: a running turn in session A must NOT cause
// session B's composer to render a Stop button. V2 is a single-page app
// with one Composer at a time, so we navigate B and observe the composer
// state — session A's running turn lives in `runningTurnBySessionId[A]`
// which derives `isRunning` for the OPEN session only.
test("cross-session: composer in session B shows Send while A is running", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);

  // Kick off session A from the home composer.
  await page.getByLabel("Message").fill("session A first");
  await page.getByLabel("Message").press("Enter");
  await daemon.waitForRequest("create_session");
  const sessionA = "session-created-1";
  await daemon.waitForRequest("run_agent_turn", (req) => req.params.sessionId === sessionA);
  await expect(page).toHaveURL(new RegExp(`#/agent/${sessionA}$`));

  const turnA = `turn-${sessionA}`;
  emitTurnStart(daemon, { sessionId: sessionA, turnId: turnA });
  await expect(page.locator("[data-stop]")).toBeVisible();

  // Navigate to a brand-new session B. Use the sidebar "+ New chat" to
  // mint sessionB so it follows the production code path.
  await page.getByLabel("New chat").click();
  const sessionB = "session-created-2";
  await expect(page).toHaveURL(new RegExp(`#/agent/${sessionB}$`));

  // B has no running turn → Send button shows, no Stop. A's turn is still
  // running in the background but doesn't leak into B's composer.
  await expect(page.getByLabel("Send")).toBeVisible();
  await expect(page.locator("[data-stop]")).toHaveCount(0);

  // Verify A's running state is still live by navigating back.
  await page.goto(`/#/agent/${sessionA}`);
  await expect(page).toHaveURL(new RegExp(`#/agent/${sessionA}$`));
  await expect(page.locator("[data-stop]")).toBeVisible();

  emitTurnComplete(daemon, { sessionId: sessionA, turnId: turnA });
});

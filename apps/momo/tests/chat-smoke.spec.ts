/**
 * Chat smoke test. Holds tiny, harness-wide regressions that aren't a
 * good fit for a per-feature spec in `chat/`. Feature flows live there.
 *
 * Today: the Sidebar "+ new chat" Promise-stringification regression.
 */
import { expect, test } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";
import { bootOnboarded } from "./support/bootHelpers";
import { deferRpc } from "./support/chatTiming";

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
  const pending = daemon.deferRpc("create_session");

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

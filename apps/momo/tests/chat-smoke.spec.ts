/**
 * Chat smoke test. Holds tiny, harness-wide regressions that aren't a
 * good fit for a per-feature spec in `chat/`. Feature flows live there.
 *
 * Today: the Sidebar "+ new chat" Promise-stringification regression.
 */
import { expect, test } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";
import { bootOnboarded } from "./support/bootHelpers";

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

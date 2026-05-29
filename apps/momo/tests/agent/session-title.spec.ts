/**
 * Session title coherence — the rail label and the chat header must agree,
 * and the rail must reflect daemon-side title changes without an app restart.
 *
 * Two long-standing mismatches motivated these specs:
 *
 *   1. The Agent page header derived its title from the raw first user
 *      message (`dynamicTitle`), while the sidebar showed the daemon's
 *      authoritative `title` (display name / generated title). So a session
 *      read "Help me fix the login button…" in the header but
 *      "Fix mobile login button" in the rail — never the same string.
 *
 *   2. The daemon generates a session title *after* the first turn completes
 *      and broadcasts `workspace:sessions:changed`, but `sessionStore` only
 *      called `loadSessions()` once on Shell mount and never subscribed to the
 *      event. The rail therefore kept the stale placeholder until the next app
 *      launch — the user saw the title "change on restart".
 */
import { expect, test, type Page } from "@playwright/test";
import { FakeDaemon } from "../support/fakeDaemon";
import { bootOnboarded } from "../support/bootHelpers";

// listProjects points the default project at the workspace root ("/tmp/puffer");
// a session must sit under that cwd to pass the rail's cwd filter.
const DEFAULT_CWD = "/tmp/puffer";

/** The flat session list in the rail. */
function railList(page: Page) {
  return page.locator(".spaces .space-items");
}

test("agent header shows the daemon title, not the raw first user message", async ({ page }) => {
  const firstMessage =
    "Help me fix the login button on my phone, it keeps freezing when I tap it twice";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "titled-1",
        displayName: null,
        generatedTitle: "Fix mobile login button",
        cwd: DEFAULT_CWD,
        folderPath: DEFAULT_CWD,
        timeline: [
          { kind: "user_message", id: "u-1", text: firstMessage, createdAtMs: 1 },
          { kind: "assistant_message", id: "a-1", text: "On it.", createdAtMs: 2 }
        ]
      }
    ]
  });
  await bootOnboarded(page, daemon);

  await page.goto("/#/agent/titled-1");
  await expect(page).toHaveURL(/#\/agent\/titled-1$/);

  // The header reads the daemon's generated title — the same string the rail
  // shows — instead of echoing the user's verbatim opening message.
  const header = page.locator(".agent__header h1");
  await expect(header).toHaveText("Fix mobile login button");
});

test("rail refreshes a session title on workspace:sessions:changed without a reload", async ({
  page
}) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "live-1",
        displayName: "Draft project plan",
        title: "Draft project plan",
        cwd: DEFAULT_CWD,
        folderPath: DEFAULT_CWD,
        timeline: []
      }
    ]
  });
  await bootOnboarded(page, daemon);

  const list = railList(page);
  await expect(list.getByText("Draft project plan", { exact: true })).toBeVisible();

  // The store must have wired a listener for the workspace event before we
  // emit — otherwise the one-shot broadcast would race the subscription.
  await daemon.waitForRequest(
    "subscribe_event",
    (req) => req.params.event === "workspace:sessions:changed"
  );

  // The daemon generates a title after the turn completes and broadcasts the
  // change (reason: "generated_title"). No client reload / app restart.
  daemon.updateSessionMetadata("live-1", {
    displayName: "Plan the Q3 roadmap",
    title: "Plan the Q3 roadmap"
  });
  daemon.emit("workspace:sessions:changed", {
    reason: "generated_title",
    sessionId: "live-1"
  });

  await expect(list.getByText("Plan the Q3 roadmap", { exact: true })).toBeVisible();
  await expect(list.getByText("Draft project plan", { exact: true })).toHaveCount(0);
});

test("rail shows a friendly placeholder, never the raw session id / slug", async ({ page }) => {
  const slug = "session-f67e7d7db9d8abcdef0123";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: slug,
        // A session with no user-set name and no generated title yet. The real
        // daemon's `title` falls back to the slug here — the raw "session-…"
        // string the screenshot showed leaking into the rail.
        displayName: null,
        generatedTitle: null,
        title: slug,
        slug,
        cwd: DEFAULT_CWD,
        folderPath: DEFAULT_CWD,
        timeline: []
      }
    ]
  });
  await bootOnboarded(page, daemon);

  const list = railList(page);
  // The rail shows the friendly default, not the slug.
  await expect(list.getByText("New task", { exact: true })).toBeVisible();
  await expect(list).not.toContainText("session-f67e7d");
});

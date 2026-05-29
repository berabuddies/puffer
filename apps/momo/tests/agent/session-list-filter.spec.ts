/**
 * Task 6 — sidebar session list sourced from the puffer daemon, filtered to
 * the default project's cwd.
 *
 * `sessionStore.loadSessions()` now reads `list_grouped_sessions` from the
 * *daemon* (via `daemonChat`), not momo's 1431 backend. The daemon list is
 * global: task-monitor sessions live under their own cwds and would otherwise
 * leak into the rail. The store resolves the default project's cwd
 * (`list_projects` → `getProjectCwd("default")`, which the FakeDaemon points
 * at its workspace root) and keeps only the matching group's sessions.
 *
 * Here a chat session sits under the default cwd and a monitor session under
 * an unrelated cwd; only the chat session may appear in the rail.
 */
import { expect, test, type Page } from "@playwright/test";
import { FakeDaemon } from "../support/fakeDaemon";
import { bootOnboarded } from "../support/bootHelpers";

/** The flat Tasks session list in the rail. */
function taskList(page: Page) {
  return page.locator(".spaces .space-items");
}

// FakeDaemon.listProjects returns the default project with cwd = workspaceRoot
// (default "/tmp/puffer"). The chat session's cwd must match that for it to
// pass the cwd filter; the monitor session lives under an unrelated cwd.
const DEFAULT_CWD = "/tmp/puffer";

test("rail shows only sessions under the default project cwd", async ({ page }) => {
  // Default ("legacy") protocol: the FakeDaemon's `protocol` flag governs
  // *both* routed paths uniformly, and only the legacy `{ type: "response",
  // ok }` envelope is understood by *both* momo's 1431 `wsClient` (used by
  // `list_projects` / `daemon_handshake`) and the daemonClient (used by
  // `list_grouped_sessions`). `protocol: "real"` would make the backend `/ws`
  // path speak the daemon wire format, which wsClient can't parse, so
  // `list_projects` never resolves and the cwd filter starves. Every passing
  // daemon-chat / daemon-auth spec relies on this same legacy default.
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "chat-1",
        displayName: "Default project chat",
        title: "Default project chat",
        cwd: DEFAULT_CWD,
        folderPath: DEFAULT_CWD,
        timeline: []
      },
      {
        sessionId: "monitor-1",
        displayName: "Monitor session",
        title: "Monitor session",
        cwd: "/some/other",
        folderPath: "/some/other",
        timeline: []
      }
    ]
  });
  await bootOnboarded(page, daemon);

  const list = taskList(page);

  // The default-cwd chat session is surfaced…
  await expect(list.getByText("Default project chat", { exact: true })).toBeVisible();
  // …and exactly one row renders — the monitor session (different cwd) is
  // filtered out, so it never reaches the rail.
  await expect(list.locator(".session-row")).toHaveCount(1);
  await expect(list.getByText("Monitor session", { exact: true })).toHaveCount(0);
});

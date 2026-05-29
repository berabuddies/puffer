import { expect, test, type Locator, type Page } from "@playwright/test";
import { FakeDaemon, type FakeDaemonSessionInput } from "./support/fakeDaemon";
import { bootOnboarded } from "./support/bootHelpers";

/**
 * V2 sidebar session management — the Tasks "+", the inline rename, and the
 * shipped-disabled Trash. Mirrors the FakeDaemon-driven pattern used by
 * chat.spec.ts so the suites stay readable side-by-side.
 *
 * Work / Life grouping is gone: every session lives under the single default
 * project (its cwd = the fake daemon's workspace root, which is also the
 * default cwd for seeded + created sessions), so the rail is one flat list.
 */

/** The flat Tasks session list in the rail. */
function taskList(page: Page): Locator {
  return page.locator(".spaces .space-items");
}

test('Tasks "+" creates a session that appears in the rail list', async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);

  await page.getByLabel("New chat").click();
  await daemon.waitForRequest("create_session");

  // FakeDaemon.createSession synthesizes `session-created-${size+1}`; mirror
  // the convention from chat.spec.ts.
  const sessionId = "session-created-1";
  await page.waitForURL(new RegExp(`#/agent/${sessionId}$`));

  // The optimistic stub built inside sessionStore.createNewSession uses title
  // "New chat" before any reload. Shell does NOT remount on hash-only route
  // changes, so the stub's title is what the sidebar shows here.
  await expect(taskList(page).getByText("New chat", { exact: true })).toBeVisible();
});

// Regression: the sidebar "+ New chat" path must NOT pin a providerId on
// create_session. It used to send `providerId: "puffer"`, which the real
// daemon's create_session routing rejects with `unknown provider \`puffer\``
// (canonical providers are openai / anthropic, not "puffer"), so the click
// errored and never created a session. Omitting providerId routes through the
// daemon's default routing — exactly what the composer's createSessionFromText
// already does. fakeDaemon now rejects unknown providers too, so a regression
// to "puffer" would both surface a toast here and fail the no-providerId check.
test('Tasks "+" New chat does not send an unknown "puffer" provider to the daemon', async ({
  page
}) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);

  const createPromise = daemon.waitForRequest("create_session");
  await page.getByLabel("New chat").click();
  const create = await createPromise;

  // The request must not carry the bogus "puffer" provider. (We assert the
  // exact failure mode that was broken; default routing sends no providerId.)
  expect(create.params.providerId).not.toBe("puffer");
  expect(create.params).not.toHaveProperty("providerId");

  // The session is actually created (no "unknown provider" rejection): the
  // route advances and the optimistic row renders.
  await page.waitForURL(/#\/agent\/session-created-1$/);
  await expect(taskList(page).getByText("New chat", { exact: true })).toBeVisible();

  // No error toast surfaced — the create succeeded rather than rejecting.
  await expect(page.locator(".toast.toast--error")).toHaveCount(0);
});

test("Rename via hover pencil updates the session title", async ({ page }) => {
  const seeded: FakeDaemonSessionInput = {
    sessionId: "seed-rename",
    displayName: "Original title",
    title: "Original title",
    timeline: []
  };
  const daemon = new FakeDaemon({ sessions: [seeded] });
  await bootOnboarded(page, daemon);

  const list = taskList(page);
  await expect(list.getByText("Original title", { exact: true })).toBeVisible();

  // Anchor on the row by text, but only to locate the pencil — once the
  // rename input is mounted the `Original title` text is gone from the row
  // (Sidebar.svelte swaps the label-button for an <input>), so we must not
  // continue scoping by `hasText: "Original title"` after the click.
  const rowByText = list.locator(".session-row", { hasText: "Original title" });
  await rowByText.hover();
  await rowByText.getByRole("button", { name: "Rename session" }).click({ force: true });

  const input = list.getByRole("textbox", { name: "Rename session" });
  await expect(input).toBeVisible();
  await input.fill("Renamed via hover");

  const renamedTitle = "Renamed via hover";
  const renamePromise = daemon.waitForRequest(
    "rename_session",
    (req) => req.params.sessionId === "seed-rename" && req.params.title === renamedTitle
  );
  await input.press("Enter");
  await renamePromise;

  await expect(list.getByText(renamedTitle, { exact: true })).toBeVisible();
  await expect(list.getByText("Original title", { exact: true })).toHaveCount(0);
});

test("Delete button is shipped-disabled with a 'coming soon' tooltip", async ({ page }) => {
  const seeded: FakeDaemonSessionInput = {
    sessionId: "seed-delete",
    displayName: "Trash target",
    title: "Trash target",
    timeline: []
  };
  const daemon = new FakeDaemon({ sessions: [seeded] });
  await bootOnboarded(page, daemon);

  const row = taskList(page).locator(".session-row", { hasText: "Trash target" });
  await expect(row).toBeVisible();
  await row.hover();

  const trash = row.getByRole("button", { name: "Delete session" });
  await expect(trash).toBeDisabled();
  await expect(trash).toHaveAttribute("title", "Coming soon — requires backend support");
});

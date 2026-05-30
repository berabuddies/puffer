import { expect, test, type Locator, type Page } from "@playwright/test";
import { FakeDaemon, type FakeDaemonSessionInput } from "./support/fakeDaemon";
import { bootOnboarded } from "./support/bootHelpers";

/**
 * V2 sidebar session management — the Tasks "+", the inline rename, and the
 * delete-with-confirm Trash. Mirrors the FakeDaemon-driven pattern used by
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

test('Tasks "+" opens the /new page without creating a session', async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);

  await page.getByLabel("New chat").click();

  // Opening the new-chat page must NOT mint a session (the old behavior did).
  await expect(page).toHaveURL(/#\/new$/);
  expect(daemon.requests.some((r) => r.method === "create_session")).toBe(false);

  // The page is just the chat composer input.
  await expect(page.getByLabel("Message")).toBeVisible();
});

// Regression: the new-session path must NOT pin a providerId on create_session.
// It used to send `providerId: "puffer"`, which the real daemon rejects with
// `unknown provider \`puffer\`` (canonical providers are openai / anthropic).
// The trigger moved from the sidebar "+" to sending the first message on /new.
test('Sending from /new creates a session without an unknown "puffer" provider', async ({
  page
}) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);

  await page.getByLabel("New chat").click();
  await expect(page).toHaveURL(/#\/new$/);

  const createPromise = daemon.waitForRequest("create_session");
  await page.getByLabel("Message").fill("hello there");
  await page.getByLabel("Message").press("Enter");
  const create = await createPromise;

  expect(create.params.providerId).not.toBe("puffer");
  expect(create.params).not.toHaveProperty("providerId");

  // FakeDaemon.createSession mints `session-created-1` with no seeded sessions.
  await page.waitForURL(/#\/agent\/session-created-1$/);
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

test("Delete via hover trash → confirm dialog → removes the session", async ({ page }) => {
  const seeded: FakeDaemonSessionInput = {
    sessionId: "seed-delete",
    displayName: "Trash target",
    title: "Trash target",
    timeline: []
  };
  const daemon = new FakeDaemon({ sessions: [seeded] });
  await bootOnboarded(page, daemon);

  const list = taskList(page);
  const row = list.locator(".session-row", { hasText: "Trash target" });
  await expect(row).toBeVisible();
  await row.hover();

  // Trash is live now (no longer shipped-disabled). Clicking pops a confirm
  // dialog rather than deleting outright — delete wipes the transcript and is
  // irreversible, so the user gets a chance to back out.
  await row.getByRole("button", { name: "Delete session" }).click({ force: true });

  const dialog = page.getByRole("dialog");
  await expect(dialog).toBeVisible();

  const deletePromise = daemon.waitForRequest(
    "delete_session",
    (req) => req.params.sessionId === "seed-delete"
  );
  await dialog.getByRole("button", { name: "Delete", exact: true }).click();
  await deletePromise;

  // The row disappears from the rail (optimistic local removal in the store).
  await expect(list.getByText("Trash target", { exact: true })).toHaveCount(0);
  // No error toast — the delete succeeded.
  await expect(page.locator(".toast.toast--error")).toHaveCount(0);
});

test("Canceling the delete dialog keeps the session and sends no delete_session", async ({
  page
}) => {
  const seeded: FakeDaemonSessionInput = {
    sessionId: "seed-keep",
    displayName: "Keep me",
    title: "Keep me",
    timeline: []
  };
  const daemon = new FakeDaemon({ sessions: [seeded] });
  await bootOnboarded(page, daemon);

  const list = taskList(page);
  const row = list.locator(".session-row", { hasText: "Keep me" });
  await row.hover();
  await row.getByRole("button", { name: "Delete session" }).click({ force: true });

  const dialog = page.getByRole("dialog");
  await expect(dialog).toBeVisible();
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  await expect(dialog).toHaveCount(0);

  await expect(list.getByText("Keep me", { exact: true })).toBeVisible();
  expect(daemon.requests.some((r) => r.method === "delete_session")).toBe(false);
});

test("Deleting the session you're viewing navigates Home", async ({ page }) => {
  const seeded: FakeDaemonSessionInput = {
    sessionId: "seed-active",
    displayName: "Active chat",
    title: "Active chat",
    timeline: []
  };
  const daemon = new FakeDaemon({ sessions: [seeded] });
  await bootOnboarded(page, daemon);

  // Land on the session's own agent view so deletion has to move us off it.
  await page.goto("/#/agent/seed-active");
  await expect(page).toHaveURL(/#\/agent\/seed-active$/);

  const list = taskList(page);
  const row = list.locator(".session-row", { hasText: "Active chat" });
  await row.hover();
  await row.getByRole("button", { name: "Delete session" }).click({ force: true });

  const deletePromise = daemon.waitForRequest("delete_session");
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "Delete", exact: true })
    .click();
  await deletePromise;

  // Deleting the open session can't leave us on a dead /agent/<id> route.
  await expect(page).toHaveURL(/#\/home$/);
});

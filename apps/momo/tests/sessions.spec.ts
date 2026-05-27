import { expect, test, type Locator, type Page } from "@playwright/test";
import { FakeDaemon, type FakeDaemonSessionInput } from "./support/fakeDaemon";

/**
 * V2 sidebar session management — the per-group "+", the inline rename,
 * and the shipped-disabled Trash. Mirrors the FakeDaemon-driven pattern
 * used by chat.spec.ts so the two suites stay readable side-by-side.
 *
 * Group membership lives in localStorage (`puffer.sessionGroups`) until
 * the backend exposes tags — see specs/puffer-desktop/469.md. For tests
 * that need a preseeded session in a specific group we seed both the
 * onboarded flag and the group map in the same init script so the
 * sessionStore module reads them on first evaluation.
 */
async function bootOnboarded(
  page: Page,
  daemon: FakeDaemon,
  groupOverrides: Record<string, "work" | "life"> = {}
): Promise<void> {
  await page.addInitScript(
    ({ groups }) => {
      try {
        window.localStorage.setItem("puffer.onboarded", "true");
        if (Object.keys(groups).length > 0) {
          window.localStorage.setItem("puffer.sessionGroups", JSON.stringify(groups));
        }
        // Auth gate (added 2026-05-26): see chat.spec.ts for the rationale —
        // we stub an Auth Station-shaped JWT so loadAuthFromStorage flips
        // the gate to signedIn and lets the tests reach /home.
        const b64url = (s: string) =>
          btoa(s).replace(/=+$/, "").replace(/\+/g, "-").replace(/\//g, "_");
        const header = b64url(JSON.stringify({ alg: "RS256", typ: "JWT" }));
        const payload = b64url(
          JSON.stringify({
            sub: "test-user",
            email: "test@example.com",
            name: "Test User",
            exp: Math.floor(Date.now() / 1000) + 60 * 60 * 24
          })
        );
        window.localStorage.setItem(
          "puffer.authToken",
          `${header}.${payload}.test-sig`
        );
      } catch {
        /* private mode — see chat.spec.ts comment */
      }
    },
    { groups: groupOverrides }
  );
  await daemon.install(page);
  await page.goto("/#/home");
  await expect(page).toHaveURL(/#\/home$/);
}

/**
 * Locate the `.space-items` container that immediately follows the group
 * header bearing `headerText` ("Work" or "Life"). Using XPath axis here
 * because the rail markup is two siblings (header-row + space-items) and
 * Playwright's CSS engine has no sibling-following primitive.
 */
function groupItems(page: Page, headerText: "Work" | "Life"): Locator {
  return page.locator(
    `xpath=//button[contains(@class,"space-header") and normalize-space(.)="${headerText}"]/ancestor::div[contains(@class,"space-group")]//div[contains(@class,"space-items")]`
  );
}

test("+ in Work creates a session that appears under the Work header", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);

  await page.getByLabel("New chat in Work").click();
  await daemon.waitForRequest("create_session");

  // FakeDaemon.createSession synthesizes `session-created-${size+1}`; mirror
  // the convention from chat.spec.ts.
  const sessionId = "session-created-1";
  await page.waitForURL(new RegExp(`#/agent/${sessionId}$`));

  // The optimistic stub built inside sessionStore.createSessionInGroup uses
  // title "New chat" before any reload. Shell does NOT remount on hash-only
  // route changes, so the stub's title is what the sidebar shows here.
  const work = groupItems(page, "Work");
  const life = groupItems(page, "Life");
  await expect(work.getByText("New chat", { exact: true })).toBeVisible();
  await expect(life.getByText("New chat", { exact: true })).toHaveCount(0);
});

test("+ in Life creates a session under Life, not Work", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);

  await page.getByLabel("New chat in Life").click();
  await daemon.waitForRequest("create_session");

  const sessionId = "session-created-1";
  await page.waitForURL(new RegExp(`#/agent/${sessionId}$`));

  const work = groupItems(page, "Work");
  const life = groupItems(page, "Life");
  await expect(life.getByText("New chat", { exact: true })).toBeVisible();
  await expect(work.getByText("New chat", { exact: true })).toHaveCount(0);
});

test("Rename via hover pencil updates the session title", async ({ page }) => {
  const seeded: FakeDaemonSessionInput = {
    sessionId: "seed-rename",
    displayName: "Original title",
    title: "Original title",
    timeline: []
  };
  const daemon = new FakeDaemon({ sessions: [seeded] });
  await bootOnboarded(page, daemon, { "seed-rename": "work" });

  const work = groupItems(page, "Work");
  await expect(work.getByText("Original title", { exact: true })).toBeVisible();

  // Anchor on the row by text, but only to locate the pencil — once the
  // rename input is mounted the `Original title` text is gone from the row
  // (Sidebar.svelte swaps the label-button for an <input>), so we must not
  // continue scoping by `hasText: "Original title"` after the click.
  const rowByText = work.locator(".session-row", { hasText: "Original title" });
  await rowByText.hover();
  await rowByText.getByRole("button", { name: "Rename session" }).click({ force: true });

  const input = work.getByRole("textbox", { name: "Rename session" });
  await expect(input).toBeVisible();
  await input.fill("Renamed via hover");

  const renamedTitle = "Renamed via hover";
  const renamePromise = daemon.waitForRequest(
    "rename_session",
    (req) => req.params.sessionId === "seed-rename" && req.params.title === renamedTitle
  );
  await input.press("Enter");
  await renamePromise;

  await expect(work.getByText(renamedTitle, { exact: true })).toBeVisible();
  await expect(work.getByText("Original title", { exact: true })).toHaveCount(0);
});

test("Delete button is shipped-disabled with a 'coming soon' tooltip", async ({ page }) => {
  const seeded: FakeDaemonSessionInput = {
    sessionId: "seed-delete",
    displayName: "Trash target",
    title: "Trash target",
    timeline: []
  };
  const daemon = new FakeDaemon({ sessions: [seeded] });
  await bootOnboarded(page, daemon, { "seed-delete": "work" });

  const work = groupItems(page, "Work");
  const row = work.locator(".session-row", { hasText: "Trash target" });
  await expect(row).toBeVisible();
  await row.hover();

  const trash = row.getByRole("button", { name: "Delete session" });
  await expect(trash).toBeDisabled();
  await expect(trash).toHaveAttribute("title", "Coming soon — requires backend support");
});

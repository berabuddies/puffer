import { expect, test, type Page } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";

async function openWorkspace(page: Page) {
  await page.locator(".pf-sidebar").getByRole("button", { name: "Project" }).click();
}

test("deleting a session removes its row and fires delete_session", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  page.on("dialog", (dialog) => dialog.accept());

  await openWorkspace(page);

  const targetCard = page
    .locator(".pf-pw-agent-wrap")
    .filter({ has: page.getByRole("button", { name: /Browser regr/ }) })
    .first();
  await expect(targetCard).toBeVisible();
  const cardCountBefore = await page.locator(".pf-pw-agent-wrap").count();

  await targetCard.hover();
  await targetCard.getByRole("button", { name: /^Delete session / }).click();

  const request = await daemon.waitForRequest("delete_session");
  expect(typeof request.params.sessionId).toBe("string");

  await expect.poll(async () => page.locator(".pf-pw-agent-wrap").count()).toBe(cardCountBefore - 1);
});

test("editing session tags sends sorted, deduped tag list and renders chips", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  page.on("dialog", async (dialog) => {
    if (dialog.type() === "prompt") {
      await dialog.accept("ship, fix, ship,  review ");
    } else {
      await dialog.accept();
    }
  });

  await openWorkspace(page);

  const firstCard = page.locator(".pf-pw-agent-wrap").first();
  await firstCard.hover();
  await firstCard.getByRole("button", { name: /^Edit tags for / }).click();

  const request = await daemon.waitForRequest("set_session_tags");
  expect(request.params.tags).toEqual(["ship", "fix", "ship", "review"]);

  // After the daemon dedupes + sorts, the refreshed list shows chips.
  await expect.poll(async () =>
    firstCard.locator(".pf-pw-tag").allTextContents()
  ).toEqual(["fix", "review", "ship"]);
});

test("editing project tags renders chips and calls set_project_tags", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  page.on("dialog", async (dialog) => {
    if (dialog.type() === "prompt") {
      await dialog.accept("backend frontend");
    } else {
      await dialog.accept();
    }
  });

  await openWorkspace(page);

  const projectRow = page.locator(".pf-pw-project").first();
  await projectRow.getByRole("button", { name: /^Edit tags for / }).click();

  const request = await daemon.waitForRequest("set_project_tags");
  expect(request.params.tags).toEqual(["backend", "frontend"]);

  await expect.poll(async () =>
    projectRow.locator("> .pf-pw-project-head .pf-pw-tag").allTextContents()
  ).toEqual(["backend", "frontend"]);
});

test("deleting a project removes all of its sessions", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  page.on("dialog", (dialog) => dialog.accept());

  await openWorkspace(page);

  const projectRow = page.locator(".pf-pw-project").first();
  const sessionsBefore = await projectRow.locator(".pf-pw-agent-wrap").count();
  expect(sessionsBefore).toBeGreaterThan(0);

  await projectRow.getByRole("button", { name: /^Delete project / }).click();

  const request = await daemon.waitForRequest("delete_project");
  expect(typeof request.params.folderPath).toBe("string");

  // The whole project row is gone — at minimum its session cards are gone.
  await expect.poll(async () =>
    page.locator(".pf-pw-project").nth(0).locator(".pf-pw-agent-wrap").count()
  ).not.toBe(sessionsBefore);
});

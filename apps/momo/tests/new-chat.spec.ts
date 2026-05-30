import { expect, test, type Locator, type Page } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";
import { bootOnboarded } from "./support/bootHelpers";

/** The flat Tasks session list in the rail. */
function taskList(page: Page): Locator {
  return page.locator(".spaces .space-items");
}

test("firstChars: collapses whitespace, code-point-safe slice, ellipsis when truncated", async ({
  page
}) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);

  const results = await page.evaluate(async () => {
    const mod = await import("/src/lib/sessionTitle.ts");
    const f = mod.firstChars as (t: string, n?: number) => string;
    return {
      short: f("hello"),
      exactly10: f("0123456789"),
      eleven: f("0123456789a"),
      whitespace: f("  a   b\nc  "),
      cjk: f("帮我订一张去东京的机票"),
      emoji: f("👍👍👍👍👍👍👍👍👍👍👍")
    };
  });

  expect(results.short).toBe("hello");
  expect(results.exactly10).toBe("0123456789");
  expect(results.eleven).toBe("0123456789…");
  expect(results.whitespace).toBe("a b c");
  // 11 CJK code points → first 10 + ellipsis.
  expect(results.cjk).toBe("帮我订一张去东京的机…");
  // 11 single-scalar emoji → first 10 + ellipsis (Array.from keeps code points whole).
  expect(results.emoji).toBe("👍👍👍👍👍👍👍👍👍👍…");
});

test("first send shows the first-10-chars title instantly, never \"New task\"", async ({
  page
}) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);

  await page.getByLabel("New chat").click();
  await expect(page).toHaveURL(/#\/new$/);

  // 11 CJK code points → optimistic title is the first 10 + ellipsis.
  const prompt = "帮我订一张去东京的机票";
  await page.getByLabel("Message").fill(prompt);
  await page.getByLabel("Message").press("Enter");

  await daemon.waitForRequest("create_session");
  await page.waitForURL(/#\/agent\/session-created-1$/);

  await expect(taskList(page).getByText("帮我订一张去东京的机…", { exact: true })).toBeVisible();
  await expect(taskList(page).getByText("New task", { exact: true })).toHaveCount(0);
});

test("daemon generated title replaces the optimistic placeholder", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);

  await page.getByLabel("New chat").click();
  const prompt = "帮我订一张去东京的机票";
  await page.getByLabel("Message").fill(prompt);
  await page.getByLabel("Message").press("Enter");

  await daemon.waitForRequest("create_session");
  const sessionId = "session-created-1";
  await page.waitForURL(new RegExp(`#/agent/${sessionId}$`));
  await expect(taskList(page).getByText("帮我订一张去东京的机…", { exact: true })).toBeVisible();

  // Wait until the store has wired the workspace listener before broadcasting,
  // or the one-shot event races the subscription.
  await daemon.waitForRequest(
    "subscribe_event",
    (req) => req.params.event === "workspace:sessions:changed"
  );
  daemon.updateSessionMetadata(sessionId, {
    displayName: null,
    generatedTitle: "预订东京机票",
    title: "预订东京机票"
  });
  daemon.emit("workspace:sessions:changed", { reason: "generated_title", sessionId });

  await expect(taskList(page).getByText("预订东京机票", { exact: true })).toBeVisible();
  await expect(taskList(page).getByText("帮我订一张去东京的机…", { exact: true })).toHaveCount(0);
});

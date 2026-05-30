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

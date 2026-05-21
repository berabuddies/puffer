import { expect, test } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";

const baseTime = Date.now();

test("PLUS-119: each project row renders as a card with a muted agent strip", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-card",
        displayName: "Card test agent",
        title: "Card test agent",
        cwd: "/tmp/puffer-card",
        folderPath: "/tmp/puffer-card",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  const project = page.locator(".pf-pw-project").first();
  await expect(project).toBeVisible();

  const styles = await project.evaluate((el) => {
    const head = el.querySelector(".pf-pw-project-head") as HTMLElement | null;
    const strip = el.querySelector(".pf-pw-agents-strip") as HTMLElement | null;
    const projectCS = getComputedStyle(el);
    return {
      borderTopWidth: projectCS.borderTopWidth,
      borderTopLeftRadius: projectCS.borderTopLeftRadius,
      projectBg: projectCS.backgroundColor,
      headBg: head ? getComputedStyle(head).backgroundColor : null,
      stripBg: strip ? getComputedStyle(strip).backgroundColor : null
    };
  });

  expect(parseFloat(styles.borderTopWidth)).toBeGreaterThan(0);
  expect(parseFloat(styles.borderTopLeftRadius)).toBeGreaterThan(0);
  expect(styles.stripBg).not.toBe(styles.headBg);
  expect(styles.stripBg).not.toBe("rgba(0, 0, 0, 0)");
});

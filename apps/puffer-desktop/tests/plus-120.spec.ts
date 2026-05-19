import { expect, test } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";

test("PLUS-120: opening an agent deactivates the Project tab and vice versa", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  const projectTab = page.locator('.pf-sidebar').getByRole("button", { name: "Project" });
  const activeAgentRow = () => page.locator('.pf-sidebar-agent-row[data-active="true"]');

  await expect(projectTab).toHaveAttribute("data-active", "true");
  await expect(activeAgentRow()).toHaveCount(0);

  await page
    .locator(".pf-sidebar-agents-list")
    .getByRole("button", { name: /^Browser regression\b/ })
    .click();
  await expect(page.locator(".pf-agent-detail")).toBeVisible();

  await expect(projectTab).toHaveAttribute("data-active", "false");
  await expect(activeAgentRow()).toHaveCount(1);

  await projectTab.click();

  await expect(projectTab).toHaveAttribute("data-active", "true");
  await expect(activeAgentRow()).toHaveCount(0);
});

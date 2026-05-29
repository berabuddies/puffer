import { test, type Page } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";

async function openWorkflows(page: Page) {
  await page.locator(".pf-sidebar").getByRole("button", { name: "Workflows" }).click();
}

async function openWorkflowDetail(page: Page) {
  await openWorkflows(page);
  await page.getByLabel("Workflow list").getByRole("button", { name: /agent-review-workflow/ }).click();
}

test("workflows overview screenshot", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openWorkflows(page);
  await page.waitForTimeout(500);
  await page.screenshot({ path: "test-results/workflows-overview.png", fullPage: true });
});

test("workflows detail screenshot (canvas + inspector open)", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openWorkflowDetail(page);
  await page.waitForTimeout(500);
  await page.screenshot({ path: "test-results/workflows-detail-open.png", fullPage: true });
});

test("workflows detail screenshot (inspector collapsed)", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openWorkflowDetail(page);
  await page.waitForTimeout(300);
  await page.getByRole("button", { name: "Collapse inspector" }).click();
  await page.waitForTimeout(300);
  await page.screenshot({ path: "test-results/workflows-detail-collapsed.png", fullPage: true });
});

test("workflows detail screenshot (runs sheet expanded)", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openWorkflowDetail(page);
  await page.waitForTimeout(300);
  const toggle = page.getByRole("button", { name: /Runs/ });
  const expanded = await toggle.getAttribute("aria-expanded");
  if (expanded !== "true") {
    await toggle.click();
    await page.waitForTimeout(300);
  }
  await page.screenshot({ path: "test-results/workflows-detail-runs-open.png", fullPage: true });
});

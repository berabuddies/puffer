import { expect, test, type Locator } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";

async function measureButton(button: Locator) {
  return button.evaluate((node) => {
    const rect = node.getBoundingClientRect();
    const style = window.getComputedStyle(node);
    return {
      left: rect.left,
      top: rect.top,
      width: rect.width,
      height: rect.height,
      fontWeight: style.fontWeight
    };
  });
}

test("provider choices keep stable geometry while hovering", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "New agent in default workspace" }).click();
  const dialog = page.getByRole("dialog", { name: "New agent" });
  await expect(dialog).toBeVisible();

  const codexChoice = dialog.getByRole("radio", { name: /Codex/ });
  const before = await measureButton(codexChoice);
  await codexChoice.hover();
  await page.waitForTimeout(80);
  const after = await measureButton(codexChoice);

  expect(after.width).toBeCloseTo(before.width, 1);
  expect(after.height).toBeCloseTo(before.height, 1);
  expect(after.fontWeight).toBe(before.fontWeight);
});

test("workspace agent controls keep stable geometry while hovering", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  const agentButton = page.locator(".pf-pw-agent").first();
  await expect(agentButton).toBeVisible();
  const agentBefore = await measureButton(agentButton);
  await agentButton.hover();
  await page.waitForTimeout(80);
  const agentAfter = await measureButton(agentButton);
  expect(agentAfter.left).toBeCloseTo(agentBefore.left, 1);
  expect(agentAfter.top).toBeCloseTo(agentBefore.top, 1);
  expect(agentAfter.width).toBeCloseTo(agentBefore.width, 1);
  expect(agentAfter.height).toBeCloseTo(agentBefore.height, 1);

  const addButton = page.locator(".pf-pw-agent-add").first();
  await expect(addButton).toBeVisible();
  const addBefore = await measureButton(addButton);
  await addButton.hover();
  await page.waitForTimeout(80);
  const addAfter = await measureButton(addButton);
  expect(addAfter.left).toBeCloseTo(addBefore.left, 1);
  expect(addAfter.top).toBeCloseTo(addBefore.top, 1);
  expect(addAfter.width).toBeCloseTo(addBefore.width, 1);
  expect(addAfter.height).toBeCloseTo(addBefore.height, 1);
});

test("create-project provider segment does not change text weight on hover", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Create Project" }).click();
  const dialog = page.getByRole("dialog", { name: "Create Project" });
  await expect(dialog).toBeVisible();

  const anthropicChoice = dialog.getByRole("radio", { name: "Anthropic" });
  const before = await measureButton(anthropicChoice);
  await anthropicChoice.hover();
  await page.waitForTimeout(80);
  const after = await measureButton(anthropicChoice);

  expect(after.width).toBeCloseTo(before.width, 1);
  expect(after.height).toBeCloseTo(before.height, 1);
  expect(after.fontWeight).toBe(before.fontWeight);
});

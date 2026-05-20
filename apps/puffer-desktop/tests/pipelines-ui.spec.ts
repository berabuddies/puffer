import { expect, test } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";

test("pipeline agent provider switcher exposes selected provider state", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  const switcher = page.getByRole("radiogroup", { name: "Agent provider" });
  const codex = switcher.getByRole("radio", { name: "Codex" });
  const claude = switcher.getByRole("radio", { name: "Claude Code" });
  const puffer = switcher.getByRole("radio", { name: "Puffer" });

  await expect(codex).toHaveAttribute("aria-checked", "true");
  await expect(claude).toHaveAttribute("aria-checked", "false");
  await expect(puffer).toHaveAttribute("aria-checked", "false");

  await claude.click();
  await expect(codex).toHaveAttribute("aria-checked", "false");
  await expect(claude).toHaveAttribute("aria-checked", "true");
  await expect(puffer).toHaveAttribute("aria-checked", "false");
  await expect(page.getByLabel("Model")).toHaveValue("claude-sonnet-4-5");

  await claude.press("ArrowRight");
  await expect(claude).toHaveAttribute("aria-checked", "false");
  await expect(puffer).toHaveAttribute("aria-checked", "true");
  await expect(page.getByLabel("Model")).toHaveValue("puffer-default");

  await puffer.press("Home");
  await expect(codex).toHaveAttribute("aria-checked", "true");
  await expect(page.getByLabel("Model")).toHaveValue("gpt-5.4-codex");
});

test("pipeline graph agent nodes expose selected state", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  const graph = page.locator(".pf-pipe-graph");
  const codexNode = graph.getByRole("button", { name: /Codex implementer/ });
  const claudeNode = graph.getByRole("button", { name: /Claude reviewer/ });

  await expect(codexNode).toHaveAttribute("aria-pressed", "true");
  await expect(claudeNode).toHaveAttribute("aria-pressed", "false");

  await claudeNode.click();
  await expect(codexNode).toHaveAttribute("aria-pressed", "false");
  await expect(claudeNode).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByLabel("Agent name")).toHaveValue("Claude reviewer");
});

test("pipeline wiring disables already connected output targets", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  const wiring = page.locator(".pf-editor-wiring");
  const claudeTarget = wiring.getByRole("button", { name: /Claude reviewer/ });
  const pufferTarget = wiring.getByRole("button", { name: /Puffer shipper/ });

  await expect(claudeTarget).toBeDisabled();
  await expect(claudeTarget).toHaveAttribute("aria-pressed", "true");
  await expect(pufferTarget).toBeEnabled();
  await expect(pufferTarget).toHaveAttribute("aria-pressed", "false");

  await pufferTarget.click();

  await expect(pufferTarget).toBeDisabled();
  await expect(pufferTarget).toHaveAttribute("aria-pressed", "true");
});

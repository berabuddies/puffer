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

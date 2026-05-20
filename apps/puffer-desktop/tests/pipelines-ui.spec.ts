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

test("pipeline refresh is disabled while the workflow snapshot loads", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.delayFailure("workflow_list", () => true, "slow workflow snapshot", 250);
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  const refresh = page.getByRole("button", { name: "Refresh workflows" });
  await expect(refresh).toBeDisabled();
  await expect(refresh).toHaveAttribute("aria-busy", "true");

  await expect(refresh).toBeEnabled();
  await expect(refresh).toHaveAttribute("aria-busy", "false");
  expect(daemon.requests.filter((request) => request.method === "workflow_list")).toHaveLength(1);
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

test("pipeline refresh preserves unsaved node drafts", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  const prompt = page.getByLabel("Prompt");
  await expect(prompt).toHaveValue("Implement the requested change.");
  await prompt.fill("local draft that must survive refresh");

  daemon.setWorkflowSnapshot({
    workflows: [
      {
        schema: "puffer.workflow.v1",
        slug: "agent-review-pipeline",
        enabled: true,
        trigger: { type: "subscription", source_topic: "workspace.task.created", pattern: "review" },
        pipeline: {
          name: "Agent review pipeline",
          working_dir: "/tmp/puffer",
          concurrency: 1,
          nodes: [
            {
              id: "codex-implement",
              type: "codex",
              agent: "Codex implementer",
              model: "gpt-5.4-codex",
              tools: ["read", "edit"],
              prompt: "server refresh should not clobber local draft"
            }
          ]
        }
      }
    ],
    runs: []
  });

  await page.getByRole("button", { name: "Refresh workflows" }).click();

  await expect(prompt).toHaveValue("local draft that must survive refresh");
});

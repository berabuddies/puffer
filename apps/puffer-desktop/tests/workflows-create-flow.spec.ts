import { expect, test, type Page } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";

async function openWorkflows(page: Page) {
  await page.locator(".pf-sidebar").getByRole("button", { name: "Workflows" }).click();
}

async function startBlankDraft(page: Page) {
  await openWorkflows(page);
  await page.getByRole("button", { name: "New workflow" }).click();
  await expect(page.locator(".pf-pipe-save-note")).toContainText("Created workflow-draft");
}

test("blank create page lands on the trigger form with zero pipeline nodes", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await startBlankDraft(page);

  // Drafts auto-select the trigger node, so the trigger + workflow form is visible.
  const selected = page.locator(".pf-canvas-selected");
  await expect(selected.getByText("Trigger & workflow")).toBeVisible();
  await expect(selected.getByLabel("Workflow name")).toHaveValue("Workflow draft");
  // No agent/tool/merge/fanout nodes on the graph yet — only the trigger.
  const graphButtons = page.locator(".pf-pipe-graph .pf-pipe-node");
  await expect(graphButtons).toHaveCount(1);
});

test("trigger select only offers Connection and Cron", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await startBlankDraft(page);

  const trigger = page.getByLabel("Trigger type");
  await expect(trigger.locator("option")).toHaveCount(2);
  await expect(trigger.locator("option").nth(0)).toHaveText("Connection");
  await expect(trigger.locator("option").nth(1)).toHaveText("Cron");
});

test("add toolbar lets the user add each node kind, and the form switches accordingly", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await startBlankDraft(page);

  const selected = page.locator(".pf-canvas-selected");
  const graphNodes = page.locator(".pf-pipe-graph .pf-pipe-node");

  // Add a Codex agent. The selected-node form should switch to the agent form.
  await page.getByRole("button", { name: "Add Codex agent" }).click();
  await expect(graphNodes).toHaveCount(2);
  await expect(selected.getByLabel("Agent name")).toHaveValue(/Codex/);
  await expect(selected.getByLabel("Model")).toHaveValue(/gpt/);

  // Add a Bash tool node. The form should show a Tool select.
  await page.getByRole("button", { name: "Add tool call node" }).click();
  await expect(graphNodes).toHaveCount(3);
  const toolSelect = selected.locator("select");
  await expect(toolSelect).toHaveValue("Bash");
  await expect(selected.locator("label", { hasText: "Input" })).toContainText("bash command");

  // Switch the tool to Read — the input hint should switch to JSON.
  await toolSelect.selectOption("Read");
  await expect(selected.locator("label", { hasText: "Input" })).toContainText("JSON");

  // Add Merge. The form should switch to a merge stub message.
  await page.getByRole("button", { name: "Add merge node" }).click();
  await expect(graphNodes).toHaveCount(4);
  await expect(selected.getByText(/Merge collects all dependency outputs/)).toBeVisible();

  // Add Fanout. The form should switch to a fanout stub.
  await page.getByRole("button", { name: "Add fanout node" }).click();
  await expect(graphNodes).toHaveCount(5);
  await expect(selected.getByText(/Fanout passes its upstream output/)).toBeVisible();
});

test("wiring chips toggle dependency edges in the graph", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await startBlankDraft(page);

  const selected = page.locator(".pf-canvas-selected");

  // Need at least two nodes to wire.
  await page.getByRole("button", { name: "Add Codex agent" }).click();
  await page.getByRole("button", { name: "Add Claude agent" }).click();

  // The Claude node should have a wiring chip for Codex.
  const wiring = selected.locator(".pf-canvas-wiring");
  await expect(wiring).toBeVisible();
  const codexChip = wiring.locator(".pf-canvas-wiring-chip", { hasText: /Codex/ });
  await expect(codexChip).toBeVisible();
  // It should default to checked (new node depends on the previously-selected node).
  await expect(codexChip.locator("input[type=checkbox]")).toBeChecked();

  await codexChip.locator("input[type=checkbox]").uncheck();
  await expect(codexChip.locator("input[type=checkbox]")).not.toBeChecked();
});

test("save round-trips a blank-create workflow with a Bash tool node", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await startBlankDraft(page);

  const selected = page.locator(".pf-canvas-selected");
  await page.getByRole("button", { name: "Add tool call node" }).click();
  // Replace the default Bash command.
  await selected.locator("textarea").fill("echo OK");

  const saveButton = page.getByRole("button", { name: "Save workflow" });
  await expect(saveButton).toBeEnabled();
  await saveButton.click();

  const request = await daemon.waitForRequest("workflow_save", (candidate) => {
    const workflow = candidate.params.workflow as { slug?: string };
    return workflow.slug === "workflow-draft";
  });
  const workflow = request.params.workflow as {
    pipeline?: { nodes?: Array<{ type?: string; tools?: string[]; prompt?: string }> };
  };
  const nodes = workflow.pipeline?.nodes ?? [];
  expect(nodes).toHaveLength(1);
  expect(nodes[0].type).toBe("tool");
  expect(nodes[0].tools).toEqual(["Bash"]);
  expect(nodes[0].prompt).toBe("echo OK");
});

test("remove button deletes the selected node and clears the form", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await startBlankDraft(page);

  const selected = page.locator(".pf-canvas-selected");
  const graphNodes = page.locator(".pf-pipe-graph .pf-pipe-node");

  await page.getByRole("button", { name: "Add Codex agent" }).click();
  await expect(graphNodes).toHaveCount(2);

  await selected.getByRole("button", { name: /Remove/ }).click();
  await expect(graphNodes).toHaveCount(1);
  await expect(
    selected.getByText(/Click the trigger to set it up, click any node to edit it/)
  ).toBeVisible();
});

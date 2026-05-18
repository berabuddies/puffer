import { expect, test } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";

test("default model cannot be saved before provider models load", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.delayResponse(
    "list_provider_models",
    (request) => request.params.providerId === "anthropic",
    160
  );
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Providers" }).click();

  const pane = page.locator(".pf-settings-pane");
  const providerSelect = pane.getByLabel("Provider");
  const modelSelect = pane.getByLabel("Model");
  const saveButton = pane.getByRole("button", { name: "Save default" });

  await providerSelect.selectOption("anthropic");
  await expect(modelSelect).toBeDisabled();
  await expect(saveButton).toBeDisabled();

  await expect(modelSelect).toBeEnabled();
  await expect(modelSelect).toHaveValue("test-model");
  await expect(saveButton).toBeEnabled();

  await saveButton.click();
  const update = await daemon.waitForRequest("update_config");
  expect(update.params).toMatchObject({
    defaultProvider: "anthropic",
    defaultModel: "test-model"
  });
});

test("permissions settings save tool policies through the daemon", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Permissions" }).click();
  await expect(page.getByText("Stored at")).toBeVisible();

  await page.getByRole("button", { name: "Add rule" }).click();
  const row = page.locator(".pf-perm-row").last();
  await row.locator("input").fill("browser_open");
  await row.locator("select").selectOption("deny");
  await page.getByRole("button", { name: "Save" }).click();

  const request = await daemon.waitForRequest("save_permissions");
  expect(request.params.tools).toMatchObject({
    bash: "ask",
    browser_open: "deny"
  });
});

test("MCP settings add server through the daemon", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "MCP Servers" }).click();
  await expect(page.locator(".pf-mcp-card .title").filter({ hasText: "Playwright" })).toBeVisible();

  await page.getByLabel("ID").fill("github");
  await page.getByLabel("Name").fill("GitHub");
  await page.getByLabel("Command").fill("npx");
  await page.getByLabel("Arguments").fill("@modelcontextprotocol/server-github");
  await page.getByLabel("Description").fill("GitHub issue and PR tools");
  await page.getByRole("button", { name: "Add server" }).click();

  const request = await daemon.waitForRequest("add_mcp_server");
  expect(request.params).toMatchObject({
    id: "github",
    displayName: "GitHub",
    description: "GitHub issue and PR tools",
    transport: "stdio",
    target: "npx @modelcontextprotocol/server-github",
    scope: "local"
  });
  await expect(page.getByText("Added github")).toBeVisible();
});

test("MCP settings do not reload-loop when no servers are configured", async ({ page }) => {
  const daemon = new FakeDaemon({ mcpServers: [] });
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "MCP Servers" }).click();
  await daemon.waitForRequest("list_mcp_servers");

  await expect(page.getByText("No MCP servers configured.")).toBeVisible();
  await page.waitForTimeout(300);
  expect(daemon.requests.filter((request) => request.method === "list_mcp_servers")).toHaveLength(1);
});

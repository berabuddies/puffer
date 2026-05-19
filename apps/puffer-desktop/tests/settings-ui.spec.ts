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

test("default model save is ignored while already saving", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.delayResponse("update_config", () => true, 500);
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Providers" }).click();

  const saveButton = page.locator(".pf-settings-pane").getByRole("button", {
    name: "Save default"
  });
  await expect(saveButton).toBeEnabled();
  await saveButton.evaluate((button) => {
    (button as HTMLButtonElement).click();
    (button as HTMLButtonElement).click();
  });
  await daemon.waitForRequest("update_config");
  await page.waitForTimeout(80);

  expect(daemon.requests.filter((request) => request.method === "update_config")).toHaveLength(1);
});

test("default model controls are disabled while saving", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.delayResponse("update_config", () => true, 500);
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Providers" }).click();

  const pane = page.locator(".pf-settings-pane");
  const providerSelect = pane.getByLabel("Provider");
  const modelSelect = pane.getByLabel("Model");
  await expect(providerSelect).toBeEnabled();
  await expect(modelSelect).toBeEnabled();

  await pane.getByRole("button", { name: "Save default" }).click();
  await daemon.waitForRequest("update_config");

  await expect(providerSelect).toBeDisabled();
  await expect(modelSelect).toBeDisabled();
});

test("advertised settings shortcut opens settings", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await expect(page.getByRole("button", { name: "Connect project" })).toBeVisible();
  await page.keyboard.press("Control+,");

  await expect(page.getByRole("heading", { name: "General" })).toBeVisible();
  await page.getByRole("button", { name: "Shortcuts" }).click();
  await expect(page.getByText("Cmd/Ctrl + ,")).toBeVisible();
  await expect(page.getByText("Open settings")).toBeVisible();
});

test("provider API key connect requires a non-empty key", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Providers" }).click();

  const input = page.getByLabel("API key for Anthropic");
  const connect = page
    .locator(".provider-card")
    .filter({ hasText: "Anthropic" })
    .getByRole("button", { name: "Connect" });

  await expect(connect).toBeDisabled();
  await input.fill("   ");
  await expect(connect).toBeDisabled();
  await input.press("Enter");
  await page.waitForTimeout(50);
  expect(
    daemon.requests.filter((request) => request.method === "login_with_api_key")
  ).toHaveLength(0);

  await input.fill("  sk-test  ");
  await expect(connect).toBeEnabled();
  await connect.click();

  const request = await daemon.waitForRequest("login_with_api_key");
  expect(request.params).toMatchObject({
    providerId: "anthropic",
    apiKey: "sk-test"
  });
});

test("provider API key enter submit is ignored while login is already busy", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.delayResponse(
    "login_with_api_key",
    (request) => request.params.providerId === "anthropic",
    500
  );
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Providers" }).click();

  const input = page.getByLabel("API key for Anthropic");
  await input.fill("sk-repeat-safe");
  await input.press("Enter");
  await daemon.waitForRequest("login_with_api_key");
  await input.press("Enter");
  await page.waitForTimeout(80);

  expect(
    daemon.requests.filter(
      (request) =>
        request.method === "login_with_api_key" &&
        request.params.providerId === "anthropic"
    )
  ).toHaveLength(1);
});

test("provider OAuth connect is ignored while login is already busy", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.delayResponse(
    "login_with_oauth",
    (request) => request.params.providerId === "codex",
    500
  );
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Providers" }).click();

  const connect = page
    .locator(".provider-card")
    .filter({ hasText: "Codex" })
    .getByRole("button", { name: "Connect with OAuth" });
  await expect(connect).toBeEnabled();
  await connect.evaluate((button) => {
    (button as HTMLButtonElement).click();
    (button as HTMLButtonElement).click();
  });
  await daemon.waitForRequest("login_with_oauth");
  await page.waitForTimeout(80);

  expect(
    daemon.requests.filter(
      (request) =>
        request.method === "login_with_oauth" &&
        request.params.providerId === "codex"
    )
  ).toHaveLength(1);
});

test("provider auth controls are disabled while another provider is busy", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.delayResponse(
    "login_with_oauth",
    (request) => request.params.providerId === "codex",
    500
  );
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Providers" }).click();

  const codexOauth = page
    .locator(".provider-card")
    .filter({ hasText: "Codex" })
    .getByRole("button", { name: "Connect with OAuth" });
  const anthropicCard = page.locator(".provider-card").filter({ hasText: "Anthropic" });
  const anthropicInput = page.getByLabel("API key for Anthropic");
  const anthropicConnect = anthropicCard.getByRole("button", { name: "Connect" });

  await anthropicInput.fill("sk-while-codex-busy");
  await expect(codexOauth).toBeEnabled();
  await expect(anthropicInput).toBeEnabled();
  await expect(anthropicConnect).toBeEnabled();

  await codexOauth.click();
  await daemon.waitForRequest("login_with_oauth");

  await expect(anthropicInput).toBeDisabled();
  await expect(anthropicConnect).toBeDisabled();
});

test("provider logout is ignored while already busy", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.delayResponse(
    "logout_provider",
    (request) => request.params.providerId === "anthropic",
    500
  );
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Settings" }).click();

  const signOut = page
    .locator(".pf-settings-row")
    .filter({ hasText: "Account" })
    .locator("div", { hasText: /^anthropic\s*·/ })
    .getByRole("button", { name: "Sign out" });
  await expect(signOut).toBeEnabled();
  await signOut.evaluate((button) => {
    (button as HTMLButtonElement).click();
    (button as HTMLButtonElement).click();
  });
  await daemon.waitForRequest("logout_provider");
  await page.waitForTimeout(80);

  expect(
    daemon.requests.filter(
      (request) =>
        request.method === "logout_provider" &&
        request.params.providerId === "anthropic"
    )
  ).toHaveLength(1);
});

test("external provider credential import is ignored while already busy", async ({ page }) => {
  const daemon = new FakeDaemon({
    externalCredentials: [
      {
        providerId: "codex",
        source: "codex",
        kind: "oauth",
        description: "Codex CLI OAuth",
        sourcePath: "/tmp/home/.codex/auth.json"
      }
    ]
  });
  daemon.delayResponse(
    "import_external_credential",
    (request) => request.params.providerId === "codex" && request.params.source === "codex",
    500
  );
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Providers" }).click();

  const importButton = page
    .locator(".provider-card")
    .filter({ hasText: "Codex" })
    .getByRole("button", { name: "Use credentials from ~/.codex" });
  await expect(importButton).toBeVisible();
  await importButton.evaluate((button) => {
    (button as HTMLButtonElement).click();
    (button as HTMLButtonElement).click();
  });
  await daemon.waitForRequest("import_external_credential");
  await page.waitForTimeout(80);

  expect(
    daemon.requests.filter(
      (request) =>
        request.method === "import_external_credential" &&
        request.params.providerId === "codex" &&
        request.params.source === "codex"
    )
  ).toHaveLength(1);
});

test("settings auth uses the configured daemon when Tauri globals exist", async ({ page }) => {
  await page.addInitScript(() => {
    (window as unknown as { __TAURI__?: unknown; __TAURI_INTERNALS__?: unknown }).__TAURI__ = {};
    (window as unknown as { __TAURI__?: unknown; __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
  });
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await daemon.waitForRequest("load_settings_snapshot");
  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Providers" }).click();

  await page.getByLabel("API key for Anthropic").fill("sk-tauri-daemon");
  await page
    .locator(".provider-card")
    .filter({ hasText: "Anthropic" })
    .getByRole("button", { name: "Connect" })
    .click();

  const request = await daemon.waitForRequest("login_with_api_key");
  expect(request.params).toMatchObject({
    providerId: "anthropic",
    apiKey: "sk-tauri-daemon"
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

test("permissions save is ignored while already saving", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.delayResponse("save_permissions", () => true, 500);
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Permissions" }).click();
  await expect(page.getByText("Stored at")).toBeVisible();

  await page.getByRole("button", { name: "Add rule" }).click();
  const row = page.locator(".pf-perm-row").last();
  await row.locator("input").fill("browser_open");
  await row.locator("select").selectOption("deny");

  const save = page.getByRole("button", { name: "Save" });
  await expect(save).toBeEnabled();
  await save.evaluate((button) => {
    (button as HTMLButtonElement).click();
    (button as HTMLButtonElement).click();
  });
  await daemon.waitForRequest("save_permissions");
  await page.waitForTimeout(80);

  expect(daemon.requests.filter((request) => request.method === "save_permissions")).toHaveLength(1);
});

test("permissions controls are disabled while saving", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.delayResponse("save_permissions", () => true, 500);
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Permissions" }).click();
  await expect(page.getByText("Stored at")).toBeVisible();

  await page.getByRole("button", { name: "Add rule" }).click();
  const row = page.locator(".pf-perm-row").last();
  const toolInput = row.locator("input");
  const modeSelect = row.locator("select");
  const removeRule = row.getByRole("button", { name: "Remove rule" });
  await toolInput.fill("browser_open");
  await modeSelect.selectOption("deny");

  await page.getByRole("button", { name: "Save" }).click();
  await daemon.waitForRequest("save_permissions");

  await expect(toolInput).toBeDisabled();
  await expect(modeSelect).toBeDisabled();
  await expect(removeRule).toBeDisabled();
  await expect(page.getByRole("button", { name: "Add rule" })).toBeDisabled();
});

test("permissions settings keep edits after a late list response", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.delayResponse("list_permissions", () => true, 220);
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Permissions" }).click();

  const addRule = page.getByRole("button", { name: "Add rule" });
  await expect(addRule).toBeDisabled();
  await expect(page.getByText("Loading permissions...")).toBeVisible();
  await expect(page.getByText("Stored at")).toBeVisible();

  await addRule.click();
  const row = page.locator(".pf-perm-row").last();
  await row.locator("input").fill("browser_open");
  await row.locator("select").selectOption("deny");

  await expect(row.locator("input")).toHaveValue("browser_open");
  await expect(row.locator("select")).toHaveValue("deny");

  await page.getByRole("button", { name: "Save" }).click();
  const request = await daemon.waitForRequest("save_permissions");
  expect(request.params.tools).toMatchObject({
    bash: "ask",
    browser_open: "deny"
  });
});

test("settings panes follow refreshed workspace state", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Providers" }).click();
  await expect(page.locator(".pf-settings-pane").getByLabel("Provider")).toHaveValue("codex");

  await page.getByRole("button", { name: "Permissions" }).click();
  await expect(page.getByText("Stored at")).toContainText("/tmp/puffer/.puffer/permissions.json");
  const permissionRequestsBefore = daemon.requests.filter(
    (request) => request.method === "list_permissions"
  ).length;

  daemon.setWorkspaceRoot("/tmp/puffer-next");
  daemon.setSettingsConfig({
    defaultProvider: "anthropic",
    defaultModel: "test-model"
  });
  daemon.setPermissions({ browser_open: "deny" });

  await page.getByRole("button", { name: "General" }).click();
  await page.getByRole("button", { name: "Refresh" }).click();
  await expect(page.locator(".pf-settings-row").filter({ hasText: "Workspace root" })).toContainText(
    "/tmp/puffer-next"
  );

  await page.getByRole("button", { name: "Providers" }).click();
  await expect(page.locator(".pf-settings-pane").getByLabel("Provider")).toHaveValue("anthropic");

  await page.getByRole("button", { name: "Permissions" }).click();
  await expect.poll(() =>
    daemon.requests.filter((request) => request.method === "list_permissions").length
  ).toBe(permissionRequestsBefore + 1);
  await expect(page.getByText("Stored at")).toContainText(
    "/tmp/puffer-next/.puffer/permissions.json"
  );
  const refreshedRow = page.locator(".pf-perm-row").last();
  await expect(refreshedRow.locator("input")).toHaveValue("browser_open");
  await expect(refreshedRow.locator("select")).toHaveValue("deny");
});

test("remember last session persists and restores agent detail", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Settings" }).click();
  const remember = page
    .locator(".pf-settings-row")
    .filter({ hasText: "Remember last session" })
    .locator("input");
  await remember.check();
  await expect(remember).toBeChecked();

  await page.getByRole("button", { name: "Workspace" }).click();
  await page
    .locator(".pf-sidebar-agents-list")
    .getByRole("button", { name: /^Browser regression\b/ })
    .click();
  await expect(page.getByRole("button", { name: "Back" })).toBeVisible();

  await page.reload();
  await expect(page.getByRole("button", { name: "Back" })).toBeVisible();
  await expect(page.locator(".pf-agent-detail .primary-title")).toContainText("Browser regression");
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

test("MCP settings add server is ignored while already saving", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.delayResponse("add_mcp_server", () => true, 500);
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "MCP Servers" }).click();
  await expect(page.locator(".pf-mcp-card .title").filter({ hasText: "Playwright" })).toBeVisible();

  await page.getByLabel("ID").fill("github");
  await page.getByLabel("Name").fill("GitHub");
  await page.getByLabel("Command").fill("npx");
  await page.getByLabel("Arguments").fill("@modelcontextprotocol/server-github");

  const addServer = page.getByRole("button", { name: "Add server" });
  await expect(addServer).toBeEnabled();
  await addServer.evaluate((button) => {
    (button as HTMLButtonElement).click();
    (button as HTMLButtonElement).click();
  });
  await daemon.waitForRequest("add_mcp_server");
  await page.waitForTimeout(80);

  expect(daemon.requests.filter((request) => request.method === "add_mcp_server")).toHaveLength(1);
});

test("MCP add server controls are disabled while saving", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.delayResponse("add_mcp_server", () => true, 500);
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "MCP Servers" }).click();
  await expect(page.locator(".pf-mcp-card .title").filter({ hasText: "Playwright" })).toBeVisible();

  const id = page.getByLabel("ID");
  const name = page.getByLabel("Name");
  const transport = page.getByLabel("Transport");
  const scope = page.getByLabel("Scope");
  const command = page.getByLabel("Command");
  const args = page.getByLabel("Arguments");
  const description = page.getByLabel("Description");

  await id.fill("github");
  await name.fill("GitHub");
  await command.fill("npx");
  await args.fill("@modelcontextprotocol/server-github");
  await description.fill("GitHub issue and PR tools");

  await page.getByRole("button", { name: "Add server" }).click();
  await daemon.waitForRequest("add_mcp_server");

  await expect(id).toBeDisabled();
  await expect(name).toBeDisabled();
  await expect(transport).toBeDisabled();
  await expect(scope).toBeDisabled();
  await expect(command).toBeDisabled();
  await expect(args).toBeDisabled();
  await expect(description).toBeDisabled();
});

test("MCP settings keep added server when the initial list resolves late", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.delayResponse("list_mcp_servers", () => true, 250);
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "MCP Servers" }).click();

  await page.getByLabel("ID").fill("github");
  await page.getByLabel("Name").fill("GitHub");
  await page.getByLabel("Command").fill("npx");
  await page.getByLabel("Arguments").fill("@modelcontextprotocol/server-github");
  await page.getByLabel("Description").fill("GitHub issue and PR tools");
  await page.getByRole("button", { name: "Add server" }).click();

  await daemon.waitForRequest("add_mcp_server");
  const title = page.locator(".pf-mcp-card .title").filter({ hasText: "GitHub" });
  await expect(title).toBeVisible();

  await page.waitForTimeout(300);
  await expect(title).toBeVisible();
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

import { expect, test, type Page } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";

async function openProviderSetup(page: Page, providerName: string) {
  const displayName = providerName === "Codex" ? "OpenAI" : providerName;
  const card = page.locator(".provider-card").filter({ hasText: displayName }).last();
  await card.getByRole("button", { name: "Add connect" }).click();
  return page.getByRole("dialog", { name: `Connect ${displayName}` });
}

test("empty provider registry still offers built-in setup options", async ({ page }) => {
  const daemon = new FakeDaemon({ auth: [], providers: [] });
  await daemon.install(page);
  await daemon.open(page, { forceOnboarding: true, skipOnboarding: false });

  await expect(page.getByText("Provider registry is empty.")).toBeVisible();
  await expect(page.getByText("No providers are registered in this workspace.")).toHaveCount(0);
  await expect(page.getByRole("heading", { name: "OpenAI" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Anthropic" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "OpenRouter" })).toBeVisible();

  await expect(page.locator(".provider-card").filter({ hasText: "Anthropic" }).getByRole("button", { name: "Add connect" })).toBeVisible();

  const modal = await openProviderSetup(page, "Anthropic");
  await modal.getByLabel("API key for Anthropic").fill("sk-ant-test");
  await modal.getByRole("button", { name: "Add connect", exact: true }).click();

  const request = await daemon.waitForRequest("login_with_api_key");
  expect(request.params).toMatchObject({
    providerId: "anthropic",
    apiKey: "sk-ant-test"
  });
  await expect(page.getByRole("button", { name: "Project", exact: true })).toBeVisible();
});

test("custom provider modal saves base URL and API key", async ({ page }) => {
  const daemon = new FakeDaemon({ auth: [], providers: [] });
  await daemon.install(page);
  await daemon.open(page, { forceOnboarding: true, skipOnboarding: false });

  const customCard = page.locator(".provider-card").filter({ hasText: "Custom provider" });
  await customCard.getByRole("button", { name: "Add connect" }).click();

  const modal = page.getByRole("dialog", { name: "Connect Custom provider" });
  await expect(modal.getByLabel("Base URL")).toBeVisible();
  await expect(modal.getByLabel("API key")).toBeVisible();
  await expect(modal.getByText("Saved credentials")).toHaveCount(0);
  await expect(modal.getByText("OAuth")).toHaveCount(0);

  await modal.getByLabel("Base URL").fill("https://proxy.example/v1");
  await modal.getByLabel("API key").fill("sk-custom-test");
  await modal.getByRole("button", { name: "Add connect" }).click();

  const configRequest = await daemon.waitForRequest("update_config", (request) =>
    request.params.openaiBaseUrl === "https://proxy.example/v1"
  );
  expect(configRequest.params).toMatchObject({
    defaultProvider: "openai",
    openaiBaseUrl: "https://proxy.example/v1"
  });

  const loginRequest = await daemon.waitForRequest("login_with_api_key", (request) =>
    request.params.apiKey === "sk-custom-test"
  );
  expect(loginRequest.params).toMatchObject({
    providerId: "openai",
    apiKey: "sk-custom-test"
  });
});

test("web preview auto-connects to local dev backend websocket", async ({ page }) => {
  const daemon = new FakeDaemon({ url: "ws://127.0.0.1:1421/ws" });
  await daemon.install(page);

  await page.goto("/?skipOnboarding=1");
  await daemon.waitForRequest("load_settings_snapshot");

  expect(daemon.socketUrls.some((url) => url.startsWith("ws://127.0.0.1:1421/ws"))).toBe(true);
  await page.getByRole("button", { name: "Settings" }).click();
  const pane = page.locator(".pf-settings-pane");
  await expect(pane.locator(".pf-settings-row").filter({ hasText: "Daemon" })).toContainText(
    "ws://127.0.0.1:1421/ws"
  );
  await expect(pane.getByText("Preview mode")).toHaveCount(0);
});

test("default model cannot be saved before provider models load", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.delayResponse(
    "list_provider_models",
    (request) => request.params.providerId === "anthropic",
    160
  );
  await daemon.install(page);
  await daemon.open(page, { allowUnauthenticatedWorkspace: true });

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Providers" }).click();

  const pane = page.locator(".pf-settings-pane");
  const providerSelect = pane.getByLabel("Provider");
  const modelSelect = pane.getByLabel("Model");
  const saveButton = pane.getByRole("button", { name: "Save default" });

  await providerSelect.selectOption("anthropic");
  await expect(modelSelect).toBeDisabled();
  await expect(saveButton).toBeDisabled();
  await expect(pane.getByText("Fetching Anthropic models...")).toBeVisible();

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

test("default model load errors stay scoped to the selected provider", async ({ page }) => {
  const daemon = new FakeDaemon({
    providerModels: {
      codex: [
        {
          id: "codex-default",
          displayName: "Codex Default",
          provider: "codex",
          api: "openai-responses",
          supportsTools: true,
          supportsVision: false,
          contextWindow: null,
          maxOutputTokens: null,
          thinkingOptions: [],
          defaultThinkingOptionId: null,
          isDefault: true
        }
      ]
    }
  });
  daemon.setSettingsConfig({
    defaultProvider: "codex",
    defaultModel: "codex-default"
  });
  daemon.delayFailure(
    "list_provider_models",
    (request) => request.params.providerId === "anthropic",
    "anthropic models failed late",
    160
  );
  await daemon.install(page);
  await daemon.open(page, { allowUnauthenticatedWorkspace: true });

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Providers" }).click();

  const pane = page.locator(".pf-settings-pane");
  const providerSelect = pane.getByLabel("Provider");
  const modelSelect = pane.getByLabel("Model");
  await expect(modelSelect).toHaveValue("codex-default");

  await providerSelect.selectOption("anthropic");
  await expect(modelSelect).toBeDisabled();
  await providerSelect.selectOption("codex");
  await expect(providerSelect).toHaveValue("codex");
  await expect(modelSelect).toHaveValue("codex-default");

  await page.waitForTimeout(220);
  await expect(pane.getByText("anthropic models failed late")).toHaveCount(0);
  await expect(modelSelect).toHaveValue("codex-default");
});

test("network proxy test renders connected latency inline", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Network" }).click();

  const pane = page.locator(".pf-settings-pane");
  const proxyCard = pane.locator(".pf-network-proxy-card").filter({
    hasText: "socks5://127.0.0.1:7890"
  });
  await proxyCard.getByRole("button", { name: "Test" }).click();

  const request = await daemon.waitForRequest("test_proxy");
  expect(request.params).toMatchObject({ proxyId: "local" });
  await expect(proxyCard.locator(".pf-network-status")).toHaveText("connected (ping: 848 ms)");
  await expect(proxyCard.locator(".pf-network-status")).toHaveAttribute("data-state", "connected");
});

test("network proxy editor uses compact controls", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Network" }).click();

  const proxySection = page.locator("section[aria-label='Proxy list']");
  await expect(
    proxySection.locator(".pf-network-section-head").getByRole("button", { name: "Add proxy" })
  ).toBeVisible();

  await proxySection.getByRole("button", { name: "Add proxy" }).click();
  const dialog = page.getByRole("dialog", { name: "Add proxy" });
  await expect(dialog.getByLabel("Scheme")).toHaveValue("socks5");
  await expect(dialog.getByText("socks5h://127.0.0.1:7890")).toHaveCount(0);
  await expect(dialog.getByRole("button", { name: "Save proxy" })).toHaveAttribute(
    "data-variant",
    "default"
  );
  await expect(dialog.getByLabel("Port")).not.toHaveAttribute("type", "number");

  const endpointGrid = dialog.locator(".pf-network-form-grid").first();
  await expect(endpointGrid.getByLabel("Host")).toBeVisible();
  await expect(endpointGrid.getByLabel("Port")).toBeVisible();

  const credentialGrid = dialog.locator(".pf-network-form-grid").nth(1);
  await expect(credentialGrid.getByLabel("Username")).toBeVisible();
  await expect(credentialGrid.getByLabel("Password")).toBeVisible();
});

test("network proxy item delete persists the remaining proxy list", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.setNetworkProxy({
    enabled: true,
    selected: "local",
    bypass: ["localhost"],
    proxies: [
      {
        id: "local",
        scheme: "socks5",
        host: "127.0.0.1",
        port: 7890,
        username: null,
        hasPassword: false,
        uri: "socks5://127.0.0.1:7890"
      },
      {
        id: "backup",
        scheme: "socks5h",
        host: "127.0.0.1",
        port: 7891,
        username: null,
        hasPassword: false,
        uri: "socks5h://127.0.0.1:7891"
      }
    ],
    lastTest: null
  });
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Network" }).click();

  const pane = page.locator(".pf-settings-pane");
  const proxyCard = pane.locator(".pf-network-proxy-card").filter({
    hasText: "socks5://127.0.0.1:7890"
  });
  await proxyCard.getByRole("button", { name: "Delete" }).click();

  const saveRequest = await daemon.waitForRequest("save_proxy_settings");
  expect(saveRequest.params).toMatchObject({
    enabled: true,
    selected: "backup",
    bypass: ["localhost"]
  });
  expect(saveRequest.params.proxies).toEqual([
    expect.objectContaining({ id: "backup", scheme: "socks5h", host: "127.0.0.1", port: 7891 })
  ]);
  await expect(pane.getByText("socks5://127.0.0.1:7890")).toHaveCount(0);
});

test("network proxy switch is disabled without proxy list items", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.setNetworkProxy({
    enabled: true,
    selected: null,
    bypass: ["localhost"],
    proxies: [],
    lastTest: null
  });
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Network" }).click();

  const pane = page.locator(".pf-settings-pane");
  const proxySwitch = pane.locator(".pf-network-switch");
  await expect(proxySwitch).not.toBeChecked();
  await expect(proxySwitch).toBeDisabled();
  await expect(pane.locator(".pf-settings-note.warn")).toHaveCount(0);
});

test("network proxy deleting the final item disables routing", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.setNetworkProxy({
    enabled: true,
    selected: "local",
    bypass: ["localhost"],
    proxies: [
      {
        id: "local",
        scheme: "socks5",
        host: "127.0.0.1",
        port: 7890,
        username: null,
        hasPassword: false,
        uri: "socks5://127.0.0.1:7890"
      }
    ],
    lastTest: null
  });
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Network" }).click();

  const pane = page.locator(".pf-settings-pane");
  const proxyCard = pane.locator(".pf-network-proxy-card").filter({
    hasText: "socks5://127.0.0.1:7890"
  });
  await proxyCard.getByRole("button", { name: "Delete" }).click();

  const saveRequest = await daemon.waitForRequest("save_proxy_settings");
  expect(saveRequest.params).toMatchObject({
    enabled: false,
    selected: null,
    proxies: []
  });
  const proxySwitch = pane.locator(".pf-network-switch");
  await expect(proxySwitch).not.toBeChecked();
  await expect(proxySwitch).toBeDisabled();
});

test("network bypass editor preserves input and validates before save", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Network" }).click();

  const bypassSection = page.locator("section[aria-label='Bypass']");
  const bypassInput = bypassSection.locator(".pf-network-bypass");
  const saveButton = bypassSection.getByRole("button", { name: "Save bypass" });
  await expect(saveButton).toHaveAttribute("data-variant", "default");

  await bypassInput.fill("localhost\napi.example.com");
  await page.waitForTimeout(80);
  await expect(bypassInput).toHaveValue("localhost\napi.example.com");

  await saveButton.click();
  const saveRequest = await daemon.waitForRequest("save_proxy_settings");
  expect(saveRequest.params.bypass).toEqual(["localhost", "api.example.com"]);

  const savedRequestCount = daemon.requests.filter(
    (request) => request.method === "save_proxy_settings"
  ).length;
  await bypassInput.fill("localhost\n*.example.com");
  await saveButton.click();
  await page.waitForTimeout(80);

  await expect(page.getByText("Invalid bypass entry: *.example.com")).toBeVisible();
  expect(daemon.requests.filter((request) => request.method === "save_proxy_settings")).toHaveLength(
    savedRequestCount
  );
});

test("default routing only offers authenticated agent providers", async ({ page }) => {
  const daemon = new FakeDaemon({
    auth: [
      {
        providerId: "openai",
        kind: "oauth",
        email: "tester@example.com",
        expiresAtMs: null,
        scopes: [],
        planType: "test",
        organizationName: null
      },
      {
        providerId: "github",
        kind: "oauth",
        email: "tester@example.com",
        expiresAtMs: null,
        scopes: [],
        planType: "test",
        organizationName: null
      }
    ],
    providers: [
      {
        id: "openai",
        displayName: "Codex",
        baseUrl: "",
        defaultApi: "openai-responses",
        modelCount: 1,
        authModes: ["oauth"],
        sourceKind: "test",
        sourcePath: null
      },
      {
        id: "github",
        displayName: "GitHub",
        baseUrl: "",
        defaultApi: "oauth",
        modelCount: 0,
        authModes: ["oauth"],
        sourceKind: "test",
        sourcePath: null
      }
    ],
    providerModels: {
      openai: [
        {
          id: "gpt-5",
          displayName: "GPT-5",
          provider: "openai",
          api: "openai-responses",
          supportsTools: true,
          supportsVision: false,
          contextWindow: null,
          maxOutputTokens: null,
          thinkingOptions: [],
          defaultThinkingOptionId: null,
          isDefault: true
        }
      ]
    }
  });
  daemon.setSettingsConfig({
    defaultProvider: "github",
    defaultModel: null
  });
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Providers" }).click();

  const pane = page.locator(".pf-settings-pane");
  const providerSelect = pane.getByLabel("Provider");
  await expect(providerSelect).toHaveValue("openai");
  await expect(providerSelect.locator('option[value="openai"]')).toHaveCount(1);
  await expect(providerSelect.locator('option[value="github"]')).toHaveCount(0);
  await expect(pane.getByLabel("Model")).toHaveValue("gpt-5");
  await pane.getByRole("button", { name: "Save default" }).click();
  const update = await daemon.waitForRequest("update_config");
  expect(update.params).toMatchObject({
    defaultProvider: "openai",
    defaultModel: "gpt-5"
  });
});

test("default routing offers auth-free local agent providers", async ({ page }) => {
  const daemon = new FakeDaemon({
    auth: [],
    providers: [
      {
        id: "ollama",
        displayName: "Ollama",
        baseUrl: "http://localhost:11434/v1",
        defaultApi: "openai-completions",
        modelCount: 1,
        authModes: [],
        sourceKind: "test",
        sourcePath: null
      }
    ],
    providerModels: {
      ollama: [
        {
          id: "llama3.2",
          displayName: "Llama 3.2",
          provider: "ollama",
          api: "openai-completions",
          supportsTools: true,
          supportsVision: false,
          contextWindow: null,
          maxOutputTokens: null,
          thinkingOptions: [],
          defaultThinkingOptionId: null,
          isDefault: true
        }
      ]
    }
  });
  daemon.setSettingsConfig({
    defaultProvider: "ollama",
    defaultModel: "llama3.2"
  });
  await daemon.install(page);
  await daemon.open(page, { allowUnauthenticatedWorkspace: true });

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Providers" }).click();

  const pane = page.locator(".pf-settings-pane");
  const providerSelect = pane.getByLabel("Provider");
  await expect(providerSelect).toHaveValue("ollama");
  await expect(providerSelect.locator('option[value="ollama"]')).toHaveCount(1);
  await expect(pane.getByLabel("Model")).toHaveValue("llama3.2");
  await pane.getByRole("button", { name: "Save default" }).click();
  const update = await daemon.waitForRequest("update_config");
  expect(update.params).toMatchObject({
    defaultProvider: "ollama",
    defaultModel: "llama3.2"
  });
});

test("default model picker replaces a cross-provider configured model after load", async ({ page }) => {
  const daemon = new FakeDaemon({
    auth: [
      {
        providerId: "openai",
        kind: "oauth",
        email: "tester@example.com",
        expiresAtMs: null,
        scopes: [],
        planType: "test",
        organizationName: null
      }
    ],
    providers: [
      {
        id: "openai",
        displayName: "OpenAI",
        baseUrl: "",
        defaultApi: "openai-responses",
        modelCount: 1,
        authModes: ["oauth"],
        sourceKind: "test",
        sourcePath: null
      }
    ],
    providerModels: {
      openai: [
        {
          id: "gpt-5",
          displayName: "GPT-5",
          provider: "openai",
          api: "openai-responses",
          supportsTools: true,
          supportsVision: false,
          contextWindow: null,
          maxOutputTokens: null,
          thinkingOptions: [],
          defaultThinkingOptionId: null,
          isDefault: true
        }
      ]
    }
  });
  daemon.setSettingsConfig({
    defaultProvider: "openai",
    defaultModel: "claude-sonnet-4-5"
  });
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Providers" }).click();

  const pane = page.locator(".pf-settings-pane");
  await expect(pane.getByLabel("Provider")).toHaveValue("openai");
  await expect(pane.getByLabel("Model")).toHaveValue("gpt-5");

  await pane.getByRole("button", { name: "Save default" }).click();
  const update = await daemon.waitForRequest("update_config");
  expect(update.params).toMatchObject({
    defaultProvider: "openai",
    defaultModel: "gpt-5"
  });
});

test("default model picker skips models without agent tool support", async ({ page }) => {
  const daemon = new FakeDaemon({
    auth: [
      {
        providerId: "openrouter",
        kind: "api_key",
        email: null,
        expiresAtMs: null,
        scopes: [],
        planType: null,
        organizationName: null
      }
    ],
    providers: [
      {
        id: "openrouter",
        displayName: "OpenRouter",
        baseUrl: "",
        defaultApi: "openai-responses",
        modelCount: 2,
        authModes: ["api_key"],
        sourceKind: "test",
        sourcePath: null
      }
    ],
    providerModels: {
      openrouter: [
        {
          id: "owl-alpha",
          displayName: "Owl Alpha",
          provider: "openrouter",
          api: "openai-responses",
          supportsTools: false,
          supportsVision: false,
          contextWindow: null,
          maxOutputTokens: null,
          thinkingOptions: [],
          defaultThinkingOptionId: null,
          isDefault: true
        },
        {
          id: "toolsmith",
          displayName: "Toolsmith",
          provider: "openrouter",
          api: "openai-responses",
          supportsTools: true,
          supportsVision: false,
          contextWindow: null,
          maxOutputTokens: null,
          thinkingOptions: [],
          defaultThinkingOptionId: null,
          isDefault: false
        }
      ]
    }
  });
  daemon.setSettingsConfig({
    defaultProvider: "openrouter",
    defaultModel: "owl-alpha"
  });
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Providers" }).click();

  const pane = page.locator(".pf-settings-pane");
  const modelSelect = pane.getByLabel("Model");
  await expect(modelSelect).toHaveValue("toolsmith");
  await expect(modelSelect.locator('option[value="owl-alpha"]')).toHaveCount(0);

  await pane.getByRole("button", { name: "Save default" }).click();
  const update = await daemon.waitForRequest("update_config");
  expect(update.params).toMatchObject({
    defaultProvider: "openrouter",
    defaultModel: "toolsmith"
  });
});

test("default model save is disabled when no provider models support agent tools", async ({ page }) => {
  const daemon = new FakeDaemon({
    auth: [
      {
        providerId: "openrouter",
        kind: "api_key",
        email: null,
        expiresAtMs: null,
        scopes: [],
        planType: null,
        organizationName: null
      }
    ],
    providers: [
      {
        id: "openrouter",
        displayName: "OpenRouter",
        baseUrl: "",
        defaultApi: "openai-responses",
        modelCount: 1,
        authModes: ["api_key"],
        sourceKind: "test",
        sourcePath: null
      }
    ],
    providerModels: {
      openrouter: [
        {
          id: "owl-alpha",
          displayName: "Owl Alpha",
          provider: "openrouter",
          api: "openai-responses",
          supportsTools: false,
          supportsVision: false,
          contextWindow: null,
          maxOutputTokens: null,
          thinkingOptions: [],
          defaultThinkingOptionId: null,
          isDefault: true
        }
      ]
    }
  });
  daemon.setSettingsConfig({
    defaultProvider: "openrouter",
    defaultModel: "owl-alpha"
  });
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Providers" }).click();

  const pane = page.locator(".pf-settings-pane");
  await expect(pane.getByLabel("Model")).toHaveValue("");
  await expect(pane.getByRole("button", { name: "Save default" })).toBeDisabled();
  await expect(pane.getByText("No OpenRouter models support agent tools.")).toBeVisible();
});

test("providers page marks connected and disconnected providers", async ({ page }) => {
  const daemon = new FakeDaemon({
    auth: [
      {
        providerId: "openrouter",
        kind: "api_key",
        email: null,
        expiresAtMs: null,
        scopes: [],
        planType: null,
        organizationName: null
      }
    ],
    providers: [
      {
        id: "openrouter",
        displayName: "OpenRouter",
        baseUrl: "",
        defaultApi: "openai-responses",
        modelCount: 2,
        authModes: ["api_key"],
        sourceKind: "test",
        sourcePath: null
      },
      {
        id: "anthropic",
        displayName: "Anthropic",
        baseUrl: "",
        defaultApi: "anthropic-messages",
        modelCount: 1,
        authModes: ["api_key"],
        sourceKind: "test",
        sourcePath: null
      }
    ],
    providerModels: {
      openrouter: [
        {
          id: "google/gemini-3.5-flash",
          displayName: "Google: Gemini 3.5 Flash",
          provider: "openrouter",
          api: "openai-responses",
          supportsTools: true,
          supportsVision: false,
          contextWindow: null,
          maxOutputTokens: null,
          thinkingOptions: [],
          defaultThinkingOptionId: null,
          isDefault: true
        }
      ]
    }
  });
  daemon.setSettingsConfig({
    defaultProvider: "openrouter",
    defaultModel: "google/gemini-3.5-flash"
  });
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Providers" }).click();

  const connectedOpenRouterCard = page.locator('[data-section="connected"] .provider-card').filter({ hasText: "OpenRouter" });
  await expect(connectedOpenRouterCard.locator(".status")).toHaveText("Connected via api_key");
  await expect(connectedOpenRouterCard.getByRole("button", { name: "Disconnect" })).toBeVisible();

  const openRouterCard = page.locator(".provider-section").filter({ hasText: "Popular providers" }).locator(".provider-card").filter({ hasText: "OpenRouter" });
  await expect(openRouterCard).toHaveCount(0);

  const anthropicCard = page.locator(".provider-card").filter({ hasText: "Anthropic" });
  await expect(anthropicCard.locator(".status")).toHaveCount(0);
  await expect(anthropicCard.getByRole("button", { name: "Add connect" })).toBeVisible();

  daemon.delayResponse(
    "logout_provider",
    (request) => request.params.providerId === "openrouter",
    150
  );
  await connectedOpenRouterCard.getByRole("button", { name: "Disconnect" }).click();
  const logout = await daemon.waitForRequest("logout_provider");
  expect(logout.params).toMatchObject({ providerId: "openrouter" });
  await expect(page.getByRole("dialog", { name: /Connect OpenRouter/ })).toHaveCount(0);
  await expect(connectedOpenRouterCard).toHaveCount(0);
  await expect(openRouterCard.getByRole("button", { name: "Add connect" })).toBeVisible();
});

test("providers page marks auth-free local providers as ready", async ({ page }) => {
  const daemon = new FakeDaemon({
    auth: [],
    providers: [
      {
        id: "ollama",
        displayName: "Ollama",
        baseUrl: "http://localhost:11434/v1",
        defaultApi: "openai-completions",
        modelCount: 1,
        authModes: [],
        sourceKind: "test",
        sourcePath: null
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page, { allowUnauthenticatedWorkspace: true });

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Providers" }).click();

  const connectionSummary = page.getByRole("status", { name: "Credential connections" });
  await expect(connectionSummary).toContainText("1 agent provider ready");
  await expect(connectionSummary.getByText("Ollama")).toBeVisible();
  await expect(connectionSummary.getByText("local")).toBeVisible();

  const ollamaCard = page.locator(".provider-card").filter({ hasText: "Ollama" });
  await expect(ollamaCard.locator(".status")).toHaveText("Ready");
  await expect(ollamaCard.getByText("No credentials required")).toBeVisible();
  await expect(ollamaCard.getByRole("button", { name: "Connect" })).toHaveCount(0);
});

test("provider model picker recovers when refreshed auth changes providers", async ({ page }) => {
  const daemon = new FakeDaemon({
    auth: [
      {
        providerId: "anthropic",
        kind: "api_key",
        email: null,
        expiresAtMs: null,
        scopes: [],
        planType: null,
        organizationName: null
      }
    ]
  });
  daemon.setSettingsConfig({
    defaultProvider: "codex",
    defaultModel: "test-model"
  });
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Providers" }).click();

  const pane = page.locator(".pf-settings-pane");
  await expect(pane.getByLabel("Provider")).toHaveValue("anthropic");
  await expect(pane.getByLabel("Model")).toHaveValue("test-model");

  daemon.setAuthStatuses([
    {
      providerId: "codex",
      kind: "oauth",
      email: "tester@example.com",
      expiresAtMs: null,
      scopes: [],
      planType: "test",
      organizationName: null
    }
  ]);
  await pane.getByRole("button", { name: "Refresh" }).click();

  await expect(pane.getByLabel("Provider")).toHaveValue("codex");
  await expect(pane.getByLabel("Model")).toHaveValue("test-model");
  await expect(pane.getByRole("button", { name: "Save default" })).toBeEnabled();
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

  await expect(page.getByRole("button", { name: "Create Project" })).toBeVisible();
  await page.keyboard.press("Control+,");

  await expect(page.getByRole("heading", { name: "General" })).toBeVisible();
  await page.getByRole("button", { name: "Shortcuts" }).click();
  await expect(page.getByText("Cmd/Ctrl + ,")).toBeVisible();
  await expect(page.getByText("Open settings")).toBeVisible();
});

test("settings shortcut is ignored while the new-agent modal is open", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: /^New agent in / }).first().click();
  const dialog = page.getByRole("dialog", { name: "New agent" });
  await expect(dialog).toBeVisible();

  await page.keyboard.press("Control+,");

  await expect(dialog).toBeVisible();
  await expect(page.getByRole("heading", { name: "General" })).toHaveCount(0);
});

test("provider API key connect requires a non-empty key", async ({ page }) => {
  const daemon = new FakeDaemon({ auth: [] });
  await daemon.install(page);
  await daemon.open(page, { allowUnauthenticatedWorkspace: true });

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Providers" }).click();

  const modal = await openProviderSetup(page, "Anthropic");
  const input = modal.getByLabel("API key for Anthropic");
  const connect = modal.getByRole("button", { name: "Add connect" });

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

test("provider API key input clears after a successful update", async ({ page }) => {
  const daemon = new FakeDaemon({ auth: [] });
  daemon.delayResponse(
    "login_with_api_key",
    (request) => request.params.providerId === "anthropic",
    120
  );
  await daemon.install(page);
  await daemon.open(page, { allowUnauthenticatedWorkspace: true });

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Providers" }).click();

  const modal = await openProviderSetup(page, "Anthropic");
  const input = modal.getByLabel("API key for Anthropic");
  await input.fill("sk-should-not-remain-visible");
  await modal.getByRole("button", { name: "Add connect" }).click();

  await daemon.waitForRequest("login_with_api_key");
  await expect(input).toBeDisabled();
  await expect(modal).toHaveCount(0);
});

test("custom provider duplicate connect shows a toast and skips login", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Providers" }).click();

  const modal = await openProviderSetup(page, "Custom provider");
  const baseUrl = modal.getByLabel("Base URL");
  const input = modal.getByLabel("API key");
  const connect = modal.getByRole("button", { name: "Add connect" });

  await baseUrl.fill("https://proxy.example/v1");
  await input.fill("sk-duplicate");
  await connect.click();
  await daemon.waitForRequest("login_with_api_key");
  await expect(modal).toHaveCount(0);

  const duplicateModal = await openProviderSetup(page, "Custom provider");
  await duplicateModal.getByLabel("Base URL").fill("https://proxy.example/v1");
  await duplicateModal.getByLabel("API key").fill("sk-duplicate");
  await duplicateModal.getByRole("button", { name: "Add connect" }).click();

  await expect(page.getByText("Provider connection already exists.")).toBeVisible();
  expect(
    daemon.requests.filter(
      (request) =>
        request.method === "login_with_api_key" &&
        request.params.providerId === "openai"
    )
  ).toHaveLength(1);
});

test("provider API key enter submit is ignored while login is already busy", async ({ page }) => {
  const daemon = new FakeDaemon({ auth: [] });
  daemon.delayResponse(
    "login_with_api_key",
    (request) => request.params.providerId === "anthropic",
    500
  );
  await daemon.install(page);
  await daemon.open(page, { allowUnauthenticatedWorkspace: true });

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Providers" }).click();

  const modal = await openProviderSetup(page, "Anthropic");
  const input = modal.getByLabel("API key for Anthropic");
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
  const daemon = new FakeDaemon({ auth: [] });
  daemon.delayResponse(
    "login_with_oauth",
    (request) => request.params.providerId === "openai",
    500
  );
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Providers" }).click();

  const modal = await openProviderSetup(page, "Codex");
  const connect = modal.getByRole("button", { name: "Connect with OAuth" });
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
        request.params.providerId === "openai"
    )
  ).toHaveLength(1);
});

test("provider auth controls are disabled while another provider is busy", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.delayResponse(
    "login_with_oauth",
    (request) => request.params.providerId === "openai",
    500
  );
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Providers" }).click();

  const codexModal = await openProviderSetup(page, "Codex");
  const codexOauth = codexModal.getByRole("button", { name: "Connect with OAuth" });
  const anthropicConnect = page
    .locator(".provider-card")
    .filter({ hasText: "Anthropic" })
    .getByRole("button", { name: "Add connect" });

  await expect(codexOauth).toBeEnabled();
  await expect(anthropicConnect).toBeEnabled();

  await codexOauth.click();
  await daemon.waitForRequest("login_with_oauth");

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
        providerId: "openai",
        source: "codex",
        kind: "oauth",
        description: "Codex CLI OAuth",
        sourcePath: "/tmp/home/.codex/auth.json"
      }
    ]
  });
  daemon.delayResponse(
    "import_external_credential",
    (request) => request.params.providerId === "openai" && request.params.source === "codex",
    500
  );
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Providers" }).click();

  const modal = await openProviderSetup(page, "Codex");
  const importButton = modal.getByRole("button", { name: "Use credentials from ~/.codex" });
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
        request.params.providerId === "openai" &&
        request.params.source === "codex"
    )
  ).toHaveLength(1);
});

test("external credential import disables provider login controls", async ({ page }) => {
  const daemon = new FakeDaemon({
    externalCredentials: [
      {
        providerId: "openai",
        source: "codex",
        kind: "oauth",
        description: "Codex CLI OAuth",
        sourcePath: "/tmp/home/.codex/auth.json"
      }
    ]
  });
  daemon.delayResponse(
    "import_external_credential",
    (request) => request.params.providerId === "openai" && request.params.source === "codex",
    500
  );
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Providers" }).click();

  const modal = await openProviderSetup(page, "Codex");
  const importButton = modal.getByRole("button", { name: "Use credentials from ~/.codex" });
  const anthropicConnect = page
    .locator(".provider-card")
    .filter({ hasText: "Anthropic" })
    .getByRole("button", { name: "Add connect" });

  await expect(importButton).toBeEnabled();
  await expect(anthropicConnect).toBeEnabled();

  await importButton.click();
  await daemon.waitForRequest("import_external_credential");

  await expect(anthropicConnect).toBeDisabled();
});

test("provider login disables external credential import controls", async ({ page }) => {
  const daemon = new FakeDaemon({
    externalCredentials: [
      {
        providerId: "openai",
        source: "codex",
        kind: "oauth",
        description: "Codex CLI OAuth",
        sourcePath: "/tmp/home/.codex/auth.json"
      }
    ]
  });
  daemon.delayResponse(
    "login_with_oauth",
    (request) => request.params.providerId === "openai",
    500
  );
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Providers" }).click();

  const modal = await openProviderSetup(page, "Codex");
  const importButton = modal.getByRole("button", { name: "Use credentials from ~/.codex" });
  const oauthButton = modal.getByRole("button", { name: "Connect with OAuth" });

  await expect(importButton).toBeEnabled();
  await expect(oauthButton).toBeEnabled();

  await oauthButton.click();
  await daemon.waitForRequest("login_with_oauth");

  await expect(importButton).toBeDisabled();
});

test("provider refresh controls are disabled while credentials are busy", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.delayResponse(
    "login_with_oauth",
    (request) => request.params.providerId === "openai",
    500
  );
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Settings" }).click();
  const accountRefresh = page
    .locator(".pf-settings-row")
    .filter({ hasText: "Account" })
    .getByRole("button", { name: "Refresh" });
  await page.getByRole("button", { name: "Providers" }).click();

  const pane = page.locator(".pf-settings-pane");
  const providerRefresh = pane.getByRole("button", { name: "Refresh" });
  const modal = await openProviderSetup(page, "Codex");
  const oauthButton = modal.getByRole("button", { name: "Connect with OAuth" });
  const settingsLoadsBefore = daemon.requests.filter(
    (request) => request.method === "load_settings_snapshot"
  ).length;

  await expect(providerRefresh).toBeEnabled();
  await oauthButton.click();
  await daemon.waitForRequest("login_with_oauth");

  await expect(providerRefresh).toBeDisabled();
  await providerRefresh.evaluate((button) => (button as HTMLButtonElement).click());
  await page.waitForTimeout(80);
  expect(
    daemon.requests.filter((request) => request.method === "load_settings_snapshot")
  ).toHaveLength(settingsLoadsBefore);

  await modal.getByRole("button", { name: "Close" }).click();
  await expect(modal).toHaveCount(0);
  await page.getByRole("button", { name: "General" }).click();
  await expect(accountRefresh).toBeDisabled();
  await accountRefresh.evaluate((button) => (button as HTMLButtonElement).click());
  await page.waitForTimeout(80);

  expect(
    daemon.requests.filter((request) => request.method === "load_settings_snapshot")
  ).toHaveLength(settingsLoadsBefore);
});

test("settings auth uses the configured daemon when Tauri globals exist", async ({ page }) => {
  await page.addInitScript(() => {
    (window as unknown as { __TAURI__?: unknown; __TAURI_INTERNALS__?: unknown }).__TAURI__ = {};
    (window as unknown as { __TAURI__?: unknown; __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
  });
  const daemon = new FakeDaemon({ auth: [] });
  await daemon.install(page);
  await daemon.open(page, { allowUnauthenticatedWorkspace: true });

  await daemon.waitForRequest("load_settings_snapshot");
  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Providers" }).click();

  const modal = await openProviderSetup(page, "Anthropic");
  await modal.getByLabel("API key for Anthropic").fill("sk-tauri-daemon");
  await modal.getByRole("button", { name: "Add connect" }).click();

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

test("permissions load errors do not auto-retry until the user retries", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.delayFailure("list_permissions", () => true, "permissions file is temporarily locked", 20);
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Permissions" }).click();

  await expect(page.getByText("permissions file is temporarily locked")).toBeVisible();
  await expect(page.getByRole("button", { name: "Retry" })).toBeEnabled();
  await page.waitForTimeout(160);
  expect(daemon.requests.filter((request) => request.method === "list_permissions")).toHaveLength(1);

  await page.getByRole("button", { name: "Retry" }).click();
  await expect.poll(() =>
    daemon.requests.filter((request) => request.method === "list_permissions").length
  ).toBe(2);
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

  await page.getByRole("button", { name: "Project" }).click();
  await page
    .locator(".pf-sidebar-agents-list")
    .getByRole("button", { name: /^Browser regression\b/ })
    .click();
  await expect(page.getByRole("button", { name: "Back" })).toBeVisible();

  await page.reload();
  await expect(page.getByRole("button", { name: "Back" })).toBeVisible();
  await expect(page.locator(".pf-agent-detail .primary-title")).toContainText("Browser regression");
});

test("remember last session restores detail even when grouped history omits it", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.setGroupedSessionFilter((metadata) => metadata.sessionId !== "session-browser");
  await daemon.install(page);
  await page.addInitScript(() => {
    window.localStorage.setItem(
      "puffer-desktop:preferences",
      JSON.stringify({ rememberSession: true })
    );
    window.localStorage.setItem(
      "puffer-desktop:remembered-session",
      JSON.stringify({ workspaceRoot: "/tmp/puffer", sessionId: "session-browser" })
    );
  });
  await daemon.open(page);

  await daemon.waitForRequest(
    "load_session_detail",
    (request) => request.params.sessionId === "session-browser"
  );
  await expect(page.getByRole("button", { name: "Back" })).toBeVisible();
  await expect(page.locator(".pf-agent-detail .primary-title")).toContainText("Browser regression");
  await expect(
    page.locator(".pf-sidebar-agents-list").getByRole("button", { name: /^Browser regression\b/ })
  ).toBeVisible();
});

test("MCP settings add server through the daemon", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "MCP Servers" }).click();
  await expect(page.locator(".pf-mcp-card .title").filter({ hasText: "Playwright" })).toBeVisible();
  await expect(page.getByLabel("ID")).toHaveCount(1);

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

test("Secrets settings save and import without rendering raw secret values", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Secrets" }).click();

  const pane = page.locator(".pf-settings-pane");
  await expect(pane.locator(".label").filter({ hasText: "Secret store" })).toBeVisible();

  await pane.getByLabel("Name").fill("Build token");
  await pane.getByLabel("Description").fill("CI deployment token");
  await pane.getByLabel("Username").fill("ci");
  await pane.getByLabel("Origin").fill("https://build.example");
  await pane.getByLabel("Value").fill("super-secret-token");
  await pane.getByRole("button", { name: "Save secret" }).click();

  const saveRequest = await daemon.waitForRequest("save_secret");
  expect(saveRequest.params).toMatchObject({
    label: "Build token",
    description: "CI deployment token",
    username: "ci",
    origin: "https://build.example",
    value: "super-secret-token"
  });
  await expect(page.getByText("super-secret-token")).toHaveCount(0);
  await expect(pane.locator(".pf-mcp-card").filter({ hasText: "Build token" })).toBeVisible();

  await pane.getByRole("button", { name: "Sync from Chrome" }).click();
  await daemon.waitForRequest("import_chrome_secrets");
  await expect(
    pane.locator(".pf-mcp-card").filter({ hasText: "Chrome developer@example.com" })
  ).toBeVisible();
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

test("MCP settings refresh reloads the visible server list", async ({ page }) => {
  const daemon = new FakeDaemon({ mcpServers: [] });
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "MCP Servers" }).click();
  await daemon.waitForRequest("list_mcp_servers");
  await expect(page.getByText("No MCP servers configured.")).toBeVisible();

  daemon.setMcpServers([
    {
      id: "github",
      displayName: "GitHub",
      description: "Issue and PR tools",
      transport: "stdio",
      endpoint: "",
      target: "npx @modelcontextprotocol/server-github",
      sourceKind: "local",
      sourcePath: "/tmp/puffer/.puffer/mcp_servers/github.json"
    }
  ]);

  const beforeRefresh = daemon.requests.filter((request) => request.method === "list_mcp_servers").length;
  await page.getByRole("button", { name: "Refresh MCP servers" }).click();
  await expect.poll(() =>
    daemon.requests.filter((request) => request.method === "list_mcp_servers").length
  ).toBe(beforeRefresh + 1);
  await expect(page.locator(".pf-mcp-card .title").filter({ hasText: "GitHub" })).toBeVisible();
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

test("connector settings renders dynamic AskUserQuestion inputs", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.setConnectorSetupCompletionDelay(500);
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Connectors" }).click();
  await daemon.waitForRequest("workflow_list");

  const pane = page.locator(".pf-settings-pane");
  await expect(pane.getByRole("tab", { name: /Connections/ })).toHaveAttribute("aria-selected", "true");
  await pane.getByRole("tab", { name: /Catalog/ }).click();
  await expect(pane.getByLabel("Connector catalog")).toBeVisible();
  await pane.getByRole("tab", { name: /Connections/ }).click();
  await expect(pane.getByLabel("Connector connections")).toBeVisible();
  await expect(pane.getByRole("button", { name: "New connection" })).toBeVisible();
  await pane.getByRole("button", { name: "New connection" }).click();
  const createDialog = page.getByRole("dialog", { name: "Create connector connection" });
  await expect(createDialog).toBeVisible();
  await expect(createDialog.locator(".pf-connector-form select")).toHaveValue("telegram-login");
  await createDialog.getByLabel("Connector connection slug").fill("telegram-test");
  await createDialog.getByRole("button", { name: "Start setup" }).click();

  const turn = await daemon.waitForRequest("start_connector_setup");
  expect(turn.params).toMatchObject({
    message: "/connect telegram-login telegram-test"
  });

  const dialog = page.getByRole("dialog", { name: "Connector setup questions" });
  await expect(dialog).toBeVisible();
  await expect(dialog).toHaveAttribute("aria-modal", "true");
  await expect(dialog.getByText("Setup questions")).toBeVisible();
  await dialog.locator(".pf-connector-question").filter({ hasText: "Connector credential" }).locator("input").fill("secret-test");
  await expect(dialog.getByLabel("Default")).toBeChecked();
  await dialog.getByRole("button", { name: "Submit answers" }).click();

  const resolved = await daemon.waitForRequest("resolve_user_question");
  expect(resolved.params).toMatchObject({
    requestId: "connector-setup",
    answers: {
      "Connector credential": "secret-test",
      "Setup mode": "Default"
    }
  });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole("status")).toContainText("Checking connector auth...");
  await expect(pane.getByText("Connector setup finished for telegram-test.")).toBeVisible();
  await expect(dialog).toBeHidden();
  await expect(pane.locator(".pf-mcp-card").filter({ hasText: "telegram-test" })).toBeVisible();
});

test("connector settings submits dynamic password, radio, and multiselect answers", async ({ page }) => {
  const daemon = new FakeDaemon();
  const accountQuestion =
    "Workspace region\n\n![QR preview](data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHdpZHRoPSIxNiIgaGVpZ2h0PSIxNiI+PHJlY3Qgd2lkdGg9IjE2IiBoZWlnaHQ9IjE2IiBmaWxsPSJ3aGl0ZSIvPjxyZWN0IHg9IjIiIHk9IjIiIHdpZHRoPSIxMiIgaGVpZ2h0PSIxMiIgZmlsbD0iYmxhY2siLz48L3N2Zz4=)";
  daemon.setConnectorSetupQuestions([
    {
      type: "input",
      header: "API Token",
      question: "Workspace token",
      options: []
    },
    {
      type: "choice",
      header: "Region",
      question: accountQuestion,
      options: [
        { label: "US", description: "United States", preview: "us-east" },
        { label: "EU", description: "European Union", preview: "eu-west" }
      ]
    },
    {
      type: "choice",
      header: "Scopes",
      question: "Enabled scopes",
      multiSelect: true,
      options: [
        { label: "messages:read", description: "Read incoming messages", preview: "read" },
        { label: "messages:write", description: "Send responses", preview: "write" }
      ]
    }
  ]);
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Connectors" }).click();
  await daemon.waitForRequest("workflow_list");

  const pane = page.locator(".pf-settings-pane");
  await pane.getByRole("button", { name: "New connection" }).click();
  const createDialog = page.getByRole("dialog", { name: "Create connector connection" });
  await createDialog.locator(".pf-connector-form select").selectOption("slack-app");
  await createDialog.getByLabel("Connector connection slug").fill("team-slack");
  await expect(createDialog.getByLabel("Connector setup command")).toContainText("/connect slack-app team-slack");
  await createDialog.getByRole("button", { name: "Start setup" }).click();

  const turn = await daemon.waitForRequest("start_connector_setup");
  expect(turn.params).toMatchObject({
    message: "/connect slack-app team-slack"
  });

  const questions = page.getByRole("dialog", { name: "Connector setup questions" });
  await expect(questions).toHaveAttribute("aria-modal", "true");
  await expect(questions).toContainText("3 questions");
  const tokenQuestion = questions.locator(".pf-connector-question").filter({ hasText: "Workspace token" });
  const regionQuestion = questions.locator(".pf-connector-question").filter({ hasText: "Workspace region" });
  const scopeQuestion = questions.locator(".pf-connector-question").filter({ hasText: "Enabled scopes" });
  const tokenInput = tokenQuestion.locator("input");
  await expect(tokenInput).toHaveAttribute("type", "password");
  await expect(regionQuestion.getByRole("img", { name: "QR preview" })).toBeVisible();
  await expect(regionQuestion.getByText("![QR preview]")).toHaveCount(0);
  await expect(questions.getByLabel("US")).toBeChecked();
  await expect(questions.getByText("us-east")).toBeVisible();
  await expect(questions.getByText("eu-west")).toBeVisible();
  await expect(questions.getByRole("button", { name: "Submit answers" })).toBeDisabled();

  await tokenInput.fill("xoxb-secret");
  await expect(questions.getByRole("button", { name: "Submit answers" })).toBeDisabled();

  await questions.getByLabel("EU").check();
  await scopeQuestion.getByLabel("messages:read").check();
  await scopeQuestion.getByLabel("messages:write").check();
  await questions.getByRole("button", { name: "Submit answers" }).click();

  const resolved = await daemon.waitForRequest("resolve_user_question");
  expect(resolved.params).toMatchObject({
    requestId: "connector-setup",
    answers: {
      "Workspace token": "xoxb-secret",
      [accountQuestion]: "EU",
      "Enabled scopes": ["messages:read", "messages:write"]
    }
  });
  await expect(pane.getByText("Connector setup finished for team-slack.")).toBeVisible();
  await expect(pane.locator(".pf-mcp-card").filter({ hasText: "team-slack" })).toBeVisible();
});

test("connector settings remain readable on narrow screens", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Connectors" }).click();
  await daemon.waitForRequest("workflow_list");

  await page.setViewportSize({ width: 390, height: 844 });

  const navBox = await page.locator(".pf-settings-nav").boundingBox();
  const paneBox = await page.locator(".pf-settings-pane").boundingBox();
  const toolbarBox = await page.locator(".pf-connector-toolbar").boundingBox();
  const tabsBox = await page.locator(".pf-connector-tabs").boundingBox();

  expect(navBox?.height).toBeLessThan(90);
  expect(paneBox?.width).toBeGreaterThan(340);
  expect(toolbarBox?.width).toBeGreaterThan(330);
  expect(tabsBox?.width).toBeGreaterThan(330);
  await expect(page.getByRole("heading", { name: "Connectors" })).toBeVisible();
  await expect(page.getByRole("button", { name: "New connection" })).toBeVisible();

  await page.getByRole("button", { name: "New connection" }).click();
  const createDialog = page.getByRole("dialog", { name: "Create connector connection" });
  await expect(createDialog).toBeVisible();
  const dialogBox = await createDialog.boundingBox();
  expect(dialogBox?.width).toBeGreaterThan(340);
  await expect(createDialog.getByLabel("Connector connection slug")).toBeVisible();
});

test("providers pane can install and start the local MiniCPM5 behavior model", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Providers" }).click();

  const pane = page.locator(".pf-settings-pane");
  const card = pane.locator(".pf-local-model-card");
  await expect(card.getByRole("heading", { name: "MiniCPM5 local model" })).toBeVisible();
  await expect(card).toContainText("MiniCPM5-1B runs on-device");

  await card.getByRole("button", { name: "Check status" }).click();
  await expect(card).toContainText("Status checked at");
  await expect(card).toContainText("Server health");
  await expect(card).toContainText("http://127.0.0.1:8088/v1/models");

  await card.getByRole("button", { name: /Install MiniCPM5/ }).click();
  const install = await daemon.waitForRequest("install_local_model");
  expect(install.params).toMatchObject({ modelId: "minicpm5" });

  await expect(card).toContainText("MiniCPM5 is installed, registered, and running");
  await expect(card.getByRole("button", { name: /MiniCPM5 ready/ })).toBeDisabled();
});

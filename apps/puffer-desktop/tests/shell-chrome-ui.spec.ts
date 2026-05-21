import { readFile } from "node:fs/promises";
import { expect, test } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";

test("Tauri mac shell exposes a drag-only titlebar without duplicate branding", async ({ page }) => {
  const daemon = new FakeDaemon();
  await page.addInitScript(() => {
    Object.defineProperty(window.navigator, "userAgent", {
      get: () => "Mozilla/5.0 (Macintosh; Intel Mac OS X 15_5) AppleWebKit/537.36"
    });
    (globalThis as unknown as { isTauri?: boolean }).isTauri = true;
  });
  await daemon.install(page);
  await daemon.open(page);

  await expect(page.locator("html")).toHaveClass(/is-tauri-mac/);

  const titlebar = page.locator(".pf-titlebar");
  await expect(titlebar).toBeVisible();
  const titlebarBox = await titlebar.boundingBox();
  expect(titlebarBox?.height).toBeGreaterThanOrEqual(44);
  await expect(titlebar).toHaveAttribute("data-tauri-drag-region", "");
  await expect(titlebar.locator(".pf-brand-logo")).toHaveCount(0);
  await expect(titlebar.locator(".pf-titlebar-drag-fill")).toHaveAttribute(
    "data-tauri-drag-region",
    ""
  );

  const sidebarLogo = page.locator(".pf-sidebar .pf-brand-logo").first();
  await expect(page.locator(".pf-sidebar .pf-brand-logo")).toHaveCount(1);
  const logoBox = await sidebarLogo.boundingBox();
  expect(logoBox).not.toBeNull();
  const trafficLightSafeRect = { left: 0, top: 0, right: 88, bottom: 44 };
  const overlapsTrafficLights =
    logoBox!.left < trafficLightSafeRect.right &&
    logoBox!.left + logoBox!.width > trafficLightSafeRect.left &&
    logoBox!.top < trafficLightSafeRect.bottom &&
    logoBox!.top + logoBox!.height > trafficLightSafeRect.top;
  expect(overlapsTrafficLights).toBe(false);

  expect(await page.locator(".pf-sidebar-collapse").evaluate((node) =>
    node.hasAttribute("data-tauri-drag-region")
  )).toBe(false);
});

test("Tauri capability permits data drag regions to move the native window", async () => {
  const raw = await readFile("src-tauri/capabilities/default.json", "utf8");
  const capability = JSON.parse(raw) as { permissions?: string[] };
  expect(capability.permissions).toContain("core:window:allow-start-dragging");
});

test("Playwright does not reuse stale Vite servers in Codex automation", async () => {
  const raw = await readFile("playwright.config.ts", "utf8");
  expect(raw).toContain("process.env.CODEX_CI");
  expect(raw).toContain("reuseExistingServer: shouldReuseExistingServer");
});

test("desktop minimum width keeps primary navigation visible", async ({ page }) => {
  const daemon = new FakeDaemon();
  await page.setViewportSize({ width: 720, height: 480 });
  await daemon.install(page);
  await daemon.open(page);

  const sidebar = page.locator(".pf-sidebar");
  await expect(sidebar).toBeVisible();
  await expect(sidebar.getByRole("button", { name: "Project" })).toBeVisible();
  await expect(sidebar.getByRole("button", { name: "Pipelines" })).toBeVisible();
  await expect(sidebar.getByRole("button", { name: "Deployments" })).toBeVisible();
  await expect(sidebar.getByRole("button", { name: "Settings" })).toBeVisible();
});

test("sidebar primary navigation exposes the current page", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  const sidebar = page.locator(".pf-sidebar");
  const project = sidebar.getByRole("button", { name: "Project" });
  const pipelines = sidebar.getByRole("button", { name: "Pipelines" });
  const settings = sidebar.getByRole("button", { name: "Settings" });

  await expect(project).toHaveAttribute("aria-current", "page");
  await expect(pipelines).not.toHaveAttribute("aria-current", "page");

  await pipelines.click();
  await expect(project).not.toHaveAttribute("aria-current", "page");
  await expect(pipelines).toHaveAttribute("aria-current", "page");

  await settings.click();
  await expect(pipelines).not.toHaveAttribute("aria-current", "page");
  await expect(settings).toHaveAttribute("aria-current", "page");
});

test("sidebar account chip opens account settings", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  const sidebar = page.locator(".pf-sidebar");
  const account = sidebar.getByRole("button", { name: "Open account for tester@example.com" });
  await expect(account).toBeVisible();
  await expect(account).toContainText("tester@example.com");

  await account.click();

  await expect(sidebar.getByRole("button", { name: "Settings", exact: true })).toHaveAttribute("aria-current", "page");
  await expect(page.getByRole("heading", { name: "General" })).toBeVisible();
  await expect(page.getByText("Signed-in providers and session controls.")).toBeVisible();
});

test("desktop user-visible copy uses Puffer branding", async () => {
  const userFacingFiles = [
    "src/App.svelte",
    "src/lib/screens/agent/BrowserPane.svelte",
    "src/lib/screens/agent/FilesPane.svelte",
    "src/lib/screens/agent/TerminalPane.svelte",
    "src/lib/screens/workspace/ConnectProjectModal.svelte"
  ];

  for (const file of userFacingFiles) {
    const source = await readFile(file, "utf8");
    expect(source, file).not.toContain("Corbina");
  }
});

test("sidebar width can be resized and persists as a local shell tweak", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  const sidebar = page.locator(".pf-sidebar");
  const resizer = page.locator(".pf-sidebar-resizer");
  await expect(resizer).toBeVisible();
  await expect(page.getByRole("button", { name: "Adjust navigation size" })).toHaveCount(1);

  const initialBox = await sidebar.boundingBox();
  const handleBox = await resizer.boundingBox();
  expect(initialBox).not.toBeNull();
  expect(handleBox).not.toBeNull();

  await page.mouse.move(handleBox!.x + handleBox!.width / 2, handleBox!.y + 120);
  await page.mouse.down();
  await page.mouse.move(handleBox!.x + handleBox!.width / 2 + 96, handleBox!.y + 120);
  await page.mouse.up();

  await expect
    .poll(async () => Math.round((await sidebar.boundingBox())?.width ?? 0))
    .toBeGreaterThan(Math.round(initialBox!.width + 72));
  const storedWidth = await page.evaluate(() => {
    const raw = window.localStorage.getItem("puffer-desktop:tweaks");
    return raw ? JSON.parse(raw).sidebarWidth : null;
  });
  expect(storedWidth).toBeGreaterThan(initialBox!.width + 72);
});

test("sidebar can open the deployments screen", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  const sidebar = page.locator(".pf-sidebar");
  await sidebar.getByRole("button", { name: "Deployments" }).click();

  await expect(page.locator(".pf-dep")).toBeVisible();
  await expect(page.getByText(/environments/)).toBeVisible();
  await expect(page.getByRole("button", { name: /New deployment/ })).toBeVisible();
});

test("deployment search filters environments and resets from Escape", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Deployments" }).click();
  await page.locator(".pf-dep-top-right").getByRole("button", { name: "Search" }).click();

  const search = page.getByRole("searchbox", { name: "Search deployments" });
  await expect(search).toBeFocused();
  await search.fill("cloudflare");

  const rows = page.locator(".pf-dep-row");
  await expect(rows).toHaveCount(1);
  await expect(rows.first()).toContainText("edge-cdn");
  await expect(page.locator(".pf-dep-detail-name")).toContainText("edge-cdn");

  await search.fill("no-match");
  await expect(rows).toHaveCount(0);
  await expect(page.getByText("No deployments match")).toBeVisible();
  const detail = page.locator(".pf-dep-detail");
  await expect(detail).toContainText("No deployment selected");
  await expect(detail).not.toContainText("edge-cdn");
  await expect(detail.getByRole("button", { name: "Open" })).toHaveCount(0);
  await expect(detail.getByRole("button", { name: "Redeploy" })).toHaveCount(0);

  await search.press("Escape");
  await expect(page.getByRole("searchbox", { name: "Search deployments" })).toHaveCount(0);
  await expect(page.locator(".pf-dep-row")).toHaveCount(6);
  await expect(page.locator(".pf-dep-row").filter({ hasText: "stripe-api · production" })).toBeVisible();
});

test("deployment provider sync button reports progress and completion", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Deployments" }).click();
  const syncButton = page.locator(".pf-dep-top-right").getByRole("button", { name: "Sync providers" });
  const syncStatus = page.locator(".pf-dep-sync-status");

  await expect(syncStatus).toHaveCount(0);
  await syncButton.click();

  await expect(syncButton).toBeDisabled();
  await expect(syncButton).toHaveAttribute("aria-busy", "true");
  await expect(syncStatus).toHaveAttribute("role", "status");
  await expect(syncStatus).toContainText("Syncing providers...");
  await expect(syncStatus).toContainText("Providers synced: 6 environments across 4 providers refreshed.");
  await expect(syncButton).toBeEnabled();
  await expect(syncButton).toHaveAttribute("aria-busy", "false");

  const statusBox = await syncStatus.boundingBox();
  const topbarBox = await page.locator(".pf-dep-top").boundingBox();
  expect(statusBox).not.toBeNull();
  expect(topbarBox).not.toBeNull();
  expect(statusBox!.height).toBeLessThanOrEqual(topbarBox!.height);
});

test("deployment new deployment button creates a local draft", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Deployments" }).click();
  const newDeployment = page.locator(".pf-dep-top-right").getByRole("button", { name: "New deployment" });

  await newDeployment.click();
  let dialog = page.getByRole("dialog", { name: "New deployment" });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByLabel("Service name")).toBeFocused();
  await expect(dialog.getByRole("button", { name: "Create deployment" })).toBeDisabled();

  await dialog.press("Escape");
  await expect(dialog).toHaveCount(0);
  await expect(page.locator(".pf-dep-row")).toHaveCount(6);

  await newDeployment.click();
  dialog = page.getByRole("dialog", { name: "New deployment" });
  await dialog.getByLabel("Service name").fill("checkout-worker");
  await dialog.getByLabel("Provider").selectOption("fly");
  await dialog.getByLabel("Environment").selectOption("preview");
  await dialog.getByLabel("Branch").fill("feature/checkouts");
  await expect(dialog.getByText("Draft will appear as checkout-worker · preview.")).toBeVisible();
  await dialog.getByRole("button", { name: "Create deployment" }).click();

  await expect(dialog).toHaveCount(0);
  await expect(newDeployment).toBeFocused();
  await expect(page.locator(".pf-dep-top-title")).toContainText("7 environments");
  const draft = page.locator(".pf-dep-row").filter({ hasText: "checkout-worker · preview" });
  await expect(draft).toBeVisible();
  await expect(draft).toContainText("Fly.io Machines");
  await expect(page.locator(".pf-dep-detail-name")).toContainText("checkout-worker · preview");
  await expect(page.locator(".pf-dep-detail-name")).toContainText(/deploying/i);
  await expect(page.locator(".pf-dep-detail-sub")).toContainText("checkout-worker-preview.puffer.app");
  await expect(page.getByRole("tab", { name: "Deploys" })).toHaveAttribute("aria-selected", "true");
});

test("deployment redeploy controls insert a live deploy history item", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Deployments" }).click();
  const detailHeader = page.locator(".pf-dep-detail-head");
  const redeploy = detailHeader.getByRole("button", { name: "Redeploy" });

  await redeploy.click();

  await expect(page.getByRole("tab", { name: "Deploys" })).toHaveAttribute("aria-selected", "true");
  await expect(redeploy).toBeDisabled();
  await expect(redeploy).toHaveAttribute("aria-busy", "true");
  await expect(detailHeader.getByRole("status")).toContainText("Redeploying stripe-api · production from main.");
  const firstRun = page.locator(".pf-dep-history-row").first();
  await expect(firstRun).toContainText("manual-1430");
  await expect(firstRun).toContainText("Otter");
  await expect(firstRun).toContainText(/deploying/i);

  await expect(detailHeader.getByRole("status")).toContainText("Redeploy complete for stripe-api · production.");
  await expect(redeploy).toBeEnabled();
  await expect(redeploy).toHaveAttribute("aria-busy", "false");
  await expect(firstRun).toContainText(/healthy/i);
  await expect(firstRun).toContainText("0m 12s");

  const trigger = page.getByRole("button", { name: "Trigger deploy" });
  await trigger.click();
  await expect(trigger).toBeDisabled();
  await expect(page.locator(".pf-dep-history-row").first()).toContainText("manual-1431");
});

test("deployment history logs button opens deploy output", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Deployments" }).click();
  await page.getByRole("tab", { name: "Deploys" }).click();

  const firstRun = page.locator(".pf-dep-history-row").first();
  await firstRun.getByRole("button", { name: "Logs for b-1428" }).click();

  const logs = page.getByLabel("Deploy logs for b-1428");
  await expect(logs).toBeVisible();
  await expect(logs).toContainText("Starting b-1428 for stripe-api · production");
  await expect(logs).toContainText("Injected 41 environment keys and 8 integrations");
  await expect(firstRun.getByRole("button", { name: "Logs for b-1428" })).toHaveAttribute(
    "aria-pressed",
    "true"
  );

  await logs.getByRole("button", { name: "Close deploy logs" }).click();
  await expect(page.getByLabel("Deploy logs for b-1428")).toHaveCount(0);
});

test("deployment open button opens public URLs and reports unavailable targets", async ({ page }) => {
  await page.addInitScript(() => {
    const target = window as typeof window & {
      __openedDeploymentUrls?: Array<{ url: string; target?: string; features?: string }>;
    };
    target.__openedDeploymentUrls = [];
    target.open = ((url?: string | URL, frameTarget?: string, features?: string) => {
      target.__openedDeploymentUrls?.push({
        url: String(url),
        target: frameTarget,
        features
      });
      return null;
    }) as typeof window.open;
  });
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Deployments" }).click();
  const detailHeader = page.locator(".pf-dep-detail-head");

  await detailHeader.getByRole("button", { name: "Open" }).click();

  await expect(detailHeader.getByRole("status")).toContainText(
    "Opened stripe-api · production at https://api.puffer.app."
  );
  await expect.poll(async () =>
    page.evaluate(() =>
      (window as typeof window & {
        __openedDeploymentUrls?: Array<{ url: string; target?: string; features?: string }>;
      }).__openedDeploymentUrls ?? []
    )
  ).toEqual([{ url: "https://api.puffer.app", target: "_blank", features: "noopener,noreferrer" }]);

  await page.locator(".pf-dep-row").filter({ hasText: "infra · shared" }).click();
  await detailHeader.getByRole("button", { name: "Open" }).click();

  await expect(detailHeader.getByRole("status")).toContainText(
    "infra · shared has no public URL to open."
  );
  await expect.poll(async () =>
    page.evaluate(() =>
      (window as typeof window & {
        __openedDeploymentUrls?: Array<{ url: string; target?: string; features?: string }>;
      }).__openedDeploymentUrls ?? []
    )
  ).toEqual([{ url: "https://api.puffer.app", target: "_blank", features: "noopener,noreferrer" }]);
});

test("deployment detail tabs expose selected state", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  const sidebar = page.locator(".pf-sidebar");
  await sidebar.getByRole("button", { name: "Deployments" }).click();

  const tabs = page.locator(".pf-dep-tabs");
  await expect(tabs).toHaveAttribute("role", "tablist");
  const askTab = tabs.getByRole("tab", { name: "Ask Puffer" });
  const secretsTab = tabs.getByRole("tab", { name: "Secrets" });
  await expect(askTab).toHaveAttribute("aria-selected", "true");
  await expect(secretsTab).toHaveAttribute("aria-selected", "false");

  await secretsTab.click();
  await expect(askTab).toHaveAttribute("aria-selected", "false");
  await expect(secretsTab).toHaveAttribute("aria-selected", "true");
  await expect(page.getByRole("heading", { name: "Secrets & env" })).toBeVisible();

  await secretsTab.press("ArrowRight");
  const providersTab = tabs.getByRole("tab", { name: "Providers" });
  await expect(providersTab).toHaveAttribute("aria-selected", "true");
  await expect(page.getByRole("heading", { name: "Providers & integrations" })).toBeVisible();

  await providersTab.press("Home");
  await expect(askTab).toHaveAttribute("aria-selected", "true");
});

test("deployment Ask Puffer composer sends prompts from button and Enter", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Deployments" }).click();
  const composer = page.locator(".pf-dep-ask-composer");
  const thread = page.locator(".pf-dep-ask-thread");
  const textbox = composer.getByRole("textbox", { name: "Ask Puffer" });

  await textbox.fill("Check failed deploys");
  await composer.getByRole("button", { name: "Send" }).click();

  await expect(textbox).toHaveValue("");
  await expect(thread.locator('.pf-msg[data-role="user"] .pf-msg-text').filter({ hasText: "Check failed deploys" })).toHaveCount(1);
  await expect(thread).toContainText("I queued an investigation for stripe-api · production: Check failed deploys.");

  await textbox.fill("Summarize logs");
  await textbox.press("Enter");

  await expect(textbox).toHaveValue("");
  await expect(thread.locator('.pf-msg[data-role="user"] .pf-msg-text').filter({ hasText: "Summarize logs" })).toHaveCount(1);
  await expect(thread).toContainText("I queued an investigation for stripe-api · production: Summarize logs.");

  await textbox.fill("zhong");
  await textbox.evaluate((node) => {
    node.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true }));
    node.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "Enter",
        bubbles: true,
        cancelable: true,
        isComposing: true
      })
    );
    node.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "Enter",
        bubbles: true,
        cancelable: true,
        keyCode: 229
      })
    );
  });
  await expect(textbox).toHaveValue("zhong");
  await expect(thread.locator('.pf-msg[data-role="user"] .pf-msg-text').filter({ hasText: "zhong" })).toHaveCount(0);

  await page.locator(".pf-dep-row").filter({ hasText: "puffer-web · production" }).click();
  await expect(thread).not.toContainText("Check failed deploys");
  await expect(thread).not.toContainText("Summarize logs");
  await expect(textbox).toHaveValue("");
});

test("deployment Ask Puffer quick actions give visible feedback", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Deployments" }).click();
  const composer = page.locator(".pf-dep-ask-composer");
  const thread = page.locator(".pf-dep-ask-thread");

  await page.getByRole("button", { name: "Open fix PR" }).click();
  await expect(thread).toContainText("I drafted the fix path for stripe-api · production");
  await expect(thread.locator('.pf-msg[data-role="user"] .pf-msg-text').filter({ hasText: "Open fix PR" })).toHaveCount(1);

  await page.getByRole("button", { name: "Roll back to 6f8c120" }).click();
  await expect(thread).toContainText("I staged the rollback plan for stripe-api · production");
  await expect(thread.locator('.pf-msg[data-role="user"] .pf-msg-text').filter({ hasText: "Roll back to 6f8c120" })).toHaveCount(1);

  await composer.getByRole("button", { name: "logs" }).click();
  await composer.getByRole("button", { name: "metrics" }).click();
  await expect(composer.getByRole("button", { name: "logs" })).toHaveAttribute("aria-pressed", "true");
  await expect(composer.getByRole("button", { name: "metrics" })).toHaveAttribute("aria-pressed", "true");

  await composer.getByRole("textbox", { name: "Ask Puffer" }).fill("Use selected context");
  await composer.getByRole("button", { name: "Send" }).click();
  await expect(thread).toContainText("I'll use logs and metrics for this environment.");
  await expect(composer.getByRole("button", { name: "logs" })).toHaveAttribute("aria-pressed", "false");
});

test("deployment Ask Puffer saves diagnostic output into Memory", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Deployments" }).click();
  const save = page.getByRole("button", { name: "Save to memory" });

  await save.click();

  await expect(page.getByRole("status")).toContainText(
    'Saved "Node 20 keep-alive regression" to Memory for stripe-api · production.'
  );
  await expect(page.getByRole("button", { name: "Saved to memory" })).toBeDisabled();

  await page.getByRole("tab", { name: "Memory" }).click();
  await expect(page.locator(".pf-dep-pane-head .sub")).toContainText("7 notes");
  const note = page.locator(".pf-dep-mem").filter({ hasText: "Node 20 keep-alive regression" });
  await expect(note).toBeVisible();
  await expect(note).toContainText("Pitfall");
  await expect(note).toContainText("POST /subscription/update p95 rose from 180ms to 480ms after f02ae81.");
  await expect(note).toContainText("ask:");
  await expect(note).toContainText("f02ae81 diagnostic");
  await expect(note).toContainText("#node-20");
  await expect(note).toContainText("#keepalive");
});

test("deployment secrets sync button reports progress and completion", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Deployments" }).click();
  await page.getByRole("tab", { name: "Secrets" }).click();
  const syncButton = page.getByRole("button", { name: "Sync secrets" });
  const status = page.locator(".pf-dep-pane-status");

  await expect(status).toHaveCount(0);
  await syncButton.click();

  await expect(syncButton).toBeDisabled();
  await expect(syncButton).toHaveAttribute("aria-busy", "true");
  await expect(status).toHaveAttribute("role", "status");
  await expect(status).toContainText("Syncing stripe-api · production secrets with Vault...");
  await expect(status).toContainText("Secrets synced: 8 keys refreshed for stripe-api · production.");
  await expect(syncButton).toBeEnabled();
  await expect(syncButton).toHaveAttribute("aria-busy", "false");
});

test("deployment add secret creates a local masked secret row", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Deployments" }).click();
  await page.getByRole("tab", { name: "Secrets" }).click();

  await page.locator(".pf-dep-pane-head").getByRole("button", { name: "Add secret" }).click();
  const form = page.locator(".pf-dep-secret-form");
  await expect(form).toBeVisible();
  await expect(form.getByLabel("Secret key")).toBeFocused();
  await expect(form.getByRole("button", { name: "Add secret" })).toBeDisabled();

  await form.getByLabel("Secret key").fill("webhook-token");
  await form.getByLabel("Secret preview value").fill("tok_live_123");
  await form.getByLabel("Secret scope").selectOption("build");
  await form.getByRole("button", { name: "Add secret" }).click();

  await expect(form).toHaveCount(0);
  await expect(page.locator(".pf-dep-pane-head .sub")).toContainText("9 keys");
  await expect(page.locator(".pf-dep-pane-status")).toContainText(
    "Added WEBHOOK_TOKEN to stripe-api · production."
  );
  const row = page.locator(".pf-dep-secrets-row").filter({ hasText: "WEBHOOK_TOKEN" });
  await expect(row).toBeVisible();
  await expect(row).toContainText("••••••••••••••");
  await expect(row).toContainText("build");

  await row.getByRole("button", { name: "Reveal WEBHOOK_TOKEN" }).click();
  await expect(row).toContainText("tok_live_123");
});

test("deployment add memory note creates a local filtered note", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Deployments" }).click();
  await page.getByRole("tab", { name: "Memory" }).click();

  await page.locator(".pf-dep-pane-head").getByRole("button", { name: "Add note" }).click();
  const form = page.locator(".pf-dep-mem-form");
  await expect(form).toBeVisible();
  await expect(form.getByLabel("Memory note title")).toBeFocused();
  await expect(form.getByRole("button", { name: "Add note" })).toBeDisabled();

  await form.getByLabel("Memory note title").fill("Queue drain shortcut");
  await form.getByLabel("Memory note kind").selectOption("runbook");
  await form.getByLabel("Memory note confidence").selectOption("high");
  await form.getByLabel("Memory note body").fill("Use scripts/drain-webhooks.ts after invoices lag clears.");
  await form.getByLabel("Memory note tags").fill("stripe queue");
  await form.getByRole("button", { name: "Add note" }).click();

  await expect(form).toHaveCount(0);
  await expect(page.locator(".pf-dep-pane-head .sub")).toContainText("7 notes");
  await expect(page.locator(".pf-dep-pane-status")).toContainText(
    'Added memory note "Queue drain shortcut" to stripe-api · production.'
  );
  const note = page.locator(".pf-dep-mem").filter({ hasText: "Queue drain shortcut" });
  await expect(note).toBeVisible();
  await expect(note).toContainText("Runbook");
  await expect(note).toContainText("Use scripts/drain-webhooks.ts after invoices lag clears.");
  await expect(note).toContainText("#stripe");
  await expect(note).toContainText("#queue");
  await expect(note).toContainText("manual:");
  await expect(note).toContainText("local draft");

  await page.getByRole("button", { name: /Runbook\s+2/ }).click();
  await expect(page.locator(".pf-dep-mem")).toHaveCount(2);
  await expect(note).toBeVisible();
});

test("deployment add provider creates a local integration card", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Deployments" }).click();
  await page.getByRole("tab", { name: "Providers" }).click();

  await expect(page.locator(".pf-dep-prov")).toHaveCount(8);
  await page.locator(".pf-dep-pane-head").getByRole("button", { name: "Add provider" }).click();
  const form = page.locator(".pf-dep-prov-form");
  await expect(form).toBeVisible();
  await expect(form.getByLabel("Provider name")).toBeFocused();
  await expect(form.getByRole("button", { name: "Add provider" })).toBeDisabled();

  await form.getByLabel("Provider name").fill("Webhook relay");
  await form.getByLabel("Provider type").selectOption("webhook");
  await form.getByLabel("Provider status").selectOption("degraded");
  await form.getByLabel("Provider connection note").fill("https://hooks.example.com/live");
  await form.getByRole("button", { name: "Add provider" }).click();

  await expect(form).toHaveCount(0);
  await expect(page.locator(".pf-dep-pane-status")).toContainText(
    "Added Webhook relay provider to stripe-api · production."
  );
  await expect(page.locator(".pf-dep-prov")).toHaveCount(9);
  const provider = page.locator(".pf-dep-prov").filter({ hasText: "Webhook relay" });
  await expect(provider).toBeVisible();
  await expect(provider).toContainText("https://hooks.example.com/live");
  await expect(provider).toContainText("degraded");
  await expect(provider).toHaveCount(1);
});

test("deployment provider settings edit the selected integration card", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Deployments" }).click();
  await page.getByRole("tab", { name: "Providers" }).click();

  const provider = page.locator(".pf-dep-prov").filter({ hasText: "OpenAI" });
  await expect(provider).toContainText("org-puffer");
  await expect(provider).toContainText("degraded");

  await provider.getByRole("button", { name: "Edit OpenAI provider settings" }).click();
  const form = page.getByRole("form", { name: "Edit OpenAI provider settings" });
  await expect(form).toBeVisible();
  await expect(form.getByLabel("Provider name")).toHaveValue("OpenAI");

  await form.getByLabel("Provider status").selectOption("connected");
  await form.getByLabel("Provider connection note").fill("org-puffer-v2");
  await form.getByRole("button", { name: "Save settings" }).click();

  await expect(form).toHaveCount(0);
  await expect(page.locator(".pf-dep-pane-status")).toContainText(
    "Updated OpenAI provider settings for stripe-api · production."
  );
  await expect(provider).toContainText("org-puffer-v2");
  await expect(provider).toContainText("connected");
});

test("deployment secret reveal controls target one key and toggle their state", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  const sidebar = page.locator(".pf-sidebar");
  await sidebar.getByRole("button", { name: "Deployments" }).click();
  await page.getByRole("tab", { name: "Secrets" }).click();

  const row = page.locator(".pf-dep-secrets-row").filter({ hasText: "DATABASE_URL" });
  await expect(row).toContainText("••••••••••••••");
  await expect(page.getByRole("button", { name: "Reveal", exact: true })).toHaveCount(0);

  const revealDatabaseUrl = page.getByRole("button", { name: "Reveal DATABASE_URL", exact: true });
  await expect(revealDatabaseUrl).toHaveCount(1);
  await revealDatabaseUrl.click();

  await expect(row).toContainText(/postgres:\/\/.*db\.puffer\.app\/prod/);
  const hideDatabaseUrl = page.getByRole("button", { name: "Hide DATABASE_URL", exact: true });
  await expect(hideDatabaseUrl).toHaveAttribute("aria-pressed", "true");
  await hideDatabaseUrl.click();

  await expect(row).toContainText("••••••••••••••");
  await expect(page.getByRole("button", { name: "Reveal DATABASE_URL", exact: true })).toHaveAttribute(
    "aria-pressed",
    "false"
  );
});

test("deployment memory and secret row more menus expose actions", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  const sidebar = page.locator(".pf-sidebar");
  await sidebar.getByRole("button", { name: "Deployments" }).click();

  await page.getByRole("tab", { name: "Memory" }).click();
  const memoryNote = page.locator(".pf-dep-mem").filter({ hasText: "Node 20 drops http-keepalive by default" });
  await memoryNote.getByRole("button", { name: "More actions for Node 20 drops http-keepalive by default" }).click();

  const memoryMenu = page.getByRole("menu", { name: "Actions for Node 20 drops http-keepalive by default" });
  await expect(memoryMenu).toBeVisible();
  await memoryMenu.getByRole("menuitem", { name: "Pin note" }).click();
  await expect(memoryNote).toContainText("pinned");
  await expect(page.getByRole("status")).toContainText(
    'Pinned "Node 20 drops http-keepalive by default" for stripe-api · production.'
  );

  await memoryNote.getByRole("button", { name: "More actions for Node 20 drops http-keepalive by default" }).click();
  await page.getByRole("menu", { name: "Actions for Node 20 drops http-keepalive by default" })
    .getByRole("menuitem", { name: "Use in Ask Puffer" })
    .click();
  await expect(page.getByRole("status")).toContainText(
    'Queued "Node 20 drops http-keepalive by default" as Ask Puffer context for stripe-api · production.'
  );

  await page.getByRole("tab", { name: "Secrets" }).click();
  const secretRow = page.locator(".pf-dep-secrets-row").filter({ hasText: "DATABASE_URL" });
  await secretRow.getByRole("button", { name: "More actions for DATABASE_URL" }).click();

  const secretMenu = page.getByRole("menu", { name: "Actions for DATABASE_URL" });
  await expect(secretMenu).toBeVisible();
  await secretMenu.getByRole("menuitem", { name: "Queue rotation" }).click();
  await expect(secretRow).toContainText("needs rotation");
  await expect(page.getByRole("status")).toContainText("Queued rotation for DATABASE_URL in stripe-api · production.");

  await secretRow.getByRole("button", { name: "More actions for DATABASE_URL" }).click();
  await page.getByRole("menu", { name: "Actions for DATABASE_URL" })
    .getByRole("menuitem", { name: "Audit access" })
    .click();
  await expect(page.getByRole("status")).toContainText("Queued access audit for DATABASE_URL in stripe-api · production.");
});

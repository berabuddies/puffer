import { expect, type Page, test } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";

async function openRegressionAgent(page: Page): Promise<void> {
  await page
    .locator(".pf-sidebar-agents-list")
    .getByRole("button", { name: /^Browser regression\b/ })
    .click();
}

async function openFilesPanel(page: Page): Promise<void> {
  await page.locator(".pf-agent-tabs").getByRole("button", { name: "Files", exact: true }).click();
}

test("Files tab close button works from the keyboard", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openFilesPanel(page);

  const libTab = page.getByRole("tab", { name: /lib\.rs/ });
  await expect(libTab).toBeVisible();

  await libTab.getByRole("button", { name: "Close lib.rs" }).focus();
  await page.keyboard.press("Enter");

  await expect(page.getByRole("tab", { name: /lib\.rs/ })).toHaveCount(0);
});

test("Files tab saves text edits through the daemon", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openFilesPanel(page);

  const editor = page.getByLabel("Edit file contents");
  await expect(editor).toHaveValue("fn main() {}\n");

  const saved = "fn main() {\n    println!(\"saved\");\n}\n";
  await editor.fill(saved);
  await expect(page.locator(".file-tab.active .dirty-dot")).toBeVisible();

  await page.getByRole("button", { name: "Save" }).click();
  const request = await daemon.waitForRequest(
    "write_file",
    (candidate) => candidate.params.path === "/tmp/puffer/src/main.rs"
  );
  expect(request.params).toMatchObject({
    path: "/tmp/puffer/src/main.rs",
    content: saved
  });

  await expect(page.getByRole("button", { name: "Save" })).toHaveCount(0);
  await expect(page.locator(".file-tab.active .dirty-dot")).toHaveCount(0);
  await expect(editor).toHaveValue(saved);
});

test("Files tab keeps dirty edits visible after save failure", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openFilesPanel(page);

  const editor = page.getByLabel("Edit file contents");
  await expect(editor).toHaveValue("fn main() {}\n");

  const draft = "fn main() {\n    println!(\"retry me\");\n}\n";
  await editor.fill(draft);
  daemon.failNext("write_file", "disk full");
  await page.getByRole("button", { name: "Save" }).click();

  await daemon.waitForRequest(
    "write_file",
    (candidate) => candidate.params.path === "/tmp/puffer/src/main.rs"
  );
  await expect(page.locator(".save-error")).toContainText("disk full");
  await expect(page.getByRole("button", { name: "Save" })).toBeVisible();
  await expect(page.locator(".file-tab.active .dirty-dot")).toBeVisible();
  await expect(editor).toHaveValue(draft);
});

test("Files tab opens symbol context from the editor cursor", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openFilesPanel(page);

  await page.getByRole("tab", { name: /lib\.rs/ }).click();
  const editor = page.getByLabel("Edit file contents");
  await expect(editor).toHaveValue("pub fn fixture() {}\n");
  await editor.evaluate((node) => {
    const textarea = node as HTMLTextAreaElement;
    textarea.focus();
    textarea.setSelectionRange(9, 9);
  });
  await editor.press("ArrowRight");

  const inspect = await daemon.waitForRequest(
    "lsp_inspect",
    (candidate) => candidate.params.path === "/tmp/puffer/src/lib.rs"
  );
  expect(inspect.params).toMatchObject({
    path: "/tmp/puffer/src/lib.rs",
    cwd: "/tmp/puffer",
    line: 0
  });

  const popup = page.getByLabel("Symbol references");
  await expect(popup).toBeVisible();
  await expect(popup.locator(".symbol")).toContainText("fixture");
  await expect(popup.getByText("fixture() -> demo value")).toBeVisible();
  await expect(popup.locator(".lsp-location")).toHaveCount(2);
  await expect(popup.locator(".lsp-location").first()).toContainText("src/lib.rs:1:8");

  await popup.getByRole("button", { name: "Close symbol popup" }).click();
  await expect(popup).toHaveCount(0);
});

test("New agent modal closes with Escape", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "New agent in puffer" }).click();
  const dialog = page.getByRole("dialog", { name: "New agent" });
  await expect(dialog).toBeVisible();

  await page.keyboard.press("Escape");

  await expect(dialog).toHaveCount(0);
});

test("new agent provider choice is used for the first turn", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "New agent in puffer" }).click();
  const dialog = page.getByRole("dialog", { name: "New agent" });
  await expect(dialog).toBeVisible();
  await dialog.getByRole("radio", { name: /Anthropic/ }).click();
  await dialog.getByRole("button", { name: "Start agent" }).click();

  const createRequest = await daemon.waitForRequest("create_session");
  expect(createRequest.params).toMatchObject({
    providerId: "anthropic"
  });

  await expect(page.locator(".pf-composer textarea")).toBeVisible();
  await page.locator(".pf-composer textarea").fill("Hello from Anthropic");
  await page.getByRole("button", { name: "Send" }).click();

  const turnRequest = await daemon.waitForRequest(
    "run_agent_turn",
    (request) => request.params.message === "Hello from Anthropic"
  );
  expect(turnRequest.params).toMatchObject({
    providerId: "anthropic",
    modelId: "test-model"
  });
});

test("empty workspace can start a new agent in the default workspace", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await daemon.install(page);
  await daemon.open(page);

  await expect(page.getByRole("heading", { name: "No sessions yet" })).toBeVisible();
  await page.getByRole("button", { name: "New agent in default workspace" }).click();

  const dialog = page.getByRole("dialog", { name: "New agent" });
  await expect(dialog).toBeVisible();
  await dialog.getByRole("button", { name: "Start agent" }).click();

  const createRequest = await daemon.waitForRequest("create_session");
  expect(createRequest.params).toMatchObject({
    cwd: "/tmp/puffer",
    providerId: "codex"
  });
});

test("connect project provider choice includes Anthropic", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Connect project" }).click();
  const dialog = page.getByRole("dialog", { name: "Connect project" });
  await expect(dialog).toBeVisible();

  await expect(dialog.getByRole("radio", { name: "Anthropic" })).toBeVisible();
  await dialog.getByRole("radio", { name: "Anthropic" }).click();
  await dialog.getByLabel("Directory").fill("/tmp/puffer-new-project");
  await dialog.getByRole("button", { name: "Start agent" }).click();

  const createRequest = await daemon.waitForRequest("create_session");
  expect(createRequest.params).toMatchObject({
    cwd: "/tmp/puffer-new-project",
    providerId: "anthropic"
  });
});

test("connect project remote mode exposes binary override", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Connect project" }).click();
  const dialog = page.getByRole("dialog", { name: "Connect project" });
  await dialog.getByRole("tab", { name: /Remote/ }).click();

  await expect(dialog.getByLabel("Remote binary")).toBeVisible();
  await dialog.getByLabel("Remote binary").fill("/opt/puffer/bin/puffer");
  await expect(dialog.getByLabel("Remote binary")).toHaveValue("/opt/puffer/bin/puffer");
});

test("failed remote project creation restores the previous daemon", async ({ page }) => {
  const localDaemon = new FakeDaemon();
  localDaemon.failNext("create_session", "remote create failed");

  await localDaemon.install(page);
  await localDaemon.open(page, {
    extraParams: {
      pufferRemoteBackend: localDaemon.url,
      pufferRemoteToken: "remote-token",
      pufferRemoteWorkspaceRoot: "/tmp/puffer-remote"
    }
  });

  await page.getByRole("button", { name: "Connect project" }).click();
  const dialog = page.getByRole("dialog", { name: "Connect project" });
  await dialog.getByRole("tab", { name: /Remote/ }).click();
  await dialog.getByLabel("SSH target").fill("devbox");
  await dialog.getByLabel("Destination directory").fill("/tmp/remote-project");
  await dialog.getByRole("button", { name: "Start agent" }).click();

  await localDaemon.waitForRequest("create_session");
  await expect(dialog.locator(".pf-modal-status")).toContainText("remote create failed");

  const activeToken = await page.evaluate(async () => {
    const mod = await import("/src/lib/api/daemonClient.ts");
    return mod.currentDaemonClient()?.handshake.token ?? null;
  });
  expect(activeToken).toBe("test");
});

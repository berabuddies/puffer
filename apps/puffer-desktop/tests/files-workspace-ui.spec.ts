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

const codexAuth = [
  {
    providerId: "codex",
    kind: "oauth",
    email: "tester@example.com",
    expiresAtMs: null,
    scopes: [],
    planType: "test",
    organizationName: null
  }
];

const canonicalProviderAuth = [
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
    providerId: "anthropic",
    kind: "api_key",
    email: null,
    expiresAtMs: null,
    scopes: [],
    planType: null,
    organizationName: null
  }
];

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

test("Files tab releases save state after switching tabs mid-save", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openFilesPanel(page);

  const editor = page.getByLabel("Edit file contents");
  const mainDraft = "fn main() {\n    println!(\"background save\");\n}\n";
  await editor.fill(mainDraft);
  daemon.delayResponse(
    "write_file",
    (candidate) => candidate.params.path === "/tmp/puffer/src/main.rs",
    250
  );
  await page.getByRole("button", { name: "Save" }).click();

  await daemon.waitForRequest(
    "write_file",
    (candidate) => candidate.params.path === "/tmp/puffer/src/main.rs"
  );
  await page.getByRole("tab", { name: /lib\.rs/ }).click();

  await expect(page.getByRole("tab", { name: /main\.rs/ }).locator(".dirty-dot")).toHaveCount(0);
  await expect(editor).toHaveValue("pub fn fixture() {}\n");

  const libDraft = "pub fn fixture() {\n    println!(\"after save\");\n}\n";
  await editor.fill(libDraft);
  await expect(page.getByRole("button", { name: "Save" })).toBeEnabled();
});

test("Files editor keeps global find shortcuts while focused", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openFilesPanel(page);

  const editor = page.getByLabel("Edit file contents");
  await editor.focus();
  await expect(editor).toBeFocused();

  await page.keyboard.press("Control+F");

  await expect(page.getByRole("search", { name: "Find in agent view" })).toHaveCount(0);
  await expect(editor).toBeFocused();
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

test("Files tab ignores stale symbol inspect results after switching files", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.delayResponse(
    "lsp_inspect",
    (request) => request.params.path === "/tmp/puffer/src/lib.rs",
    120
  );
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
  await daemon.waitForRequest(
    "lsp_inspect",
    (request) => request.params.path === "/tmp/puffer/src/lib.rs"
  );

  await page.getByRole("tab", { name: /main\.rs/ }).click();
  await expect(editor).toHaveValue("fn main() {}\n");
  await editor.evaluate((node) => {
    const textarea = node as HTMLTextAreaElement;
    textarea.focus();
    textarea.setSelectionRange(3, 3);
  });
  await editor.press("ArrowRight");
  await daemon.waitForRequest(
    "lsp_inspect",
    (request) => request.params.path === "/tmp/puffer/src/main.rs"
  );

  const popup = page.getByLabel("Symbol references");
  await expect(popup).toBeVisible();
  await expect(popup.locator(".symbol")).toContainText("main");
  await expect(popup.getByText("main() -> demo value")).toBeVisible();

  await page.waitForTimeout(170);
  await expect(popup.locator(".symbol")).toContainText("main");
  await expect(popup.getByText("main() -> demo value")).toBeVisible();
  await expect(popup.getByText("fixture() -> demo value")).toHaveCount(0);
  await expect(popup.locator(".lsp-location").first()).toContainText("src/main.rs:1:4");
});

test("Files tab does not reopen a linked file from the previous session", async ({ page }) => {
  const linkedPath = "/tmp/project-a/src/main.rs";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-files-a",
        displayName: "Files A",
        title: "Files A",
        cwd: "/tmp/project-a",
        folderPath: "/tmp/project-a",
        timeline: [
          {
            kind: "assistant_message",
            id: "files-a-link",
            text: `Open [alpha main](${linkedPath}) for context.`
          }
        ]
      },
      {
        sessionId: "session-files-b",
        displayName: "Files B",
        title: "Files B",
        cwd: "/tmp/project-b",
        folderPath: "/tmp/project-b",
        timeline: [
          {
            kind: "assistant_message",
            id: "files-b-note",
            text: "This session should not inherit linked files from Files A."
          }
        ]
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await page
    .locator(".pf-sidebar-agents-list")
    .getByRole("button", { name: /^Files A\b/ })
    .click();
  await page.getByRole("link", { name: "alpha main" }).click();
  await daemon.waitForRequest("read_file", (request) => request.params.path === linkedPath);

  await page.locator(".pf-agent-tabs").getByRole("button", { name: "Chat", exact: true }).click();
  const linkedReadsBefore = daemon.requests.filter(
    (request) => request.method === "read_file" && request.params.path === linkedPath
  ).length;

  await page
    .locator(".pf-sidebar-agents-list")
    .getByRole("button", { name: /^Files B\b/ })
    .click();
  await openFilesPanel(page);
  await page.waitForTimeout(150);

  const linkedReadsAfter = daemon.requests.filter(
    (request) => request.method === "read_file" && request.params.path === linkedPath
  ).length;
  expect(linkedReadsAfter).toBe(linkedReadsBefore);
  await expect(page.locator(".viewer-head .path", { hasText: linkedPath })).toHaveCount(0);
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

test("new agent ignores repeated start clicks while creating", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.delayResponse("create_session", () => true, 250);
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "New agent in puffer" }).click();
  const dialog = page.getByRole("dialog", { name: "New agent" });
  await expect(dialog).toBeVisible();

  await dialog.getByRole("button", { name: "Start agent" }).evaluate((button) => {
    (button as HTMLButtonElement).click();
    (button as HTMLButtonElement).click();
  });

  await daemon.waitForRequest("create_session");
  await page.waitForTimeout(50);
  expect(daemon.requests.filter((request) => request.method === "create_session")).toHaveLength(1);
});

test("new agent fallback providers use daemon provider ids", async ({ page }) => {
  const daemon = new FakeDaemon({ auth: canonicalProviderAuth, providers: [] });
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "New agent in puffer" }).click();
  const dialog = page.getByRole("dialog", { name: "New agent" });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole("radio", { name: /Codex/ })).toBeVisible();
  await expect(dialog.getByRole("radio", { name: /Anthropic/ })).toBeVisible();
  await dialog.getByRole("button", { name: "Start agent" }).click();

  const createRequest = await daemon.waitForRequest("create_session");
  expect(createRequest.params).toMatchObject({
    providerId: "openai"
  });
});

test("new agent provider picker only shows authenticated providers", async ({ page }) => {
  const daemon = new FakeDaemon({ auth: codexAuth });
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "New agent in puffer" }).click();
  const dialog = page.getByRole("dialog", { name: "New agent" });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole("radio", { name: /Codex/ })).toBeVisible();
  await expect(dialog.getByRole("radio", { name: /Anthropic/ })).toHaveCount(0);
  await dialog.getByRole("button", { name: "Start agent" }).click();

  const createRequest = await daemon.waitForRequest("create_session");
  expect(createRequest.params).toMatchObject({
    providerId: "openai"
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
    providerId: "openai"
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

test("connect project ignores repeated start clicks while creating", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.delayResponse("create_session", () => true, 250);
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Connect project" }).click();
  const dialog = page.getByRole("dialog", { name: "Connect project" });
  await expect(dialog).toBeVisible();
  await dialog.getByLabel("Directory").fill("/tmp/puffer-new-project");

  await dialog.getByRole("button", { name: "Start agent" }).evaluate((button) => {
    (button as HTMLButtonElement).click();
    (button as HTMLButtonElement).click();
  });

  await daemon.waitForRequest("create_session");
  await page.waitForTimeout(50);
  expect(daemon.requests.filter((request) => request.method === "create_session")).toHaveLength(1);
});

test("connect project provider picker only shows authenticated providers", async ({ page }) => {
  const daemon = new FakeDaemon({ auth: codexAuth });
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Connect project" }).click();
  const dialog = page.getByRole("dialog", { name: "Connect project" });
  await expect(dialog).toBeVisible();

  await expect(dialog.getByRole("radio", { name: "Codex" })).toBeVisible();
  await expect(dialog.getByRole("radio", { name: "Anthropic" })).toHaveCount(0);
  await dialog.getByLabel("Directory").fill("/tmp/puffer-new-project");
  await dialog.getByRole("button", { name: "Start agent" }).click();

  const createRequest = await daemon.waitForRequest("create_session");
  expect(createRequest.params).toMatchObject({
    cwd: "/tmp/puffer-new-project",
    providerId: "openai"
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

test("connect project directory picker ignores stale path responses", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.delayResponse(
    "list_dir",
    (request) => request.params.path === "/tmp/puffer",
    220
  );
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Connect project" }).click();
  const dialog = page.getByRole("dialog", { name: "Connect project" });
  await dialog.getByRole("button", { name: "Browse…" }).click();

  const picker = dialog.getByLabel("Choose directory");
  const pickerInput = picker.getByPlaceholder("/Users/me/src");
  await expect(picker).toBeVisible();
  await pickerInput.fill("/tmp/puffer/src");
  await picker.getByRole("button", { name: "Go" }).click();

  await expect(pickerInput).toHaveValue("/tmp/puffer/src");
  await expect(picker.getByText("No child directories.")).toBeVisible();
  await page.waitForTimeout(260);
  await expect(pickerInput).toHaveValue("/tmp/puffer/src");
  await expect(picker.getByText("No child directories.")).toBeVisible();
  await expect(picker.getByRole("button", { name: "src" })).toHaveCount(0);
});

test("connect project mode switch closes the local directory picker", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Connect project" }).click();
  const dialog = page.getByRole("dialog", { name: "Connect project" });
  await dialog.getByRole("button", { name: "Browse…" }).click();

  const picker = dialog.getByLabel("Choose directory");
  await expect(picker).toBeVisible();

  await dialog.getByRole("tab", { name: /Remote/ }).click();

  await expect(picker).toHaveCount(0);
  await expect(dialog.getByLabel("SSH target")).toBeVisible();
});

test("connect project Escape closes directory picker before parent modal", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "Connect project" }).click();
  const dialog = page.getByRole("dialog", { name: "Connect project" });
  await dialog.getByLabel("Directory").fill("/tmp/puffer-new-project");
  await dialog.getByRole("button", { name: "Browse…" }).click();

  const picker = dialog.getByLabel("Choose directory");
  await expect(picker).toBeVisible();

  await page.keyboard.press("Escape");

  await expect(dialog).toBeVisible();
  await expect(picker).toHaveCount(0);
  await expect(dialog.getByLabel("Directory")).toHaveValue("/tmp/puffer-new-project");
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

test("successful remote project creation adopts remote daemon state", async ({ page }) => {
  const localDaemon = new FakeDaemon({ workspaceRoot: "/tmp/puffer-local" });
  const remoteDaemon = new FakeDaemon({
    url: "ws://127.0.0.1:17778/ws",
    workspaceRoot: "/tmp/puffer-remote"
  });
  await localDaemon.install(page);
  await remoteDaemon.install(page);
  await localDaemon.open(page, {
    extraParams: {
      pufferRemoteBackend: remoteDaemon.url,
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

  const createRequest = await remoteDaemon.waitForRequest("create_session");
  expect(createRequest.params).toMatchObject({
    cwd: "/tmp/remote-project",
    providerId: "openai"
  });
  await expect(dialog).toHaveCount(0);

  const active = await page.evaluate(async () => {
    const mod = await import("/src/lib/api/daemonClient.ts");
    const handshake = mod.currentDaemonClient()?.handshake ?? null;
    return handshake
      ? { token: handshake.token, workspaceRoot: handshake.workspaceRoot }
      : null;
  });
  expect(active).toEqual({
    token: "remote-token",
    workspaceRoot: "/tmp/puffer-remote"
  });

  await page.getByRole("button", { name: "Settings" }).click();
  await expect(page.getByRole("heading", { name: "General" })).toBeVisible();
  const settingsPane = page.locator(".pf-settings-pane");
  await expect(settingsPane.locator(".pf-settings-row").filter({ hasText: "Workspace root" })).toContainText(
    "/tmp/puffer-remote"
  );
  await expect(settingsPane.locator(".pf-settings-row").filter({ hasText: "Daemon" })).toContainText(
    remoteDaemon.url
  );

  const localPermissionRequestsBefore = localDaemon.requests.filter(
    (request) => request.method === "list_permissions"
  ).length;
  await page.getByRole("button", { name: "Permissions" }).click();
  await remoteDaemon.waitForRequest("list_permissions");
  expect(
    localDaemon.requests.filter((request) => request.method === "list_permissions")
  ).toHaveLength(localPermissionRequestsBefore);
  await expect(page.getByText("Stored at")).toContainText(
    "/tmp/puffer-remote/.puffer/permissions.json"
  );
});

test("remote project creation uses remote authenticated provider", async ({ page }) => {
  const localDaemon = new FakeDaemon({
    workspaceRoot: "/tmp/puffer-local",
    auth: canonicalProviderAuth
  });
  const remoteDaemon = new FakeDaemon({
    url: "ws://127.0.0.1:17779/ws",
    workspaceRoot: "/tmp/puffer-remote",
    auth: codexAuth
  });
  await localDaemon.install(page);
  await remoteDaemon.install(page);
  await localDaemon.open(page, {
    extraParams: {
      pufferRemoteBackend: remoteDaemon.url,
      pufferRemoteToken: "remote-token",
      pufferRemoteWorkspaceRoot: "/tmp/puffer-remote"
    }
  });

  await page.getByRole("button", { name: "Connect project" }).click();
  const dialog = page.getByRole("dialog", { name: "Connect project" });
  await dialog.getByRole("radio", { name: "Anthropic" }).click();
  await dialog.getByRole("tab", { name: /Remote/ }).click();
  await dialog.getByLabel("SSH target").fill("devbox");
  await dialog.getByLabel("Destination directory").fill("/tmp/remote-project");
  await dialog.getByRole("button", { name: "Start agent" }).click();

  const createRequest = await remoteDaemon.waitForRequest("create_session");
  expect(createRequest.params).toMatchObject({
    cwd: "/tmp/remote-project",
    providerId: "openai"
  });
  expect(
    remoteDaemon.requests.some((request) => request.method === "load_settings_snapshot")
  ).toBe(true);
});

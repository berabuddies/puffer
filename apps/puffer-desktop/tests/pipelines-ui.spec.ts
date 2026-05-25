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

test("pipeline connector search selects a connection trigger", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  await expect(page.getByLabel("Trigger type")).toHaveValue("subscription");
  await page.getByLabel("Search connectors").fill("telegram");
  await expect(page.locator('[aria-label="Connector catalog"]').getByText("telegram-login")).toBeVisible();

  await page.getByRole("button", { name: "Use telegram-user as workflow trigger" }).click();

  await expect(page.getByLabel("Trigger type")).toHaveValue("connection");
  await expect(page.getByLabel("Workflow connection")).toHaveValue("telegram-user");
  await expect(page.locator(".pf-pipe-graph").getByRole("button", { name: /telegram-user/ })).toBeVisible();
});

test("pipeline editor saves workflow changes through daemon", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  const saveButton = page.getByRole("button", { name: "Save workflow" });
  await expect(saveButton).toBeDisabled();

  await page.locator(".pf-editor-config").getByLabel("Name").fill("Saved monitor pipeline");
  await expect(saveButton).toBeEnabled();
  await expect(page.locator(".pf-pipe-save-note")).toContainText("Save to persist");

  await saveButton.click();
  const request = await daemon.waitForRequest("workflow_save");
  const workflow = request.params.workflow as {
    slug?: string;
    pipeline?: { name?: string; nodes?: Array<{ type?: string }> };
  };
  expect(workflow.slug).toBe("agent-review-pipeline");
  expect(workflow.pipeline?.name).toBe("Saved monitor pipeline");
  expect(workflow.pipeline?.nodes?.[0]?.type).toBe("codex");
  await expect(page.locator(".pf-pipe-save-note")).toContainText("Saved agent-review-pipeline.");
  await expect(saveButton).toBeDisabled();
});

test("pipeline editor creates new workflow drafts before saving", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  await page.getByRole("button", { name: "New workflow" }).click();

  await expect(page.locator(".pf-pipe-save-note")).toContainText("Created workflow-draft locally");
  await expect(page.locator(".pf-editor-config").getByLabel("Name")).toHaveValue("Workflow draft");
  await expect(page.locator(".pf-editor-config").getByLabel("Slug")).toHaveValue("workflow-draft");
  await expect(page.locator(".pf-editor-inline").getByRole("checkbox")).not.toBeChecked();
  await expect(page.getByLabel("Trigger type")).toHaveValue("connection");
  await expect(page.getByLabel("Workflow connection")).toHaveValue("telegram-user");

  const saveButton = page.getByRole("button", { name: "Save workflow" });
  await expect(saveButton).toBeEnabled();
  await saveButton.click();

  const request = await daemon.waitForRequest("workflow_save", (candidate) => {
    const workflow = candidate.params.workflow as { slug?: string };
    return workflow.slug === "workflow-draft";
  });
  const workflow = request.params.workflow as {
    slug?: string;
    enabled?: boolean;
    trigger?: { type?: string; connection_slug?: string };
    pipeline?: { name?: string };
  };
  expect(workflow.enabled).toBe(false);
  expect(workflow.trigger).toMatchObject({ type: "connection", connection_slug: "telegram-user" });
  expect(workflow.pipeline?.name).toBe("Workflow draft");
  await expect(page.locator(".pf-pipe-save-note")).toContainText("Saved workflow-draft.");
  await expect(saveButton).toBeDisabled();
});

test("pipeline editor can pause and resume workflows through daemon", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  const pauseButton = page.getByRole("button", { name: "Pause workflow" });
  await expect(pauseButton).toBeEnabled();
  await pauseButton.click();

  const pauseRequest = await daemon.waitForRequest(
    "workflow_toggle",
    (candidate) => candidate.params.slug === "agent-review-pipeline" && candidate.params.enabled === false
  );
  expect(pauseRequest.params.slug).toBe("agent-review-pipeline");
  await expect(page.locator(".pf-run-header-state")).toHaveText("disabled");

  const resumeButton = page.getByRole("button", { name: "Resume workflow" });
  await expect(resumeButton).toBeEnabled();
  await resumeButton.click();

  const resumeRequest = await daemon.waitForRequest(
    "workflow_toggle",
    (candidate) => candidate.params.slug === "agent-review-pipeline" && candidate.params.enabled === true
  );
  expect(resumeRequest.params.enabled).toBe(true);
  await expect(page.locator(".pf-run-header-state")).toHaveText("enabled");
});

test("pipeline connector search matches multiple metadata terms", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  const catalog = page.locator('[aria-label="Connector catalog"]');
  await page.getByLabel("Search connectors").fill("personal mtproto");
  await expect(catalog.getByText("telegram-login")).toBeVisible();
  await expect(catalog.getByText("slack-app")).not.toBeVisible();

  await page.getByLabel("Search connectors").fill("web actions");
  await expect(catalog.getByRole("button", { name: "Select slack-app connector setup" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Plan telegram-login workflow trigger" })).not.toBeVisible();
});

test("pipeline connector catalog can create a workflow draft for a connector", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  await page.getByLabel("Search connectors").fill("telegram personal");
  await page.getByRole("button", { name: "Create workflow draft for telegram-login" }).click();

  await expect(page.locator(".pf-pipe-save-note")).toContainText("Created telegram-user-backed workflow locally");
  await expect(page.locator(".pf-editor-config").getByLabel("Name", { exact: true })).toHaveValue("Telegram User workflow");
  await expect(page.locator(".pf-editor-config").getByLabel("Slug")).toHaveValue("telegram-user-workflow");
  await expect(page.getByLabel("Trigger type")).toHaveValue("connection");
  await expect(page.getByLabel("Workflow connection")).toHaveValue("telegram-user");

  await page.getByRole("button", { name: "Save workflow" }).click();
  const request = await daemon.waitForRequest("workflow_save", (candidate) => {
    const workflow = candidate.params.workflow as { slug?: string };
    return workflow.slug === "telegram-user-workflow";
  });
  const workflow = request.params.workflow as {
    enabled?: boolean;
    trigger?: { type?: string; connection_slug?: string; pattern?: string };
  };
  expect(workflow.enabled).toBe(false);
  expect(workflow.trigger).toMatchObject({
    type: "connection",
    connection_slug: "telegram-user",
    pattern: ".*"
  });
});

test("pipeline selected connector can create a planned workflow draft", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  const catalog = page.locator('[aria-label="Connector catalog"]');
  await page.getByLabel("Search connectors").fill("email events");
  await catalog.getByRole("button", { name: "Plan email workflow trigger" }).click();
  await page.getByLabel("Connector connection name").fill("email-personal");
  await page.getByRole("button", { name: "Create workflow draft for selected connector" }).click();

  await expect(page.locator(".pf-pipe-save-note")).toContainText("Run /connect email email-personal before enabling it");
  await expect(page.locator(".pf-editor-config").getByLabel("Name", { exact: true })).toHaveValue("Email Personal workflow");
  await expect(page.locator(".pf-editor-config").getByLabel("Slug")).toHaveValue("email-personal-workflow");
  await expect(page.getByLabel("Trigger type")).toHaveValue("connection");
  await expect(page.getByLabel("Workflow connection")).toHaveValue("email-personal");

  await page.getByRole("button", { name: "Save workflow" }).click();
  const request = await daemon.waitForRequest("workflow_save", (candidate) => {
    const workflow = candidate.params.workflow as { slug?: string };
    return workflow.slug === "email-personal-workflow";
  });
  const workflow = request.params.workflow as {
    enabled?: boolean;
    trigger?: { type?: string; connection_slug?: string; pattern?: string };
  };
  expect(workflow.enabled).toBe(false);
  expect(workflow.trigger).toMatchObject({
    type: "connection",
    connection_slug: "email-personal",
    pattern: ".*"
  });
});

test("pipeline connector catalog shows built-in coverage and result counts", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  const catalog = page.locator('[aria-label="Connector catalog"]');
  const resultSummary = page.getByLabel("Connector search results");
  const connectorSlugs = [
    "telegram-login",
    "telegram-bot",
    "discord-bot",
    "lark-app",
    "lark-login",
    "matrix-bot",
    "slack-app",
    "slack-login",
    "slack-bot",
    "email",
    "webhook"
  ];

  await expect(resultSummary).toHaveText("11/11 connectors; 2/2 connections");
  for (const slug of connectorSlugs) {
    await expect(catalog).toContainText(slug);
  }

  await page.getByLabel("Search connectors").fill("workspace local session");
  await expect(resultSummary).toHaveText("1/11 connectors; 0/2 connections");
  await expect(catalog.getByRole("button", { name: "Select slack-login connector setup" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Select slack-app connector setup" })).not.toBeVisible();

  await page.getByLabel("Search connectors").fill("serve webhook");
  await expect(resultSummary).toHaveText("1/11 connectors; 0/2 connections");
  await expect(catalog.getByRole("button", { name: "Select webhook connector setup" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Select matrix-bot connector setup" })).not.toBeVisible();
});

test("pipeline connector catalog shows and searches existing connection names", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  const catalog = page.locator('[aria-label="Connector catalog"]');
  const resultSummary = page.getByLabel("Connector search results");

  await page.getByLabel("Search connectors").fill("telegram-user");
  await expect(resultSummary).toHaveText("1/11 connectors; 1/2 connections");
  const telegram = catalog.getByRole("button", { name: "Plan telegram-login workflow trigger" });
  await expect(telegram).toContainText("conn:telegram-user");
  await expect(catalog.getByRole("button", { name: "Select slack-app connector setup" })).not.toBeVisible();

  await page.getByLabel("Search connectors").fill("workspace slack-app");
  await expect(resultSummary).toHaveText("1/11 connectors; 1/2 connections");
  const slack = catalog.getByRole("button", { name: "Select slack-app connector setup" });
  await expect(slack).toContainText("conn:slack-app");
  await expect(catalog.getByRole("button", { name: "Plan telegram-login workflow trigger" })).not.toBeVisible();
});

test("pipeline connector catalog shows and searches runtime source hints", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  const catalog = page.locator('[aria-label="Connector catalog"]');
  const connections = page.locator('[aria-label="Connections"]');
  const resultSummary = page.getByLabel("Connector search results");

  await page.getByLabel("Search connectors").fill("serve");
  await expect(resultSummary).toHaveText("4/11 connectors; 0/2 connections");
  await expect(catalog.getByRole("button", { name: "Select webhook connector setup" })).toContainText("serve");
  await expect(catalog.getByRole("button", { name: "Select discord-bot connector setup" })).toContainText("serve");
  await expect(catalog.getByRole("button", { name: "Select slack-app connector setup" })).not.toBeVisible();

  await page.getByLabel("Search connectors").fill("subscriber telegram");
  await expect(resultSummary).toHaveText("1/11 connectors; 1/2 connections");
  await expect(catalog.getByRole("button", { name: "Plan telegram-login workflow trigger" })).toContainText("subscriber");
  await expect(connections.getByRole("button", { name: "Use telegram-user as workflow trigger" })).toContainText("subscriber");
});

test("pipeline connector filter presets apply stable search terms", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  const filters = page.getByLabel("Connector filters");
  const resultSummary = page.getByLabel("Connector search results");

  await filters.getByRole("button", { name: "Trigger", exact: true }).click();
  await expect(page.getByLabel("Search connectors")).toHaveValue("trigger-ready");
  await expect(resultSummary).toHaveText("2/11 connectors; 1/2 connections");
  await expect(filters.getByRole("button", { name: "Trigger", exact: true })).toHaveAttribute("aria-pressed", "true");

  await filters.getByRole("button", { name: "Monitor" }).click();
  await expect(page.getByLabel("Search connectors")).toHaveValue("monitor");
  await expect(resultSummary).toHaveText("0/11 connectors; 1/2 connections");
  await expect(page.locator('[aria-label="Connections"]')).toContainText("telegram-user");

  await filters.getByRole("button", { name: "Tasks" }).click();
  await expect(page.getByLabel("Search connectors")).toHaveValue("monitor task");
  await expect(resultSummary).toHaveText("0/11 connectors; 0/2 connections");
  await expect(page.getByLabel("Monitor task search results")).toHaveText("1/1 monitor tasks");
  await expect(page.getByLabel("Monitor tasks")).toContainText("Reply to Telegram support ping");

  await filters.getByRole("button", { name: "Repair" }).click();
  await expect(page.getByLabel("Search connectors")).toHaveValue("repair");
  await expect(resultSummary).toHaveText("0/11 connectors; 2/2 connections");

  await filters.getByRole("button", { name: "Active" }).click();
  await expect(page.getByLabel("Search connectors")).toHaveValue("active");
  await expect(resultSummary).toHaveText("0/11 connectors; 1/2 connections");
  await expect(page.locator('[aria-label="Connections"]')).toContainText("telegram-user");

  await filters.getByRole("button", { name: "Idle" }).click();
  await expect(page.getByLabel("Search connectors")).toHaveValue("idle");
  await expect(resultSummary).toHaveText("0/11 connectors; 1/2 connections");
  await expect(page.locator('[aria-label="Connections"]')).toContainText("slack-app");

  await filters.getByRole("button", { name: "Actions" }).click();
  await expect(page.getByLabel("Search connectors")).toHaveValue("has-actions");
  await expect(resultSummary).toHaveText("7/11 connectors; 2/2 connections");

  await filters.getByRole("button", { name: "Serve" }).click();
  await expect(page.getByLabel("Search connectors")).toHaveValue("serve");
  await expect(resultSummary).toHaveText("4/11 connectors; 0/2 connections");

  await filters.getByRole("button", { name: "All" }).click();
  await expect(page.getByLabel("Search connectors")).toHaveValue("");
  await expect(resultSummary).toHaveText("11/11 connectors; 2/2 connections");
});

test("pipeline connector search matches setup-only capability terms", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  const catalog = page.locator('[aria-label="Connector catalog"]');
  const connections = page.locator('[aria-label="Connections"]');
  const resultSummary = page.getByLabel("Connector search results");

  await page.getByLabel("Search connectors").fill("no trigger");
  await expect(resultSummary).toHaveText("9/11 connectors; 1/2 connections");
  await expect(catalog.getByRole("button", { name: "Select slack-app connector setup" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Select webhook connector setup" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Plan telegram-login workflow trigger" })).not.toBeVisible();
  await expect(connections.getByRole("button", { name: "slack-app cannot start workflow triggers" })).toBeVisible();
  await expect(connections.getByRole("button", { name: "Use telegram-user as workflow trigger" })).not.toBeVisible();

  await page.getByLabel("Search connectors").fill("setup-only webhook");
  await expect(resultSummary).toHaveText("1/11 connectors; 0/2 connections");
  await expect(catalog.getByRole("button", { name: "Select webhook connector setup" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Select slack-app connector setup" })).not.toBeVisible();
});

test("pipeline connector search shows action matches", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  await page.getByLabel("Search connectors").fill("send message");

  const catalog = page.locator('[aria-label="Connector catalog"]');
  const telegram = catalog.getByRole("button", { name: "Plan telegram-login workflow trigger" });
  const slack = catalog.getByRole("button", { name: "Select slack-app connector setup" });

  await expect(telegram).toContainText("send_message");
  await expect(slack).toContainText("send_message");

  await page.getByLabel("Search connectors").fill("vote poll");
  await expect(telegram).toContainText("vote_poll");
  await expect(slack).not.toBeVisible();
});

test("pipeline connector catalog stages a deterministic connect command", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  await page.getByLabel("Search connectors").fill("email");
  await page.getByRole("button", { name: "Plan email workflow trigger" }).click();

  await expect(page.getByLabel("Trigger type")).toHaveValue("connection");
  await expect(page.getByLabel("Workflow connection")).toHaveValue("email");
  await expect(page.locator(".pf-pipe-graph").getByRole("button", { name: /email/ })).toBeVisible();
  await expect(page.getByLabel("Selected connector command")).toContainText("/connect email email");
  await expect(page.locator(".pf-connector-row", { hasText: "email" })).toHaveAttribute("data-selected", "true");
});

test("pipeline connector command can start setup from the picker", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  await page.getByLabel("Search connectors").fill("email");
  await page.getByRole("button", { name: "Plan email workflow trigger" }).click();

  const connectionName = page.getByLabel("Connector connection name");
  await expect(connectionName).toHaveValue("email");
  await connectionName.fill("Team Email");
  await expect(page.getByLabel("Selected connector command")).toContainText("Enter a valid connection name.");
  await expect(page.getByRole("button", { name: "Run connector command" })).toBeDisabled();

  await connectionName.fill("team-email");
  await expect(page.getByLabel("Workflow connection")).toHaveValue("team-email");
  await expect(page.locator(".pf-pipe-graph").getByRole("button", { name: /team-email/ })).toBeVisible();
  await expect(page.getByLabel("Selected connector command")).toContainText("/connect email team-email");
  await page.getByRole("button", { name: "Run connector command" }).click();

  const request = await daemon.waitForRequest(
    "run_agent_turn",
    (candidate) => candidate.params.message === "/connect email team-email"
  );
  expect(String(request.params.sessionId ?? "")).not.toHaveLength(0);
});

test("pipeline connector catalog can run default setup from a connector row", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  await page.getByLabel("Search connectors").fill("setup lark app react");
  const connector = page
    .locator('[aria-label="Connector catalog"]')
    .getByRole("button", { name: "Select lark-app connector setup" });
  await expect(connector).toContainText("react");
  await expect(connector).toContainText("no trigger");

  const runButton = page.getByRole("button", { name: "Run /connect lark-app lark-app" });
  await expect(runButton).toHaveAttribute("title", "/connect lark-app lark-app");
  await runButton.click();

  const request = await daemon.waitForRequest(
    "run_agent_turn",
    (candidate) => candidate.params.message === "/connect lark-app lark-app"
  );
  expect(String(request.params.sessionId ?? "")).not.toHaveLength(0);
});

test("pipeline connection picker can start connector task monitors", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  await page.getByLabel("Search connectors").fill("monitor telegram");
  const monitorButton = page.getByRole("button", { name: "Run /monitor telegram-user" });
  await expect(monitorButton).toHaveAttribute("title", "/monitor telegram-user");
  await expect(page.locator('[aria-label="Connections"]')).toContainText("monitor");
  await monitorButton.click();

  const request = await daemon.waitForRequest(
    "run_agent_turn",
    (candidate) => candidate.params.message === "/monitor telegram-user"
  );
  expect(String(request.params.sessionId ?? "")).not.toHaveLength(0);
});

test("pipeline monitor workflow panel can pause and resume monitor bindings", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  const monitors = page.getByLabel("Monitor workflows");
  await expect(monitors).toContainText("monitor-telegram-user");
  await expect(monitors).toContainText("telegram-user");
  await expect(page.getByLabel("Monitor workflow search results")).toHaveText("1/1 monitors");

  await page.getByLabel("Search connectors").fill("triage telegram");
  await expect(monitors).toContainText("1/1");
  await monitors.getByRole("button", { name: "Pause monitor-telegram-user" }).click();

  const pauseRequest = await daemon.waitForRequest(
    "workflow_toggle",
    (candidate) => candidate.params.slug === "monitor-telegram-user" && candidate.params.enabled === false
  );
  expect(pauseRequest.params.slug).toBe("monitor-telegram-user");
  await expect(monitors).toContainText("paused");

  await monitors.getByRole("button", { name: "Resume monitor-telegram-user" }).click();
  const resumeRequest = await daemon.waitForRequest(
    "workflow_toggle",
    (candidate) => candidate.params.slug === "monitor-telegram-user" && candidate.params.enabled === true
  );
  expect(resumeRequest.params.enabled).toBe(true);
  await expect(monitors).toContainText("enabled");
});

test("pipeline monitor task panel exposes task actions", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  const tasks = page.getByLabel("Monitor tasks");
  await expect(tasks).toContainText("Reply to Telegram support ping");
  await expect(tasks).toContainText("telegram-user");

  await page.getByLabel("Search connectors").fill("support ping");
  await expect(tasks).toContainText("1/1");
  await expect(page.getByLabel("Monitor task search results")).toHaveText("1/1 monitor tasks");
  await expect(tasks).toContainText("Draft reply");

  await tasks.getByRole("button", { name: "Run monitor action monitor-1 Draft reply" }).click();
  const actionRequest = await daemon.waitForRequest(
    "run_agent_turn",
    (candidate) =>
      String(candidate.params.message ?? "").startsWith("Act on monitored task monitor-1:")
      && String(candidate.params.message ?? "").includes("Draft a concise reply to Alice")
  );
  expect(String(actionRequest.params.sessionId ?? "")).not.toHaveLength(0);
});

test("pipeline monitor task panel can start ignore flows", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  const tasks = page.getByLabel("Monitor tasks");
  await tasks.getByRole("button", { name: "Ignore monitor-1" }).click();
  const ignoreRequest = await daemon.waitForRequest(
    "run_agent_turn",
    (candidate) => candidate.params.message === "/tasks ignore monitor-1 duplicate support ping"
  );
  expect(String(ignoreRequest.params.sessionId ?? "")).not.toHaveLength(0);
});

test("pipeline connection picker can start connection repair setup", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  await page.getByLabel("Search connectors").fill("repair slack");
  const repairButton = page.getByRole("button", { name: "Run /connect slack-app slack-app" });
  await expect(repairButton).toHaveAttribute("title", "/connect slack-app slack-app");
  await expect(page.locator('[aria-label="Connections"]')).toContainText("connect");
  await repairButton.click();

  const request = await daemon.waitForRequest(
    "run_agent_turn",
    (candidate) => candidate.params.message === "/connect slack-app slack-app"
  );
  expect(String(request.params.sessionId ?? "")).not.toHaveLength(0);
});

test("pipeline connector picker keeps non-trigger connections disabled while setup rows stay selectable", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  await page.getByLabel("Search connectors").fill("slack");

  const connection = page
    .locator('[aria-label="Connections"]')
    .getByRole("button", { name: "slack-app cannot start workflow triggers" });
  const connector = page
    .locator('[aria-label="Connector catalog"]')
    .getByRole("button", { name: "Select slack-app connector setup" });

  await expect(connection).toBeDisabled();
  await expect(connector).toBeEnabled();
  await expect(connector).toContainText("no trigger");
  await connector.click();
  await expect(page.getByLabel("Selected connector command")).toContainText("/connect slack-app slack-app");
  await expect(page.getByLabel("Trigger type")).toHaveValue("subscription");
});

test("pipeline connector catalog stages telegram bot setup", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  await page.getByLabel("Search connectors").fill("telegram bot");

  const connector = page
    .locator('[aria-label="Connector catalog"]')
    .getByRole("button", { name: "Select telegram-bot connector setup" });

  await expect(connector).toBeEnabled();
  await expect(connector).toContainText("events");
  await expect(connector).toContainText("proxy");
  await expect(connector).toContainText("no trigger");
  await expect(connector).toContainText("send_message");
  await connector.click();
  await expect(page.getByLabel("Selected connector command")).toContainText("/connect telegram-bot telegram-bot");
  await expect(page.getByLabel("Trigger type")).toHaveValue("subscription");
});

test("pipeline connector catalog can search serve-mode connectors as unavailable triggers", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  await page.getByLabel("Search connectors").fill("discord");

  const connector = page
    .locator('[aria-label="Connector catalog"]')
    .getByRole("button", { name: "Select discord-bot connector setup" });

  await expect(connector).toBeEnabled();
  await expect(connector).toContainText("no trigger");
  await expect(connector).toContainText("auth");
  await connector.click();
  await expect(page.getByLabel("Selected connector command")).toContainText("/connect discord-bot discord-bot");
  await expect(connector).toHaveAttribute("data-selected", "true");
  await expect(page.getByLabel("Trigger type")).toHaveValue("subscription");
});

test("pipeline connection dropdown skips connections that cannot trigger workflows", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  await page.getByLabel("Trigger type").selectOption("connection");

  const connectionSelect = page.getByLabel("Workflow connection");
  await expect(connectionSelect).toHaveValue("telegram-user");

  const slackOption = connectionSelect.locator('option[value="slack-app"]');
  await expect(slackOption).toHaveAttribute("disabled", "");
  await expect(slackOption).toContainText("no trigger");
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

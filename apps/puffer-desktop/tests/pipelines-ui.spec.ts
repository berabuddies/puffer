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

test("pipeline workflow list search filters by workflow and run metadata", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.setWorkflowSnapshot({
    workflows: [
      {
        schema: "puffer.workflow.v1",
        slug: "release-pipeline",
        enabled: true,
        trigger: { type: "connection", connection_slug: "telegram-user", pattern: "ship" },
        pipeline: {
          name: "Release pipeline",
          working_dir: "/tmp/puffer",
          concurrency: 1,
          nodes: [
            {
              id: "deploy",
              type: "codex",
              agent: "Codex deployer",
              model: "gpt-5.4-codex",
              tools: ["bash", "git"],
              prompt: "Deploy and report release status."
            }
          ]
        }
      },
      {
        schema: "puffer.workflow.v1",
        slug: "daily-digest",
        enabled: false,
        trigger: { type: "cron", cron: "0 9 * * *" },
        pipeline: {
          name: "Daily digest",
          working_dir: "/tmp/puffer",
          concurrency: 1,
          nodes: [
            {
              id: "summarize",
              type: "puffer",
              agent: "Puffer summarizer",
              model: "puffer-default",
              tools: ["workflow"],
              prompt: "Summarize daily connector activity."
            }
          ]
        }
      }
    ],
    runs: [
      {
        idx: 12,
        workflow_slug: "release-pipeline",
        run_id: "run-release",
        trigger: { text: "ship this" },
        status: "failed",
        started_at_ms: Date.now() - 10_000,
        ended_at_ms: Date.now(),
        nodes: [
          {
            id: "deploy",
            status: "failed",
            started_at_ms: Date.now() - 10_000,
            ended_at_ms: Date.now(),
            output: null,
            error: "deploy failed"
          }
        ],
        error: "deploy failed",
        trigger_key: "telegram-user:ship"
      }
    ],
    connectors: [],
    connections: []
  });
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  const workflowList = page.locator('[aria-label="Workflow list"]');
  await expect(page.getByLabel("Workflow search results")).toHaveText("2/2 workflows");
  await expect(workflowList.getByRole("button", { name: /release-pipeline/ })).toBeVisible();
  await expect(workflowList.getByRole("button", { name: /daily-digest/ })).toBeVisible();

  await page.getByLabel("Search workflows").fill("failed deploy");
  await expect(page.getByLabel("Workflow search results")).toHaveText("1/2 workflows");
  await expect(workflowList.getByRole("button", { name: /release-pipeline/ })).toBeVisible();
  await expect(workflowList.getByRole("button", { name: /daily-digest/ })).not.toBeVisible();

  await page.getByLabel("Search workflows").fill("cron digest");
  await expect(page.getByLabel("Workflow search results")).toHaveText("1/2 workflows");
  await expect(workflowList.getByRole("button", { name: /daily-digest/ })).toBeVisible();
  await workflowList.getByRole("button", { name: /daily-digest/ }).click();
  await expect(page.locator(".pf-run-header-label")).toHaveText("Daily digest");

  await page.getByLabel("Search workflows").fill("does-not-exist");
  await expect(page.getByLabel("Workflow search results")).toHaveText("0/2 workflows");
  await expect(workflowList.getByText("No matching workflows.")).toBeVisible();
});

test("pipeline workflow run search filters selected workflow runs", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.setWorkflowSnapshot({
    workflows: [
      {
        schema: "puffer.workflow.v1",
        slug: "release-pipeline",
        enabled: true,
        trigger: { type: "connection", connection_slug: "telegram-user", pattern: "ship" },
        pipeline: {
          name: "Release pipeline",
          working_dir: "/tmp/puffer",
          concurrency: 1,
          nodes: [
            {
              id: "deploy",
              type: "codex",
              agent: "Codex deployer",
              model: "gpt-5.4-codex",
              tools: ["bash", "git"],
              prompt: "Deploy and report release status."
            }
          ]
        }
      }
    ],
    runs: [
      {
        idx: 12,
        workflow_slug: "release-pipeline",
        run_id: "run-release-failed",
        trigger: { text: "ship this" },
        status: "failed",
        started_at_ms: Date.now() - 20_000,
        ended_at_ms: Date.now() - 10_000,
        nodes: [
          {
            id: "deploy",
            status: "failed",
            started_at_ms: Date.now() - 20_000,
            ended_at_ms: Date.now() - 10_000,
            output: null,
            error: "deploy failed"
          }
        ],
        error: "deploy failed",
        trigger_key: "telegram-user:ship"
      },
      {
        idx: 11,
        workflow_slug: "release-pipeline",
        run_id: "run-release-retry",
        trigger: { text: "manual retry" },
        status: "completed",
        started_at_ms: Date.now() - 40_000,
        ended_at_ms: Date.now() - 30_000,
        nodes: [
          {
            id: "deploy",
            status: "completed",
            started_at_ms: Date.now() - 40_000,
            ended_at_ms: Date.now() - 30_000,
            output: "retry deployed",
            error: null
          }
        ],
        error: null,
        trigger_key: "manual:retry"
      }
    ],
    connectors: [],
    connections: []
  });
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  const runList = page.getByLabel("Workflow runs");
  await expect(page.getByLabel("Workflow run search results")).toHaveText("2/2 runs");
  await expect(runList.getByRole("button", { name: /#12/ })).toBeVisible();
  await expect(runList.getByRole("button", { name: /#11/ })).toBeVisible();

  await page.getByLabel("Search workflow runs").fill("failed deploy");
  await expect(page.getByLabel("Workflow run search results")).toHaveText("1/2 runs");
  await expect(runList.getByRole("button", { name: /#12/ })).toBeVisible();
  await expect(runList.getByRole("button", { name: /#11/ })).not.toBeVisible();

  await page.getByLabel("Search workflow runs").fill("manual retry");
  await expect(page.getByLabel("Workflow run search results")).toHaveText("1/2 runs");
  const retryRun = runList.getByRole("button", { name: /#11/ });
  await expect(retryRun).toBeVisible();
  await retryRun.click();
  await expect(page.locator(".pf-pipe-traj-list")).toContainText("retry deployed");

  await page.getByLabel("Search workflow runs").fill("does-not-exist");
  await expect(page.getByLabel("Workflow run search results")).toHaveText("0/2 runs");
  await expect(runList.getByText("No matching runs.")).toBeVisible();
  await expect(page.locator(".pf-pipe-traj-list")).toContainText("retry deployed");
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
  await expect(page.locator(".pf-editor-config").getByLabel("Pattern", { exact: true })).toHaveValue(".*");
  await page.locator(".pf-editor-config").getByLabel("Pattern", { exact: true }).fill("hi");

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
    trigger?: { type?: string; connection_slug?: string; pattern?: string };
    pipeline?: { name?: string };
  };
  expect(workflow.enabled).toBe(false);
  expect(workflow.trigger).toMatchObject({ type: "connection", connection_slug: "telegram-user", pattern: "hi" });
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
  const draftButton = page.getByRole("button", { name: "Create workflow draft for telegram-login" });
  await expect(draftButton).toHaveAttribute("title", "/workflows new telegram-user-workflow telegram-user");
  await draftButton.click();

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

test("pipeline connector search matches workflow draft commands", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  const catalog = page.locator('[aria-label="Connector catalog"]');
  const connections = page.locator('[aria-label="Connections"]');
  const resultSummary = page.getByLabel("Connector search results");

  await page.getByLabel("Search connectors").fill("draft /workflows new telegram-user");
  await expect(resultSummary).toHaveText("1/30 connectors; 1/2 connections");
  await expect(catalog.getByRole("button", { name: "Plan telegram-login workflow trigger" })).toBeVisible();
  await expect(connections.getByRole("button", { name: "Use telegram-user as workflow trigger" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Create workflow draft for telegram-user" })).toHaveAttribute(
    "title",
    "/workflows new telegram-user-workflow telegram-user"
  );

  await page.getByLabel("Search connectors").fill("draft /workflows new email-workflow email");
  await expect(resultSummary).toHaveText("1/30 connectors; 0/2 connections");
  await expect(catalog.getByRole("button", { name: "Plan email workflow trigger" })).toBeVisible();
});

test("pipeline selected connector exposes a copyable workflow draft command", async ({ page }) => {
  await page.addInitScript(() => {
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: {
        writeText: async (text: string) => {
          (window as Window & { __copiedWorkflowCommand?: string }).__copiedWorkflowCommand = text;
        }
      }
    });
  });

  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  await page.getByLabel("Search connectors").fill("email events");
  await page.locator('[aria-label="Connector catalog"]').getByRole("button", { name: "Plan email workflow trigger" }).click();
  await page.getByLabel("Connector connection name").fill("email-personal");
  await page.getByLabel("Workflow draft pattern").fill("hello world");

  const draftCommand = page.getByLabel("Selected workflow draft command");
  await expect(draftCommand).toContainText("/workflows new email-personal-workflow email-personal 'hello world'");
  await draftCommand.getByRole("button", { name: "Copy workflow draft command" }).click();
  await expect(page.locator(".pf-pipe-save-note")).toContainText(
    "Copied /workflows new email-personal-workflow email-personal 'hello world'."
  );

  const copied = await page.evaluate(() => (window as Window & { __copiedWorkflowCommand?: string }).__copiedWorkflowCommand);
  expect(copied).toBe("/workflows new email-personal-workflow email-personal 'hello world'");
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
  await page.getByLabel("Workflow draft pattern").fill("hi");
  await page.getByRole("button", { name: "Create workflow draft for selected connector" }).click();

  await expect(page.locator(".pf-pipe-save-note")).toContainText("Run /connect email email-personal before enabling it");
  await expect(page.locator(".pf-editor-config").getByLabel("Name", { exact: true })).toHaveValue("Email Personal workflow");
  await expect(page.locator(".pf-editor-config").getByLabel("Slug")).toHaveValue("email-personal-workflow");
  await expect(page.getByLabel("Trigger type")).toHaveValue("connection");
  await expect(page.getByLabel("Workflow connection")).toHaveValue("email-personal");
  await expect(page.locator(".pf-editor-config").getByLabel("Pattern", { exact: true })).toHaveValue("hi");

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
    pattern: "hi"
  });
});

test("pipeline selected connector can create an append workflow binding", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  const catalog = page.locator('[aria-label="Connector catalog"]');
  await page.getByLabel("Search connectors").fill("email events");
  await catalog.getByRole("button", { name: "Plan email workflow trigger" }).click();
  await page.getByLabel("Connector connection name").fill("email-personal");
  await page.getByLabel("Workflow draft pattern").fill("hi");
  await page.getByLabel("Append file path").fill("/tmp/hi");
  await page.getByRole("button", { name: "Create append workflow for selected connector" }).click();

  const request = await daemon.waitForRequest("workflow_binding_create", (candidate) => {
    return candidate.params.slug === "append-email-personal-hi";
  });
  expect(request.params).toMatchObject({
    slug: "append-email-personal-hi",
    connection_slug: "email-personal",
    connector_slug: "email",
    pattern: "hi",
    file_append_path: "/tmp/hi",
    enabled: true
  });

  await expect(page.locator(".pf-pipe-save-note")).toContainText("Created append workflow append-email-personal-hi.");
  const actions = page.getByLabel("Workflow actions");
  await expect(actions).toContainText("append-email-personal-hi");
  await expect(actions).toContainText("email-personal");
  await expect(actions).toContainText("/tmp/hi");
  await expect(actions).toContainText("hi");
  await expect(page.getByLabel("Workflow action search results")).toHaveText("1/1 actions");

  await page.getByLabel("Search connectors").fill("delete append-email-personal-hi");
  await expect(actions).toContainText("append-email-personal-hi");
  await expect(page.getByLabel("Workflow action search results")).toHaveText("1/1 actions");

  await actions.getByRole("button", { name: "Delete workflow action append-email-personal-hi" }).click();
  const deleteRequest = await daemon.waitForRequest("workflow_binding_delete", (candidate) => {
    return candidate.params.slug === "append-email-personal-hi";
  });
  expect(deleteRequest.params.slug).toBe("append-email-personal-hi");
  await expect(page.locator(".pf-pipe-save-note")).toContainText("Deleted append-email-personal-hi.");
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
    "alertmanager-webhook",
    "asana-webhook",
    "datadog-webhook",
    "newrelic-webhook",
    "opsgenie-webhook",
    "azure-devops-webhook",
    "bitbucket-webhook",
    "figma-webhook",
    "github-webhook",
    "grafana-webhook",
    "gitlab-webhook",
    "jira-webhook",
    "linear-webhook",
    "pagerduty-webhook",
    "sentry-webhook",
    "shopify-webhook",
    "stripe-webhook",
    "trello-webhook",
    "vercel-webhook",
    "webhook"
  ];

  await expect(resultSummary).toHaveText("30/30 connectors; 2/2 connections");
  for (const slug of connectorSlugs) {
    await expect(catalog).toContainText(slug);
  }

  await page.getByLabel("Search connectors").fill("workspace local session");
  await expect(resultSummary).toHaveText("1/30 connectors; 0/2 connections");
  await expect(catalog.getByRole("button", { name: "Select slack-login connector setup" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Select slack-app connector setup" })).not.toBeVisible();

  await page.getByLabel("Search connectors").fill("serve webhook");
  await expect(resultSummary).toHaveText("20/30 connectors; 0/2 connections");
  await expect(catalog.getByRole("button", { name: "Select alertmanager-webhook connector setup" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Select asana-webhook connector setup" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Select datadog-webhook connector setup" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Select newrelic-webhook connector setup" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Select opsgenie-webhook connector setup" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Select azure-devops-webhook connector setup" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Select bitbucket-webhook connector setup" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Select figma-webhook connector setup" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Select github-webhook connector setup" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Select grafana-webhook connector setup" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Select gitlab-webhook connector setup" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Select jira-webhook connector setup" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Select linear-webhook connector setup" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Select pagerduty-webhook connector setup" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Select sentry-webhook connector setup" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Select shopify-webhook connector setup" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Select stripe-webhook connector setup" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Select trello-webhook connector setup" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Select vercel-webhook connector setup" })).toBeVisible();
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
  await expect(resultSummary).toHaveText("1/30 connectors; 1/2 connections");
  const telegram = catalog.getByRole("button", { name: "Plan telegram-login workflow trigger" });
  await expect(telegram).toContainText("conn:telegram-user");
  await expect(catalog.getByRole("button", { name: "Select slack-app connector setup" })).not.toBeVisible();

  await page.getByLabel("Search connectors").fill("workspace slack-app");
  await expect(resultSummary).toHaveText("1/30 connectors; 1/2 connections");
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
  await expect(resultSummary).toHaveText("23/30 connectors; 0/2 connections");
  await expect(catalog.getByRole("button", { name: "Select alertmanager-webhook connector setup" })).toContainText("serve");
  await expect(catalog.getByRole("button", { name: "Select asana-webhook connector setup" })).toContainText("serve");
  await expect(catalog.getByRole("button", { name: "Select datadog-webhook connector setup" })).toContainText("serve");
  await expect(catalog.getByRole("button", { name: "Select newrelic-webhook connector setup" })).toContainText("serve");
  await expect(catalog.getByRole("button", { name: "Select opsgenie-webhook connector setup" })).toContainText("serve");
  await expect(catalog.getByRole("button", { name: "Select azure-devops-webhook connector setup" })).toContainText("serve");
  await expect(catalog.getByRole("button", { name: "Select bitbucket-webhook connector setup" })).toContainText("serve");
  await expect(catalog.getByRole("button", { name: "Select figma-webhook connector setup" })).toContainText("serve");
  await expect(catalog.getByRole("button", { name: "Select github-webhook connector setup" })).toContainText("serve");
  await expect(catalog.getByRole("button", { name: "Select grafana-webhook connector setup" })).toContainText("serve");
  await expect(catalog.getByRole("button", { name: "Select gitlab-webhook connector setup" })).toContainText("serve");
  await expect(catalog.getByRole("button", { name: "Select jira-webhook connector setup" })).toContainText("serve");
  await expect(catalog.getByRole("button", { name: "Select linear-webhook connector setup" })).toContainText("serve");
  await expect(catalog.getByRole("button", { name: "Select pagerduty-webhook connector setup" })).toContainText("serve");
  await expect(catalog.getByRole("button", { name: "Select sentry-webhook connector setup" })).toContainText("serve");
  await expect(catalog.getByRole("button", { name: "Select shopify-webhook connector setup" })).toContainText("serve");
  await expect(catalog.getByRole("button", { name: "Select stripe-webhook connector setup" })).toContainText("serve");
  await expect(catalog.getByRole("button", { name: "Select trello-webhook connector setup" })).toContainText("serve");
  await expect(catalog.getByRole("button", { name: "Select vercel-webhook connector setup" })).toContainText("serve");
  await expect(catalog.getByRole("button", { name: "Select webhook connector setup" })).toContainText("serve");
  await expect(catalog.getByRole("button", { name: "Select discord-bot connector setup" })).toContainText("serve");
  await expect(catalog.getByRole("button", { name: "Select slack-app connector setup" })).not.toBeVisible();

  await page.getByLabel("Search connectors").fill("subscriber telegram");
  await expect(resultSummary).toHaveText("1/30 connectors; 1/2 connections");
  await expect(catalog.getByRole("button", { name: "Plan telegram-login workflow trigger" })).toContainText("subscriber");
  await expect(connections.getByRole("button", { name: "Use telegram-user as workflow trigger" })).toContainText("subscriber");
});

test("pipeline connector catalog exposes Linear setup details", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  await page.getByLabel("Search connectors").fill("linear issue webhook");
  const catalog = page.locator('[aria-label="Connector catalog"]');
  const connector = catalog.getByRole("button", { name: "Select linear-webhook connector setup" });
  await expect(connector).toContainText("serve");
  await expect(connector).toContainText("no trigger");
  await connector.click();

  const details = page.getByLabel("Selected connector details");
  await expect(details).toContainText("linear-webhook");
  await expect(details).toContainText("skill:linear-webhook");
  await expect(details).toContainText("serve");
  await expect(page.getByLabel("Selected connector command")).toContainText("/connect linear-webhook linear-webhook");
});

test("pipeline connector catalog exposes Alertmanager setup details", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  await page.getByLabel("Search connectors").fill("prometheus alertmanager webhook");
  const catalog = page.locator('[aria-label="Connector catalog"]');
  const connector = catalog.getByRole("button", { name: "Select alertmanager-webhook connector setup" });
  await expect(connector).toContainText("serve");
  await expect(connector).toContainText("no trigger");
  await connector.click();

  const details = page.getByLabel("Selected connector details");
  await expect(details).toContainText("alertmanager-webhook");
  await expect(details).toContainText("skill:alertmanager-webhook");
  await expect(details).toContainText("serve");
  await expect(page.getByLabel("Selected connector command")).toContainText(
    "/connect alertmanager-webhook alertmanager-webhook"
  );
});

test("pipeline connector catalog exposes Datadog setup details", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  await page.getByLabel("Search connectors").fill("datadog monitor webhook");
  const catalog = page.locator('[aria-label="Connector catalog"]');
  const connector = catalog.getByRole("button", { name: "Select datadog-webhook connector setup" });
  await expect(connector).toContainText("serve");
  await expect(connector).toContainText("no trigger");
  await connector.click();

  const details = page.getByLabel("Selected connector details");
  await expect(details).toContainText("datadog-webhook");
  await expect(details).toContainText("skill:datadog-webhook");
  await expect(details).toContainText("serve");
  await expect(page.getByLabel("Selected connector command")).toContainText("/connect datadog-webhook datadog-webhook");
});

test("pipeline connector catalog exposes New Relic setup details", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  await page.getByLabel("Search connectors").fill("new relic alert webhook");
  const catalog = page.locator('[aria-label="Connector catalog"]');
  const connector = catalog.getByRole("button", { name: "Select newrelic-webhook connector setup" });
  await expect(connector).toContainText("serve");
  await expect(connector).toContainText("no trigger");
  await connector.click();

  const details = page.getByLabel("Selected connector details");
  await expect(details).toContainText("newrelic-webhook");
  await expect(details).toContainText("skill:newrelic-webhook");
  await expect(details).toContainText("serve");
  await expect(page.getByLabel("Selected connector command")).toContainText(
    "/connect newrelic-webhook newrelic-webhook"
  );
});

test("pipeline connector catalog exposes Opsgenie setup details", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  await page.getByLabel("Search connectors").fill("opsgenie alert action webhook");
  const catalog = page.locator('[aria-label="Connector catalog"]');
  const connector = catalog.getByRole("button", { name: "Select opsgenie-webhook connector setup" });
  await expect(connector).toContainText("serve");
  await expect(connector).toContainText("no trigger");
  await connector.click();

  const details = page.getByLabel("Selected connector details");
  await expect(details).toContainText("opsgenie-webhook");
  await expect(details).toContainText("skill:opsgenie-webhook");
  await expect(details).toContainText("serve");
  await expect(page.getByLabel("Selected connector command")).toContainText("/connect opsgenie-webhook opsgenie-webhook");
});

test("pipeline connector catalog exposes Azure DevOps setup details", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  await page.getByLabel("Search connectors").fill("azure devops work item webhook");
  const catalog = page.locator('[aria-label="Connector catalog"]');
  const connector = catalog.getByRole("button", { name: "Select azure-devops-webhook connector setup" });
  await expect(connector).toContainText("serve");
  await expect(connector).toContainText("no trigger");
  await connector.click();

  const details = page.getByLabel("Selected connector details");
  await expect(details).toContainText("azure-devops-webhook");
  await expect(details).toContainText("skill:azure-devops-webhook");
  await expect(details).toContainText("serve");
  await expect(page.getByLabel("Selected connector command")).toContainText(
    "/connect azure-devops-webhook azure-devops-webhook"
  );
});

test("pipeline connector catalog exposes Bitbucket setup details", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  await page.getByLabel("Search connectors").fill("bitbucket pull request webhook");
  const catalog = page.locator('[aria-label="Connector catalog"]');
  const connector = catalog.getByRole("button", { name: "Select bitbucket-webhook connector setup" });
  await expect(connector).toContainText("serve");
  await expect(connector).toContainText("no trigger");
  await connector.click();

  const details = page.getByLabel("Selected connector details");
  await expect(details).toContainText("bitbucket-webhook");
  await expect(details).toContainText("skill:bitbucket-webhook");
  await expect(details).toContainText("serve");
  await expect(page.getByLabel("Selected connector command")).toContainText("/connect bitbucket-webhook bitbucket-webhook");
});

test("pipeline connector catalog exposes Figma setup details", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  await page.getByLabel("Search connectors").fill("figma comment dev mode webhook");
  const catalog = page.locator('[aria-label="Connector catalog"]');
  const connector = catalog.getByRole("button", { name: "Select figma-webhook connector setup" });
  await expect(connector).toContainText("serve");
  await expect(connector).toContainText("no trigger");
  await connector.click();

  const details = page.getByLabel("Selected connector details");
  await expect(details).toContainText("figma-webhook");
  await expect(details).toContainText("skill:figma-webhook");
  await expect(details).toContainText("serve");
  await expect(page.getByLabel("Selected connector command")).toContainText("/connect figma-webhook figma-webhook");
});

test("pipeline connector catalog exposes Grafana setup details", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  await page.getByLabel("Search connectors").fill("grafana alert webhook");
  const catalog = page.locator('[aria-label="Connector catalog"]');
  const connector = catalog.getByRole("button", { name: "Select grafana-webhook connector setup" });
  await expect(connector).toContainText("serve");
  await expect(connector).toContainText("no trigger");
  await connector.click();

  const details = page.getByLabel("Selected connector details");
  await expect(details).toContainText("grafana-webhook");
  await expect(details).toContainText("skill:grafana-webhook");
  await expect(details).toContainText("serve");
  await expect(page.getByLabel("Selected connector command")).toContainText("/connect grafana-webhook grafana-webhook");
});

test("pipeline connector catalog exposes PagerDuty setup details", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  await page.getByLabel("Search connectors").fill("pagerduty incident webhook");
  const catalog = page.locator('[aria-label="Connector catalog"]');
  const connector = catalog.getByRole("button", { name: "Select pagerduty-webhook connector setup" });
  await expect(connector).toContainText("serve");
  await expect(connector).toContainText("no trigger");
  await connector.click();

  const details = page.getByLabel("Selected connector details");
  await expect(details).toContainText("pagerduty-webhook");
  await expect(details).toContainText("skill:pagerduty-webhook");
  await expect(details).toContainText("serve");
  await expect(page.getByLabel("Selected connector command")).toContainText("/connect pagerduty-webhook pagerduty-webhook");
});

test("pipeline connector catalog exposes Sentry setup details", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  await page.getByLabel("Search connectors").fill("sentry issue alert webhook");
  const catalog = page.locator('[aria-label="Connector catalog"]');
  const connector = catalog.getByRole("button", { name: "Select sentry-webhook connector setup" });
  await expect(connector).toContainText("serve");
  await expect(connector).toContainText("no trigger");
  await connector.click();

  const details = page.getByLabel("Selected connector details");
  await expect(details).toContainText("sentry-webhook");
  await expect(details).toContainText("skill:sentry-webhook");
  await expect(details).toContainText("serve");
  await expect(page.getByLabel("Selected connector command")).toContainText("/connect sentry-webhook sentry-webhook");
});

test("pipeline connector catalog exposes Asana setup details", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  await page.getByLabel("Search connectors").fill("asana task project webhook");
  const catalog = page.locator('[aria-label="Connector catalog"]');
  const connector = catalog.getByRole("button", { name: "Select asana-webhook connector setup" });
  await expect(connector).toContainText("serve");
  await expect(connector).toContainText("no trigger");
  await connector.click();

  const details = page.getByLabel("Selected connector details");
  await expect(details).toContainText("asana-webhook");
  await expect(details).toContainText("skill:asana-webhook");
  await expect(details).toContainText("serve");
  await expect(page.getByLabel("Selected connector command")).toContainText("/connect asana-webhook asana-webhook");
});

test("pipeline connector catalog exposes GitLab setup details", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  await page.getByLabel("Search connectors").fill("gitlab merge request webhook");
  const catalog = page.locator('[aria-label="Connector catalog"]');
  const connector = catalog.getByRole("button", { name: "Select gitlab-webhook connector setup" });
  await expect(connector).toContainText("serve");
  await expect(connector).toContainText("no trigger");
  await connector.click();

  const details = page.getByLabel("Selected connector details");
  await expect(details).toContainText("gitlab-webhook");
  await expect(details).toContainText("skill:gitlab-webhook");
  await expect(details).toContainText("serve");
  await expect(page.getByLabel("Selected connector command")).toContainText("/connect gitlab-webhook gitlab-webhook");
});

test("pipeline connector catalog exposes Jira setup details", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  await page.getByLabel("Search connectors").fill("jira issue comment webhook");
  const catalog = page.locator('[aria-label="Connector catalog"]');
  const connector = catalog.getByRole("button", { name: "Select jira-webhook connector setup" });
  await expect(connector).toContainText("serve");
  await expect(connector).toContainText("no trigger");
  await connector.click();

  const details = page.getByLabel("Selected connector details");
  await expect(details).toContainText("jira-webhook");
  await expect(details).toContainText("skill:jira-webhook");
  await expect(details).toContainText("serve");
  await expect(page.getByLabel("Selected connector command")).toContainText("/connect jira-webhook jira-webhook");
});

test("pipeline connector catalog exposes Stripe setup details", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  await page.getByLabel("Search connectors").fill("stripe invoice payment webhook");
  const catalog = page.locator('[aria-label="Connector catalog"]');
  const connector = catalog.getByRole("button", { name: "Select stripe-webhook connector setup" });
  await expect(connector).toContainText("serve");
  await expect(connector).toContainText("no trigger");
  await connector.click();

  const details = page.getByLabel("Selected connector details");
  await expect(details).toContainText("stripe-webhook");
  await expect(details).toContainText("skill:stripe-webhook");
  await expect(details).toContainText("serve");
  await expect(page.getByLabel("Selected connector command")).toContainText("/connect stripe-webhook stripe-webhook");
});

test("pipeline connector catalog exposes Shopify setup details", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  await page.getByLabel("Search connectors").fill("shopify order product webhook");
  const catalog = page.locator('[aria-label="Connector catalog"]');
  const connector = catalog.getByRole("button", { name: "Select shopify-webhook connector setup" });
  await expect(connector).toContainText("serve");
  await expect(connector).toContainText("no trigger");
  await connector.click();

  const details = page.getByLabel("Selected connector details");
  await expect(details).toContainText("shopify-webhook");
  await expect(details).toContainText("skill:shopify-webhook");
  await expect(details).toContainText("serve");
  await expect(page.getByLabel("Selected connector command")).toContainText("/connect shopify-webhook shopify-webhook");
});

test("pipeline connector catalog exposes Trello setup details", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  await page.getByLabel("Search connectors").fill("trello board card webhook");
  const catalog = page.locator('[aria-label="Connector catalog"]');
  const connector = catalog.getByRole("button", { name: "Select trello-webhook connector setup" });
  await expect(connector).toContainText("serve");
  await expect(connector).toContainText("no trigger");
  await connector.click();

  const details = page.getByLabel("Selected connector details");
  await expect(details).toContainText("trello-webhook");
  await expect(details).toContainText("skill:trello-webhook");
  await expect(details).toContainText("serve");
  await expect(page.getByLabel("Selected connector command")).toContainText("/connect trello-webhook trello-webhook");
});

test("pipeline connector catalog exposes Vercel setup details", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  await page.getByLabel("Search connectors").fill("vercel deployment project webhook");
  const catalog = page.locator('[aria-label="Connector catalog"]');
  const connector = catalog.getByRole("button", { name: "Select vercel-webhook connector setup" });
  await expect(connector).toContainText("serve");
  await expect(connector).toContainText("no trigger");
  await connector.click();

  const details = page.getByLabel("Selected connector details");
  await expect(details).toContainText("vercel-webhook");
  await expect(details).toContainText("skill:vercel-webhook");
  await expect(details).toContainText("serve");
  await expect(page.getByLabel("Selected connector command")).toContainText("/connect vercel-webhook vercel-webhook");
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
  await expect(resultSummary).toHaveText("2/30 connectors; 1/2 connections");
  await expect(filters.getByRole("button", { name: "Trigger", exact: true })).toHaveAttribute("aria-pressed", "true");

  await filters.getByRole("button", { name: "Draft" }).click();
  await expect(page.getByLabel("Search connectors")).toHaveValue("draft");
  await expect(resultSummary).toHaveText("2/30 connectors; 1/2 connections");
  await expect(filters.getByRole("button", { name: "Draft" })).toHaveAttribute("aria-pressed", "true");

  await filters.getByRole("button", { name: "Append" }).click();
  await expect(page.getByLabel("Search connectors")).toHaveValue("append");
  await expect(resultSummary).toHaveText("2/30 connectors; 1/2 connections");
  await expect(filters.getByRole("button", { name: "Append" })).toHaveAttribute("aria-pressed", "true");

  await filters.getByRole("button", { name: "Monitor" }).click();
  await expect(page.getByLabel("Search connectors")).toHaveValue("monitor");
  await expect(resultSummary).toHaveText("1/30 connectors; 1/2 connections");
  await expect(page.locator('[aria-label="Connector catalog"]')).toContainText("datadog-webhook");
  await expect(page.locator('[aria-label="Connections"]')).toContainText("telegram-user");

  await filters.getByRole("button", { name: "Tasks" }).click();
  await expect(page.getByLabel("Search connectors")).toHaveValue("monitor task");
  await expect(resultSummary).toHaveText("0/30 connectors; 0/2 connections");
  await expect(page.getByLabel("Monitor task search results")).toHaveText("1/1 monitor tasks");
  await expect(page.getByLabel("Monitor tasks")).toContainText("Reply to Telegram support ping");

  await filters.getByRole("button", { name: "Repair" }).click();
  await expect(page.getByLabel("Search connectors")).toHaveValue("repair");
  await expect(resultSummary).toHaveText("0/30 connectors; 2/2 connections");

  await filters.getByRole("button", { name: "Active" }).click();
  await expect(page.getByLabel("Search connectors")).toHaveValue("active");
  await expect(resultSummary).toHaveText("0/30 connectors; 1/2 connections");
  await expect(page.locator('[aria-label="Connections"]')).toContainText("telegram-user");

  await filters.getByRole("button", { name: "Idle" }).click();
  await expect(page.getByLabel("Search connectors")).toHaveValue("idle");
  await expect(resultSummary).toHaveText("0/30 connectors; 1/2 connections");
  await expect(page.locator('[aria-label="Connections"]')).toContainText("slack-app");

  await filters.getByRole("button", { name: "Actions" }).click();
  await expect(page.getByLabel("Search connectors")).toHaveValue("has-actions");
  await expect(resultSummary).toHaveText("7/30 connectors; 2/2 connections");

  await filters.getByRole("button", { name: "Serve" }).click();
  await expect(page.getByLabel("Search connectors")).toHaveValue("serve");
  await expect(resultSummary).toHaveText("23/30 connectors; 0/2 connections");

  await filters.getByRole("button", { name: "All" }).click();
  await expect(page.getByLabel("Search connectors")).toHaveValue("");
  await expect(resultSummary).toHaveText("30/30 connectors; 2/2 connections");
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
  await expect(resultSummary).toHaveText("28/30 connectors; 1/2 connections");
  await expect(catalog.getByRole("button", { name: "Select slack-app connector setup" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Select webhook connector setup" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Plan telegram-login workflow trigger" })).not.toBeVisible();
  await expect(connections.getByRole("button", { name: "slack-app cannot start workflow triggers" })).toBeVisible();
  await expect(connections.getByRole("button", { name: "Use telegram-user as workflow trigger" })).not.toBeVisible();

  await page.getByLabel("Search connectors").fill("setup-only webhook");
  await expect(resultSummary).toHaveText("20/30 connectors; 0/2 connections");
  await expect(catalog.getByRole("button", { name: "Select alertmanager-webhook connector setup" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Select asana-webhook connector setup" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Select datadog-webhook connector setup" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Select newrelic-webhook connector setup" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Select opsgenie-webhook connector setup" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Select azure-devops-webhook connector setup" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Select bitbucket-webhook connector setup" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Select figma-webhook connector setup" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Select github-webhook connector setup" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Select grafana-webhook connector setup" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Select gitlab-webhook connector setup" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Select jira-webhook connector setup" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Select linear-webhook connector setup" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Select pagerduty-webhook connector setup" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Select sentry-webhook connector setup" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Select shopify-webhook connector setup" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Select stripe-webhook connector setup" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Select trello-webhook connector setup" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Select vercel-webhook connector setup" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Select webhook connector setup" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Select slack-app connector setup" })).not.toBeVisible();
});

test("pipeline connector search matches append workflow commands", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  await page.getByLabel("Search connectors").fill("append /tmp/email.log");

  const catalog = page.locator('[aria-label="Connector catalog"]');
  await expect(page.getByLabel("Connector search results")).toHaveText("1/30 connectors; 0/2 connections");
  await expect(catalog.getByRole("button", { name: "Plan email workflow trigger" })).toBeVisible();
  await expect(catalog.getByRole("button", { name: "Plan telegram-login workflow trigger" })).not.toBeVisible();
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

test("pipeline connector catalog expands action chips for unique connector searches", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  await page.getByLabel("Search connectors").fill("telegram-login");

  const catalog = page.locator('[aria-label="Connector catalog"]');
  const telegram = catalog.getByRole("button", { name: "Plan telegram-login workflow trigger" });

  await expect(page.getByLabel("Connector search results")).toHaveText("1/30 connectors; 1/2 connections");
  await expect(telegram).toContainText("send_message");
  await expect(telegram).toContainText("edit_message");
  await expect(telegram).toContainText("delete_messages");
  await expect(telegram).toContainText("vote_poll");
  await expect(telegram).not.toContainText("+1 actions");
});

test("pipeline selected connector detail shows all connector actions", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  await page.getByLabel("Search connectors").fill("telegram-login");
  await page
    .getByLabel("Connector catalog")
    .getByRole("button", { name: "Plan telegram-login workflow trigger" })
    .click();

  const detail = page.getByLabel("Selected connector details");
  await expect(detail).toContainText("send_message");
  await expect(detail).toContainText("edit_message");
  await expect(detail).toContainText("delete_messages");
  await expect(detail).toContainText("vote_poll");
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
  await expect(page.getByLabel("Selected append workflow command")).toContainText("/workflows append email /tmp/hi --connector email");
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

  await page.getByLabel("Search connectors").fill("github event webhook");
  const connector = page
    .locator('[aria-label="Connector catalog"]')
    .getByRole("button", { name: "Select github-webhook connector setup" });
  await expect(connector).toContainText("serve");
  await expect(connector).toContainText("no trigger");

  const runButton = page.getByRole("button", { name: "Run /connect github-webhook github-webhook" });
  await expect(runButton).toHaveAttribute("title", "/connect github-webhook github-webhook");
  await runButton.click();

  const request = await daemon.waitForRequest(
    "run_agent_turn",
    (candidate) => candidate.params.message === "/connect github-webhook github-webhook"
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

  await monitors.getByRole("button", { name: "Delete monitor workflow monitor-telegram-user" }).click();
  const deleteRequest = await daemon.waitForRequest(
    "workflow_binding_delete",
    (candidate) => candidate.params.slug === "monitor-telegram-user"
  );
  expect(deleteRequest.params.slug).toBe("monitor-telegram-user");
  await expect(page.getByLabel("Monitor workflows")).toHaveCount(0);
});

test("pipeline monitor task panel exposes task actions", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  const tasks = page.getByLabel("Monitor tasks");
  await expect(tasks).toContainText("Reply to Telegram support ping");
  await expect(tasks).toContainText("telegram-user");
  await expect(tasks).toContainText("Alice asked whether the deployment is finished.");

  await page.getByLabel("Search connectors").fill("support ping");
  await expect(tasks).toContainText("1/1");
  await expect(page.getByLabel("Monitor task search results")).toHaveText("1/1 monitor tasks");
  await expect(tasks).toContainText("Draft reply");
  await expect(tasks).toContainText("Open context");
  await expect(tasks).toContainText("Escalate owner");
  await expect(tasks).toContainText("already answered in thread");
  await expect(tasks).toContainText("not actionable");
  await expect(tasks).toContainText("3 actions");
  await expect(tasks).toContainText("3 ignores");

  await tasks.getByRole("button", { name: "Run monitor action monitor-1 Escalate owner" }).click();
  const actionRequest = await daemon.waitForRequest(
    "run_agent_turn",
    (candidate) =>
      String(candidate.params.message ?? "").startsWith("Act on monitored task monitor-1:")
      && String(candidate.params.message ?? "").includes("Escalate the deployment question")
  );
  expect(String(actionRequest.params.sessionId ?? "")).not.toHaveLength(0);
});

test("pipeline monitor task panel can start ignore flows", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.locator(".pf-sidebar").getByRole("button", { name: "Pipelines" }).click();

  const tasks = page.getByLabel("Monitor tasks");
  await page.getByLabel("Search connectors").fill("already answered");
  await expect(page.getByLabel("Monitor task search results")).toHaveText("1/1 monitor tasks");
  await expect(tasks.getByRole("button", { name: "Ignore monitor-1 already answered in thread" })).toBeVisible();

  await page.getByLabel("Search connectors").fill("support ping");
  await expect(tasks.getByRole("button", { name: "Ignore monitor-1 duplicate support ping" })).toBeVisible();
  await tasks.getByRole("button", { name: "Ignore monitor-1 not actionable" }).click();
  const ignoreRequest = await daemon.waitForRequest(
    "run_agent_turn",
    (candidate) => candidate.params.message === "/tasks ignore monitor-1 not actionable"
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

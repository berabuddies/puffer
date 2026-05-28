import { expect, test, type Page } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";

async function openWorkflows(page: Page) {
  await page.locator(".pf-sidebar").getByRole("button", { name: "Workflows" }).click();
}

async function openWorkflowDetail(page: Page, workflowName: string | RegExp = /agent-review-workflow/) {
  await openWorkflows(page);
  const backToOverview = page.getByRole("button", { name: "Back to workflows" });
  if (await backToOverview.isVisible()) {
    await backToOverview.click();
  }
  await page.getByLabel("Workflow list").getByRole("button", { name: workflowName }).click();
  await expect(page.getByRole("button", { name: "Back to workflows" })).toBeVisible();
}

test("workflow agent provider switcher exposes selected provider state", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openWorkflowDetail(page);

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

test("workflow graph agent nodes expose selected state", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openWorkflowDetail(page);

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

test("workflow list search filters by workflow and run metadata", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.setWorkflowSnapshot({
    workflows: [
      {
        schema: "puffer.workflow.v1",
        slug: "release-workflow",
        enabled: true,
        trigger: { type: "connection", connection_slug: "telegram-user", pattern: "ship" },
        pipeline: {
          name: "Release workflow",
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
        workflow_slug: "release-workflow",
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

  await openWorkflows(page);

  const workflowList = page.locator('[aria-label="Workflow list"]');
  await expect(page.getByLabel("Workflow search results")).toHaveText("2/2 workflows");
  await expect(workflowList.getByRole("button", { name: /release-workflow/ })).toBeVisible();
  await expect(workflowList.getByRole("button", { name: /daily-digest/ })).toBeVisible();

  await page.getByLabel("Search workflows").fill("failed deploy");
  await expect(page.getByLabel("Workflow search results")).toHaveText("1/2 workflows");
  await expect(workflowList.getByRole("button", { name: /release-workflow/ })).toBeVisible();
  await expect(workflowList.getByRole("button", { name: /daily-digest/ })).not.toBeVisible();

  await page.getByLabel("Search workflows").fill("cron digest");
  await expect(page.getByLabel("Workflow search results")).toHaveText("1/2 workflows");
  await expect(workflowList.getByRole("button", { name: /daily-digest/ })).toBeVisible();
  await workflowList.getByRole("button", { name: /daily-digest/ }).click();
  await expect(page.locator(".pf-run-header-label")).toHaveText("Daily digest");

  await page.getByRole("button", { name: "Back to workflows" }).click();
  await expect(page.getByLabel("Workflow list")).toBeVisible();
  await page.getByLabel("Search workflows").fill("does-not-exist");
  await expect(page.getByLabel("Workflow search results")).toHaveText("0/2 workflows");
  await expect(workflowList.getByText("No matching workflows.")).toBeVisible();
});

test("workflow overview and create pages keep focused headers", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openWorkflows(page);

  const title = page.locator(".pf-pipe-top-id");
  await expect(title).toContainText("Workflows");
  await expect(title).not.toContainText("agent-review-workflow");

  await page.getByRole("button", { name: "New workflow" }).click();

  await expect(title).toContainText("Create workflow");
  await expect(title).toContainText("workflow-draft");
  await expect(page.getByRole("button", { name: "New workflow" })).toHaveCount(0);
  const toolbar = page.locator(".pf-canvas-toolbar");
  await expect(toolbar.getByRole("button", { name: "Add Codex agent" })).toBeVisible();
  await expect(toolbar.getByRole("button", { name: "Add Claude agent" })).toBeVisible();
  await expect(toolbar.getByRole("button", { name: "Add Puffer agent" })).toBeVisible();
  await expect(toolbar.getByRole("button", { name: "Add tool call node" })).toBeVisible();
  await expect(toolbar.getByRole("button", { name: "Add merge node" })).toBeVisible();
  await expect(toolbar.getByRole("button", { name: "Add fanout node" })).toBeVisible();
});

test("workflow overview opens workflow and run details on separate pages", async ({ page }) => {
  const daemon = new FakeDaemon();
  const now = Date.now();
  daemon.setWorkflowSnapshot({
    workflows: [
      {
        schema: "puffer.workflow.v1",
        slug: "release-workflow",
        enabled: true,
        trigger: { type: "connection", connection_slug: "telegram-user", pattern: "ship" },
        pipeline: {
          name: "Release workflow",
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
        enabled: true,
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
              prompt: "Summarize connector activity."
            }
          ]
        }
      }
    ],
    runs: [
      {
        idx: 21,
        workflow_slug: "release-workflow",
        run_id: "run-release-live",
        trigger: { text: "ship this" },
        status: "running",
        started_at_ms: now - 60_000,
        ended_at_ms: null,
        nodes: [
          {
            id: "deploy",
            status: "running",
            started_at_ms: now - 60_000,
            ended_at_ms: null,
            output: "deploying release",
            error: null
          }
        ],
        error: null,
        trigger_key: "telegram-user:ship"
      },
      {
        idx: 20,
        workflow_slug: "daily-digest",
        run_id: "run-digest",
        trigger: { text: "cron" },
        status: "completed",
        started_at_ms: now - 120_000,
        ended_at_ms: now - 100_000,
        nodes: [
          {
            id: "summarize",
            status: "completed",
            started_at_ms: now - 120_000,
            ended_at_ms: now - 100_000,
            output: "digest complete",
            error: null
          }
        ],
        error: null,
        trigger_key: "cron:daily"
      }
    ],
    connectors: [],
    connections: []
  });
  await daemon.install(page);
  await daemon.open(page);

  await openWorkflows(page);

  const title = page.locator(".pf-pipe-top-id");
  await expect(title).toContainText("Workflows");

  const ongoing = page.getByLabel("Ongoing workflows");
  const liveRun = ongoing.getByRole("button", { name: /Release workflow/ });
  await expect(liveRun).toContainText("#21");
  await expect(liveRun).toContainText("running");
  await liveRun.click();

  await expect(title).toContainText("Workflow detail");
  await expect(title).toContainText("Release workflow");
  await expect(title).toContainText("release-workflow");
  await expect(page.getByLabel("Workflow runs").getByRole("button", { name: /#21/ })).toHaveAttribute(
    "data-selected",
    "true"
  );
  await expect(page.locator(".pf-pipe-traj-list")).toContainText("deploying release");

  await page.getByRole("button", { name: "Back to workflows" }).click();
  await expect(title).toContainText("Workflows");
  await expect(page.getByLabel("Workflow list")).toBeVisible();

  await page.getByLabel("Workflow list").getByRole("button", { name: /Daily digest/ }).click();
  await expect(title).toContainText("Workflow detail");
  await expect(title).toContainText("Daily digest");
  await expect(title).toContainText("daily-digest");
  await expect(page.getByRole("button", { name: "Back to workflows" })).toBeVisible();
  await expect(page.getByRole("button", { name: "New workflow" })).toBeVisible();
});

test("workflow run search filters selected workflow runs", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.setWorkflowSnapshot({
    workflows: [
      {
        schema: "puffer.workflow.v1",
        slug: "release-workflow",
        enabled: true,
        trigger: { type: "connection", connection_slug: "telegram-user", pattern: "ship" },
        pipeline: {
          name: "Release workflow",
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
        workflow_slug: "release-workflow",
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
        workflow_slug: "release-workflow",
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

  await openWorkflowDetail(page, /release-workflow/);

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


test("workflow editor saves workflow changes through daemon", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openWorkflowDetail(page);

  const saveButton = page.getByRole("button", { name: "Save workflow" });
  await expect(saveButton).toBeDisabled();

  // The trigger node is always the first node in the graph; click it to open
  // the workflow-setup form in the bottom drawer.
  await page.locator(".pf-pipe-graph .pf-pipe-node").first().click();
  await page.locator(".pf-canvas-selected").getByLabel("Workflow name").fill("Saved monitor workflow");
  await expect(saveButton).toBeEnabled();
  await expect(page.locator(".pf-pipe-save-note")).toContainText("Save to persist");

  await saveButton.click();
  const request = await daemon.waitForRequest("workflow_save");
  const workflow = request.params.workflow as {
    slug?: string;
    pipeline?: { name?: string; nodes?: Array<{ type?: string }> };
  };
  expect(workflow.slug).toBe("agent-review-workflow");
  expect(workflow.pipeline?.name).toBe("Saved monitor workflow");
  expect(workflow.pipeline?.nodes?.[0]?.type).toBe("codex");
  await expect(page.locator(".pf-pipe-save-note")).toContainText("Saved agent-review-workflow.");
  await expect(saveButton).toBeDisabled();
});

test("workflow editor creates new workflow drafts before saving", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openWorkflows(page);

  await page.getByRole("button", { name: "New workflow" }).click();

  await expect(page.locator(".pf-pipe-save-note")).toContainText("Created workflow-draft locally");
  // New drafts auto-select the trigger node so the workflow + trigger form is visible.
  const selected = page.locator(".pf-canvas-selected");
  await expect(selected.getByLabel("Workflow name")).toHaveValue("Workflow draft");
  await expect(selected.getByLabel("Slug")).toHaveValue("workflow-draft");
  await expect(selected.locator(".pf-editor-inline").getByRole("checkbox")).not.toBeChecked();
  await expect(selected.getByLabel("Trigger type")).toHaveValue("connection");
  await expect(selected.getByLabel("Workflow connection")).toHaveValue("telegram-user");
  await expect(selected.getByLabel("Pattern", { exact: true })).toHaveValue(".*");
  await selected.getByLabel("Pattern", { exact: true }).fill("hi");

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

test("workflow editor can pause and resume workflows through daemon", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openWorkflowDetail(page);

  const pauseButton = page.getByRole("button", { name: "Pause workflow" });
  await expect(pauseButton).toBeEnabled();
  await pauseButton.click();

  const pauseRequest = await daemon.waitForRequest(
    "workflow_toggle",
    (candidate) => candidate.params.slug === "agent-review-workflow" && candidate.params.enabled === false
  );
  expect(pauseRequest.params.slug).toBe("agent-review-workflow");
  await expect(page.locator(".pf-run-header-state")).toHaveText("disabled");

  const resumeButton = page.getByRole("button", { name: "Resume workflow" });
  await expect(resumeButton).toBeEnabled();
  await resumeButton.click();

  const resumeRequest = await daemon.waitForRequest(
    "workflow_toggle",
    (candidate) => candidate.params.slug === "agent-review-workflow" && candidate.params.enabled === true
  );
  expect(resumeRequest.params.enabled).toBe(true);
  await expect(page.locator(".pf-run-header-state")).toHaveText("enabled");
});

test("workflow refresh is disabled while the workflow snapshot loads", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.delayFailure("workflow_list", () => true, "slow workflow snapshot", 250);
  await daemon.install(page);
  await daemon.open(page);

  await openWorkflows(page);

  const refresh = page.getByRole("button", { name: "Refresh workflows" });
  await expect(refresh).toBeDisabled();
  await expect(refresh).toHaveAttribute("aria-busy", "true");

  await expect(refresh).toBeEnabled();
  await expect(refresh).toHaveAttribute("aria-busy", "false");
  expect(daemon.requests.filter((request) => request.method === "workflow_list")).toHaveLength(1);
});

test("workflow refresh preserves unsaved node drafts", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openWorkflowDetail(page);

  const prompt = page.getByLabel("Prompt");
  await expect(prompt).toHaveValue("Implement the requested change.");
  await prompt.fill("local draft that must survive refresh");

  daemon.setWorkflowSnapshot({
    workflows: [
      {
        schema: "puffer.workflow.v1",
        slug: "agent-review-workflow",
        enabled: true,
        trigger: { type: "subscription", source_topic: "workspace.task.created", pattern: "review" },
        pipeline: {
          name: "Agent review workflow",
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

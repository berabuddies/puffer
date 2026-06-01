import { expect, test, type Page } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";

async function openTasks(page: Page) {
  await page.locator(".pf-sidebar").getByRole("button", { name: "Tasks" }).click();
}

test("tasks history shows received monitor messages and agent outcomes", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openTasks(page);
  await page.getByRole("button", { name: "History" }).click();
  await daemon.waitForRequest("task_monitor_history_list");

  const dialog = page.getByRole("dialog", { name: "Task history" });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByLabel("Received messages").getByRole("button", {
    name: /Telegram from Alice/
  })).toBeVisible();
  await expect(dialog.getByLabel("Agent history")).toContainText("Triage agent");
  await expect(dialog.getByLabel("Agent history")).toContainText("Created monitor task monitor-1.");
  await expect(dialog.getByLabel("Agent history")).toContainText("38 tokens");
  await expect(dialog.getByLabel("Agent history")).toContainText("No ignored task or ignore analysis");
});

test("tasks history links ignored task analysis by monitor envelope", async ({ page }) => {
  const daemon = new FakeDaemon();
  const now = Date.now();
  daemon.setWorkflowSnapshot({
    workflows: [],
    runs: [],
    connections: [],
    connectors: [],
    workflow_bindings: [],
    monitor_tasks: [
      {
        task_id: "monitor-ignored",
        subject: "Ignore noisy sender",
        description: "This sender is not actionable.",
        status: "completed",
        source: "monitor",
        monitor_connection: "telegram-user",
        monitor_connector: "telegram-login",
        monitor_envelope_id: "env-ignore",
        ignored: true,
        ignore_reason: "duplicate bot alert",
        ignore_analysis_started: true,
        ignore_analysis_status: "completed",
        ignore_analysis_result: "The filter is scoped to the room and sender.",
        ignore_analysis_usage: {
          input_tokens: 20,
          output_tokens: 5,
          cache_read_tokens: 4,
          spent_tokens: 21
        },
        started_at_ms: now - 20_000,
        updated_at_ms: now - 10_000
      }
    ],
    monitor_task_error: null
  });
  daemon.setMonitorHistory({
    messages: [
      {
        idx: 7,
        run_id: "run-ignore",
        workflow_slug: "monitor-telegram-user",
        connection_slug: "telegram-user",
        connector_slug: "telegram-login",
        envelope_id: "env-ignore",
        received_at_ms: now - 21_000,
        topic: "telegram-user",
        kind: "message",
        summary: "Telegram from bot: duplicate alert",
        text: "duplicate alert",
        payload: { chat_title: "Ops", sender_username: "bot", message: "duplicate alert" },
        action_log: [
          {
            action: "monitor_ignore_filter",
            status: "completed",
            summary: "Matched an installed monitor ignore filter before triage.",
            started_at_ms: now - 21_000,
            ended_at_ms: now - 21_000,
            usage: null
          }
        ],
        status: "completed",
        started_at_ms: now - 21_000,
        ended_at_ms: now - 21_000
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openTasks(page);
  await page.getByRole("button", { name: "History" }).click();

  const dialog = page.getByRole("dialog", { name: "Task history" });
  await expect(dialog).toContainText("Ignore filter");
  await expect(dialog).toContainText("duplicate bot alert");
  await expect(dialog).toContainText("The filter is scoped to the room and sender.");
  await expect(dialog).toContainText("21 tokens");
});

test("tasks history shows monitor messages while triage is still processing", async ({ page }) => {
  const daemon = new FakeDaemon();
  const now = Date.now();
  daemon.setMonitorHistory({
    messages: [
      {
        idx: 12,
        run_id: "run-processing",
        workflow_slug: "monitor-telegram-user",
        connection_slug: "telegram-user",
        connector_slug: "telegram-login",
        envelope_id: "env-processing",
        received_at_ms: now - 2_000,
        topic: "telegram-user",
        kind: "message",
        summary: "Telegram from Alice: can you check this?",
        text: "can you check this?",
        payload: { chat_title: "Support", sender_username: "alice", message: "can you check this?" },
        action_log: [
          {
            action: "triage_agent",
            status: "running",
            summary: "triage agent is processing this message.",
            started_at_ms: now - 1_000,
            ended_at_ms: now - 1_000,
            usage: null
          }
        ],
        status: "running",
        started_at_ms: now - 1_000,
        ended_at_ms: now - 1_000
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openTasks(page);
  await page.getByRole("button", { name: "History" }).click();

  const dialog = page.getByRole("dialog", { name: "Task history" });
  await expect(dialog.getByLabel("Received messages")).toContainText("Telegram from Alice");
  await expect(dialog.getByLabel("Agent history")).toContainText("processing · tokens n/a");
  await expect(dialog.getByLabel("Agent history")).toContainText("triage agent is processing this message.");
});

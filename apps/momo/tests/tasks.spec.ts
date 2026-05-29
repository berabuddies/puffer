import { expect, test, type Locator, type Page } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";
import { bootOnboarded } from "./support/bootHelpers";

/**
 * Tasks page — the Telegram monitor task feed.
 *
 * Exercises the full daemonClient round-trip under test: the page calls
 * `daemon_handshake` on the backend `/ws` path, then dials the FakeDaemon's
 * second `/daemon` path for `workflow_list` / `task_monitor_ignore`. The
 * FakeDaemon stands up both paths (see tests/support/fakeDaemon.ts).
 */

type MonitorTask = {
  task_id: string;
  subject: string;
  description: string;
  status: string;
  monitor_connection: string;
  updated_at_ms: number;
};

/** Build a workflow snapshot whose only meaningful content is monitor_tasks. */
function snapshotWith(tasks: MonitorTask[]) {
  return {
    workflows: [],
    runs: [],
    connectors: [],
    connections: [],
    connector_error: null,
    workflow_bindings: [],
    workflow_binding_error: null,
    monitor_tasks: tasks,
    monitor_task_error: null
  };
}

const now = Date.now();

function twoTasks(): MonitorTask[] {
  return [
    {
      task_id: "monitor-a",
      subject: "Reply to Alice about the deploy",
      description: "Alice asked whether the deployment is finished.",
      status: "pending",
      monitor_connection: "telegram-user",
      updated_at_ms: now - 5_000
    },
    {
      task_id: "monitor-b",
      subject: "Confirm the lunch reservation",
      description: "Bob wants to know if 12:30 still works.",
      status: "pending",
      monitor_connection: "telegram-user",
      updated_at_ms: now - 10_000
    }
  ];
}

function taskRow(page: Page, subject: string): Locator {
  return page.locator(".task-row", { hasText: subject });
}

test("navigating to /tasks renders both monitor task subjects", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.setWorkflowSnapshot(snapshotWith(twoTasks()));
  await bootOnboarded(page, daemon);

  await page.goto("/#/tasks");
  await expect(page).toHaveURL(/#\/tasks$/);

  await daemon.waitForRequest("workflow_list");

  await expect(
    page.getByRole("heading", { name: "Reply to Alice about the deploy" })
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Confirm the lunch reservation" })
  ).toBeVisible();
});

test("clicking Ignore sends task_monitor_ignore with the right task_id", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.setWorkflowSnapshot(snapshotWith(twoTasks()));
  await bootOnboarded(page, daemon);

  await page.goto("/#/tasks");
  await daemon.waitForRequest("workflow_list");

  const row = taskRow(page, "Reply to Alice about the deploy");
  await expect(row).toBeVisible();

  const ignorePromise = daemon.waitForRequest(
    "task_monitor_ignore",
    (req) => req.params.task_id === "monitor-a"
  );
  await row.getByRole("button", { name: "Ignore" }).click();
  await ignorePromise;

  // ignore drops the task from the snapshot; the store refetches via
  // workflow_list, so the ignored row disappears while the other stays.
  await expect(
    page.getByRole("heading", { name: "Reply to Alice about the deploy" })
  ).toHaveCount(0);
  await expect(
    page.getByRole("heading", { name: "Confirm the lunch reservation" })
  ).toBeVisible();
});

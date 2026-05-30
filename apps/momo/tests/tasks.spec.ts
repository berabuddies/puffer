import { expect, test, type Locator, type Page } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";
import { bootOnboarded } from "./support/bootHelpers";

/**
 * Home monitor task feed — the Telegram monitor task list (the standalone
 * /tasks page was folded into Home).
 *
 * Exercises the full daemonClient round-trip: Home calls `daemon_handshake`
 * on the backend `/ws` path, then dials the FakeDaemon's `/daemon` path for
 * `workflow_list` / `task_monitor_ignore`. Clicking a task action or Open
 * mints a fresh agent thread via `create_session` + `run_agent_turn`. The
 * FakeDaemon stands up both paths (see tests/support/fakeDaemon.ts).
 */

type MonitorAction = { name: string; prompt: string };

type MonitorTask = {
  task_id: string;
  subject: string;
  description: string;
  status: string;
  monitor_connection: string;
  updated_at_ms: number;
  actions?: MonitorAction[];
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
      updated_at_ms: now - 5_000,
      // First action becomes the cream primary button.
      actions: [
        { name: "Draft reply", prompt: "Draft a concise reply to Alice with the deployment status." }
      ]
    },
    {
      task_id: "monitor-b",
      subject: "Confirm the lunch reservation",
      description: "Bob wants to know if 12:30 still works.",
      status: "pending",
      monitor_connection: "telegram-user",
      updated_at_ms: now - 10_000
      // No actions → the primary button falls back to "Open".
    }
  ];
}

function taskCard(page: Page, subject: string): Locator {
  return page.locator(".task-card", { hasText: subject });
}

test("Home renders both monitor task subjects", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.setWorkflowSnapshot(snapshotWith(twoTasks()));
  await bootOnboarded(page, daemon);

  await daemon.waitForRequest("workflow_list");

  await expect(
    page.getByRole("heading", { name: "Reply to Alice about the deploy" })
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Confirm the lunch reservation" })
  ).toBeVisible();
});

test("Home auto-refreshes new monitor tasks without navigating away", async ({ page }) => {
  const daemon = new FakeDaemon();
  // Start empty: the user is sitting on Home, "all caught up".
  daemon.setWorkflowSnapshot(snapshotWith([]));
  await bootOnboarded(page, daemon);
  await daemon.waitForRequest("workflow_list");

  await expect(
    page.getByRole("heading", { name: "You're all caught up" })
  ).toBeVisible();

  // A new Telegram message arrives and triage creates a monitor task while the
  // user never leaves Home. The store must poll and surface it on its own —
  // without this, the feed only refreshes on mount (navigate away + back).
  daemon.setWorkflowSnapshot(snapshotWith(twoTasks()));

  await expect(
    page.getByRole("heading", { name: "Reply to Alice about the deploy" })
  ).toBeVisible({ timeout: 15_000 });
});

test("opening Connected Apps with Telegram connected auto-creates the monitor", async ({ page }) => {
  const daemon = new FakeDaemon();
  // Simulate a returning user whose Telegram is already authenticated: the
  // first refreshStatus on mount sees telegram=true and fires createMonitor.
  daemon.setConnectorStatus({ telegram: true });
  await bootOnboarded(page, daemon);

  const monitorPromise = daemon.waitForRequest(
    "task_monitor_create",
    (req) => req.params.connection_slug === "telegram-user"
  );

  await page.goto("/#/apps");
  await expect(page).toHaveURL(/#\/apps$/);

  const request = await monitorPromise;
  expect(request.params.connection_slug).toBe("telegram-user");
});

test("opening Connected Apps with Telegram disconnected does not create a monitor", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.setConnectorStatus({ telegram: false });
  await bootOnboarded(page, daemon);

  await page.goto("/#/apps");
  await expect(page).toHaveURL(/#\/apps$/);

  // Status is polled on mount; wait for that round-trip to land so we know the
  // guard has had its chance to (not) fire.
  await daemon.waitForRequest("connector_status");
  await expect
    .poll(() => daemon.requests.filter((r) => r.method === "task_monitor_create").length)
    .toBe(0);
});

test("ignored monitor tasks are filtered out of the Home feed", async ({ page }) => {
  const daemon = new FakeDaemon();
  const [a, b] = twoTasks();
  // The daemon keeps ignored tasks in the snapshot (ignored:true); the feed
  // must filter them client-side, so only the non-ignored one renders.
  daemon.setWorkflowSnapshot(snapshotWith([a, { ...b, ignored: true }]));
  await bootOnboarded(page, daemon);
  await daemon.waitForRequest("workflow_list");

  await expect(
    page.getByRole("heading", { name: "Reply to Alice about the deploy" })
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Confirm the lunch reservation" })
  ).toHaveCount(0);
});

test("clicking Ignore sends task_monitor_ignore with the right task_id", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.setWorkflowSnapshot(snapshotWith(twoTasks()));
  await bootOnboarded(page, daemon);
  await daemon.waitForRequest("workflow_list");

  const card = taskCard(page, "Reply to Alice about the deploy");
  await expect(card).toBeVisible();

  const ignorePromise = daemon.waitForRequest(
    "task_monitor_ignore",
    (req) => req.params.task_id === "monitor-a"
  );
  await card.getByRole("button", { name: "Ignore" }).click();
  await ignorePromise;

  // ignore drops the task from the snapshot; the store refetches via
  // workflow_list, so the ignored card disappears while the other stays.
  await expect(
    page.getByRole("heading", { name: "Reply to Alice about the deploy" })
  ).toHaveCount(0);
  await expect(
    page.getByRole("heading", { name: "Confirm the lunch reservation" })
  ).toBeVisible();
});

test("clicking a task action runs its prompt in a fresh agent session", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.setWorkflowSnapshot(snapshotWith(twoTasks()));
  await bootOnboarded(page, daemon);
  await daemon.waitForRequest("workflow_list");

  const card = taskCard(page, "Reply to Alice about the deploy");
  const turnPromise = daemon.waitForRequest(
    "run_agent_turn",
    (req) => String(req.params.message).includes("Selected action: Draft reply")
  );

  // The first action renders as the cream primary button (labelled by name).
  await card.getByRole("button", { name: "Draft reply" }).click();

  const turn = await turnPromise;
  // The action prompt carries the task id + the selected action's prompt.
  expect(String(turn.params.message)).toContain("Act on monitored task monitor-a");
  expect(String(turn.params.message)).toContain(
    "Draft a concise reply to Alice with the deployment status."
  );
  // It must NOT tell the agent to TaskUpdate the monitor task complete: from a
  // momo chat session that call resolves under the session cwd, never finds the
  // monitor task, and silently fails. Completion is the user's Ignore action.
  expect(String(turn.params.message)).not.toContain("TaskUpdate");
  // A brand-new session was minted and we navigated into it.
  await expect(page).toHaveURL(/#\/agent\/session-created-/);
});

test("clicking Open seeds a fresh session with the task context inlined", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.setWorkflowSnapshot(snapshotWith(twoTasks()));
  await bootOnboarded(page, daemon);
  await daemon.waitForRequest("workflow_list");

  const card = taskCard(page, "Confirm the lunch reservation");
  const turnPromise = daemon.waitForRequest(
    "run_agent_turn",
    (req) => String(req.params.message).includes("monitor-b")
  );

  // The Bot source-icon button is the Open affordance.
  await card.getByRole("button", { name: "Open in source app" }).click();

  const turn = await turnPromise;
  const message = String(turn.params.message);
  // Open must inline the task's subject + description so the agent has the full
  // context without a `/tasks show <id>` / TaskGet lookup. That lookup resolves
  // the monitor task store under the chat session's cwd (~/.momo/projects/default),
  // not the daemon root (~/.puffer) where the Telegram monitor actually writes
  // monitor_tasks.json — so it would always come back "not found".
  expect(message).toContain("monitor-b");
  expect(message).toContain("Confirm the lunch reservation");
  expect(message).toContain("Bob wants to know if 12:30 still works.");
  expect(message).not.toContain("/tasks show");
  await expect(page).toHaveURL(/#\/agent\/session-created-/);
});

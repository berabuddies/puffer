/**
 * Task 4 — chat state-machine reducer (`agentChat.svelte.ts`).
 *
 * The reducer (`handleSessionEvent`) is pure per-session state: it folds
 * daemon stream events into a `ChatState` and never touches the daemon. So
 * these tests don't drive any RPC — they create a controller in the page,
 * feed events straight into `handleSessionEvent`, and assert the derived
 * `combinedTimeline()` / `pendingPermissions()`.
 *
 * The DEV-only `window.__agentChat` bridge is installed when
 * `agentChat.svelte.ts` loads; we import it explicitly so it's present
 * regardless of which routes pulled the module in already.
 *
 * `bootOnboarded` + FakeDaemon stand up the same logged-in/onboarded page the
 * other agent specs use; the FakeDaemon is only here so module imports that
 * reach for the daemon client don't throw — the reducer path itself stays
 * offline.
 */
import { expect, test, type Page } from "@playwright/test";
import { FakeDaemon } from "../support/fakeDaemon";
import { bootOnboarded } from "../support/bootHelpers";

type TimelineRow = {
  id: string;
  kind: string;
  body: string;
  toolName?: string;
  status?: string;
};

/** Install the controller bridge and create a controller bound to `sessionId`. */
async function createController(page: Page, sessionId: string): Promise<void> {
  await page.evaluate(async () => {
    await import("/src/lib/agent/agentChat.svelte.ts");
  });
  await page.evaluate((sid) => {
    const bridge = (window as unknown as {
      __agentChat: { createController: (id: string) => unknown };
    }).__agentChat;
    (window as unknown as { __ctrl: unknown }).__ctrl = bridge.createController(sid);
  }, sessionId);
}

/** Feed one or more stream events through the reducer. */
async function feed(page: Page, events: unknown[]): Promise<void> {
  await page.evaluate((evs) => {
    const ctrl = (window as unknown as {
      __ctrl: { handleSessionEvent: (ev: unknown) => void };
    }).__ctrl;
    for (const ev of evs) ctrl.handleSessionEvent(ev);
  }, events);
}

async function combinedTimeline(page: Page): Promise<TimelineRow[]> {
  return page.evaluate(() =>
    (
      window as unknown as { __ctrl: { combinedTimeline: () => TimelineRow[] } }
    ).__ctrl.combinedTimeline()
  );
}

async function pendingPermissions(page: Page): Promise<TimelineRow[]> {
  return page.evaluate(() =>
    (
      window as unknown as { __ctrl: { pendingPermissions: () => TimelineRow[] } }
    ).__ctrl.pendingPermissions()
  );
}

async function resolvePermission(page: Page, permissionId: string, choice: string): Promise<void> {
  await page.evaluate(
    ([id, c]) =>
      (
        window as unknown as {
          __ctrl: { resolvePermission: (i: string, ch: string) => Promise<void> };
        }
      ).__ctrl.resolvePermission(id, c),
    [permissionId, choice] as const
  );
}

test("text-delta accumulates into one assistant bubble; replay deltas don't double-count", async ({
  page
}) => {
  const daemon = new FakeDaemon({ sessions: [{ sessionId: "s-1", timeline: [] }] });
  await bootOnboarded(page, daemon);
  await createController(page, "s-1");

  // Two non-replay deltas accumulate in order into one bubble.
  await feed(page, [
    { type: "turn-start", turnId: "t-1" },
    { type: "text-delta", turnId: "t-1", delta: "Hello" },
    { type: "text-delta", turnId: "t-1", delta: " world" }
  ]);

  let assistant = (await combinedTimeline(page)).filter((r) => r.kind === "assistant");
  expect(assistant).toHaveLength(1);
  expect(assistant[0].body).toBe("Hello world");

  // A reconnect re-streams the same text with replay:true. replaySafeDelta
  // must collapse it so the bubble stays "Hello world", not doubled.
  await feed(page, [
    { type: "text-delta", turnId: "t-1", delta: "Hello", replay: true },
    { type: "text-delta", turnId: "t-1", delta: " world", replay: true }
  ]);

  assistant = (await combinedTimeline(page)).filter((r) => r.kind === "assistant");
  expect(assistant).toHaveLength(1);
  expect(assistant[0].body).toBe("Hello world");
});

test("permission-request surfaces a pending permission; resolvePermission dismisses it", async ({
  page
}) => {
  const daemon = new FakeDaemon({ sessions: [{ sessionId: "s-1", timeline: [] }] });
  await bootOnboarded(page, daemon);
  await createController(page, "s-1");

  await feed(page, [
    { type: "turn-start", turnId: "t-1" },
    {
      type: "permission-request",
      turnId: "t-1",
      requestId: "req-1",
      toolId: "bash",
      summary: "Run `ls`",
      reason: "List the directory"
    }
  ]);

  const before = await pendingPermissions(page);
  expect(before).toHaveLength(1);
  expect(before[0].toolName).toBe("bash");
  const permId = before[0].id;

  // Resolve it — the FakeDaemon answers resolve_permission with {}; the
  // controller then dismisses the id so it drops out of pendingPermissions.
  await resolvePermission(page, permId, "Approve once");

  expect(await pendingPermissions(page)).toHaveLength(0);
});

test("tool-calls-requested renders a running pill; tool-invocations flips it to success", async ({
  page
}) => {
  const daemon = new FakeDaemon({ sessions: [{ sessionId: "s-1", timeline: [] }] });
  await bootOnboarded(page, daemon);
  await createController(page, "s-1");

  await feed(page, [
    { type: "turn-start", turnId: "t-1" },
    {
      type: "tool-calls-requested",
      turnId: "t-1",
      requests: [{ callId: "c-1", toolId: "read_file", input: '{"path":"a.txt"}' }]
    }
  ]);

  let tool = (await combinedTimeline(page)).filter((r) => r.kind === "tool");
  expect(tool).toHaveLength(1);
  expect(tool[0].toolName).toBe("read_file");
  expect(tool[0].status).toBe("running");

  // Same callId + same turnId → the pill is upgraded in place (not a 2nd row).
  await feed(page, [
    {
      type: "tool-invocations",
      turnId: "t-1",
      invocations: [
        {
          callId: "c-1",
          toolId: "read_file",
          input: '{"path":"a.txt"}',
          output: "file body",
          success: true
        }
      ]
    }
  ]);

  tool = (await combinedTimeline(page)).filter((r) => r.kind === "tool");
  expect(tool).toHaveLength(1);
  expect(tool[0].status).toBe("success");
  expect(tool[0].body).toBe("file body");
});

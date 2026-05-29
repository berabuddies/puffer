/**
 * Bug 2 repro — historical-turn tool calls must NOT accumulate into the
 * latest turn's agent activity across consecutive multi-turn conversations.
 *
 * Root cause under test (see agentChat.svelte.ts):
 *   - `turn-complete` reconciles the just-finished turn's live items against
 *     the *old* persisted timeline (synchronous), so the turn's own tool rows
 *     stay in `liveStreamItems` until the async `refreshSessionAfterTurn`
 *     re-loads the persisted transcript and clears them.
 *   - `refreshSessionAfterTurn` awaits `load_session_detail`. If the user
 *     sends a second turn while that load is in flight, `state.currentTurnId`
 *     flips to the new turn. The guard
 *       `if (state.currentTurnId !== null && state.currentTurnId !== completedTurnId) return;`
 *     then ABANDONS the refresh: turn1's persisted timeline is never written
 *     and turn1's live tool rows are never cleared. They linger in
 *     `liveStreamItems` and `combinedTimeline()` appends them after turn2's
 *     user message → every prior turn's tools pile onto the newest agent row.
 *
 * We drive the pure reducer via the `__agentChat` controller bridge (same as
 * agent-chat-reducer.spec.ts) and use the FakeDaemon's `deferRpc` to stall
 * turn1's `load_session_detail` while turn2 starts.
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

const SESSION = "s-multiturn";

/** Backend timeline (load_session_detail shape) after N turns are persisted. */
function persistedAfterTurn1(): Record<string, unknown>[] {
  return [
    { kind: "user_message", id: "p-u-1", text: "turn one please", createdAtMs: 10 },
    {
      kind: "tool_call",
      id: "p-tool-1",
      toolId: "read_file",
      inputText: '{"path":"one.txt"}',
      outputText: "one body",
      status: "success",
      createdAtMs: 11
    },
    { kind: "assistant_message", id: "p-a-1", text: "did turn one", createdAtMs: 12 }
  ];
}

function persistedAfterTurn2(): Record<string, unknown>[] {
  return [
    ...persistedAfterTurn1(),
    { kind: "user_message", id: "p-u-2", text: "turn two please", createdAtMs: 20 },
    {
      kind: "tool_call",
      id: "p-tool-2",
      toolId: "web_search",
      inputText: '{"query":"two"}',
      outputText: "two body",
      status: "success",
      createdAtMs: 21
    },
    { kind: "assistant_message", id: "p-a-2", text: "did turn two", createdAtMs: 22 }
  ];
}

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

async function loadDetail(page: Page): Promise<void> {
  await page.evaluate(() =>
    (window as unknown as { __ctrl: { loadDetail: () => Promise<void> } }).__ctrl.loadDetail()
  );
}

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
    (window as unknown as { __ctrl: { combinedTimeline: () => TimelineRow[] } }).__ctrl.combinedTimeline()
  );
}

async function liveStreamItems(page: Page): Promise<TimelineRow[]> {
  return page.evaluate(
    () =>
      (
        window as unknown as { __ctrl: { state: () => { liveStreamItems: TimelineRow[] } } }
      ).__ctrl.state().liveStreamItems
  );
}

/** Which user message precedes a given tool row in combinedTimeline. */
function toolOwnerByUserBoundary(rows: TimelineRow[]): Record<string, string> {
  const owner: Record<string, string> = {};
  let lastUser = "(none)";
  for (const row of rows) {
    if (row.kind === "user") lastUser = row.body;
    if (row.kind === "tool") owner[row.toolName ?? row.id] = lastUser;
  }
  return owner;
}

test("consecutive turns: prior turn's tools stay in their own turn (no live accumulation)", async ({
  page
}) => {
  const daemon = new FakeDaemon({ sessions: [{ sessionId: SESSION, timeline: [] }] });
  await bootOnboarded(page, daemon);
  await createController(page, SESSION);

  // Hydrate the empty starting transcript.
  await loadDetail(page);

  // ── Turn 1: user → tool → text → turn-complete ──────────────────────
  // Server now has turn1 persisted; arrange the *next* load_session_detail
  // (the one refreshSessionAfterTurn fires) to return turn1's transcript.
  daemon.setSessionTimeline(SESSION, persistedAfterTurn1());

  // Stall turn1's post-turn refresh so we can interleave turn2 before it lands.
  const turn1Refresh = daemon.deferRpc("load_session_detail");

  await feed(page, [
    { type: "turn-start", turnId: "t-1" },
    {
      type: "tool-invocations",
      turnId: "t-1",
      invocations: [
        {
          callId: "c-1",
          toolId: "read_file",
          input: '{"path":"one.txt"}',
          output: "one body",
          success: true
        }
      ]
    },
    { type: "text-delta", turnId: "t-1", delta: "did turn one" },
    { type: "turn-complete", turnId: "t-1", assistantText: "did turn one" }
  ]);

  // turn-complete fired `void refreshSessionAfterTurn(...)`, which is now
  // awaiting the deferred load_session_detail. Before it resolves, the user
  // sends turn 2 — its turn-start flips currentTurnId to "t-2".
  await daemon.waitForRequest("load_session_detail");

  // ── Turn 2 starts while turn1's refresh is still pending ────────────
  await feed(page, [
    { type: "turn-start", turnId: "t-2" },
    {
      type: "tool-invocations",
      turnId: "t-2",
      invocations: [
        {
          callId: "c-2",
          toolId: "web_search",
          input: '{"query":"two"}',
          output: "two body",
          success: true
        }
      ]
    }
  ]);

  // Now let turn1's refresh resolve. It returns turn1's persisted timeline
  // (which already contains turn1's tool), so a correct refresh would clear
  // turn1's live tool. The buggy guard drops the whole refresh.
  turn1Refresh.resolve(undefined);
  // Give the microtask chain inside refreshSessionAfterTurn a tick to settle.
  await page.evaluate(() => new Promise((r) => setTimeout(r, 0)));

  const live = await liveStreamItems(page);
  const liveToolNames = live.filter((r) => r.kind === "tool").map((r) => r.toolName);

  // The crux: turn1's read_file must NOT still be sitting in live items
  // (it was persisted; turn1's refresh should have cleared it). If the guard
  // abandoned the refresh, read_file lingers here alongside turn2's web_search.
  expect(liveToolNames).not.toContain("read_file");

  // And in the combined view, turn1's tool must still belong to turn1 — never
  // re-homed under turn2's user message.
  const rows = await combinedTimeline(page);
  const owner = toolOwnerByUserBoundary(rows);
  expect(owner["read_file"]).toBe("turn one please");
});

test("consecutive turns: completed turns both render their own tool once turn2 settles", async ({
  page
}) => {
  const daemon = new FakeDaemon({ sessions: [{ sessionId: SESSION, timeline: [] }] });
  await bootOnboarded(page, daemon);
  await createController(page, SESSION);
  await loadDetail(page);

  daemon.setSessionTimeline(SESSION, persistedAfterTurn1());
  const turn1Refresh = daemon.deferRpc("load_session_detail");

  await feed(page, [
    { type: "turn-start", turnId: "t-1" },
    {
      type: "tool-invocations",
      turnId: "t-1",
      invocations: [
        { callId: "c-1", toolId: "read_file", input: '{"path":"one.txt"}', output: "one body", success: true }
      ]
    },
    { type: "turn-complete", turnId: "t-1", assistantText: "did turn one" }
  ]);
  await daemon.waitForRequest("load_session_detail");

  await feed(page, [
    { type: "turn-start", turnId: "t-2" },
    {
      type: "tool-invocations",
      turnId: "t-2",
      invocations: [
        { callId: "c-2", toolId: "web_search", input: '{"query":"two"}', output: "two body", success: true }
      ]
    }
  ]);
  turn1Refresh.resolve(undefined);
  await page.evaluate(() => new Promise((r) => setTimeout(r, 0)));

  // Turn 2 completes; its refresh sees the full two-turn transcript.
  daemon.setSessionTimeline(SESSION, persistedAfterTurn2());
  await feed(page, [{ type: "turn-complete", turnId: "t-2", assistantText: "did turn two" }]);
  await daemon.waitForRequest(
    "load_session_detail",
    (req) => daemon.requests.filter((r) => r.method === "load_session_detail").length >= 3
  );
  await page.evaluate(() => new Promise((r) => setTimeout(r, 0)));

  const rows = await combinedTimeline(page);
  const tools = rows.filter((r) => r.kind === "tool");
  // Exactly one read_file and one web_search, each grouped under its own turn.
  expect(tools.filter((t) => t.toolName === "read_file")).toHaveLength(1);
  expect(tools.filter((t) => t.toolName === "web_search")).toHaveLength(1);
  const owner = toolOwnerByUserBoundary(rows);
  expect(owner["read_file"]).toBe("turn one please");
  expect(owner["web_search"]).toBe("turn two please");
});

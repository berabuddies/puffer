/**
 * Task 2 — thin daemon chat RPC layer.
 *
 * `apps/momo/src/lib/agent/daemonChat.ts` is a thin wrapper over the puffer
 * *daemon* protocol (via `daemonClient`). Each helper sends one daemon RPC
 * and unwraps its result. There is deliberately no Tauri-invoke fallback /
 * browser-preview / mockData (those would mask daemon errors), so these tests
 * drive the real daemon round-trip end-to-end through the FakeDaemon's
 * `/daemon` path, asserting both the outbound request shape and the parsed
 * return value.
 *
 * The DEV-only `window.__daemonChat` bridge (installed when daemonChat.ts
 * loads) lets Playwright invoke the helpers without a full chat UI wired up.
 */
import { expect, test } from "@playwright/test";
import { FakeDaemon } from "../support/fakeDaemon";
import { bootOnboarded } from "../support/bootHelpers";

type DaemonChatBridge = {
  createSession: (
    cwd: string,
    providerId?: string
  ) => Promise<{ sessionId: string; cwd: string; [key: string]: unknown }>;
  renameSession: (
    sessionId: string,
    title: string
  ) => Promise<{ sessionId: string; displayName: string | null; title: string; updatedAtMs: number }>;
  runAgentTurn: (
    sessionId: string,
    message: string,
    options?: Record<string, unknown>
  ) => Promise<string>;
  cancelTurn: (turnId: string) => Promise<void>;
  resolvePermission: (turnId: string, requestId: string, action: string) => Promise<void>;
  resolveUserQuestion: (
    turnId: string,
    requestId: string,
    answers: Record<string, string | string[]>,
    annotations?: Record<string, Record<string, string>>
  ) => Promise<void>;
  loadSessionDetail: (sessionId: string) => Promise<unknown>;
  listGroupedSessions: () => Promise<unknown[]>;
};

async function importBridge(page: import("@playwright/test").Page): Promise<void> {
  // The bridge is installed on import; pull the module in explicitly so it's
  // present regardless of which routes already loaded it.
  await page.evaluate(async () => {
    await import("/src/lib/agent/daemonChat.ts");
  });
}

test("runAgentTurn sends run_agent_turn and resolves the turnId", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [{ sessionId: "s-1", timeline: [] }] });
  await bootOnboarded(page, daemon);
  await importBridge(page);

  const turnPromise = daemon.waitForRequest("run_agent_turn");

  const turnId = await page.evaluate(async () => {
    return await (window as unknown as { __daemonChat: DaemonChatBridge }).__daemonChat.runAgentTurn(
      "s-1",
      "hi"
    );
  });

  const turn = await turnPromise;
  expect(turn.params).toMatchObject({
    sessionId: "s-1",
    message: "hi",
    permissionMode: "workspace-write"
  });
  // Default turn carries no `mode` key unless options.mode is set.
  expect(turn.params).not.toHaveProperty("mode");

  // FakeDaemon answers run_agent_turn with `{ turnId: "turn-<sessionId>" }`;
  // the helper must parse and return that string.
  expect(turnId).toBe("turn-s-1");
});

test("runAgentTurn forwards an explicit permissionMode and mode", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [{ sessionId: "s-1", timeline: [] }] });
  await bootOnboarded(page, daemon);
  await importBridge(page);

  const turnPromise = daemon.waitForRequest("run_agent_turn");

  await page.evaluate(async () => {
    await (window as unknown as { __daemonChat: DaemonChatBridge }).__daemonChat.runAgentTurn(
      "s-1",
      "plan it",
      { permissionMode: "read-only", mode: "plan" }
    );
  });

  const turn = await turnPromise;
  expect(turn.params).toMatchObject({
    sessionId: "s-1",
    message: "plan it",
    permissionMode: "read-only",
    mode: "plan"
  });
});

test("createSession sends create_session with the cwd and resolves the full result", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);
  await importBridge(page);

  const createPromise = daemon.waitForRequest("create_session");

  const result = await page.evaluate(async () => {
    return await (window as unknown as { __daemonChat: DaemonChatBridge }).__daemonChat.createSession(
      "/tmp/momo-work",
      "puffer"
    );
  });

  const create = await createPromise;
  // The provider routing is forwarded so daemon-side session routing matches.
  expect(create.params).toMatchObject({ cwd: "/tmp/momo-work", providerId: "puffer" });
  // FakeDaemon mints `session-created-N`; the helper now returns the full
  // create_session payload (the sidebar needs cwd / timestamps for its stub).
  expect(result.sessionId).toMatch(/^session-created-/);
  expect(result.cwd).toBe("/tmp/momo-work");
});

test("renameSession sends rename_session and resolves the patched fields", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [{ sessionId: "s-1", timeline: [] }] });
  await bootOnboarded(page, daemon);
  await importBridge(page);

  const renamePromise = daemon.waitForRequest("rename_session");

  const result = await page.evaluate(async () => {
    return await (window as unknown as { __daemonChat: DaemonChatBridge }).__daemonChat.renameSession(
      "s-1",
      "Renamed chat"
    );
  });

  const rename = await renamePromise;
  expect(rename.params).toMatchObject({ sessionId: "s-1", title: "Renamed chat" });
  // FakeDaemon echoes the latest session detail; displayName / title reflect
  // the new name the sidebar patches in place.
  expect(result.sessionId).toBe("s-1");
  expect(result.title).toBe("Renamed chat");
  expect(result.displayName).toBe("Renamed chat");
});

test("cancelTurn / resolvePermission / resolveUserQuestion send the right RPCs", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [{ sessionId: "s-1", timeline: [] }] });
  await bootOnboarded(page, daemon);
  await importBridge(page);

  // cancel_turn only succeeds for an active turn, so start one first.
  await page.evaluate(async () => {
    const bridge = (window as unknown as { __daemonChat: DaemonChatBridge }).__daemonChat;
    const turnId = await bridge.runAgentTurn("s-1", "go");
    await bridge.cancelTurn(turnId);
    await bridge.resolvePermission(turnId, "req-1", "allow_once");
    await bridge.resolveUserQuestion(turnId, "req-2", { Q1: "answer" });
  });

  const cancel = await daemon.waitForRequest("cancel_turn");
  expect(cancel.params).toMatchObject({ turnId: "turn-s-1" });

  const perm = await daemon.waitForRequest("resolve_permission");
  expect(perm.params).toMatchObject({
    turnId: "turn-s-1",
    requestId: "req-1",
    action: "allow_once"
  });

  const question = await daemon.waitForRequest("resolve_user_question");
  expect(question.params).toMatchObject({
    turnId: "turn-s-1",
    requestId: "req-2",
    answers: { Q1: "answer" },
    annotations: {}
  });
});

test("loadSessionDetail normalizes the backend session detail", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "s-1",
        timeline: [
          { kind: "user_message", id: "u-1", text: "hello there", createdAtMs: 1 },
          { kind: "assistant_message", id: "a-1", text: "hi back", createdAtMs: 2 }
        ]
      }
    ]
  });
  await bootOnboarded(page, daemon);
  await importBridge(page);

  const detail = await page.evaluate(async () => {
    return await (window as unknown as { __daemonChat: DaemonChatBridge }).__daemonChat.loadSessionDetail(
      "s-1"
    );
  });

  // normalizeSessionDetail flattens the backend item into { session, timeline, … }
  // and maps each backend timeline kind to its UI variant.
  expect(detail).toMatchObject({
    session: { id: "s-1" },
    timeline: [
      { id: "u-1", kind: "user", body: "hello there" },
      { id: "a-1", kind: "assistant", body: "hi back" }
    ]
  });
});

test("listGroupedSessions sends list_grouped_sessions", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [{ sessionId: "s-1", timeline: [] }] });
  await bootOnboarded(page, daemon);
  await importBridge(page);

  const listPromise = daemon.waitForRequest("list_grouped_sessions");

  await page.evaluate(async () => {
    await (window as unknown as { __daemonChat: DaemonChatBridge }).__daemonChat.listGroupedSessions();
  });

  await listPromise;
});

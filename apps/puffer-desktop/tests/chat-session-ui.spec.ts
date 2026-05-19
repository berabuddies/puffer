import { expect, type Page, test } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";

const baseTime = Date.now();

async function openSession(page: Page, name: RegExp): Promise<void> {
  await page.getByRole("button", { name }).first().click();
}

async function enterWorkspaceThroughForcedOnboarding(page: Page): Promise<void> {
  await expect(page.getByRole("heading", { name: "Workspace is ready" })).toBeVisible();
  await page.getByRole("button", { name: /Continue/ }).click();
}

test("turn completion reload does not leak live chat into a newly selected session", async ({
  page
}) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-alpha",
        displayName: "Alpha session",
        title: "Alpha session",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "alpha-seed",
            text: "Alpha seed",
            createdAtMs: baseTime - 30_000
          }
        ]
      },
      {
        sessionId: "session-beta",
        displayName: "Beta session",
        title: "Beta session",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "beta-seed",
            text: "Beta seed",
            createdAtMs: baseTime - 90_000
          }
        ]
      }
    ]
  });

  await daemon.install(page);
  await daemon.open(page);

  await expect(page.getByRole("button", { name: /Alpha session/ }).first()).toBeVisible();
  await openSession(page, /Alpha session/);
  await expect(page.getByText("Alpha seed")).toBeVisible();

  await page.locator(".pf-composer textarea").fill("Race from alpha");
  await page.getByRole("button", { name: "Send" }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-alpha" &&
      request.params.message === "Race from alpha"
  );

  daemon.delayResponse(
    "load_session_detail",
    (request) => request.params.sessionId === "session-alpha",
    500
  );
  daemon.emit("session:session-alpha:event", {
    type: "turn-complete",
    turnId: "turn-session-alpha",
    assistantText: "Alpha completion should stay with alpha"
  });

  await openSession(page, /Beta session/);
  await expect(page.getByText("Beta seed")).toBeVisible();

  await page.waitForTimeout(650);
  await expect(page.getByText("Beta seed")).toBeVisible();
  await expect(page.getByText("Alpha completion should stay with alpha")).toHaveCount(0);
  await expect(page.getByText("Race from alpha")).toHaveCount(0);
});

test("late turn start responses do not leak into a switched session", async ({
  page
}) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-alpha-start",
        displayName: "Alpha start",
        title: "Alpha start",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "alpha-start-seed",
            text: "Alpha start seed",
            createdAtMs: baseTime - 30_000
          }
        ]
      },
      {
        sessionId: "session-beta-start",
        displayName: "Beta start",
        title: "Beta start",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "beta-start-seed",
            text: "Beta start seed",
            createdAtMs: baseTime - 90_000
          }
        ]
      }
    ]
  });
  daemon.delayResponse(
    "run_agent_turn",
    (request) => request.params.sessionId === "session-alpha-start",
    120
  );

  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Alpha start/);
  await expect(page.getByText("Alpha start seed")).toBeVisible();
  await page.locator(".pf-composer textarea").fill("Alpha delayed prompt");
  await page.getByRole("button", { name: "Send" }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-alpha-start" &&
      request.params.message === "Alpha delayed prompt"
  );

  await openSession(page, /Beta start/);
  await expect(page.getByText("Beta start seed")).toBeVisible();

  await page.waitForTimeout(170);
  await expect(page.getByText("Beta start seed")).toBeVisible();
  await expect(page.getByText("Alpha delayed prompt")).toHaveCount(0);

  const composer = page.locator(".pf-composer textarea");
  await composer.fill("Beta prompt after alpha race");
  await expect(page.getByRole("button", { name: "Send" })).toBeEnabled();
});

test("pending turn start in one session does not disable another session composer", async ({
  page
}) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-alpha-inflight",
        displayName: "Alpha inflight",
        title: "Alpha inflight",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      },
      {
        sessionId: "session-beta-inflight",
        displayName: "Beta inflight",
        title: "Beta inflight",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });
  daemon.delayResponse(
    "run_agent_turn",
    (request) => request.params.sessionId === "session-alpha-inflight",
    5_000
  );

  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Alpha inflight/);
  await page.locator(".pf-composer textarea").fill("Alpha waits for turn id");
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-alpha-inflight" &&
      request.params.message === "Alpha waits for turn id"
  );

  await openSession(page, /Beta inflight/);
  const betaComposer = page.locator(".pf-composer textarea");
  await betaComposer.fill("Beta should still send");
  const sendButton = page.getByRole("button", { name: "Send", exact: true });
  await expect(sendButton).toBeEnabled({ timeout: 500 });
  await sendButton.click();
  await page.waitForTimeout(100);
  expect(
    daemon.requests.filter(
      (request) =>
        request.method === "run_agent_turn" &&
        request.params.sessionId === "session-beta-inflight" &&
        request.params.message === "Beta should still send"
    )
  ).toHaveLength(1);
});

test("sidebar keeps non-selected running agent live while another session is open", async ({
  page
}) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-alpha-sidebar-live",
        displayName: "Alpha sidebar live",
        title: "Alpha sidebar live",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        activityStatus: "idle",
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      },
      {
        sessionId: "session-beta-sidebar-live",
        displayName: "Beta sidebar live",
        title: "Beta sidebar live",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        eventCount: 0,
        activityStatus: "idle",
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });

  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Alpha sidebar live/);
  await page.locator(".pf-composer textarea").fill("Keep alpha running in the sidebar");
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-alpha-sidebar-live" &&
      request.params.message === "Keep alpha running in the sidebar"
  );

  const alphaRow = page.locator(".pf-sidebar-agent-row").filter({ hasText: "Alpha sidebar live" });
  await expect(alphaRow.locator(".pf-task-status")).toContainText("thinking");

  await openSession(page, /Beta sidebar live/);
  await expect(page.locator(".pf-agent-detail")).toBeVisible();
  await expect(alphaRow).toBeVisible();
  await expect(alphaRow.locator(".pf-task-status")).toContainText("thinking");

  daemon.emit("session:session-alpha-sidebar-live:event", {
    type: "turn-complete",
    turnId: "turn-session-alpha-sidebar-live",
    assistantText: "Alpha sidebar turn complete"
  });
  daemon.emit("workspace:sessions:changed", {
    sessionId: "session-alpha-sidebar-live",
    reason: "turn_complete"
  });
  await expect(alphaRow.locator(".pf-task-status")).toContainText("idle");
});

test("composer enter does not submit while IME composition is active", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-ime-compose",
        displayName: "IME compose",
        title: "IME compose",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });

  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /IME compose/);
  const composer = page.locator(".pf-composer textarea");
  await expect(composer).toBeEnabled();
  await composer.fill("zhong");

  await composer.evaluate((node) => {
    node.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true }));
    node.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "Enter",
        bubbles: true,
        cancelable: true,
        isComposing: true
      })
    );
    node.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "Enter",
        bubbles: true,
        cancelable: true,
        keyCode: 229
      })
    );
  });

  await page.waitForTimeout(50);
  await expect(composer).toHaveValue("zhong");
  expect(daemon.requests.filter((request) => request.method === "run_agent_turn")).toHaveLength(0);
});

test("composer moves submitted prompt into the thread while turn start is pending", async ({
  page
}) => {
  const prompt = "Render this send without a flash";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-smooth-send",
        displayName: "Smooth send",
        title: "Smooth send",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });
  daemon.delayResponse(
    "run_agent_turn",
    (request) => request.params.sessionId === "session-smooth-send",
    250
  );

  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Smooth send/);
  const composer = page.locator(".pf-composer textarea");
  await composer.fill(prompt);
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-smooth-send" &&
      request.params.message === prompt
  );

  await page.waitForTimeout(50);
  expect(await composer.inputValue()).toBe("");
  await expect(page.locator('.pf-msg[data-role="user"]').filter({ hasText: prompt })).toHaveCount(1);
  await expect(page.getByText("No messages in this session yet. Send a prompt to get started.")).toHaveCount(0);
});

test("rapid send activation submits the prompt only once", async ({ page }) => {
  const prompt = "Do not duplicate this prompt";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-rapid-send",
        displayName: "Rapid send",
        title: "Rapid send",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });
  daemon.delayResponse(
    "run_agent_turn",
    (request) => request.params.sessionId === "session-rapid-send",
    220
  );

  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Rapid send/);
  await page.locator(".pf-composer textarea").fill(prompt);
  await page.getByRole("button", { name: "Send", exact: true }).evaluate((button) => {
    (button as HTMLButtonElement).click();
    (button as HTMLButtonElement).click();
  });

  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-rapid-send" &&
      request.params.message === prompt
  );
  await expect(page.locator('.pf-msg[data-role="user"]').filter({ hasText: prompt })).toHaveCount(1);
  expect(
    daemon.requests.filter(
      (request) =>
        request.method === "run_agent_turn" &&
        request.params.sessionId === "session-rapid-send" &&
        request.params.message === prompt
    )
  ).toHaveLength(1);
});

test("sidebar marks the selected agent thinking while turn start is pending", async ({ page }) => {
  const prompt = "Show sidebar thinking state";
  const daemon = new FakeDaemon();
  daemon.delayResponse(
    "run_agent_turn",
    (request) => request.params.message === prompt,
    240
  );
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /^Browser regression\b/);
  await page.locator(".pf-composer textarea").fill(prompt);
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) => request.params.message === prompt
  );

  const activeRow = page.locator(".pf-sidebar-agent-row").filter({ hasText: "Browser regression" });
  await expect(activeRow).toContainText("thinking");
  await expect(activeRow.locator('.pf-puffer[data-state="thinking"]')).toBeVisible();
});

test("persisted prompt during pending turn replaces the optimistic row", async ({ page }) => {
  const prompt = "Persist this prompt once during title reload";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-pending-persist",
        displayName: "Pending persist",
        title: "Pending persist",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });
  daemon.delayResponse(
    "run_agent_turn",
    (request) => request.params.sessionId === "session-pending-persist",
    300
  );

  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Pending persist/);
  await page.locator(".pf-composer textarea").fill(prompt);
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-pending-persist" &&
      request.params.message === prompt
  );
  await expect(page.locator('.pf-msg[data-role="user"]').filter({ hasText: prompt })).toHaveCount(1);

  const loadRequestsBefore = daemon.requests.filter(
    (request) =>
      request.method === "load_session_detail" &&
      request.params.sessionId === "session-pending-persist"
  ).length;
  daemon.setSessionTimeline("session-pending-persist", [
    {
      kind: "user_message",
      id: "persisted-pending-user",
      text: prompt,
      createdAtMs: Date.now()
    }
  ]);
  daemon.emit("workspace:sessions:changed", {
    reason: "generated_title",
    sessionId: "session-pending-persist"
  });

  await expect
    .poll(() =>
      daemon.requests.filter(
        (request) =>
          request.method === "load_session_detail" &&
          request.params.sessionId === "session-pending-persist"
      ).length
    )
    .toBe(loadRequestsBefore + 1);
  await expect(page.locator('.pf-msg[data-role="user"]').filter({ hasText: prompt })).toHaveCount(1);
});

test("same text can be submitted again after a recent earlier turn", async ({ page }) => {
  const prompt = "Repeatable prompt text";
  const earlierTurnAt = Date.now() - 60_000;
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-repeat-prompt",
        displayName: "Repeat prompt",
        title: "Repeat prompt",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: earlierTurnAt,
        eventCount: 1,
        providerId: "codex",
        modelId: "test-model",
        timeline: [
          {
            kind: "user_message",
            id: "old-repeat-user",
            text: prompt,
            createdAtMs: earlierTurnAt
          }
        ]
      }
    ]
  });
  daemon.delayResponse(
    "run_agent_turn",
    (request) => request.params.sessionId === "session-repeat-prompt",
    240
  );

  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Repeat prompt/);
  await expect(page.locator('.pf-msg[data-role="user"]').filter({ hasText: prompt })).toHaveCount(1);
  await page.locator(".pf-composer textarea").fill(prompt);
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-repeat-prompt" &&
      request.params.message === prompt
  );

  await expect(page.locator('.pf-msg[data-role="user"]').filter({ hasText: prompt })).toHaveCount(2);
});

test("next turn start keeps previous live answer visible during reload", async ({ page }) => {
  const firstReply = "First answer should stay visible.";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-next-turn",
        displayName: "Next turn",
        title: "Next turn",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Next turn/);
  daemon.emit("session:session-next-turn:event", { type: "turn-start", turnId: "turn-first" });
  daemon.emit("session:session-next-turn:event", {
    type: "text-delta",
    turnId: "turn-first",
    delta: firstReply
  });
  await expect(page.getByText(firstReply)).toBeVisible();

  daemon.delayResponse(
    "load_session_detail",
    (request) => request.params.sessionId === "session-next-turn",
    360
  );
  daemon.emit("session:session-next-turn:event", {
    type: "turn-complete",
    turnId: "turn-first",
    assistantText: firstReply
  });
  await expect(page.getByText(firstReply)).toBeVisible();

  await page.locator(".pf-composer textarea").fill("Start the next turn");
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) => request.params.message === "Start the next turn"
  );
  daemon.emit("session:session-next-turn:event", {
    type: "turn-start",
    turnId: "turn-session-next-turn"
  });

  await expect(page.getByText(firstReply)).toBeVisible();
});

test("new turn can reuse a tool call id without replacing the previous live tool", async ({
  page
}) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-tool-reuse",
        displayName: "Tool reuse",
        title: "Tool reuse",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Tool reuse/);
  daemon.emit("session:session-tool-reuse:event", {
    type: "turn-start",
    turnId: "turn-tool-first"
  });
  daemon.emit("session:session-tool-reuse:event", {
    type: "tool-invocations",
    turnId: "turn-tool-first",
    invocations: [
      {
        callId: "call-reused",
        toolId: "FirstTool",
        input: "{\"path\":\"first.txt\"}",
        output: "first output",
        success: true
      }
    ]
  });
  await expect(page.locator(".pf-tool").filter({ hasText: "FirstTool" })).toHaveCount(1);

  daemon.delayResponse(
    "load_session_detail",
    (request) => request.params.sessionId === "session-tool-reuse",
    360
  );
  daemon.emit("session:session-tool-reuse:event", {
    type: "turn-complete",
    turnId: "turn-tool-first",
    assistantText: ""
  });
  await expect(page.locator(".pf-tool").filter({ hasText: "FirstTool" })).toHaveCount(1);

  daemon.emit("session:session-tool-reuse:event", {
    type: "turn-start",
    turnId: "turn-tool-second"
  });
  daemon.emit("session:session-tool-reuse:event", {
    type: "tool-invocations",
    turnId: "turn-tool-second",
    invocations: [
      {
        callId: "call-reused",
        toolId: "SecondTool",
        input: "{\"path\":\"second.txt\"}",
        output: "second output",
        success: true
      }
    ]
  });

  await expect(page.locator(".pf-tool").filter({ hasText: "FirstTool" })).toHaveCount(1);
  await expect(page.locator(".pf-tool").filter({ hasText: "SecondTool" })).toHaveCount(1);
});

test("transcript reload replaces pending live tool card when invocation event is missed", async ({
  page
}) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-pending-tool",
        displayName: "Pending tool",
        title: "Pending tool",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Pending tool/);
  daemon.emit("session:session-pending-tool:event", {
    type: "turn-start",
    turnId: "turn-pending-tool"
  });
  daemon.emit("session:session-pending-tool:event", {
    type: "tool-calls-requested",
    turnId: "turn-pending-tool",
    requests: [
      {
        callId: "call-pending",
        toolId: "Read",
        input: "{\"path\":\"README.md\"}"
      }
    ]
  });
  await expect(page.locator(".pf-tool").filter({ hasText: "Read" })).toHaveCount(1);
  await expect(page.locator(".pf-tool").filter({ hasText: "running" })).toHaveCount(1);

  daemon.setSessionTimeline("session-pending-tool", [
    {
      kind: "tool_call",
      id: "persisted-tool-call",
      toolId: "Read",
      status: "success",
      inputText: "{\"path\":\"README.md\"}",
      inputJson: { path: "README.md" },
      outputText: "{\"content\":\"done\"}",
      createdAtMs: baseTime + 1
    }
  ]);
  daemon.emit("session:session-pending-tool:event", {
    type: "turn-complete",
    turnId: "turn-pending-tool",
    assistantText: ""
  });

  await expect(page.locator(".pf-tool").filter({ hasText: "Read" })).toHaveCount(1);
  await expect(page.locator(".pf-tool").filter({ hasText: "running" })).toHaveCount(0);
});

test("stop turn is disabled until the daemon returns a turn id", async ({ page }) => {
  const prompt = "Wait for a real turn id before cancel";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-pending-cancel",
        displayName: "Pending cancel",
        title: "Pending cancel",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });
  daemon.delayResponse(
    "run_agent_turn",
    (request) => request.params.sessionId === "session-pending-cancel",
    240
  );

  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Pending cancel/);
  await page.locator(".pf-composer textarea").fill(prompt);
  await page.getByRole("button", { name: "Send" }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-pending-cancel" &&
      request.params.message === prompt
  );

  await expect(page.getByRole("button", { name: "Stop turn" })).toBeDisabled();
  await expect(page.getByRole("button", { name: "Stop turn" })).toBeEnabled({ timeout: 1_000 });
});

test("turn completion preserves live chat row identity after transcript reload", async ({ page }) => {
  const prompt = "Keep this row stable";
  const reply = "Stable streamed reply is visible.";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-stable-chat",
        displayName: "Stable chat",
        title: "Stable chat",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });

  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Stable chat/);
  await page.locator(".pf-composer textarea").fill(prompt);
  await page.getByRole("button", { name: "Send" }).click();

  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-stable-chat" &&
      request.params.message === prompt
  );

  const userRow = page.locator('.pf-msg[data-role="user"]').filter({ hasText: prompt }).last();
  await expect(userRow).toBeVisible();
  await userRow.evaluate((node) => node.setAttribute("data-probe", "local-user-row"));

  const turnId = "turn-session-stable-chat";
  daemon.emit("session:session-stable-chat:event", { type: "turn-start", turnId });
  daemon.emit("session:session-stable-chat:event", {
    type: "text-delta",
    turnId,
    delta: reply
  });

  const agentRow = page.locator('.pf-msg[data-role="agent"]').filter({ hasText: reply }).last();
  await expect(agentRow).toBeVisible();
  await agentRow.evaluate((node) => node.setAttribute("data-probe", "live-agent-row"));

  const loadRequestsBefore = daemon.requests.filter(
    (request) =>
      request.method === "load_session_detail" &&
      request.params.sessionId === "session-stable-chat"
  ).length;
  daemon.setSessionTimeline("session-stable-chat", [
    {
      kind: "user_message",
      id: "persisted-user-different-id",
      text: prompt,
      createdAtMs: baseTime + 1
    },
    {
      kind: "assistant_message",
      id: "persisted-assistant-different-id",
      text: reply,
      createdAtMs: baseTime + 2
    }
  ]);
  daemon.emit("session:session-stable-chat:event", {
    type: "turn-complete",
    turnId,
    assistantText: reply
  });

  await expect
    .poll(() =>
      daemon.requests.filter(
        (request) =>
          request.method === "load_session_detail" &&
          request.params.sessionId === "session-stable-chat"
      ).length
    )
    .toBe(loadRequestsBefore + 1);
  await expect(page.locator('.pf-msg[data-role="user"][data-probe="local-user-row"]')).toContainText(
    prompt
  );
  await expect(page.locator('.pf-msg[data-role="agent"][data-probe="live-agent-row"]')).toContainText(
    reply
  );
  await expect(page.locator('.pf-msg[data-role="user"]').filter({ hasText: prompt })).toHaveCount(1);
  await expect(page.locator('.pf-msg[data-role="agent"]').filter({ hasText: reply })).toHaveCount(1);
});

test("generated title reload does not duplicate the first submitted prompt", async ({ page }) => {
  const prompt = "First prompt should not flash twice";
  const reply = "First reply stays single.";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-title-race",
        displayName: "Title race",
        title: "Title race",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });

  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Title race/);
  await page.locator(".pf-composer textarea").fill(prompt);
  await page.getByRole("button", { name: "Send" }).click();

  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-title-race" &&
      request.params.message === prompt
  );
  const userRows = page.locator('.pf-msg[data-role="user"]').filter({ hasText: prompt });
  await expect(userRows).toHaveCount(1);

  const loadRequestsBefore = daemon.requests.filter(
    (request) =>
      request.method === "load_session_detail" &&
      request.params.sessionId === "session-title-race"
  ).length;
  daemon.setSessionTimeline("session-title-race", [
    {
      kind: "user_message",
      id: "persisted-first-user",
      text: prompt,
      createdAtMs: baseTime + 1
    }
  ]);
  daemon.emit("workspace:sessions:changed", {
    reason: "generated_title",
    sessionId: "session-title-race"
  });

  await expect
    .poll(() =>
      daemon.requests.filter(
        (request) =>
          request.method === "load_session_detail" &&
          request.params.sessionId === "session-title-race"
      ).length
    )
    .toBe(loadRequestsBefore + 1);
  await expect(userRows).toHaveCount(1);

  daemon.setSessionTimeline("session-title-race", [
    {
      kind: "user_message",
      id: "persisted-first-user",
      text: prompt,
      createdAtMs: baseTime + 1
    },
    {
      kind: "assistant_message",
      id: "persisted-first-assistant",
      text: reply,
      createdAtMs: baseTime + 2
    }
  ]);
  daemon.emit("session:session-title-race:event", {
    type: "turn-complete",
    turnId: "turn-session-title-race",
    assistantText: reply
  });

  await expect(userRows).toHaveCount(1);
  await expect(page.locator('.pf-msg[data-role="agent"]').filter({ hasText: reply })).toHaveCount(1);
});

test("clock-skewed transcript reload does not duplicate the submitted prompt", async ({
  page
}) => {
  const prompt = "Clock skew should not duplicate me";
  const reply = "Clock skew reply stays single.";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-clock-skew",
        displayName: "Clock skew",
        title: "Clock skew",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });

  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Clock skew/);
  await page.locator(".pf-composer textarea").fill(prompt);
  await page.getByRole("button", { name: "Send" }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-clock-skew" &&
      request.params.message === prompt
  );

  const userRows = page.locator('.pf-msg[data-role="user"]').filter({ hasText: prompt });
  await expect(userRows).toHaveCount(1);

  const loadRequestsBefore = daemon.requests.filter(
    (request) =>
      request.method === "load_session_detail" &&
      request.params.sessionId === "session-clock-skew"
  ).length;
  daemon.setSessionTimeline("session-clock-skew", [
    {
      kind: "user_message",
      id: "persisted-clock-skew-user",
      text: prompt,
      createdAtMs: baseTime - 10 * 60_000
    },
    {
      kind: "assistant_message",
      id: "persisted-clock-skew-assistant",
      text: reply,
      createdAtMs: baseTime - 10 * 60_000 + 1
    }
  ]);
  daemon.emit("session:session-clock-skew:event", {
    type: "turn-complete",
    turnId: "turn-session-clock-skew",
    assistantText: reply
  });

  await expect
    .poll(() =>
      daemon.requests.filter(
        (request) =>
          request.method === "load_session_detail" &&
          request.params.sessionId === "session-clock-skew"
      ).length
    )
    .toBe(loadRequestsBefore + 1);
  await expect(userRows).toHaveCount(1);
  await expect(page.locator('.pf-msg[data-role="agent"]').filter({ hasText: reply })).toHaveCount(1);
});

test("failed turn start keeps composer draft and avoids an unsent user row", async ({ page }) => {
  const prompt = "Do not lose this failed prompt";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-failed-start",
        displayName: "Failed start",
        title: "Failed start",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });

  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Failed start/);
  const composer = page.locator(".pf-composer textarea");
  await composer.fill(prompt);
  daemon.failNext("run_agent_turn", "daemon unavailable");
  await page.getByRole("button", { name: "Send" }).click();

  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-failed-start" &&
      request.params.message === prompt
  );

  await expect(page.getByText("Agent start failed")).toBeVisible();
  await expect(composer).toHaveValue(prompt);
  await expect(page.locator('.pf-msg[data-role="user"]').filter({ hasText: prompt })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Send" })).toBeEnabled();
});

test("turn errors keep the submitted prompt visible when it is not persisted", async ({ page }) => {
  const prompt = "Keep my prompt after turn error";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-turn-error",
        displayName: "Turn error",
        title: "Turn error",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });

  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Turn error/);
  await page.locator(".pf-composer textarea").fill(prompt);
  await page.getByRole("button", { name: "Send" }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-turn-error" &&
      request.params.message === prompt
  );
  await expect(page.locator('.pf-msg[data-role="user"]').filter({ hasText: prompt })).toHaveCount(1);

  const loadRequestsBefore = daemon.requests.filter(
    (request) =>
      request.method === "load_session_detail" &&
      request.params.sessionId === "session-turn-error"
  ).length;
  daemon.emit("session:session-turn-error:event", {
    type: "turn-start",
    turnId: "turn-session-turn-error"
  });
  daemon.emit("session:session-turn-error:event", {
    type: "turn-error",
    turnId: "turn-session-turn-error",
    error: "provider exploded before transcript append"
  });

  await expect
    .poll(() =>
      daemon.requests.filter(
        (request) =>
          request.method === "load_session_detail" &&
          request.params.sessionId === "session-turn-error"
      ).length
    )
    .toBe(loadRequestsBefore + 1);
  await expect(page.locator('.pf-msg[data-role="user"]').filter({ hasText: prompt })).toHaveCount(1);
  await expect(page.getByText("provider exploded before transcript append")).toBeVisible();
});

test("rapid turn errors keep separate inline error rows", async ({ page }) => {
  await page.addInitScript(() => {
    Date.now = () => 1_700_000_000_000;
  });
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /^Browser regression\b/);
  await expect(page.getByText("Ready to exercise the managed browser.")).toBeVisible();
  daemon.delayResponse(
    "load_session_detail",
    (request) => request.params.sessionId === "session-browser",
    400
  );
  daemon.delayResponse(
    "load_session_detail",
    (request) => request.params.sessionId === "session-browser",
    400
  );

  daemon.emit("session:session-browser:event", {
    type: "turn-error",
    turnId: "turn-error-first",
    error: "First rapid turn failure."
  });
  daemon.emit("session:session-browser:event", {
    type: "turn-error",
    turnId: "turn-error-second",
    error: "Second rapid turn failure."
  });

  await expect(page.getByText("First rapid turn failure.")).toBeVisible();
  await expect(page.getByText("Second rapid turn failure.")).toBeVisible();
});

test("unsent composer draft clears when switching sessions", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-alpha-draft",
        displayName: "Alpha draft",
        title: "Alpha draft",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "alpha-draft-seed",
            text: "Alpha draft seed",
            createdAtMs: baseTime - 30_000
          }
        ]
      },
      {
        sessionId: "session-beta-draft",
        displayName: "Beta draft",
        title: "Beta draft",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "beta-draft-seed",
            text: "Beta draft seed",
            createdAtMs: baseTime - 90_000
          }
        ]
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Alpha draft/);
  await expect(page.getByText("Alpha draft seed")).toBeVisible();
  const composer = page.locator(".pf-composer textarea");
  await composer.fill("alpha-only draft");
  await expect(composer).toHaveValue("alpha-only draft");

  await openSession(page, /Beta draft/);
  await expect(page.getByText("Beta draft seed")).toBeVisible();
  await expect(composer).toHaveValue("");
  await expect(page.getByRole("button", { name: "Send" })).toBeDisabled();
  await page.keyboard.press("Enter");
  await page.waitForTimeout(50);
  expect(daemon.requests.filter((request) => request.method === "run_agent_turn")).toHaveLength(0);
});

test("resolved transcript permissions do not reappear as pending approvals", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-resolved-permission",
        displayName: "Resolved permission",
        title: "Resolved permission",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 3,
        timeline: [
          {
            kind: "user_message",
            id: "perm-user",
            text: "Run the command.",
            createdAtMs: baseTime - 50_000
          },
          {
            kind: "permission_dialog",
            id: "perm-allowed",
            toolId: "bash",
            state: "allowed",
            summary: "bash was allowed",
            reason: "User approved this earlier.",
            inputText: "echo ok",
            createdAtMs: baseTime - 45_000
          },
          {
            kind: "assistant_message",
            id: "perm-assistant",
            text: "The command finished.",
            createdAtMs: baseTime - 40_000
          }
        ]
      }
    ]
  });

  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Resolved permission/);
  await expect(page.getByText("The command finished.")).toBeVisible();
  await expect(page.getByText("Approval needed")).toHaveCount(0);
  await expect(page.locator(".pf-agent-status-pill")).toHaveAttribute("data-status", "idle");
});

test("logged-out provider sessions cannot start new turns", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-anthropic-history",
        displayName: "Claude history",
        title: "Claude history",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        providerId: "anthropic",
        modelId: "test-model",
        timeline: [
          {
            kind: "assistant_message",
            id: "anthropic-seed",
            text: "Anthropic seed",
            createdAtMs: baseTime - 30_000
          }
        ]
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Claude history/);
  await expect(page.getByText("Anthropic seed")).toBeVisible();

  await page.getByRole("button", { name: "Settings" }).click();
  const accountRow = page.locator(".pf-settings-row").filter({ hasText: "Account" });
  await accountRow
    .locator("div", { hasText: /^anthropic\s*·/ })
    .getByRole("button", { name: "Sign out" })
    .click();
  const logout = await daemon.waitForRequest("logout_provider");
  expect(logout.params).toMatchObject({ providerId: "anthropic" });

  await page.getByRole("button", { name: "Workspace" }).click();
  await openSession(page, /Claude history/);
  const composer = page.locator(".pf-composer textarea");
  await expect(composer).toBeDisabled();
  await expect(page.locator(".pf-composer-hint")).toContainText(
    "Reconnect Claude to continue this session."
  );
  await expect(page.getByRole("button", { name: "Send" })).toBeDisabled();
  await page.keyboard.press("Enter");
  await page.waitForTimeout(50);
  expect(
    daemon.requests.filter((request) => request.method === "run_agent_turn")
  ).toHaveLength(0);
});

test("failed permission responses keep the approval prompt retryable", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /^Browser regression\b/);
  daemon.emit("session:session-browser:event", {
    type: "permission-request",
    turnId: "turn-permission",
    requestId: "permission-1",
    toolId: "bash",
    summary: "Run shell command",
    reason: "Needs workspace write access."
  });

  await expect(page.getByText("Approval needed")).toBeVisible();
  daemon.failNext("resolve_permission", "permission channel closed");
  await page.getByRole("button", { name: "Deny" }).click();

  const request = await daemon.waitForRequest("resolve_permission");
  expect(request.params).toMatchObject({
    turnId: "turn-permission",
    requestId: "permission-1",
    action: "deny"
  });
  await expect(page.getByText("Approval needed")).toBeVisible();
  await expect(page.getByRole("button", { name: "Deny" })).toBeVisible();
});

test("successful permission response clears the awaiting approval hint", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /^Browser regression\b/);
  daemon.emit("session:session-browser:event", {
    type: "permission-request",
    turnId: "turn-permission-success",
    requestId: "permission-success",
    toolId: "bash",
    summary: "Run approved shell command",
    reason: "Needs workspace write access."
  });

  await expect(page.getByText("Approval needed")).toBeVisible();
  await expect(page.getByText(/Awaiting approval/)).toBeVisible();
  await page.getByRole("button", { name: "Allow once" }).click();

  const request = await daemon.waitForRequest("resolve_permission");
  expect(request.params).toMatchObject({
    turnId: "turn-permission-success",
    requestId: "permission-success",
    action: "allow_once"
  });
  await expect(page.getByText("Approval needed")).toHaveCount(0);
  await expect(page.getByText(/Awaiting approval/)).toHaveCount(0);
  await expect(page.getByText(/Running/)).toBeVisible();
});

test("permission responses ignore duplicate clicks while the choice is in flight", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /^Browser regression\b/);
  daemon.delayResponse("resolve_permission", () => true, 500);
  daemon.emit("session:session-browser:event", {
    type: "permission-request",
    turnId: "turn-permission-duplicate",
    requestId: "permission-duplicate",
    toolId: "bash",
    summary: "Run duplicate approval command",
    reason: "Needs a single approval."
  });

  await expect(page.getByText("Needs a single approval.")).toBeVisible();
  const allowOnce = page.getByRole("button", { name: "Allow once" });
  await allowOnce.click();
  await allowOnce.click();

  const request = await daemon.waitForRequest("resolve_permission");
  expect(request.params).toMatchObject({
    turnId: "turn-permission-duplicate",
    requestId: "permission-duplicate",
    action: "allow_once"
  });
  await page.waitForTimeout(50);
  expect(
    daemon.requests.filter((request) => request.method === "resolve_permission")
  ).toHaveLength(1);
});

test("new turn can reuse a permission request id after earlier approval", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /^Browser regression\b/);
  daemon.emit("session:session-browser:event", {
    type: "permission-request",
    turnId: "turn-permission-first",
    requestId: "permission-reused",
    toolId: "bash",
    summary: "Approve first command",
    reason: "First turn needs approval."
  });

  await expect(page.getByText("First turn needs approval.")).toBeVisible();
  await page.getByRole("button", { name: "Allow once" }).click();
  await daemon.waitForRequest("resolve_permission", (request) =>
    request.params.turnId === "turn-permission-first" &&
    request.params.requestId === "permission-reused"
  );
  daemon.emit("session:session-browser:event", {
    type: "turn-complete",
    turnId: "turn-permission-first",
    assistantText: ""
  });

  daemon.emit("session:session-browser:event", { type: "turn-start", turnId: "turn-permission-second" });
  daemon.emit("session:session-browser:event", {
    type: "permission-request",
    turnId: "turn-permission-second",
    requestId: "permission-reused",
    toolId: "bash",
    summary: "Approve second command",
    reason: "Second turn reuses the backend request id."
  });

  await expect(page.getByText("Second turn reuses the backend request id.")).toBeVisible();
  await expect(page.getByText("Approval needed")).toBeVisible();
});

test("failed question responses keep the question prompt retryable", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /^Browser regression\b/);
  daemon.emit("session:session-browser:event", {
    type: "user-question-request",
    turnId: "turn-question",
    requestId: "question-1",
    questions: [
      {
        header: "Path",
        question: "Which path should I use?",
        options: [
          { label: "src", description: "Use the src directory." },
          { label: "tests", description: "Use the tests directory." }
        ]
      }
    ]
  });

  await expect(page.getByText("Which path should I use?")).toBeVisible();
  await page.getByPlaceholder("Type another answer").fill("examples");
  daemon.failNext("resolve_user_question", "question channel closed");
  await page.getByRole("button", { name: "Send answer" }).click();

  const request = await daemon.waitForRequest("resolve_user_question");
  expect(request.params).toMatchObject({
    turnId: "turn-question",
    requestId: "question-1",
    answers: { "Which path should I use?": "examples" },
    annotations: {}
  });
  await expect(page.getByText("Which path should I use?")).toBeVisible();
  await expect(page.getByRole("button", { name: "Send answer" })).toBeEnabled();
});

test("question responses ignore duplicate sends while the answer is in flight", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /^Browser regression\b/);
  daemon.delayResponse("resolve_user_question", () => true, 500);
  daemon.emit("session:session-browser:event", {
    type: "user-question-request",
    turnId: "turn-question-duplicate",
    requestId: "question-duplicate",
    questions: [
      {
        header: "Path",
        question: "Which duplicate path should I use?",
        options: [
          { label: "src", description: "Use the src directory." },
          { label: "tests", description: "Use the tests directory." }
        ]
      }
    ]
  });

  await expect(page.getByText("Which duplicate path should I use?")).toBeVisible();
  await page.getByPlaceholder("Type another answer").fill("examples");
  const submit = page.getByRole("button", { name: "Send answer" });
  await submit.click();
  await submit.click();

  const request = await daemon.waitForRequest("resolve_user_question");
  expect(request.params).toMatchObject({
    turnId: "turn-question-duplicate",
    requestId: "question-duplicate",
    answers: { "Which duplicate path should I use?": "examples" },
    annotations: {}
  });
  await page.waitForTimeout(50);
  expect(
    daemon.requests.filter((request) => request.method === "resolve_user_question")
  ).toHaveLength(1);
});

test("replayed approval and question events do not duplicate live prompts", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /^Browser regression\b/);
  const permissionEvent = {
    type: "permission-request",
    turnId: "turn-replayed-prompts",
    requestId: "permission-replayed",
    toolId: "bash",
    summary: "Run repeated shell command",
    reason: "Run repeated shell command"
  };
  daemon.emit("session:session-browser:event", permissionEvent);
  daemon.emit("session:session-browser:event", permissionEvent);

  await expect(page.locator(".pf-approval")).toHaveCount(1);
  await expect(page.locator(".pf-approval")).toContainText("Run repeated shell command");

  const questionEvent = {
    type: "user-question-request",
    turnId: "turn-replayed-prompts",
    requestId: "question-replayed",
    questions: [
      {
        header: "Target",
        question: "Which target should I use?",
        options: [
          { label: "src", description: "Use source." },
          { label: "tests", description: "Use tests." }
        ]
      }
    ]
  };
  daemon.emit("session:session-browser:event", questionEvent);
  daemon.emit("session:session-browser:event", questionEvent);

  await expect(page.locator(".pf-question")).toHaveCount(1);
  await expect(page.locator(".pf-question")).toContainText("Which target should I use?");
});

test("composer sends selected thinking option with the turn request", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /^Browser regression\b/);
  const thinkingSelect = page.getByLabel("Thinking level");
  await expect(thinkingSelect).toBeEnabled();
  await expect(thinkingSelect).toHaveValue("low");
  await thinkingSelect.selectOption("high");

  await page.locator(".pf-composer textarea").fill("Use high reasoning");
  await page.getByRole("button", { name: "Send" }).click();

  const request = await daemon.waitForRequest(
    "run_agent_turn",
    (item) => item.params.message === "Use high reasoning"
  );
  expect(request.params).toMatchObject({
    providerId: "codex",
    modelId: "test-model",
    thinkingOptionId: "high"
  });
});

test("composer sends fast mode and permission mode with the turn request", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-fast-controls",
        displayName: "Fast controls",
        title: "Fast controls",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "gpt-5",
        timeline: []
      }
    ],
    providerModels: {
      codex: [
        {
          id: "gpt-5",
          displayName: "GPT-5",
          provider: "codex",
          api: "openai-responses",
          contextWindow: 128000,
          maxOutputTokens: 4096,
          supportsReasoning: true,
          thinkingOptions: [
            {
              id: "medium",
              label: "Medium",
              description: "Use medium reasoning effort.",
              isDefault: true
            }
          ],
          defaultThinkingOptionId: "medium",
          isDefault: true
        }
      ]
    }
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Fast controls/);
  const fastToggle = page.locator(".pf-toggle-chip").filter({ hasText: "Fast" });
  await expect(fastToggle.locator("input")).toBeEnabled();
  await fastToggle.click();
  await page.getByLabel("Codex permissions").selectOption("full-access");

  await page.locator(".pf-composer textarea").fill("Use fast full access");
  await page.getByRole("button", { name: "Send" }).click();

  const request = await daemon.waitForRequest(
    "run_agent_turn",
    (item) => item.params.message === "Use fast full access"
  );
  expect(request.params).toMatchObject({
    providerId: "codex",
    modelId: "gpt-5",
    fastMode: true,
    permissionMode: "full-access"
  });
});

test("composer controls handle provider-prefixed session model ids", async ({ page }) => {
  const daemon = new FakeDaemon({
    auth: [
      {
        providerId: "openai",
        kind: "api_key",
        email: null,
        expiresAtMs: null,
        scopes: [],
        planType: null,
        organizationName: null
      }
    ],
    providers: [
      {
        id: "openai",
        displayName: "OpenAI",
        baseUrl: "",
        defaultApi: "openai-responses",
        modelCount: 1,
        authModes: ["api_key"],
        sourceKind: "test",
        sourcePath: null
      }
    ],
    sessions: [
      {
        sessionId: "session-prefixed-model",
        displayName: "Prefixed model",
        title: "Prefixed model",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "openai",
        modelId: "openai/gpt-5",
        timeline: []
      }
    ],
    providerModels: {
      openai: [
        {
          id: "gpt-5",
          displayName: "GPT-5",
          provider: "openai",
          api: "openai-responses",
          contextWindow: 128000,
          maxOutputTokens: 4096,
          supportsReasoning: true,
          thinkingOptions: [
            {
              id: "medium",
              label: "Medium",
              description: "Use medium reasoning effort.",
              isDefault: true
            },
            {
              id: "high",
              label: "High",
              description: "Use high reasoning effort."
            }
          ],
          defaultThinkingOptionId: "medium",
          isDefault: true
        }
      ]
    }
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Prefixed model/);
  const fastToggle = page.locator(".pf-toggle-chip").filter({ hasText: "Fast" });
  await expect(fastToggle.locator("input")).toBeEnabled();
  const thinkingSelect = page.getByLabel("Thinking level");
  await expect(thinkingSelect).toBeEnabled();
  await expect(thinkingSelect).toHaveValue("medium");
  await thinkingSelect.selectOption("high");

  await page.locator(".pf-composer textarea").fill("Use normalized model");
  await page.getByRole("button", { name: "Send" }).click();

  const request = await daemon.waitForRequest(
    "run_agent_turn",
    (item) => item.params.message === "Use normalized model"
  );
  expect(request.params).toMatchObject({
    providerId: "openai",
    modelId: "gpt-5",
    thinkingOptionId: "high"
  });
});

test("model picker only offers authenticated agent providers", async ({ page }) => {
  const daemon = new FakeDaemon({
    auth: [
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
        providerId: "github",
        kind: "oauth",
        email: "tester@example.com",
        expiresAtMs: null,
        scopes: [],
        planType: "test",
        organizationName: null
      }
    ],
    providers: [
      {
        id: "openai",
        displayName: "Codex",
        baseUrl: "",
        defaultApi: "openai-responses",
        modelCount: 1,
        authModes: ["oauth"],
        sourceKind: "test",
        sourcePath: null
      },
      {
        id: "github",
        displayName: "GitHub",
        baseUrl: "",
        defaultApi: "oauth",
        modelCount: 0,
        authModes: ["oauth"],
        sourceKind: "test",
        sourcePath: null
      }
    ],
    sessions: [
      {
        sessionId: "session-provider-picker-agent-only",
        displayName: "Agent provider picker",
        title: "Agent provider picker",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "openai",
        modelId: "gpt-5",
        timeline: []
      }
    ],
    providerModels: {
      openai: [
        {
          id: "gpt-5",
          displayName: "GPT-5",
          provider: "openai",
          api: "openai-responses",
          supportsTools: true,
          supportsVision: false,
          contextWindow: null,
          maxOutputTokens: null,
          thinkingOptions: [],
          defaultThinkingOptionId: null,
          isDefault: true
        }
      ]
    }
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Agent provider picker/);
  await page.locator(".pf-composer .picker .trigger").click();

  const providerList = page.locator(".pf-composer .picker .providers");
  await expect(providerList.getByRole("button", { name: "Codex", exact: true })).toBeVisible();
  await expect(providerList.getByRole("button", { name: "GitHub", exact: true })).toHaveCount(0);
});

for (const scenario of [
  {
    label: "Codex",
    providerId: "codex",
    canonicalProviderId: "openai",
    authKind: "oauth",
    providerName: /Codex/,
    assistantText: "Codex reply is visible in the UI."
  },
  {
    label: "Claude",
    providerId: "claude",
    canonicalProviderId: "anthropic",
    authKind: "api_key",
    providerName: /Claude/,
    assistantText: "Claude reply is visible in the UI."
  }
]) {
  test(`new ${scenario.label} agent can send a turn and render the reply`, async ({ page }) => {
    const daemon = new FakeDaemon({
      sessions: [],
      auth: [
        {
          providerId: scenario.providerId,
          kind: scenario.authKind,
          email: scenario.authKind === "oauth" ? "tester@example.com" : null,
          expiresAtMs: null,
          scopes: [],
          planType: scenario.authKind === "oauth" ? "test" : null,
          organizationName: null
        }
      ],
      providers: [
        {
          id: scenario.providerId,
          displayName: scenario.label,
          baseUrl: "",
          defaultApi:
            scenario.canonicalProviderId === "openai"
              ? "openai-responses"
              : "anthropic-messages",
          modelCount: 1,
          authModes: [scenario.authKind],
          sourceKind: "test",
          sourcePath: null
        }
      ]
    });
    await daemon.install(page);
    await daemon.open(page);

    await expect(page.getByRole("heading", { name: "No sessions yet" })).toBeVisible();
    await page.getByRole("button", { name: "New agent in default workspace" }).click();
    const dialog = page.getByRole("dialog", { name: "New agent" });
    await expect(dialog).toBeVisible();
    await expect(dialog.getByRole("radio", { name: scenario.providerName })).toBeVisible();
    await dialog.getByRole("button", { name: "Start agent" }).click();

    const createRequest = await daemon.waitForRequest("create_session");
    expect(createRequest.params).toMatchObject({
      cwd: "/tmp/puffer",
      providerId: scenario.canonicalProviderId
    });

    const composer = page.locator(".pf-composer textarea");
    await expect(page.getByText(/Reconnect .* to continue this session\./)).toHaveCount(0);
    await expect(composer).toBeEnabled();
    await composer.fill(`Hello from ${scenario.label}`);
    await page.getByRole("button", { name: "Send" }).click();

    const turnRequest = await daemon.waitForRequest(
      "run_agent_turn",
      (request) => request.params.message === `Hello from ${scenario.label}`
    );
    expect(turnRequest.params).toMatchObject({
      sessionId: "session-created-1",
      providerId: scenario.canonicalProviderId,
      modelId: "test-model"
    });

    const turnId = "turn-session-created-1";
    daemon.emit("session:session-created-1:event", { type: "turn-start", turnId });
    daemon.emit("session:session-created-1:event", {
      type: "text-delta",
      turnId,
      delta: scenario.assistantText
    });
    await expect(page.getByText(scenario.assistantText)).toBeVisible();
  });
}

test("new empty agent keeps first-message composer usable if detail load fails", async ({
  page
}) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await daemon.install(page);
  await daemon.open(page);

  await expect(page.getByRole("heading", { name: "No sessions yet" })).toBeVisible();
  await page.getByRole("button", { name: "New agent in default workspace" }).click();
  const dialog = page.getByRole("dialog", { name: "New agent" });
  await expect(dialog).toBeVisible();
  daemon.failNext("load_session_detail", "detail temporarily unavailable");
  await dialog.getByRole("button", { name: "Start agent" }).click();

  const createRequest = await daemon.waitForRequest("create_session");
  expect(createRequest.params).toMatchObject({
    cwd: "/tmp/puffer",
    providerId: "openai"
  });

  const composer = page.locator(".pf-composer textarea");
  await expect(page.getByText("Conversation load failed")).toBeVisible();
  await expect(page.getByText("detail temporarily unavailable")).toBeVisible();
  await expect(composer).toBeEnabled();
  await composer.fill("First prompt after detail failure");
  await page.getByRole("button", { name: "Send" }).click();

  const turnRequest = await daemon.waitForRequest(
    "run_agent_turn",
    (request) => request.params.message === "First prompt after detail failure"
  );
  expect(turnRequest.params).toMatchObject({
    sessionId: "session-created-1",
    providerId: "openai",
    modelId: "test-model"
  });
});

test("empty agent can recover by switching away from a disconnected provider", async ({
  page
}) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-empty-disconnected-provider",
        displayName: "Disconnected empty agent",
        title: "Disconnected empty agent",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "anthropic",
        modelId: "claude-sonnet-4-5",
        timeline: []
      }
    ],
    auth: [
      {
        providerId: "openai",
        kind: "oauth",
        email: "tester@example.com",
        expiresAtMs: null,
        scopes: [],
        planType: "test",
        organizationName: null
      }
    ],
    providers: [
      {
        id: "openai",
        displayName: "Codex",
        baseUrl: "",
        defaultApi: "openai-responses",
        modelCount: 1,
        authModes: ["oauth"],
        sourceKind: "test",
        sourcePath: null
      },
      {
        id: "anthropic",
        displayName: "Claude",
        baseUrl: "",
        defaultApi: "anthropic-messages",
        modelCount: 1,
        authModes: ["api_key"],
        sourceKind: "test",
        sourcePath: null
      }
    ],
    providerModels: {
      openai: [
        {
          id: "gpt-5",
          displayName: "GPT-5",
          provider: "openai",
          api: "openai-responses",
          supportsTools: true,
          supportsVision: false,
          contextWindow: null,
          maxOutputTokens: null,
          thinkingOptions: [],
          defaultThinkingOptionId: null,
          isDefault: true
        }
      ]
    }
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Disconnected empty agent/);
  await expect(page.getByText("No messages in this session yet. Send a prompt to get started.")).toBeVisible();
  const composer = page.locator(".pf-composer textarea");
  await expect(composer).toBeEnabled();
  await composer.fill("Use the connected provider");
  await expect(page.locator(".pf-composer-hint")).toContainText(
    "Switch to a connected provider"
  );
  await expect(page.getByRole("button", { name: "Send" })).toBeDisabled();

  await page.locator(".pf-composer .picker .trigger").click();
  await page.getByRole("button", { name: "Codex" }).click();
  await expect(page.locator(".pf-composer .picker .trigger")).toContainText("gpt-5");
  await expect(page.getByRole("button", { name: "Send" })).toBeEnabled();
  await page.getByRole("button", { name: "Send" }).click();

  const turnRequest = await daemon.waitForRequest(
    "run_agent_turn",
    (request) => request.params.message === "Use the connected provider"
  );
  expect(turnRequest.params).toMatchObject({
    sessionId: "session-empty-disconnected-provider",
    providerId: "openai",
    modelId: "gpt-5"
  });
});

test("empty agent does not recover through non-agent provider credentials", async ({
  page
}) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-empty-github-only",
        displayName: "GitHub only empty agent",
        title: "GitHub only empty agent",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "anthropic",
        modelId: "claude-sonnet-4-5",
        timeline: []
      }
    ],
    auth: [
      {
        providerId: "github",
        kind: "oauth",
        email: "tester@example.com",
        expiresAtMs: null,
        scopes: [],
        planType: "test",
        organizationName: null
      }
    ],
    providers: [
      {
        id: "github",
        displayName: "GitHub",
        baseUrl: "",
        defaultApi: "oauth",
        modelCount: 0,
        authModes: ["oauth"],
        sourceKind: "test",
        sourcePath: null
      },
      {
        id: "anthropic",
        displayName: "Claude",
        baseUrl: "",
        defaultApi: "anthropic-messages",
        modelCount: 1,
        authModes: ["api_key"],
        sourceKind: "test",
        sourcePath: null
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page, { forceOnboarding: true, skipOnboarding: false });
  await enterWorkspaceThroughForcedOnboarding(page);

  await openSession(page, /GitHub only empty agent/);
  const composer = page.locator(".pf-composer textarea");
  await expect(page.getByText("No messages in this session yet. Send a prompt to get started.")).toBeVisible();
  await expect(composer).toBeDisabled();
  await expect(page.locator(".pf-composer-hint")).toContainText(
    "Reconnect Claude to continue this session."
  );
  await expect(page.locator(".pf-composer .picker .trigger")).toBeDisabled();
  await page.getByRole("button", { name: "Send" }).evaluate((button) => {
    (button as HTMLButtonElement).click();
  });

  await page.waitForTimeout(50);
  expect(daemon.requests.filter((request) => request.method === "run_agent_turn")).toHaveLength(0);
});

test("stop turn requests cancellation for the active turn", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /^Browser regression\b/);
  await page.locator(".pf-composer textarea").fill("Cancel this turn");
  await page.getByRole("button", { name: "Send" }).click();

  await daemon.waitForRequest(
    "run_agent_turn",
    (request) => request.params.message === "Cancel this turn"
  );
  await expect(page.getByRole("button", { name: "Stop turn" })).toBeVisible();
  await page.getByRole("button", { name: "Stop turn" }).click();

  const cancelRequest = await daemon.waitForRequest("cancel_turn");
  expect(cancelRequest.params).toMatchObject({
    turnId: "turn-session-browser"
  });
});

test("stop turn is disabled while cancellation is in flight", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.delayResponse(
    "cancel_turn",
    (request) => request.params.turnId === "turn-session-browser",
    240
  );
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /^Browser regression\b/);
  await page.locator(".pf-composer textarea").fill("Cancel this turn once");
  await page.getByRole("button", { name: "Send" }).click();

  await daemon.waitForRequest(
    "run_agent_turn",
    (request) => request.params.message === "Cancel this turn once"
  );
  const stop = page.getByRole("button", { name: "Stop turn" });
  await expect(stop).toBeEnabled();

  await stop.click();
  await daemon.waitForRequest("cancel_turn");
  await expect(stop).toBeDisabled();
  await stop.click({ force: true });
  await page.waitForTimeout(40);

  expect(daemon.requests.filter((request) => request.method === "cancel_turn")).toHaveLength(1);
});

test("session title edit saves through the daemon", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-title-edit",
        displayName: "Title edit",
        title: "Title edit",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Title edit/);
  await page.getByRole("button", { name: "Edit session title" }).click();
  await page.getByLabel("Session title").fill("Renamed mission");
  await page.getByRole("button", { name: "Save title" }).click();

  const request = await daemon.waitForRequest("rename_session");
  expect(request.params).toMatchObject({
    sessionId: "session-title-edit",
    title: "Renamed mission"
  });
  await expect(page.locator(".primary-title")).toHaveText("Renamed mission");
  await expect(page.getByRole("button", { name: /Renamed mission/ }).first()).toBeVisible();
});

test("late title rename responses do not overwrite a switched session", async ({
  page
}) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-alpha-rename",
        displayName: "Alpha rename",
        title: "Alpha rename",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        timeline: []
      },
      {
        sessionId: "session-beta-rename",
        displayName: "Beta rename",
        title: "Beta rename",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        eventCount: 0,
        timeline: []
      }
    ]
  });
  daemon.delayResponse(
    "rename_session",
    (request) => request.params.sessionId === "session-alpha-rename",
    120
  );
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Alpha rename/);
  await page.getByRole("button", { name: "Edit session title" }).click();
  await page.getByLabel("Session title").fill("Alpha renamed late");
  await page.getByRole("button", { name: "Save title" }).click();
  await daemon.waitForRequest("rename_session", (request) =>
    request.params.sessionId === "session-alpha-rename" &&
    request.params.title === "Alpha renamed late"
  );

  await openSession(page, /Beta rename/);
  await expect(page.locator(".primary-title")).toHaveText("Beta rename");

  await page.waitForTimeout(170);
  await expect(page.locator(".primary-title")).toHaveText("Beta rename");
  await expect(page.getByText("Alpha renamed late")).toHaveCount(0);
});

test("auto recap does not start a second turn while one is running", async ({ page }) => {
  await page.clock.install({ time: baseTime });
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /^Browser regression\b/);
  await page.locator(".pf-composer textarea").fill("Keep this turn running");
  await page.getByRole("button", { name: "Send" }).click();

  await daemon.waitForRequest(
    "run_agent_turn",
    (request) => request.params.message === "Keep this turn running"
  );
  await expect(page.getByRole("button", { name: "Stop turn" })).toBeVisible();

  await page.evaluate(() => window.dispatchEvent(new Event("blur")));
  await page.clock.fastForward(180_001);
  await page.evaluate(() => Promise.resolve());

  expect(
    daemon.requests.filter(
      (request) => request.method === "run_agent_turn" && request.params.message === "/recap"
    )
  ).toHaveLength(0);
});

test("auto recap waits while the composer has an unsent draft", async ({ page }) => {
  await page.clock.install({ time: baseTime });
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /^Browser regression\b/);
  const composer = page.locator(".pf-composer textarea");
  await composer.fill("Half-written thought");

  await page.evaluate(() => window.dispatchEvent(new Event("blur")));
  await page.clock.fastForward(180_001);
  await page.evaluate(() => Promise.resolve());

  expect(
    daemon.requests.filter(
      (request) => request.method === "run_agent_turn" && request.params.message === "/recap"
    )
  ).toHaveLength(0);
  await expect(composer).toHaveValue("Half-written thought");
});

test("streamed assistant text stays visible through transcript reload", async ({ page }) => {
  const streamedText = "Streaming answer stays stable across reload.";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-streaming",
        displayName: "Streaming session",
        title: "Streaming session",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Streaming session/);
  await page.evaluate((phrase) => {
    const win = window as typeof window & {
      __chatSamples?: number[];
      __stopChatSampling?: () => void;
    };
    const samples: number[] = [];
    let stopped = false;
    const sample = () => {
      const text = document.querySelector(".pf-chat-thread")?.textContent ?? "";
      samples.push(text.split(phrase).length - 1);
      if (!stopped) window.requestAnimationFrame(sample);
    };
    win.__chatSamples = samples;
    win.__stopChatSampling = () => {
      stopped = true;
    };
    window.requestAnimationFrame(sample);
  }, streamedText);

  await page.locator(".pf-composer textarea").fill("Stream this answer");
  await page.getByRole("button", { name: "Send" }).click();
  const turnRequest = await daemon.waitForRequest(
    "run_agent_turn",
    (request) => request.params.sessionId === "session-streaming"
  );
  expect(turnRequest.params.message).toBe("Stream this answer");
  const turnId = "turn-session-streaming";
  daemon.emit("session:session-streaming:event", { type: "turn-start", turnId });
  daemon.emit("session:session-streaming:event", {
    type: "text-delta",
    turnId,
    delta: streamedText
  });

  await expect(page.getByText(streamedText)).toBeVisible();
  daemon.delayResponse(
    "load_session_detail",
    (request) => request.params.sessionId === "session-streaming",
    180
  );
  daemon.setSessionTimeline("session-streaming", [
    {
      kind: "user_message",
      id: "persisted-user",
      text: "Stream this answer",
      createdAtMs: baseTime + 1
    },
    {
      kind: "assistant_message",
      id: "persisted-assistant",
      text: streamedText,
      createdAtMs: baseTime + 2
    }
  ]);
  daemon.emit("session:session-streaming:event", {
    type: "turn-complete",
    turnId,
    assistantText: streamedText
  });

  await expect(page.getByText(streamedText)).toBeVisible();
  await page.waitForTimeout(260);
  const samples = await page.evaluate(() => {
    const win = window as typeof window & {
      __chatSamples?: number[];
      __stopChatSampling?: () => void;
    };
    win.__stopChatSampling?.();
    return win.__chatSamples ?? [];
  });
  const firstVisible = samples.findIndex((count) => count > 0);
  expect(firstVisible).toBeGreaterThanOrEqual(0);
  expect(samples.slice(firstVisible)).not.toContain(0);
  expect(Math.max(...samples.slice(firstVisible))).toBe(1);
});

test("final-only assistant text appears before delayed transcript reload", async ({ page }) => {
  const finalText = "Final-only answer appears before reload finishes.";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-final-only",
        displayName: "Final-only session",
        title: "Final-only session",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Final-only session/);
  await page.evaluate((phrase) => {
    const win = window as typeof window & {
      __chatSamples?: number[];
      __stopChatSampling?: () => void;
    };
    const samples: number[] = [];
    let stopped = false;
    const sample = () => {
      const text = document.querySelector(".pf-chat-thread")?.textContent ?? "";
      samples.push(text.split(phrase).length - 1);
      if (!stopped) window.requestAnimationFrame(sample);
    };
    win.__chatSamples = samples;
    win.__stopChatSampling = () => {
      stopped = true;
    };
    window.requestAnimationFrame(sample);
  }, finalText);

  await page.locator(".pf-composer textarea").fill("Return a final-only answer");
  await page.getByRole("button", { name: "Send" }).click();
  const turnRequest = await daemon.waitForRequest(
    "run_agent_turn",
    (request) => request.params.sessionId === "session-final-only"
  );
  expect(turnRequest.params.message).toBe("Return a final-only answer");
  const turnId = "turn-session-final-only";
  daemon.emit("session:session-final-only:event", { type: "turn-start", turnId });

  daemon.delayResponse(
    "load_session_detail",
    (request) => request.params.sessionId === "session-final-only",
    220
  );
  daemon.setSessionTimeline("session-final-only", [
    {
      kind: "user_message",
      id: "persisted-user-final-only",
      text: "Return a final-only answer",
      createdAtMs: baseTime + 1
    },
    {
      kind: "assistant_message",
      id: "persisted-assistant-final-only",
      text: finalText,
      createdAtMs: baseTime + 2
    }
  ]);
  daemon.emit("session:session-final-only:event", {
    type: "turn-complete",
    turnId,
    assistantText: finalText
  });

  await page.waitForTimeout(80);
  const preReloadSamples = await page.evaluate(() => {
    const win = window as typeof window & {
      __chatSamples?: number[];
    };
    return win.__chatSamples ?? [];
  });
  expect(Math.max(...preReloadSamples)).toBeGreaterThan(0);

  await expect(page.getByText(finalText)).toBeVisible();
  await page.waitForTimeout(300);
  const samples = await page.evaluate(() => {
    const win = window as typeof window & {
      __chatSamples?: number[];
      __stopChatSampling?: () => void;
    };
    win.__stopChatSampling?.();
    return win.__chatSamples ?? [];
  });
  const firstVisible = samples.findIndex((count) => count > 0);
  expect(firstVisible).toBeGreaterThanOrEqual(0);
  expect(samples.slice(firstVisible)).not.toContain(0);
  expect(Math.max(...samples.slice(firstVisible))).toBe(1);
});

test("replayed turn-start does not clear visible streamed text", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /^Browser regression\b/);
  daemon.emit("session:session-browser:event", {
    type: "text-delta",
    turnId: "turn-replay",
    delta: "Visible text before replay."
  });
  await expect(page.getByText("Visible text before replay.")).toBeVisible();

  daemon.emit("session:session-browser:event", {
    type: "turn-start",
    turnId: "turn-replay",
    replay: true
  });

  await expect(page.getByText("Visible text before replay.")).toBeVisible();
});

test("replayed text deltas only fill missing streamed text", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /^Browser regression\b/);
  const turnId = "turn-replay-delta";
  daemon.emit("session:session-browser:event", {
    type: "text-delta",
    turnId,
    delta: "ha"
  });
  const latestAgentParagraph = page.locator('.pf-msg[data-role="agent"] p').last();
  await expect(latestAgentParagraph).toHaveText("ha");

  daemon.emit("session:session-browser:event", {
    type: "text-delta",
    turnId,
    delta: "ha",
    replay: true
  });
  await expect(latestAgentParagraph).toHaveText("ha");

  daemon.emit("session:session-browser:event", {
    type: "text-delta",
    turnId,
    delta: "ha",
    replay: true
  });
  await expect(latestAgentParagraph).toHaveText("haha");
  await expect(latestAgentParagraph).not.toHaveText("hahaha");
});

test("stale turn reloads do not clear the active streamed answer", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-overlap",
        displayName: "Overlap session",
        title: "Overlap session",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Overlap session/);
  daemon.delayResponse(
    "load_session_detail",
    (request) => request.params.sessionId === "session-overlap",
    180
  );
  daemon.setSessionTimeline("session-overlap", [
    {
      kind: "assistant_message",
      id: "persisted-old",
      text: "Persisted old answer.",
      createdAtMs: baseTime + 1
    }
  ]);

  daemon.emit("session:session-overlap:event", { type: "turn-start", turnId: "turn-old" });
  daemon.emit("session:session-overlap:event", {
    type: "text-delta",
    turnId: "turn-old",
    delta: "Transient old answer."
  });
  daemon.emit("session:session-overlap:event", {
    type: "turn-complete",
    turnId: "turn-old",
    assistantText: "Persisted old answer."
  });

  daemon.emit("session:session-overlap:event", { type: "turn-start", turnId: "turn-new" });
  daemon.emit("session:session-overlap:event", {
    type: "text-delta",
    turnId: "turn-new",
    delta: "Current answer must stay visible."
  });
  await expect(page.getByText("Current answer must stay visible.")).toBeVisible();

  await page.waitForTimeout(260);
  await expect(page.getByText("Current answer must stay visible.")).toBeVisible();

  daemon.emit("session:session-overlap:event", {
    type: "text-delta",
    turnId: "turn-old",
    delta: "Late stale text should be ignored."
  });
  await expect(page.getByText("Late stale text should be ignored.")).toHaveCount(0);
  await expect(page.getByText("Current answer must stay visible.")).toBeVisible();
});

test("streaming agent row keeps its DOM identity without a local user row", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-remote-stream",
        displayName: "Remote stream",
        title: "Remote stream",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Remote stream/);
  daemon.emit("session:session-remote-stream:event", {
    type: "turn-start",
    turnId: "turn-remote-stream"
  });
  daemon.emit("session:session-remote-stream:event", {
    type: "text-delta",
    turnId: "turn-remote-stream",
    delta: "Identity"
  });
  await expect(page.getByText("Identity")).toBeVisible();

  await page.evaluate(() => {
    const win = window as typeof window & {
      __agentRowStillConnected?: () => boolean;
    };
    const row = document.querySelector(".pf-msg[data-role='agent']");
    win.__agentRowStillConnected = () => row?.isConnected === true;
  });

  daemon.emit("session:session-remote-stream:event", {
    type: "text-delta",
    turnId: "turn-remote-stream",
    delta: " safe"
  });
  daemon.emit("session:session-remote-stream:event", {
    type: "text-delta",
    turnId: "turn-remote-stream",
    delta: " stream"
  });

  await expect(page.getByText("Identity safe stream")).toBeVisible();
  await expect.poll(() =>
    page.evaluate(() => {
      const win = window as typeof window & {
        __agentRowStillConnected?: () => boolean;
      };
      return win.__agentRowStillConnected?.() ?? false;
    })
  ).toBe(true);
});

import { expect, type Page, test } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";

const baseTime = Date.now();

async function openSession(page: Page, name: RegExp): Promise<void> {
  await page.getByRole("button", { name }).first().click();
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

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

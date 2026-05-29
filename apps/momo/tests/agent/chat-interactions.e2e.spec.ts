/**
 * Daemon chat interaction e2e (Task 8).
 *
 * momo's old chat link left permission approval / askUserQuestion as no-ops;
 * after the daemon migration these are live, so they need real end-to-end
 * coverage: a daemon event must render the right inline card, a click must
 * fire the right resolve RPC back to the daemon. Cancel-mid-turn is covered
 * too (it replaces the deleted legacy stop-button spec, rewritten against the
 * new ConversationView + shell Composer surface).
 *
 * These drive the full stack: shell Composer → create_session/run_agent_turn
 * → `daemon.emit("session:<id>:event", …)` → the chat controller's reducer
 * (agentChat.svelte.ts) → ConversationView/Approval/QuestionPrompt DOM →
 * click → resolve_permission / resolve_user_question / cancel_turn assertion.
 *
 * Selectors are read off the real components, not guessed:
 *   - Approval.svelte renders `.pf-approval` with a button per `item.choices`
 *     (text "Approve once" / "Always allow" / "Deny").
 *   - QuestionPrompt.svelte renders `.pf-question` with `.pf-question-option`
 *     radios and a "Send answer" submit button.
 *   - shell Composer renders the Stop button with `aria-label="Stop"` while a
 *     turn is running.
 *
 * Boots via `bootOnboarded` (default legacy protocol — NOT protocol:"real",
 * which would break the 1431 wsClient handshake) exactly like chat-smoke.
 */
import { expect, test } from "@playwright/test";
import { FakeDaemon } from "../support/fakeDaemon";
import { bootOnboarded } from "../support/bootHelpers";
import { emitTurnStart } from "../support/chatEmit";

/**
 * Send the home composer's first message, land on the new session's agent
 * route, and return the conventional ids the FakeDaemon mints:
 *   - sessionId: `session-created-1` (first new chat with no seeded sessions)
 *   - turnId:    `turn-<sessionId>` (FakeDaemon.runAgentTurn synthesizes this)
 * After this resolves the controller has bound the live turn to that turnId,
 * so subsequent `session:<id>:event` frames target the right stream.
 */
async function startTurnFromHome(
  page: import("@playwright/test").Page,
  daemon: FakeDaemon,
  prompt = "Do the thing"
): Promise<{ sessionId: string; turnId: string }> {
  await page.getByLabel("Message").fill(prompt);
  await page.getByLabel("Message").press("Enter");

  await daemon.waitForRequest("create_session");
  const sessionId = "session-created-1";
  await daemon.waitForRequest(
    "run_agent_turn",
    (req) => req.params.sessionId === sessionId && req.params.message === prompt
  );
  await expect(page).toHaveURL(new RegExp(`#/agent/${sessionId}$`));
  return { sessionId, turnId: `turn-${sessionId}` };
}

test("permission-request renders an Approval card; clicking Approve sends resolve_permission", async ({
  page
}) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);
  const { sessionId, turnId } = await startTurnFromHome(page, daemon);

  // Stream a turn that pauses on a tool permission gate. The reducer routes
  // permission-request into pendingPermissions, which Agent.svelte feeds to
  // ConversationView, which renders an <Approval> card under the agent row.
  emitTurnStart(daemon, { sessionId, turnId });
  const requestId = "perm-req-1";
  daemon.emit(`session:${sessionId}:event`, {
    type: "permission-request",
    turnId,
    requestId,
    toolId: "bash",
    summary: "Run `ls -la`",
    reason: "List the working directory"
  });

  // Approval.svelte: `.pf-approval` wrapper, with the tool name and the
  // reason text rendered inline.
  const approval = page.locator(".pf-approval");
  await expect(approval).toBeVisible();
  await expect(approval).toContainText("Approval needed");
  await expect(approval).toContainText("bash");
  await expect(approval).toContainText("List the working directory");

  // The choice buttons are the literal `item.choices` strings. For a
  // non-browser permission the reducer mints ["Approve once","Always allow",
  // "Deny"]. Clicking "Approve once" maps (via mapPermissionAction) to the
  // "allow_once" action on the wire.
  const resolvePromise = daemon.waitForRequest(
    "resolve_permission",
    (req) => req.params.turnId === turnId && req.params.requestId === requestId
  );
  await approval.getByRole("button", { name: "Approve once" }).click();

  const resolve = await resolvePromise;
  expect(resolve.params).toMatchObject({
    turnId,
    requestId,
    action: "allow_once"
  });

  // After the daemon answers resolve_permission ({}), the controller dismisses
  // the id and the card drops out of pendingPermissions.
  await expect(approval).toHaveCount(0);
});

test("user-question-request renders a QuestionPrompt; answering sends resolve_user_question", async ({
  page
}) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);
  const { sessionId, turnId } = await startTurnFromHome(page, daemon, "Which deploy target?");

  emitTurnStart(daemon, { sessionId, turnId });
  const requestId = "q-req-1";
  daemon.emit(`session:${sessionId}:event`, {
    type: "user-question-request",
    turnId,
    requestId,
    questions: [
      {
        question: "Pick an environment",
        header: "Deploy",
        type: "choice",
        options: [
          { label: "Staging", description: "Safe preview" },
          { label: "Production", description: "Live traffic" }
        ]
      }
    ]
  });

  // QuestionPrompt.svelte: `.pf-question` form with `.pf-question-option`
  // radios and a "Send answer" submit button.
  const question = page.locator(".pf-question");
  await expect(question).toBeVisible();
  await expect(question).toContainText("Pick an environment");

  // "Send answer" is disabled until an option is chosen (canSubmit gate).
  const sendAnswer = question.getByRole("button", { name: "Send answer" });
  await expect(sendAnswer).toBeDisabled();

  // Select the "Staging" radio. The option label is the visible text inside
  // `.pf-question-option`; click the option label to flip its radio.
  await question.getByText("Staging", { exact: true }).click();
  await expect(sendAnswer).toBeEnabled();

  const resolvePromise = daemon.waitForRequest(
    "resolve_user_question",
    (req) => req.params.turnId === turnId && req.params.requestId === requestId
  );
  await sendAnswer.click();

  const resolve = await resolvePromise;
  // The answer is keyed by the question text (answerKeyFor falls back to the
  // raw question when it's unique) and carries the chosen label.
  expect(resolve.params).toMatchObject({
    turnId,
    requestId,
    answers: { "Pick an environment": "Staging" }
  });

  // After the daemon answers resolve_user_question ({}), the controller
  // dismisses the question id; it drops out of pendingQuestions and the inline
  // prompt is removed (momo renders questions only while pending — the live
  // item never persists into the timeline as an "answered" card).
  await expect(question).toHaveCount(0);
});

test("Stop button during a running turn sends cancel_turn", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);
  const { sessionId, turnId } = await startTurnFromHome(page, daemon, "Long running task");

  // The turn is live (run_agent_turn resolved → controller.currentTurnId set,
  // and FakeDaemon.runAgentTurn added the turn to activeTurnIds so cancel_turn
  // succeeds). turn-start keeps the running state pinned for the cancel path.
  emitTurnStart(daemon, { sessionId, turnId });

  // While running, the shell Composer swaps Send for a Stop button.
  const stop = page.getByRole("button", { name: "Stop" });
  await expect(stop).toBeVisible();

  const cancelPromise = daemon.waitForRequest(
    "cancel_turn",
    (req) => req.params.turnId === turnId
  );
  await stop.click();

  const cancel = await cancelPromise;
  expect(cancel.params).toMatchObject({ turnId });
});

test("tool-calls-requested renders a running ToolCard that flips to success", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);
  const { sessionId, turnId } = await startTurnFromHome(page, daemon, "Read a file");

  emitTurnStart(daemon, { sessionId, turnId });

  // A requested tool call renders a running pill. With a single child and no
  // final assistant text yet, ConversationView renders the ToolCard directly
  // (not the collapsed activity group). ToolCard surfaces the tool name and a
  // status; the running state shows the "running" data-state on the head.
  const callId = "call-1";
  daemon.emit(`session:${sessionId}:event`, {
    type: "tool-calls-requested",
    turnId,
    requests: [{ callId, toolId: "read_file", input: '{"path":"notes.md"}' }]
  });

  const tool = page.locator(".pf-tool");
  await expect(tool).toBeVisible();
  await expect(tool).toContainText("read_file");
  // ToolCard.statusLabel maps "running" → "running"; the status pill carries
  // data-state on `.pf-tool-status`.
  await expect(tool.locator('.pf-tool-status[data-state="running"]')).toBeVisible();

  // BubbleConversation renders ToolCard with the default `defaultCollapsed`
  // (true), so a lone running tool starts as a collapsed pill. The head is a
  // toggle (a pending tool is toggleable), so clicking it expands the card.
  // The pill↔card bubble.css skin keys off this `data-collapsed` attribute.
  await expect(tool).toHaveAttribute("data-collapsed", "true");
  await tool.locator(".pf-tool-head").click();
  await expect(tool).toHaveAttribute("data-collapsed", "false");
  // Raw toolId still shows in dev (SHOW_RAW_AGENT_ACTIVITY === import.meta.env.DEV).
  await expect(tool).toContainText("read_file");
  // Collapse back so the success-flip assertions below run against the pill.
  await tool.locator(".pf-tool-head").click();
  await expect(tool).toHaveAttribute("data-collapsed", "true");

  // The matching invocation (same callId + turnId) upgrades the pill in place.
  daemon.emit(`session:${sessionId}:event`, {
    type: "tool-invocations",
    turnId,
    invocations: [
      {
        callId,
        toolId: "read_file",
        input: '{"path":"notes.md"}',
        output: "file body",
        success: true
      }
    ]
  });

  // Still a single tool card (upgraded, not duplicated). A successful
  // invocation maps (statusLabel) to the "done" state — not "running".
  await expect(tool).toHaveCount(1);
  await expect(tool.locator('.pf-tool-status[data-state="running"]')).toHaveCount(0);
  await expect(tool.locator('.pf-tool-status[data-state="done"]')).toBeVisible();
});

test("user + assistant text render as left/right bubbles", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);
  const { sessionId, turnId } = await startTurnFromHome(page, daemon, "Say hi");
  emitTurnStart(daemon, { sessionId, turnId });
  daemon.emit(`session:${sessionId}:event`, { type: "text-delta", turnId, delta: "Hello there" });

  await expect(page.locator('.mb-row[data-role="user"] .mb-bubble')).toContainText("Say hi");
  await expect(page.locator('.mb-row[data-role="assistant"] .mb-bubble')).toContainText("Hello there");
});

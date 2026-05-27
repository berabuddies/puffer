/**
 * Task 5 — interactive answer form (askUserQuestion).
 *
 * When the agent emits `user-question-request`, an `<form
 * data-testid="answer-form">` renders inline ahead of the (still-pending)
 * assistant bubble. Single-select MVP — multi-question / multi-select
 * are deferred (see plan's Out-of-Scope table).
 *
 * On submit we POST `resolve_user_question(turnId, requestId, answers)`
 * where `answers` is keyed by question index (`{ "0": "Alice" }`). The
 * FakeDaemon accepts the RPC and returns `{}` by default (see
 * `tests/support/fakeDaemon.ts:1309`); production momo routes the same
 * call through `BackendState::resolve_user_question` to unblock the
 * pending worker via mpsc (see
 * `apps/momo/src-tauri/src/backend.rs::resolve_user_question`).
 *
 * Coverage:
 *   - render on event arrival
 *   - single-select pick + repick
 *   - submit fires `resolve_user_question` with the chosen answer
 *   - answered form locks (disabled buttons, submit hidden)
 *   - submit disabled until an option is picked
 *   - form slots BEFORE the pending assistant bubble for that turn
 *   - RPC failure rolls back to interactive + surfaces a toast
 *   - two concurrent questions with distinct requestIds don't collide
 *   - typing-dot suppression while a question is unanswered
 */
import { expect, test, type Page } from "@playwright/test";
import { FakeDaemon } from "../support/fakeDaemon";
import { bootOnboarded } from "../support/bootHelpers";
import {
  emitTurnStart,
  emitTextDelta,
  emitQuestion,
  emitTurnComplete
} from "../support/chatEmit";
import { locateQuestionForm } from "../support/chatLocators";

/**
 * Stand up an onboarded page, submit a prompt to mint a session, and
 * fire `turn-start` so the assistant bubble exists. Returns the ids so
 * the test body can drive question events without re-deriving the
 * stringified sessionId / turnId.
 */
async function bootAndStartTurn(
  page: Page,
  daemon: FakeDaemon,
  prompt = "ask me something"
): Promise<{ sessionId: string; turnId: string }> {
  await page.getByLabel("Message").fill(prompt);
  await page.getByLabel("Message").press("Enter");
  await daemon.waitForRequest("create_session");
  const sessionId = "session-created-1";
  await daemon.waitForRequest("run_agent_turn");
  const turnId = `turn-${sessionId}`;
  emitTurnStart(daemon, { sessionId, turnId });
  return { sessionId, turnId };
}

test("user-question-request renders the form with the question heading", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);
  const { sessionId, turnId } = await bootAndStartTurn(page, daemon);

  emitQuestion(daemon, {
    sessionId,
    turnId,
    requestId: "q-render-1",
    questions: [
      {
        question: "Pick a recipient",
        options: [
          { label: "Alice", description: "Mom" },
          { label: "Bob", description: "Roommate" }
        ]
      }
    ]
  });

  const form = locateQuestionForm(page);
  await expect(form).toBeVisible();
  await expect(form.locator(".answer-form__heading")).toHaveText("Pick a recipient");
  await expect(form.locator("[data-option-label]")).toHaveCount(2);

  emitTurnComplete(daemon, { sessionId, turnId });
});

test("single-select: picking toggles data-selected; repicking moves it", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);
  const { sessionId, turnId } = await bootAndStartTurn(page, daemon);

  emitQuestion(daemon, {
    sessionId,
    turnId,
    requestId: "q-pick-1",
    questions: [
      {
        question: "Yes or no?",
        options: [{ label: "Yes" }, { label: "No" }]
      }
    ]
  });

  const yes = page.locator('[data-option-label="Yes"]');
  const no = page.locator('[data-option-label="No"]');
  await expect(yes).toHaveAttribute("data-selected", "false");
  await expect(no).toHaveAttribute("data-selected", "false");

  await yes.click();
  await expect(yes).toHaveAttribute("data-selected", "true");
  await expect(no).toHaveAttribute("data-selected", "false");

  await no.click();
  await expect(yes).toHaveAttribute("data-selected", "false");
  await expect(no).toHaveAttribute("data-selected", "true");

  emitTurnComplete(daemon, { sessionId, turnId });
});

test("submit fires resolve_user_question with the picked answer", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);
  const { sessionId, turnId } = await bootAndStartTurn(page, daemon);

  emitQuestion(daemon, {
    sessionId,
    turnId,
    requestId: "q-submit-1",
    questions: [
      {
        question: "Pick a recipient",
        options: [{ label: "Alice" }, { label: "Bob" }]
      }
    ]
  });

  await page.locator('[data-option-label="Alice"]').click();
  await page.locator('[data-testid="answer-form-submit"]').click();

  const req = await daemon.waitForRequest(
    "resolve_user_question",
    (r) =>
      r.params.requestId === "q-submit-1" &&
      (r.params.answers as Record<string, string>)["0"] === "Alice"
  );
  // Sanity-check the rest of the payload while we have it.
  expect(req.params.turnId).toBe(turnId);

  emitTurnComplete(daemon, { sessionId, turnId });
});

test("answered form locks: data-answered=true, options disabled, submit hidden", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);
  const { sessionId, turnId } = await bootAndStartTurn(page, daemon);

  emitQuestion(daemon, {
    sessionId,
    turnId,
    requestId: "q-lock-1",
    questions: [
      {
        question: "Pick one",
        options: [{ label: "A" }, { label: "B" }]
      }
    ]
  });

  await page.locator('[data-option-label="A"]').click();
  await page.locator('[data-testid="answer-form-submit"]').click();
  await daemon.waitForRequest(
    "resolve_user_question",
    (r) => r.params.requestId === "q-lock-1"
  );

  const form = locateQuestionForm(page);
  await expect(form).toHaveAttribute("data-answered", "true");
  await expect(page.locator('[data-option-label="A"]')).toBeDisabled();
  await expect(page.locator('[data-option-label="B"]')).toBeDisabled();
  // Submit is hidden once answered (component drops it from the DOM).
  await expect(page.locator('[data-testid="answer-form-submit"]')).toHaveCount(0);

  // Clicking a disabled option is a no-op (selection stays on A).
  await page.locator('[data-option-label="B"]').click({ force: true }).catch(() => {});
  await expect(page.locator('[data-option-label="A"]')).toHaveAttribute("data-selected", "true");

  emitTurnComplete(daemon, { sessionId, turnId });
});

test("submit is disabled until an option is picked", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);
  const { sessionId, turnId } = await bootAndStartTurn(page, daemon);

  emitQuestion(daemon, {
    sessionId,
    turnId,
    requestId: "q-disabled-1",
    questions: [
      {
        question: "Pick one",
        options: [{ label: "A" }, { label: "B" }]
      }
    ]
  });

  const submit = page.locator('[data-testid="answer-form-submit"]');
  await expect(submit).toBeDisabled();

  await page.locator('[data-option-label="A"]').click();
  await expect(submit).toBeEnabled();

  emitTurnComplete(daemon, { sessionId, turnId });
});

test("form renders BEFORE the assistant final answer for the same turn", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);
  const { sessionId, turnId } = await bootAndStartTurn(page, daemon);

  emitQuestion(daemon, {
    sessionId,
    turnId,
    requestId: "q-order-1",
    questions: [
      {
        question: "Choose path",
        options: [{ label: "Left" }, { label: "Right" }]
      }
    ]
  });
  emitTextDelta(daemon, { sessionId, turnId, delta: "Going left it is." });
  emitTurnComplete(daemon, {
    sessionId,
    turnId,
    assistantText: "Going left it is."
  });

  // Pull the inner thread once both nodes are present, then compare
  // bounding boxes — the form must sit above the assistant bubble.
  const form = locateQuestionForm(page);
  const assistantBubble = page.locator(".assistant-bubble").last();
  await expect(form).toBeVisible();
  await expect(assistantBubble).toContainText("Going left it is.");

  const formBox = await form.boundingBox();
  const assistantBox = await assistantBubble.boundingBox();
  expect(formBox).not.toBeNull();
  expect(assistantBox).not.toBeNull();
  // Compare top coordinates — strictly less means the form is rendered
  // ahead of the bubble in document flow.
  expect(formBox!.y).toBeLessThan(assistantBox!.y);
});

test("RPC failure rolls back answered + surfaces toast", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);
  const { sessionId, turnId } = await bootAndStartTurn(page, daemon);

  emitQuestion(daemon, {
    sessionId,
    turnId,
    requestId: "q-fail-1",
    questions: [
      {
        question: "Pick one",
        options: [{ label: "A" }, { label: "B" }]
      }
    ]
  });

  daemon.failNext("resolve_user_question", "no such request");
  await page.locator('[data-option-label="A"]').click();
  await page.locator('[data-testid="answer-form-submit"]').click();

  // Toast surfaces and matches `Could not send answer: …` per
  // answerQuestion's catch block (chat.svelte.ts).
  const errorToast = page
    .locator(".toast")
    .filter({ hasText: "Could not send answer:" })
    .filter({ hasText: "no such request" });
  await expect(errorToast).toBeVisible({ timeout: 2500 });

  // Form returns to the interactive state — data-answered flips back to
  // "false" and options re-enable.
  const form = locateQuestionForm(page);
  await expect(form).toHaveAttribute("data-answered", "false");
  await expect(page.locator('[data-option-label="A"]')).toBeEnabled();
  await expect(page.locator('[data-testid="answer-form-submit"]')).toBeVisible();

  // The user can retry: a successful retry fires the RPC again.
  await page.locator('[data-testid="answer-form-submit"]').click();
  await daemon.waitForRequest(
    "resolve_user_question",
    (r) => r.params.requestId === "q-fail-1"
  );

  emitTurnComplete(daemon, { sessionId, turnId });
});

test("two questions with distinct requestIds don't collide", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);
  const { sessionId, turnId } = await bootAndStartTurn(page, daemon);

  emitQuestion(daemon, {
    sessionId,
    turnId,
    requestId: "q-multi-1",
    questions: [
      {
        question: "First?",
        options: [{ label: "Yes" }, { label: "No" }]
      }
    ]
  });
  emitQuestion(daemon, {
    sessionId,
    turnId,
    requestId: "q-multi-2",
    questions: [
      {
        question: "Second?",
        options: [{ label: "Up" }, { label: "Down" }]
      }
    ]
  });

  // Both forms render. Filter by heading text so we can target each
  // independently — they share the .answer-form class.
  const firstForm = page.locator(".answer-form", { hasText: "First?" });
  const secondForm = page.locator(".answer-form", { hasText: "Second?" });
  await expect(firstForm).toBeVisible();
  await expect(secondForm).toBeVisible();

  // Answer the first; the second must stay interactive.
  await firstForm.locator('[data-option-label="Yes"]').click();
  await firstForm.locator('[data-testid="answer-form-submit"]').click();
  await daemon.waitForRequest(
    "resolve_user_question",
    (r) => r.params.requestId === "q-multi-1"
  );

  await expect(firstForm).toHaveAttribute("data-answered", "true");
  await expect(secondForm).toHaveAttribute("data-answered", "false");
  await expect(secondForm.locator('[data-option-label="Up"]')).toBeEnabled();

  emitTurnComplete(daemon, { sessionId, turnId });
});

test("typing dots are suppressed while a question form is unanswered", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);
  const { sessionId, turnId } = await bootAndStartTurn(page, daemon);

  emitQuestion(daemon, {
    sessionId,
    turnId,
    requestId: "q-typing-1",
    questions: [
      {
        question: "Pick one",
        options: [{ label: "A" }, { label: "B" }]
      }
    ]
  });

  // No text-delta yet — the assistant bubble would normally show typing
  // dots, but the unanswered form is an "agent waiting" signal so the
  // dots get suppressed (mirrors thinking/tool sibling handling in
  // Agent.svelte's hasActivitySibling logic).
  await expect(locateQuestionForm(page)).toBeVisible();
  await expect(page.locator(".typing")).toHaveCount(0);

  emitTurnComplete(daemon, { sessionId, turnId });
});

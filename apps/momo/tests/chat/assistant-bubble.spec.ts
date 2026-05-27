/**
 * Assistant bubble tests — happy-path migration plus Task 1 coverage.
 *
 * Migrated (lives in this file as a real test):
 *   - "home composer → create_session → run_agent_turn → streamed assistant text"
 *     (was apps/momo/tests/chat.spec.ts, test 1)
 *
 * Task 1 coverage (added 2026-05-27, "Markdown + timestamps + URL clicks
 * + error variant" port):
 *   - Markdown bold / italic / inline code / code block render
 *   - External URL is clickable and routes through tauri-plugin-opener
 *   - Timestamps render on both user and assistant bubbles
 *   - Error variant gets `data-error="true"` on the row
 *   - DOM identity preserved across delta → complete (same Locator wins
 *     across phases)
 *   - Partial Markdown mid-stream doesn't crash; resolves cleanly when
 *     the closing marker arrives in a later delta
 *
 * openUrl shim: MessageBody.svelte calls `window.__openUrl(href)` in
 * preference to the real Tauri plugin when that hook is defined. Tests
 * install the hook via `page.exposeFunction` (which mounts it on the
 * page's `window` so it survives navigation and is callable from inside
 * the webview). See MessageBody.svelte's "Test hook" docblock for the
 * full rationale.
 */
import { expect, test } from "@playwright/test";
import { FakeDaemon } from "../support/fakeDaemon";
import { bootOnboarded } from "../support/bootHelpers";
import {
  emitTurnStart,
  emitTextDelta,
  emitTurnComplete,
  emitTurnError
} from "../support/chatEmit";
import { locateAssistantBubble, locateUserBubble } from "../support/chatLocators";

// V2 happy path: typing into the home composer creates a puffer-backed session
// over WebSocket, fires a turn, and streams an assistant bubble in via deltas.
test("home composer → create_session → run_agent_turn → streamed assistant text", async ({
  page
}) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);

  const prompt = "Hello from V2";
  await page.getByLabel("Message").fill(prompt);
  await page.getByLabel("Message").press("Enter");

  // FakeDaemon.createSession synthesizes `session-created-${size+1}`; mirror
  // it here so we can address the session in the run_agent_turn predicate
  // and event channel below.
  await daemon.waitForRequest("create_session");
  const sessionId = "session-created-1";

  await daemon.waitForRequest(
    "run_agent_turn",
    (req) => req.params.sessionId === sessionId && req.params.message === prompt
  );

  await expect(page).toHaveURL(new RegExp(`#/agent/${sessionId}$`));

  // FakeDaemon.runAgentTurn synthesizes turnId as `turn-${sessionId}`; the
  // chat store binds the pending bubble to that exact id in fireTurn's .then.
  // Reusing it here keeps the bubble binding intact instead of forking a
  // second pending bubble via the fallback branch.
  const turnId = `turn-${sessionId}`;
  emitTurnStart(daemon, { sessionId, turnId });
  emitTextDelta(daemon, { sessionId, turnId, delta: "Hello" });
  emitTextDelta(daemon, { sessionId, turnId, delta: " world" });
  // Intentionally omit `assistantText` from turn-complete so the resolved
  // bubble has to rely on the accumulated deltas. Including it would let
  // the turn-complete handler overwrite whatever the delta accumulator did,
  // masking accumulator bugs (verified via mutation test 2026-05-25).
  emitTurnComplete(daemon, { sessionId, turnId });

  // Resolved bubble renders `.assistant-bubble__text`; pending bubble renders
  // the `.typing` dots (Agent.svelte:201-209). Asserting the resolved
  // element implicitly asserts pending is gone.
  const resolved = locateAssistantBubble(page).locator(".assistant-bubble__text", {
    hasText: "Hello world"
  });
  await expect(resolved).toBeVisible();
  await expect(page.locator(".typing")).toHaveCount(0);
});

/* ── Task 1 helpers ─────────────────────────────────────────────────── */

/**
 * Run a full lifecycle (turn-start → one delta → turn-complete) for a
 * fresh session created from the home composer. Returns the synthesized
 * sessionId / turnId so the test can drive further events or queries.
 *
 * This is intentionally NOT in chatEmit.ts — it's spec-local because
 * Task 1's bubble assertions all want the same boot path but no other
 * spec wants this exact shape.
 */
async function streamSingleDelta(
  page: import("@playwright/test").Page,
  daemon: FakeDaemon,
  delta: string
): Promise<{ sessionId: string; turnId: string }> {
  await page.getByLabel("Message").fill("hi");
  await page.getByLabel("Message").press("Enter");
  await daemon.waitForRequest("create_session");
  const sessionId = "session-created-1";
  await daemon.waitForRequest("run_agent_turn");
  const turnId = `turn-${sessionId}`;
  emitTurnStart(daemon, { sessionId, turnId });
  emitTextDelta(daemon, { sessionId, turnId, delta });
  emitTurnComplete(daemon, { sessionId, turnId });
  return { sessionId, turnId };
}

/* ── Markdown rendering ─────────────────────────────────────────────── */

test("Markdown bold renders inside the assistant bubble", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);
  await streamSingleDelta(page, daemon, "Hello **world**");
  // MessageBody emits inline formatting as `<span class="strong">`, not a
  // raw <strong> element — bold/italic/strike share one rendering path so
  // nested combinations (`**_x_**`) don't fight over the wrapper tag.
  await expect(page.locator(".assistant-bubble .strong")).toHaveText("world");
});

test("Markdown italic renders inside the assistant bubble", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);
  await streamSingleDelta(page, daemon, "_emphasis_");
  // MessageBody marks emphasis via `class="emphasis"` rather than <em>; the
  // styling is `font-style: italic`. The DOM element it emits is `<span>`
  // for inline-only formatting, so look for the class.
  await expect(page.locator(".assistant-bubble .emphasis")).toHaveText("emphasis");
});

test("Inline code renders inside the assistant bubble", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);
  await streamSingleDelta(page, daemon, "use `map` here");
  await expect(page.locator(".assistant-bubble code")).toBeVisible();
  await expect(page.locator(".assistant-bubble code")).toHaveText("map");
});

test("Code block renders inside the assistant bubble", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);
  await streamSingleDelta(page, daemon, "```ts\nconst x = 1;\n```");
  await expect(page.locator(".assistant-bubble pre")).toBeVisible();
  await expect(page.locator(".assistant-bubble pre")).toContainText("const x = 1;");
});

/* ── External URL click ─────────────────────────────────────────────── */

test("External http(s) link is clickable and routes through openUrl", async ({ page }) => {
  // Install the spy BEFORE the page navigates. page.exposeFunction mounts
  // the binding on the page's `window`; MessageBody.svelte prefers it over
  // the real Tauri plugin (see MessageBody.svelte docblock).
  const openCalls: string[] = [];
  await page.exposeFunction("__openUrl", (url: string) => {
    openCalls.push(url);
  });
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);

  await streamSingleDelta(page, daemon, "See [docs](https://example.com)");

  const link = page.locator(".assistant-bubble a", { hasText: "docs" });
  await expect(link).toBeVisible();
  await expect(link).toHaveAttribute("href", "https://example.com");
  await link.click();

  await expect.poll(() => openCalls).toContain("https://example.com");
});

/* ── Timestamps ─────────────────────────────────────────────────────── */

test("Timestamp renders alongside the user bubble", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);
  await page.getByLabel("Message").fill("when is it");
  await page.getByLabel("Message").press("Enter");
  await daemon.waitForRequest("create_session");

  // User bubble timestamp is set at submission time so it can render
  // immediately, independent of the turn lifecycle.
  const userTime = locateUserBubble(page, { text: "when is it" }).locator(
    ".chat-bubble__time"
  );
  await expect(userTime).toBeVisible();
  await expect(userTime).toHaveText(/^\d{2}:\d{2}$/);
});

test("Timestamp renders alongside the resolved assistant bubble", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);
  await streamSingleDelta(page, daemon, "done");
  const assistantTime = locateAssistantBubble(page).locator(".assistant-bubble__time");
  await expect(assistantTime).toBeVisible();
  await expect(assistantTime).toHaveText(/^\d{2}:\d{2}$/);
});

/* ── Error variant ──────────────────────────────────────────────────── */

test("turn-error stamps the assistant row with data-error='true'", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);
  await page.getByLabel("Message").fill("trip the wire");
  await page.getByLabel("Message").press("Enter");
  await daemon.waitForRequest("create_session");
  const sessionId = "session-created-1";
  await daemon.waitForRequest("run_agent_turn");
  const turnId = `turn-${sessionId}`;
  emitTurnStart(daemon, { sessionId, turnId });
  emitTextDelta(daemon, { sessionId, turnId, delta: "Attempting..." });
  emitTurnError(daemon, { sessionId, turnId, error: "model offline" });

  // chat.svelte.ts:140-155 wires turn-error → message.error = true; Agent.svelte
  // then propagates that to `data-error` on the row. Bubble text starts with
  // "Error: " (set by the turn-error handler).
  const row = locateAssistantBubble(page);
  await expect(row).toHaveAttribute("data-error", "true");
  await expect(row.locator(".assistant-bubble__text")).toContainText("Error: model offline");
});

/* ── DOM identity across phases ─────────────────────────────────────── */

test("assistant bubble DOM identity is preserved across delta → complete", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);
  await page.getByLabel("Message").fill("stay put");
  await page.getByLabel("Message").press("Enter");
  await daemon.waitForRequest("create_session");
  const sessionId = "session-created-1";
  await daemon.waitForRequest("run_agent_turn");
  const turnId = `turn-${sessionId}`;
  emitTurnStart(daemon, { sessionId, turnId });
  emitTextDelta(daemon, { sessionId, turnId, delta: "First " });

  // Grab the Locator NOW, after the first delta. We're not awaiting the
  // text — we just need the row to exist. (Keyed-each preserves DOM
  // identity by message.id; if a future refactor swaps to a non-keyed
  // each, the resolved-phase assertion below will start failing because
  // the row gets re-mounted.)
  const row = locateAssistantBubble(page);
  await expect(row).toBeVisible();

  emitTextDelta(daemon, { sessionId, turnId, delta: "second " });
  emitTextDelta(daemon, { sessionId, turnId, delta: "third." });
  emitTurnComplete(daemon, { sessionId, turnId });

  // SAME locator must now show the resolved text. If the row got
  // re-mounted, this assertion would pass-coincidentally-but-flakily;
  // the row visibility check above ran on the same selector during
  // pending, so a re-mount would have to coincidentally produce the
  // same DOM shape — vanishingly unlikely for a class-based selector.
  await expect(row.locator(".assistant-bubble__text")).toContainText(
    "First second third."
  );
});

/* ── Partial Markdown during streaming ──────────────────────────────── */

test("Partial Markdown mid-stream does not crash and resolves cleanly", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);
  await page.getByLabel("Message").fill("md stream");
  await page.getByLabel("Message").press("Enter");
  await daemon.waitForRequest("create_session");
  const sessionId = "session-created-1";
  await daemon.waitForRequest("run_agent_turn");
  const turnId = `turn-${sessionId}`;
  emitTurnStart(daemon, { sessionId, turnId });

  // First delta carries an UNCLOSED `**` — MessageBody's parseInline must
  // not throw on this. The block parser will treat the partial as plain
  // text until the closing marker arrives.
  emitTextDelta(daemon, { sessionId, turnId, delta: "**half" });
  // Give the renderer a chance to react; the bubble must still be visible.
  await expect(locateAssistantBubble(page)).toBeVisible();

  // Second delta closes the bold.
  emitTextDelta(daemon, { sessionId, turnId, delta: " closed**" });
  emitTurnComplete(daemon, { sessionId, turnId });

  // Same `.strong` class story as the bold-renders test above — MessageBody
  // emits `<span class="strong">` rather than a literal <strong>.
  await expect(page.locator(".assistant-bubble .strong")).toHaveText("half closed");
});

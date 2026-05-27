import type { Locator, Page } from "@playwright/test";

/**
 * DOM locators for chat surfaces. ALWAYS return a `Locator` — never await
 * or assert in here. Test authors chain their own `expect(...)` calls so
 * identity-preservation / cross-phase tests can observe the same element
 * across multiple states (e.g. "this bubble was the pending bubble before
 * the delta and the resolved bubble after — same DOM node").
 *
 * Selectors are class-based against today's Agent.svelte:
 *   - user bubble: `.bubble` (rendered by ChatBubble.svelte:24)
 *   - assistant bubble: `.assistant-bubble` (Agent.svelte:135)
 *   - assistant text: `.assistant-bubble__text` (Agent.svelte:143)
 *   - typing dots: `.typing` (Agent.svelte:137)
 *
 * Some components don't exist yet (tool pills, thinking block, answer
 * form) — those locators target the testids and classes planned in the
 * port (`.tool-call-pill`, `.thinking-block`, `.answer-form`). Specs that
 * use them are placeholders today.
 */

export interface UserBubbleOptions {
  text?: string;
}
export function locateUserBubble(page: Page, options: UserBubbleOptions = {}): Locator {
  const base = page.locator(".bubble");
  if (options.text !== undefined) {
    return base.filter({ hasText: options.text });
  }
  return base;
}

export interface AssistantBubbleOptions {
  /** Future: filter by data-turn-id once Agent.svelte tags bubbles. */
  turnId?: string;
  /** Pick the Nth assistant bubble (0-indexed). */
  nth?: number;
}
export function locateAssistantBubble(
  page: Page,
  options: AssistantBubbleOptions = {}
): Locator {
  let base = page.locator(".assistant-row");
  if (options.turnId !== undefined) {
    base = base.filter({ has: page.locator(`[data-turn-id="${options.turnId}"]`) });
  }
  if (options.nth !== undefined) {
    return base.nth(options.nth);
  }
  return base;
}

export function locateThinkingBlock(page: Page): Locator {
  return page.locator(".thinking-block");
}

export interface ToolPillOptions {
  callId?: string;
  toolId?: string;
  status?: "running" | "success" | "failed";
}
/**
 * Targets `ToolCallPill.svelte`'s root element — `.tool-call-pill` carries
 * `data-status`, `data-tool-id`, and `data-call-id` directly (not on a
 * child), so we chain attribute selectors on the same element instead of
 * `filter({ has })` which expects descendants. Backwards-compatible with
 * the prior signature.
 */
export function locateToolPill(page: Page, options: ToolPillOptions = {}): Locator {
  const attrs: string[] = [];
  if (options.status !== undefined) attrs.push(`[data-status="${options.status}"]`);
  if (options.callId !== undefined) attrs.push(`[data-call-id="${options.callId}"]`);
  if (options.toolId !== undefined) attrs.push(`[data-tool-id="${options.toolId}"]`);
  return page.locator(`.tool-call-pill${attrs.join("")}`);
}

export interface QuestionFormOptions {
  requestId?: string;
}
export function locateQuestionForm(page: Page, options: QuestionFormOptions = {}): Locator {
  if (options.requestId !== undefined) {
    return page.locator(`.answer-form[data-request-id="${options.requestId}"]`);
  }
  return page.locator(".answer-form");
}

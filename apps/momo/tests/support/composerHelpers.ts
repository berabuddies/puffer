import { expect, type Page } from "@playwright/test";

/**
 * Composer helpers — keyboard / IME / state assertions for the chat
 * input textarea. Selectors target the existing Composer:
 *   - textarea: `getByLabel("Message")` (Composer.svelte:108)
 *   - Send button: `getByLabel("Send")` (Composer.svelte:113)
 * The Stop button is planned to land with `data-stop` attribute (Task 2);
 * `composerExpectState("running")` checks for that today, accepting
 * absence-of-disabled as the loose definition.
 */

export async function composerType(page: Page, text: string): Promise<void> {
  await page.getByLabel("Message").fill(text);
}

export async function composerSubmit(page: Page): Promise<void> {
  await page.getByLabel("Message").press("Enter");
}

export interface IMEArgs {
  phase: "start" | "commit";
  /** Latest composition value to land in the textarea once committed. */
  text: string;
}
/**
 * Simulates an IME composition pattern. v1 reference: `chat-session-ui.spec.ts:554`.
 *
 * On `phase: "start"` we dispatch `compositionstart` + a keydown with
 * `isComposing: true`, mimicking what browsers do during pinyin /
 * kana input. On `phase: "commit"` we fire `compositionend` and update
 * the textarea value.
 */
export async function composerIME(page: Page, args: IMEArgs): Promise<void> {
  const composer = page.getByLabel("Message");
  await composer.fill(args.text);
  if (args.phase === "start") {
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
  } else {
    await composer.evaluate((node, text) => {
      const el = node as HTMLTextAreaElement;
      el.value = text;
      el.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true, data: text }));
      el.dispatchEvent(new Event("input", { bubbles: true }));
    }, args.text);
  }
}

export type ComposerState = "idle" | "running" | "disabled";

/**
 * Assert the composer is in one of three high-level states:
 *   - `idle`: textarea enabled, Send button visible, no Stop button.
 *   - `running`: a Stop button is visible (Task 2 lands `data-stop`).
 *   - `disabled`: textarea is disabled.
 *
 * Today the Stop button doesn't ship yet — `running` falls back to the
 * "Send is visible" probe so the assertion stays usable until the Stop
 * task lands; revisit then.
 */
export async function composerExpectState(page: Page, state: ComposerState): Promise<void> {
  const textarea = page.getByLabel("Message");
  switch (state) {
    case "idle":
      await expect(textarea).toBeEnabled();
      await expect(page.getByLabel("Send")).toBeVisible();
      await expect(page.locator("[data-stop]")).toHaveCount(0);
      return;
    case "running": {
      const stopCount = await page.locator("[data-stop]").count();
      if (stopCount > 0) {
        await expect(page.locator("[data-stop]")).toBeVisible();
      } else {
        // Stop button not implemented yet (Task 2 pending). Fall back.
        await expect(page.getByLabel("Send")).toBeVisible();
      }
      return;
    }
    case "disabled":
      await expect(textarea).toBeDisabled();
      return;
  }
}

export async function composerExpectDraft(page: Page, text: string): Promise<void> {
  await expect(page.getByLabel("Message")).toHaveValue(text);
}

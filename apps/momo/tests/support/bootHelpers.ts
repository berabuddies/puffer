import { expect, type Page } from "@playwright/test";
import type { FakeDaemon } from "./fakeDaemon";

/**
 * Stub auth + onboarding state and install the FakeDaemon WebSocket route.
 *
 * Lifted verbatim from the original `apps/momo/tests/chat.spec.ts` so the
 * migrated specs and the smoke spec agree on what "logged-in, onboarded"
 * looks like. Mints a not-yet-expired Auth Station-shaped JWT — the
 * frontend doesn't verify the signature, it only decodes the payload and
 * checks `exp`.
 *
 * Always lands on /#/home so each test starts from a known route.
 */
export async function bootOnboarded(page: Page, daemon: FakeDaemon): Promise<void> {
  await page.addInitScript(() => {
    try {
      window.localStorage.setItem("puffer.onboarded", "true");
      const header = btoa(JSON.stringify({ alg: "RS256", typ: "JWT" }))
        .replace(/=+$/, "")
        .replace(/\+/g, "-")
        .replace(/\//g, "_");
      const payload = btoa(
        JSON.stringify({
          sub: "test-user",
          email: "test@example.com",
          name: "Test User",
          exp: Math.floor(Date.now() / 1000) + 60 * 60 * 24
        })
      )
        .replace(/=+$/, "")
        .replace(/\+/g, "-")
        .replace(/\//g, "_");
      window.localStorage.setItem(
        "puffer.authToken",
        `${header}.${payload}.test-sig`
      );
    } catch {
      /* private mode — auth.svelte.ts treats absence as not-onboarded; tests that
         care will fail loudly. */
    }
  });
  // Sidebar mounts on /home and immediately polls credits against
  // control-api. Specs that boot here don't assert on credits, so abort
  // those requests to keep the suite off the live internet — the pill just
  // renders "—". Credit specs use their own boot + route stubs instead.
  await page.route("https://control-api.worldrouter.ai/**", (route) =>
    route.abort()
  );
  await daemon.install(page);
  await page.goto("/#/home");
  await expect(page).toHaveURL(/#\/home$/);
}

/**
 * Navigate to a specific session in the agent view.
 *
 * STUB: today this only takes a `sessionId` (string) and uses hash routing
 * to land directly on /#/agent/<id>. A future iteration that needs to
 * exercise the sidebar click path (e.g. to verify selection state /
 * sidebar highlighting) should accept a `predicate` matching a sidebar
 * row's accessible name — mirror v1's `openSession(page, /title/)` shape.
 *
 * For now the stub is sufficient because every migrated test in Phase 0
 * either uses the home composer (no sidebar) or hard-navigates to
 * `/#/agent/<id>` to mimic a webview reload.
 */
export async function openSession(page: Page, sessionId: string): Promise<void> {
  await page.goto(`/#/agent/${sessionId}`);
  await expect(page).toHaveURL(new RegExp(`#/agent/${sessionId}$`));
}

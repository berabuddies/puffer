/**
 * Smoke tests for the worldrouter OIDC login gate.
 *
 * These don't actually go through Auth Station — they verify the UI layer:
 * (1) unauthenticated traffic lands on /login, (2) the page renders the
 * Momo hero, (3) clicking the button builds the correct Auth Station URL.
 *
 * Full E2E (real login with wangshun+3@tomo.inc) is gated on adding
 * http://127.0.0.1:1456 to Auth Station's ALLOWED_REDIRECT_ORIGINS.
 */
import { test, expect } from "@playwright/test";

test.describe("worldrouter login gate", () => {
  test.beforeEach(async ({ page }) => {
    // Make sure no leftover token from a previous run pretends we're signed in.
    await page.addInitScript(() => {
      try {
        localStorage.removeItem("puffer.authToken");
        localStorage.removeItem("puffer.authRefreshToken");
        sessionStorage.clear();
      } catch {
        /* private mode — ignore */
      }
    });
  });

  test("root redirects to /login when signed out", async ({ page }) => {
    await page.goto("/");
    await expect(page).toHaveURL(/#\/login$/);
  });

  test("login page renders the Momo hero", async ({ page }) => {
    await page.goto("/#/login");
    await expect(page.getByRole("heading", { name: "Momo" })).toBeVisible();
    await expect(
      page.getByText("Your proactive AI assistant for real life.")
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: "Sign in / Sign up" })
    ).toBeVisible();
    await page.screenshot({
      path: "test-results/v2-login-page.png",
      fullPage: false
    });
  });

  test("clicking the button targets Auth Station with correct params", async ({
    page
  }) => {
    // Intercept all auth.worldrouter.ai navigations so the browser doesn't
    // actually try to load it (would 4xx without us being whitelisted, plus
    // headless tests shouldn't reach the public internet). Abort means the
    // navigation never completes — page stays on /login — and we capture
    // the URL for assertions.
    let outboundUrl: string | null = null;
    await page.route("https://auth.worldrouter.ai/**", (route) => {
      outboundUrl = route.request().url();
      return route.abort();
    });
    await page.goto("/#/login");
    await page.getByRole("button", { name: "Sign in / Sign up" }).click();
    // Wait until the navigation gets intercepted.
    await expect.poll(() => outboundUrl, { timeout: 5000 }).not.toBeNull();
    const parsed = new URL(outboundUrl!);
    expect(parsed.origin).toBe("https://auth.worldrouter.ai");
    expect(parsed.pathname).toBe("/login");
    // No fragment — RFC 6749 forbids it in redirect_uri (and Auth Station
    // silently rejects URIs containing one). App.svelte's
    // absorbOAuthCallbackInUrl rewrites the post-callback URL into the
    // hash-routed equivalent on landing.
    expect(parsed.searchParams.get("redirect_uri")).toBe(
      "http://localhost:1466/auth/callback"
    );
    expect(parsed.searchParams.get("client_state")).toMatch(/.{8,}/);
  });

  test("callback with state_mismatch shows inline error on /login", async ({
    page
  }) => {
    // No state stashed in sessionStorage → callback must reject.
    await page.goto(
      "/#/auth/callback?token=fake.jwt.payload&state=does-not-match"
    );
    await expect(page).toHaveURL(/#\/login\?error=/);
    // friendlyError("state_mismatch") → "Your sign-in session expired. Please try again."
    await expect(page.getByText(/sign-in session expired/i)).toBeVisible();
  });
});

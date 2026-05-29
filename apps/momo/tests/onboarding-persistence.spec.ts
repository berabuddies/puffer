import { expect, test } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";
import { bootOnboarded } from "./support/bootHelpers";

/**
 * Onboarding is a machine-level, one-time gate — not an account-level one.
 * The WorldClaude account only supplies a provider key; signing out (or
 * switching accounts) must NOT replay onboarding on a machine that has
 * already completed it. These tests pin that contract:
 *
 *   1. signOut() clears auth/token state but keeps `puffer.onboarded`.
 *   2. signedIn + onboarded → root resolves to /home (never onboarding).
 *   3. signedOut + onboarded → root resolves to /login (never onboarding).
 *
 * (2) + (3) together prove an onboarded machine is never sent back through
 * onboarding regardless of who is (or isn't) signed in — i.e. an account
 * switch can't trigger it.
 */

test("signOut clears auth but keeps the machine-level onboarded flag", async ({
  page
}) => {
  // signOut()'s web path bounces to Auth Station's /logout with a
  // post_logout_redirect_uri back to our origin. Aborting that would strand
  // the page on an error document we can't read localStorage from, so we 302
  // it straight back to the local /login — same origin, so the puffer
  // localStorage (where the onboarded flag lives) stays readable.
  await page.route("https://auth.worldrouter.ai/**", (route) => {
    const url = new URL(route.request().url());
    const back =
      url.searchParams.get("post_logout_redirect_uri") ??
      "http://localhost:1466/#/login";
    return route.fulfill({ status: 302, headers: { location: back } });
  });

  // Seed onboarded + a valid JWT, but only on the FIRST load — a
  // sessionStorage guard stops the initScript from re-seeding the token when
  // the logout redirect reloads the page, so we can observe the token
  // actually being cleared while the onboarded flag survives.
  await page.addInitScript(() => {
    try {
      if (sessionStorage.getItem("__seeded")) return;
      sessionStorage.setItem("__seeded", "1");
      localStorage.setItem("puffer.onboarded", "true");
      const b64url = (s: string) =>
        btoa(s).replace(/=+$/, "").replace(/\+/g, "-").replace(/\//g, "_");
      const header = b64url(JSON.stringify({ alg: "RS256", typ: "JWT" }));
      const payload = b64url(
        JSON.stringify({
          sub: "test-user",
          email: "test@example.com",
          name: "Test User",
          exp: Math.floor(Date.now() / 1000) + 60 * 60 * 24
        })
      );
      localStorage.setItem("puffer.authToken", `${header}.${payload}.test-sig`);
    } catch {
      /* private mode — ignore */
    }
  });

  const daemon = new FakeDaemon({ sessions: [] });
  await daemon.install(page);

  await page.goto("/#/home");
  await expect(page).toHaveURL(/#\/home$/);

  // Sign out via the rail user menu.
  await page.locator(".user-menu__trigger").click();
  await page.getByRole("menuitem", { name: "Sign out" }).click();

  // The logout redirect lands us back on /login (signed out). Crucially NOT
  // /onboarding/where: the machine-level onboarded flag survived.
  await page.waitForURL(/#\/login$/);
  await expect
    .poll(() => page.evaluate(() => localStorage.getItem("puffer.onboarded")))
    .toBe("true");
  // Auth/token state, by contrast, must be cleared.
  await expect
    .poll(() => page.evaluate(() => localStorage.getItem("puffer.authToken")))
    .toBeNull();
});

test("signed-in + onboarded resolves the root to /home, never onboarding", async ({
  page
}) => {
  const daemon = new FakeDaemon({ sessions: [] });
  // bootOnboarded installs an initScript that seeds onboarded=true + a valid
  // JWT on every load, so reloading from "/" re-runs the gate from scratch.
  await bootOnboarded(page, daemon);

  await page.goto("/");
  await expect(page).toHaveURL(/#\/home$/);
});

test("signed-out + onboarded resolves the root to /login, never onboarding", async ({
  page
}) => {
  // Machine-level onboarded flag set, but no auth token → signed out. A
  // signed-out onboarded machine must land on /login (to re-authenticate),
  // never on /onboarding/where.
  await page.addInitScript(() => {
    try {
      localStorage.setItem("puffer.onboarded", "true");
      localStorage.removeItem("puffer.authToken");
      localStorage.removeItem("puffer.authRefreshToken");
    } catch {
      /* private mode — ignore */
    }
  });

  await page.goto("/");
  await expect(page).toHaveURL(/#\/login$/);
});

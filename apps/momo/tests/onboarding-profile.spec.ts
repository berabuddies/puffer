import { expect, test, type Page } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";

/**
 * Onboarding profile integration (Task 7).
 *
 * Two contracts, end-to-end through the real onboarding routes:
 *
 *   1. Completing onboarding writes the collected {country, role} profile to
 *      puffer via the 1431 `write_user_profile` RPC AND sets the machine-level
 *      `puffer.onboarded` flag. The profile write only fires on Done.svelte's
 *      onMount (`commitProfile()`), so the flow must actually reach
 *      /onboarding/done — here via Apps' skip link (skipTo="/onboarding/done").
 *
 *   2. Skipping straight from /onboarding/where marks the machine onboarded so
 *      re-resolving the root never sends it back through onboarding (the gate
 *      regression OnboardingShell.onSkip + App.svelte's /home effect guard).
 *
 * Both boot SIGNED-IN but NOT onboarded: a valid JWT with NO `puffer.onboarded`
 * flag makes getRootRedirect() (src/routes.ts) send "/" → "/onboarding/where".
 * This mirrors bootHelpers.bootOnboarded's JWT minting but deliberately OMITS
 * (and removes) the onboarded flag.
 *
 * Under Playwright wsClient reads VITE_PUFFER_WS_URL=ws://127.0.0.1:17777/ws
 * (playwright.config.ts), so `write_user_profile` lands on the FakeDaemon `/ws`
 * dispatcher rather than the real Tauri host.
 */
async function seedSignedInNotOnboarded(page: Page): Promise<void> {
  await page.addInitScript(() => {
    try {
      // Seed only ONCE per context. A sessionStorage guard stops a later
      // page.reload() (the gate-regression re-resolution in test 2) from
      // re-running the `removeItem("puffer.onboarded")` below — otherwise the
      // reload would wipe the onboarded flag the skip just set and bounce us
      // back into onboarding. (Same pattern as bootHelpers' signOut spec.)
      if (sessionStorage.getItem("__seeded")) return;
      sessionStorage.setItem("__seeded", "1");
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
      window.localStorage.setItem(
        "puffer.authToken",
        `${header}.${payload}.test-sig`
      );
      // Crucial: signed in but NOT onboarded → root resolves to onboarding.
      window.localStorage.removeItem("puffer.onboarded");
    } catch {
      /* private mode — ignore */
    }
  });
  // The post-onboarding /home landing mounts the sidebar, which polls credits
  // against control-api. We don't assert on credits, so keep it off the live
  // internet (same stub the other specs use).
  await page.route("https://control-api.worldrouter.ai/**", (route) =>
    route.abort()
  );
}

test("completing onboarding writes the profile and sets the onboarded flag", async ({
  page
}) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await seedSignedInNotOnboarded(page);
  await daemon.install(page);

  // Signed-in + not onboarded → root redirect lands on /onboarding/where.
  // NB: navigate to the explicit root hash "/#/", not "/". An empty hash makes
  // the router default straight to DEFAULT_PATH (/home) without ever consulting
  // getRootRedirect() (router.svelte.ts initRouter), so "/" would skip the gate
  // and land on /home. "/#/" yields currentRoute.path === "/", which App.svelte's
  // onMount resolves via getRootRedirect() → /onboarding/where here.
  await page.goto("/#/");
  await expect(page).toHaveURL(/#\/onboarding\/where$/);

  // Chips render as <button> (Chip.svelte: a click handler is passed, so the
  // button branch renders). Pick a non-default country/role so the assertion
  // can't pass on the page defaults (United States / Founder).
  await page.getByRole("button", { name: "Japan" }).click();
  // Where auto-advances 300ms after a pick.
  await expect(page).toHaveURL(/#\/onboarding\/role$/);

  await page.getByRole("button", { name: "Engineer", exact: true }).click();
  // Role auto-advances 300ms after a preset pick.
  await expect(page).toHaveURL(/#\/onboarding\/apps$/);

  // Apps' skip link (skipTo="/onboarding/done") reaches Done, whose onMount
  // calls commitProfile() → write_user_profile. (The Continue CTA also goes to
  // Done; the skip link is the documented "via Apps skip link" path.)
  await page.getByText("Skip for now and explore the app").click();

  const req = await daemon.waitForRequest("write_user_profile");
  expect(req.params).toMatchObject({ country: "Japan", role: "Engineer" });

  // Done.svelte's onMount also marks the machine onboarded.
  await expect
    .poll(() => page.evaluate(() => localStorage.getItem("puffer.onboarded")))
    .toBe("true");
});

test("skipping from Where marks the machine onboarded (gate regression)", async ({
  page
}) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await seedSignedInNotOnboarded(page);
  await daemon.install(page);

  // "/#/" forces a real getRootRedirect() pass (see test above); signed-in +
  // not onboarded → /onboarding/where.
  await page.goto("/#/");
  await expect(page).toHaveURL(/#\/onboarding\/where$/);

  // OnboardingShell.onSkip persists the onboarded flag before navigating.
  await page.getByText("Skip for now and explore the app").click();
  await expect
    .poll(() => page.evaluate(() => localStorage.getItem("puffer.onboarded")))
    .toBe("true");

  // Re-resolve the root through getRootRedirect() again from a fresh mount: an
  // onboarded signed-in machine must land on /home, never back in onboarding.
  // page.goto("/#/") from the already-loaded onboarding doc would be a same-doc
  // hash change (App.svelte's onMount redirect wouldn't re-run, leaving us on
  // "/#/"), so force a real document reload at the root hash.
  await page.evaluate(() => {
    window.location.hash = "#/";
  });
  await page.reload();
  await expect(page).toHaveURL(/#\/home$/);
});

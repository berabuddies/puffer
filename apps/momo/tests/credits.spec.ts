/**
 * Sidebar credits — real WorldRouter balance.
 *
 * The bottom-left "Credits" pill shows the user's WorldRouter billing
 * balance (`credit_balance_usd`), formatted as USD × 100 + " Credits"
 * (1 USD = 100 Credits), exactly like worldagent-frontend.
 *
 * These specs mock the control-api network (exchange + billing-account)
 * via page.route, so nothing reaches the real internet and there is no
 * CORS surface. Auth is stubbed with stubSignedIn (a not-yet-expired
 * Auth Station-shaped JWT in localStorage).
 */
import { test, expect, type Page } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";

const CONTROL = "https://control-api.worldrouter.ai";

/**
 * Stub a not-yet-expired Auth Station-shaped JWT in localStorage so
 * `loadAuthFromStorage()` resolves to signedIn (sub = "test-user").
 * Standalone (no FakeDaemon) — these specs exercise the credit path,
 * which talks to control-api over plain fetch, not the daemon ws.
 */
async function stubSignedIn(page: Page): Promise<void> {
  await page.addInitScript(() => {
    try {
      const b64 = (o: unknown) =>
        btoa(JSON.stringify(o))
          .replace(/=+$/, "")
          .replace(/\+/g, "-")
          .replace(/\//g, "_");
      const header = b64({ alg: "RS256", typ: "JWT" });
      const payload = b64({
        sub: "test-user",
        email: "test@example.com",
        name: "Test User",
        exp: Math.floor(Date.now() / 1000) + 60 * 60 * 24
      });
      window.localStorage.setItem("puffer.onboarded", "true");
      window.localStorage.setItem(
        "puffer.authToken",
        `${header}.${payload}.test-sig`
      );
    } catch {
      /* private mode — tests that depend on auth will fail loudly */
    }
  });
}

/**
 * Boot straight to /home as a signed-in, onboarded user with the
 * FakeDaemon installed. Unlike bootOnboarded, this does NOT auto-stub
 * control-api — credit specs register their own exchange/billing routes.
 */
async function bootHomeSignedIn(
  page: Page,
  daemon: FakeDaemon
): Promise<void> {
  await stubSignedIn(page);
  await daemon.install(page);
  await page.goto("/#/home");
  await expect(page).toHaveURL(/#\/home$/);
}

test.describe("sidebar credits — formatting", () => {
  test("formatCredits renders USD × 100 with separators + ' Credits'", async ({
    page
  }) => {
    // Any page that boots the Vite module graph is fine; we import the
    // pure module directly and assert on its output (mirrors apikey.e2e).
    await page.goto("/#/login");
    const out = await page.evaluate(async () => {
      const mod = await import("/src/lib/format.ts");
      return {
        cents: mod.formatCredits(12.34), // 1234 → "1,234 Credits"
        zero: mod.formatCredits(0), // "0 Credits"
        big: mod.formatCredits(1000), // 100000 → "100,000 Credits"
        half: mod.formatCredits(0.005), // 0.5 → "0.5 Credits"
        dust: mod.formatCredits(0.00005) // 0.005 → "< 0.01 Credits"
      };
    });
    expect(out.cents).toBe("1,234 Credits");
    expect(out.zero).toBe("0 Credits");
    expect(out.big).toBe("100,000 Credits");
    expect(out.half).toBe("0.5 Credits");
    expect(out.dust).toBe("< 0.01 Credits");
  });
});

test.describe("sidebar credits — WorldRouter session reuse", () => {
  test("ensureWrSession exchanges once, then serves the cached token", async ({
    page
  }) => {
    let exchangeCalls = 0;
    await page.route(`${CONTROL}/auth/exchange`, async (route) => {
      exchangeCalls++;
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          session_token: "sess-xyz",
          default_team_id: "team-1"
        })
      });
    });
    await stubSignedIn(page);
    await page.goto("/#/login");

    const out = await page.evaluate(async () => {
      const mod = await import("/src/lib/auth.svelte.ts");
      await mod.loadAuthFromStorage();
      const a = await mod.ensureWrSession();
      const b = await mod.ensureWrSession();
      return { a, b };
    });

    expect(out.a).toEqual({ sessionToken: "sess-xyz", teamId: "team-1" });
    expect(out.b).toEqual({ sessionToken: "sess-xyz", teamId: "team-1" });
    // Second call must hit the in-memory cache, not re-exchange.
    expect(exchangeCalls).toBe(1);
  });
});

/** Stub a successful exchange + billing-account pair returning `usd`. */
async function stubBilling(page: Page, usd: number): Promise<void> {
  await page.route(`${CONTROL}/auth/exchange`, (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        session_token: "sess-xyz",
        default_team_id: "team-1"
      })
    })
  );
  await page.route(
    `${CONTROL}/platform/v1/teams/team-1/billing-account`,
    (route) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          team_id: "team-1",
          credit_balance_usd: usd,
          billing_account: {}
        })
      })
  );
}

test.describe("sidebar credits — loading", () => {
  test("loadCredits resolves credit_balance_usd into ready state", async ({
    page
  }) => {
    await stubBilling(page, 12.34);
    await stubSignedIn(page);
    await page.goto("/#/login");

    const out = await page.evaluate(async () => {
      const auth = await import("/src/lib/auth.svelte.ts");
      await auth.loadAuthFromStorage();
      const mod = await import("/src/lib/creditStore.svelte.ts");
      await mod.loadCredits();
      return {
        creditsUsd: mod.creditState.creditsUsd,
        status: mod.creditState.status
      };
    });

    expect(out.creditsUsd).toBe(12.34);
    expect(out.status).toBe("ready");
  });

  test("loadCredits re-exchanges once on 401 then retries", async ({
    page
  }) => {
    let exchangeCalls = 0;
    await page.route(`${CONTROL}/auth/exchange`, (route) => {
      exchangeCalls++;
      // First exchange yields a stale token, the forced re-exchange a fresh one.
      const token = exchangeCalls === 1 ? "stale-sess" : "fresh-sess";
      return route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          session_token: token,
          default_team_id: "team-1"
        })
      });
    });
    await page.route(
      `${CONTROL}/platform/v1/teams/team-1/billing-account`,
      (route) => {
        const auth = route.request().headers()["authorization"];
        if (auth === "Bearer fresh-sess") {
          return route.fulfill({
            status: 200,
            contentType: "application/json",
            body: JSON.stringify({
              team_id: "team-1",
              credit_balance_usd: 7.5,
              billing_account: {}
            })
          });
        }
        return route.fulfill({
          status: 401,
          contentType: "application/json",
          body: JSON.stringify({ error: "unauthorized" })
        });
      }
    );
    await stubSignedIn(page);
    await page.goto("/#/login");

    const out = await page.evaluate(async () => {
      const auth = await import("/src/lib/auth.svelte.ts");
      await auth.loadAuthFromStorage();
      const mod = await import("/src/lib/creditStore.svelte.ts");
      await mod.loadCredits();
      return {
        creditsUsd: mod.creditState.creditsUsd,
        status: mod.creditState.status
      };
    });

    expect(out.creditsUsd).toBe(7.5);
    expect(out.status).toBe("ready");
    // One initial exchange + one forced re-exchange after the 401.
    expect(exchangeCalls).toBe(2);
  });
});

test.describe("sidebar credits — pill UI", () => {
  test("pill shows the real balance after sign-in", async ({ page }) => {
    await stubBilling(page, 12.34);
    const daemon = new FakeDaemon({ sessions: [] });
    await bootHomeSignedIn(page, daemon);

    await expect(page.locator(".credits-pill__value")).toHaveText(
      "1,234 Credits"
    );
  });

  test("pill falls back to — when billing fails", async ({ page }) => {
    await page.route(`${CONTROL}/auth/exchange`, (route) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          session_token: "sess-xyz",
          default_team_id: "team-1"
        })
      })
    );
    await page.route(
      `${CONTROL}/platform/v1/teams/team-1/billing-account`,
      (route) => route.fulfill({ status: 500, body: "boom" })
    );
    const daemon = new FakeDaemon({ sessions: [] });
    await bootHomeSignedIn(page, daemon);

    await expect(page.locator(".credits-pill__value")).toHaveText("—");
  });

  test("clicking the pill opens the top-up page", async ({ page }) => {
    await stubBilling(page, 12.34);
    const daemon = new FakeDaemon({ sessions: [] });
    await bootHomeSignedIn(page, daemon);
    await expect(page.locator(".credits-pill__value")).toHaveText(
      "1,234 Credits"
    );

    // Capture window.open (the non-Tauri branch) instead of letting it
    // navigate to the live worldrouter dashboard.
    await page.evaluate(() => {
      (window as unknown as { __openedUrl: string | null }).__openedUrl = null;
      window.open = ((url?: string | URL) => {
        (window as unknown as { __openedUrl: string | null }).__openedUrl =
          url ? String(url) : null;
        return null;
      }) as typeof window.open;
    });
    await page.locator(".credits-pill").click();

    const opened = await page.evaluate(
      () => (window as unknown as { __openedUrl: string | null }).__openedUrl
    );
    expect(opened).toBe("https://www.worldrouter.ai/dashboard/credits");
  });
});

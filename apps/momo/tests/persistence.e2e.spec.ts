/**
 * Verifies the new login persistence guarantees:
 *   - 7-day re-login: refresh token can mint a new access JWT
 *   - logout clears API key alongside auth state
 *   - cached API key survives an app restart (signedIn → key still there)
 *
 * Uses the cached storage state populated by login.e2e.spec.ts. Run
 * login.e2e first to get a fresh JWT + refresh token + sk-worldrouter key.
 */
import { test, expect } from "@playwright/test";
import * as fs from "node:fs";
import * as path from "node:path";

const STORAGE_STATE_FILE = path.resolve(
  process.env.PUFFER_E2E_STORAGE_STATE ?? "tests/v2/.auth-storage.json"
);

/**
 * Ensure the cached storage state has a worldrouter API key in localStorage.
 * login.e2e currently saves state BEFORE the fire-and-forget mint resolves
 * (mint takes ~15s, navigation happens immediately), so the file may have
 * a valid JWT but no apiKey. We patch that here so downstream "key tracks
 * login" assertions are testing the real cached-restart behaviour rather
 * than a half-written cache.
 */
async function ensureCachedApiKey(): Promise<void> {
  if (!fs.existsSync(STORAGE_STATE_FILE)) return;
  const state = JSON.parse(fs.readFileSync(STORAGE_STATE_FILE, "utf8")) as {
    origins?: Array<{
      origin: string;
      localStorage?: Array<{ name: string; value: string }>;
    }>;
  };
  const localOrigin = state.origins?.find(
    (o) => o.origin === "http://localhost:1456"
  );
  // Both apiKey AND apiKeyOwner must be present — the owner tag is the
  // new (P0 fix) ownership guard. An untagged legacy entry is treated
  // as untrusted by getWorldRouterApiKey, so re-mint to refresh the
  // tagging.
  const hasKey = localOrigin?.localStorage?.some(
    (e) => e.name === "puffer.worldrouterApiKey" && e.value.startsWith("sk-")
  );
  const hasOwner = localOrigin?.localStorage?.some(
    (e) => e.name === "puffer.worldrouterApiKeyOwner" && e.value.length > 0
  );
  if (hasKey && hasOwner) return;
  // Mint a key against the cached JWT and write it back into the state
  // file. loadAuthFromStorage MUST run first so authState is signedIn
  // — the mint race guard requires it before persisting.
  const { chromium } = await import("@playwright/test");
  const browser = await chromium.launch();
  const ctx = await browser.newContext({ storageState: STORAGE_STATE_FILE });
  const page = await ctx.newPage();
  await page.goto("http://localhost:1456/");
  const minted = await page.evaluate(async () => {
    const mod = await import("/src/lib/auth.svelte.ts");
    await mod.loadAuthFromStorage();
    const token = mod.getAuthToken();
    if (!token) return { ok: false, reason: "no token in cached state" };
    try {
      const key = await mod.mintWorldRouterApiKey(token);
      return { ok: true, key };
    } catch (err) {
      return { ok: false, reason: String(err) };
    }
  });
  if (!minted.ok) {
    await browser.close();
    throw new Error(`could not seed apiKey into storage state: ${minted.reason}`);
  }
  await ctx.storageState({ path: STORAGE_STATE_FILE });
  await browser.close();
}

test.describe("login persistence", () => {
  test.skip(
    !fs.existsSync(STORAGE_STATE_FILE),
    "no cached storage state — run login.e2e.spec.ts first"
  );
  test.beforeAll(async () => {
    await ensureCachedApiKey();
  });
  test.use({ storageState: STORAGE_STATE_FILE });

  test("refresh token mints a fresh access JWT (7-day persistence)", async ({
    page
  }) => {
    test.setTimeout(60_000);
    await page.goto("/");

    const result = await page.evaluate(async () => {
      const refresh = localStorage.getItem("puffer.authRefreshToken");
      if (!refresh) return { ok: false, reason: "no refresh token cached" };
      try {
        const res = await fetch(
          "https://auth.worldrouter.ai/token/refresh",
          {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ refresh_token: refresh })
          }
        );
        if (!res.ok) {
          return { ok: false, reason: `HTTP ${res.status}`, body: await res.text() };
        }
        const body = (await res.json()) as { token?: string };
        return { ok: true, token: body.token };
      } catch (err) {
        return { ok: false, reason: String(err) };
      }
    });

    if (!result.ok) {
      throw new Error(`refresh failed:\n${JSON.stringify(result, null, 2)}`);
    }
    expect(typeof result.token).toBe("string");
    expect(result.token!.split(".")).toHaveLength(3);
    const payload = JSON.parse(
      Buffer.from(
        result.token!.split(".")[1].replace(/-/g, "+").replace(/_/g, "/"),
        "base64"
      ).toString("utf8")
    );
    expect(payload.email).toBe("wangshun+3@tomo.inc");
    // Access JWT TTL ~24h per auth-docs; expect at least 23h of headroom.
    expect(payload.exp * 1000 - Date.now()).toBeGreaterThan(23 * 60 * 60 * 1000);
    // eslint-disable-next-line no-console
    console.log("[persistence.e2e] refresh produced JWT, exp = ", new Date(payload.exp * 1000));
  });

  test("loadAuthFromStorage refreshes when access JWT is expired", async ({
    page
  }) => {
    test.setTimeout(60_000);
    // Replace the cached access JWT with one whose `exp` is in the past
    // so decodeJwtPayload rejects it. The refresh token + API key are
    // untouched, so loadAuthFromStorage should mint a fresh access JWT
    // via /token/refresh, then settle on signedIn.
    await page.addInitScript(() => {
      const b64url = (s: string) =>
        btoa(s).replace(/=+$/, "").replace(/\+/g, "-").replace(/\//g, "_");
      const header = b64url(JSON.stringify({ alg: "RS256", typ: "JWT" }));
      const payload = b64url(
        JSON.stringify({
          sub: "user_expired",
          email: "wangshun+3@tomo.inc",
          // Expired 1h ago — decodeJwtPayload should return null.
          exp: Math.floor(Date.now() / 1000) - 3600
        })
      );
      localStorage.setItem("puffer.authToken", `${header}.${payload}.sig`);
      localStorage.setItem("puffer.onboarded", "true");
    });

    await page.goto("/");
    // App.svelte awaits loadAuthFromStorage. On success the gate routes us
    // to /home (signedIn + onboarded), proving refresh worked.
    await expect(page).toHaveURL(/localhost:1456\/#\/home$/, { timeout: 30_000 });

    // The expired token in localStorage should have been replaced.
    const refreshedToken = await page.evaluate(() =>
      localStorage.getItem("puffer.authToken")
    );
    expect(refreshedToken).not.toBeNull();
    const payload = JSON.parse(
      Buffer.from(
        refreshedToken!.split(".")[1].replace(/-/g, "+").replace(/_/g, "/"),
        "base64"
      ).toString("utf8")
    );
    expect(payload.exp * 1000).toBeGreaterThan(Date.now() + 23 * 60 * 60 * 1000);
  });

  test("restart with cached session: lands on /home without re-minting API key", async ({
    page
  }) => {
    test.setTimeout(60_000);
    // Spy on the /platform/v1/teams/*/keys POST that mintWorldRouterApiKey
    // would issue. If our cache guard works, this call MUST NOT fire on a
    // restart that already has a key.
    let mintCalls = 0;
    let exchangeCalls = 0;
    await page.route("**/control-api.worldrouter.ai/**", (route) => {
      const url = route.request().url();
      if (
        route.request().method() === "POST" &&
        /\/platform\/v1\/teams\/[^/]+\/keys$/.test(url)
      ) {
        mintCalls += 1;
      }
      if (
        route.request().method() === "POST" &&
        url.endsWith("/auth/exchange")
      ) {
        exchangeCalls += 1;
      }
      // Don't actually hit prod; this test only asserts on traffic shape.
      return route.fulfill({
        status: 500,
        body: '{"error":"intercepted"}'
      });
    });

    await page.goto("/");
    // Storage state has onboarded=true + JWT + apiKey → land on /home.
    await expect(page).toHaveURL(/localhost:1456\/#\/home$/, {
      timeout: 15_000
    });

    // Confirm the cache survived — same key, same JWT.
    const surviving = await page.evaluate(() => ({
      token: localStorage.getItem("puffer.authToken"),
      apiKey: localStorage.getItem("puffer.worldrouterApiKey")
    }));
    expect(surviving.token).not.toBeNull();
    expect(surviving.apiKey).not.toBeNull();
    expect(surviving.apiKey).toMatch(/^sk-/);

    // Give any racing async tasks a beat, then assert no extra mint.
    await page.waitForTimeout(1500);
    expect(mintCalls, "mint must not fire when a key is already cached").toBe(0);
    expect(
      exchangeCalls,
      "auth/exchange must not fire when a key is already cached"
    ).toBe(0);
  });

  test("expired JWT + failed refresh → wipes ALL auth state, gates to /login", async ({
    page
  }) => {
    test.setTimeout(60_000);
    // Intercept the refresh call to force a failure even when the docs say
    // it should work. Models the "refresh server returned 401" case (e.g.
    // refresh token revoked, server outage).
    await page.route("**/auth.worldrouter.ai/token/refresh", (route) =>
      route.fulfill({
        status: 401,
        contentType: "application/json",
        body: '{"error":"invalid_token"}'
      })
    );

    // Seed an expired access JWT alongside a (stub) refresh token + cached
    // API key. loadAuthFromStorage should try refresh, get rejected, wipe
    // EVERYTHING (including the API key — that's the lifecycle guarantee
    // we're verifying), and route us to /login.
    await page.addInitScript(() => {
      const b64url = (s: string) =>
        btoa(s).replace(/=+$/, "").replace(/\+/g, "-").replace(/\//g, "_");
      const header = b64url(JSON.stringify({ alg: "RS256", typ: "JWT" }));
      const expiredPayload = b64url(
        JSON.stringify({
          sub: "user_expired",
          email: "wangshun+3@tomo.inc",
          exp: Math.floor(Date.now() / 1000) - 3600 // 1h ago
        })
      );
      localStorage.setItem("puffer.authToken", `${header}.${expiredPayload}.sig`);
      // Refresh token can be any non-empty string here — server will reject.
      localStorage.setItem("puffer.authRefreshToken", "stub-refresh-token");
      localStorage.setItem("puffer.worldrouterApiKey", "sk-worldrouter-DUMMY");
      localStorage.setItem("puffer.onboarded", "true");
    });

    await page.goto("/");
    await expect(page).toHaveURL(/localhost:1456\/#\/login(\?|$)/, {
      timeout: 15_000
    });

    const wiped = await page.evaluate(() => ({
      token: localStorage.getItem("puffer.authToken"),
      refresh: localStorage.getItem("puffer.authRefreshToken"),
      apiKey: localStorage.getItem("puffer.worldrouterApiKey"),
      apiKeyOwner: localStorage.getItem("puffer.worldrouterApiKeyOwner")
    }));
    expect(wiped.token).toBeNull();
    expect(wiped.refresh).toBeNull();
    expect(wiped.apiKey, "API key MUST be cleared when login is wiped").toBeNull();
    expect(
      wiped.apiKeyOwner,
      "API key owner tag MUST be cleared alongside the key"
    ).toBeNull();
  });

  test("getWorldRouterApiKey rejects a cached key whose owner doesn't match current user (P0 cross-user fix)", async ({
    page
  }) => {
    test.setTimeout(60_000);
    // Land on /home with whatever the cached state gives us, then
    // overwrite localStorage AND re-run loadAuthFromStorage so authState
    // re-decodes from our test JWT (rather than the cached real one).
    // Doing the overwrite via addInitScript would race with Playwright's
    // own storageState restore — explicit page.evaluate after the gate
    // settles is deterministic.
    await page.goto("/");
    await page.waitForURL(/localhost:1456\/#\/(home|onboarding)/, {
      timeout: 15_000
    });

    const result = await page.evaluate(async () => {
      const mod = await import("/src/lib/auth.svelte.ts");
      const b64url = (s: string) =>
        btoa(s).replace(/=+$/, "").replace(/\+/g, "-").replace(/\//g, "_");
      const header = b64url(JSON.stringify({ alg: "RS256", typ: "JWT" }));
      const payload = b64url(
        JSON.stringify({
          sub: "user-B-sub",
          email: "userB@example.com",
          exp: Math.floor(Date.now() / 1000) + 60 * 60 * 24
        })
      );
      // Cross-user scenario: user-B's JWT in token slot, user-A's key+owner
      // in the key slots. P0 fix says getWorldRouterApiKey returns null.
      localStorage.setItem("puffer.authToken", `${header}.${payload}.sig`);
      localStorage.setItem(
        "puffer.worldrouterApiKey",
        "sk-worldrouter-belongs-to-A"
      );
      localStorage.setItem("puffer.worldrouterApiKeyOwner", "user-A-sub");
      // Reload authState from the overwritten storage so authState.user.sub
      // is now "user-B-sub" — without this the test reads stale authState
      // from the initial cached-state load.
      await mod.loadAuthFromStorage();
      return {
        getKey: mod.getWorldRouterApiKey(),
        rawKey: localStorage.getItem("puffer.worldrouterApiKey"),
        rawOwner: localStorage.getItem("puffer.worldrouterApiKeyOwner")
      };
    });
    expect(result.rawKey).toBe("sk-worldrouter-belongs-to-A");
    expect(result.rawOwner).toBe("user-A-sub");
    expect(
      result.getKey,
      "getWorldRouterApiKey MUST return null when cached owner != current user"
    ).toBeNull();
  });

  test("mintWorldRouterApiKey drops the result if authState changed mid-mint (P0 race-guard fix)", async ({
    page
  }) => {
    test.setTimeout(60_000);
    // Stub control-api so we don't actually mint against prod.
    await page.route("**/control-api.worldrouter.ai/**", (route) => {
      const url = route.request().url();
      if (url.endsWith("/auth/exchange")) {
        return route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            session_token: "stub-session",
            default_team_id: "stub-team"
          })
        });
      }
      return route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ key: "sk-worldrouter-RACE-LOSER" })
      });
    });

    await page.goto("/");
    await page.waitForURL(/localhost:1456\/#\/(home|onboarding|login)/, {
      timeout: 15_000
    });
    const outcome = await page.evaluate(async () => {
      const mod = await import("/src/lib/auth.svelte.ts");
      // Force authState into a state where it CAN'T match a "ghost-user"
      // JWT: wipe storage, re-load → status=signedOut, user=null.
      localStorage.removeItem("puffer.authToken");
      localStorage.removeItem("puffer.authRefreshToken");
      localStorage.removeItem("puffer.worldrouterApiKey");
      localStorage.removeItem("puffer.worldrouterApiKeyOwner");
      await mod.loadAuthFromStorage();
      // Build a valid-looking JWT for "ghost-user". The race guard
      // inside mint requires authState.user.sub === jwt.sub; here
      // authState.user is null → guard rejects.
      const b64url = (s: string) =>
        btoa(s).replace(/=+$/, "").replace(/\+/g, "-").replace(/\//g, "_");
      const header = b64url(JSON.stringify({ alg: "RS256", typ: "JWT" }));
      const payload = b64url(
        JSON.stringify({
          sub: "ghost-user",
          email: "ghost@example.com",
          exp: Math.floor(Date.now() / 1000) + 3600
        })
      );
      const token = `${header}.${payload}.sig`;
      const returned = await mod.mintWorldRouterApiKey(token);
      return {
        returnedKey: returned,
        persistedKey: localStorage.getItem("puffer.worldrouterApiKey"),
        persistedOwner: localStorage.getItem("puffer.worldrouterApiKeyOwner"),
        authStatus: mod.authState.status
      };
    });
    expect(outcome.authStatus).toBe("signedOut");
    // The function returns the key (caller convenience) but does NOT
    // persist it when the owner session is no longer current.
    expect(outcome.returnedKey).toBe("sk-worldrouter-RACE-LOSER");
    expect(
      outcome.persistedKey,
      "mint MUST NOT persist the key when authState's user doesn't match jwt.sub"
    ).toBeNull();
    expect(outcome.persistedOwner).toBeNull();
  });

  test("signOut clears the API key alongside auth state", async ({ page }) => {
    test.setTimeout(60_000);
    await page.goto("/");
    const before = await page.evaluate(() => ({
      token: localStorage.getItem("puffer.authToken"),
      refresh: localStorage.getItem("puffer.authRefreshToken"),
      apiKey: localStorage.getItem("puffer.worldrouterApiKey"),
      apiKeyOwner: localStorage.getItem("puffer.worldrouterApiKeyOwner")
    }));
    expect(before.token).not.toBeNull();

    // Call signOut AND read localStorage in the same evaluate, BEFORE
    // signOut's `window.location.href` assignment kicks off a cross-origin
    // navigation. Browsers deny localStorage access during the transitional
    // about:blank state that an aborted cross-origin nav drops us into, so
    // a separate page.evaluate() afterwards races and 50% errors with
    // SecurityError.
    const after = await page.evaluate(async () => {
      const mod = await import("/src/lib/auth.svelte.ts");
      // Stub location.href so signOut's bounce to Auth Station /logout is
      // a no-op for the purpose of this test — we only care that the
      // synchronous local-clear happened.
      const proto = Object.getOwnPropertyDescriptor(
        window.Location.prototype,
        "href"
      );
      Object.defineProperty(window.Location.prototype, "href", {
        configurable: true,
        get() {
          return proto?.get?.call(window.location) ?? "";
        },
        set() {
          /* swallow cross-origin navigation in this test */
        }
      });
      mod.signOut();
      // Read storage right after signOut's sync clearing finishes.
      return {
        token: localStorage.getItem("puffer.authToken"),
        refresh: localStorage.getItem("puffer.authRefreshToken"),
        apiKey: localStorage.getItem("puffer.worldrouterApiKey"),
        apiKeyOwner: localStorage.getItem("puffer.worldrouterApiKeyOwner")
      };
    });
    expect(after.token).toBeNull();
    expect(after.refresh).toBeNull();
    expect(after.apiKey).toBeNull();
    expect(after.apiKeyOwner).toBeNull();
  });
});

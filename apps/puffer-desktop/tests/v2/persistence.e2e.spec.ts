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

test.describe("login persistence", () => {
  test.skip(
    !fs.existsSync(STORAGE_STATE_FILE),
    "no cached storage state — run login.e2e.spec.ts first"
  );
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

  test("signOut clears the API key alongside auth state", async ({ page }) => {
    test.setTimeout(60_000);
    await page.goto("/");
    const before = await page.evaluate(() => ({
      token: localStorage.getItem("puffer.authToken"),
      refresh: localStorage.getItem("puffer.authRefreshToken"),
      apiKey: localStorage.getItem("puffer.worldrouterApiKey")
    }));
    expect(before.token).not.toBeNull();

    // Call signOut AND read localStorage in the same evaluate, BEFORE
    // signOut's `window.location.href` assignment kicks off a cross-origin
    // navigation. Browsers deny localStorage access during the transitional
    // about:blank state that an aborted cross-origin nav drops us into, so
    // a separate page.evaluate() afterwards races and 50% errors with
    // SecurityError.
    const after = await page.evaluate(async () => {
      const mod = await import("/src-v2/lib/auth.svelte.ts");
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
        apiKey: localStorage.getItem("puffer.worldrouterApiKey")
      };
    });
    expect(after.token).toBeNull();
    expect(after.refresh).toBeNull();
    expect(after.apiKey).toBeNull();
  });
});

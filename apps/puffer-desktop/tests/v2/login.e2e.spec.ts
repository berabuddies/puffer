/**
 * REAL end-to-end login against Production Auth Station.
 *
 * Quarantined off the smoke spec (login.spec.ts) because this one actually
 * hits auth.worldrouter.ai. Prereqs:
 *   - `http://127.0.0.1:1456` in ALLOWED_REDIRECT_ORIGINS on Production
 *     (re-uses the donor branch's pre-whitelisted CLI port)
 *   - A working test account (default: wangshun+3@tomo.inc)
 *
 * Run:
 *   npx playwright test tests/v2/login.e2e.spec.ts --headed --reporter=line
 *
 * Env:
 *   PUFFER_E2E_EMAIL          (default: wangshun+3@tomo.inc)
 *   PUFFER_E2E_PASSWORD       (required)
 *   PUFFER_E2E_OTP_FILE       (default: /tmp/puffer_e2e_otp.txt)
 *   PUFFER_E2E_STORAGE_STATE  (default: tests/v2/.auth-storage.json)
 *
 * --- OTP fatigue mitigation ---
 *
 * WorkOS Radar challenges every fresh browser context, and OTP is invalidated
 * on every new send — so naive re-runs spam the user's inbox. We cache the
 * browser's storageState (auth.worldrouter.ai cookies + 127.0.0.1 localStorage)
 * to disk after a successful login. Subsequent runs reuse it:
 *
 *   Run 1 (no cache) → drives full flow, asks orchestrator for OTP once, saves
 *                      state on success.
 *   Run 2+ (cache)   → loads state, skips Auth Station entirely, navigates
 *                      directly to /home with the persisted JWT and verifies
 *                      the post-login app behaviour.
 *
 * The cache file is gitignored. Auth Station session is good for ~24h; refresh
 * token good for 7 days. Delete the cache file to force a fresh login.
 */
import { test, expect } from "@playwright/test";
import * as fs from "node:fs";
import * as path from "node:path";

const EMAIL = process.env.PUFFER_E2E_EMAIL ?? "wangshun+3@tomo.inc";
const PASSWORD = process.env.PUFFER_E2E_PASSWORD ?? "";
const OTP_SIGNAL_FILE =
  process.env.PUFFER_E2E_OTP_FILE ?? "/tmp/puffer_e2e_otp.txt";
const STORAGE_STATE_FILE = path.resolve(
  process.env.PUFFER_E2E_STORAGE_STATE ?? "tests/v2/.auth-storage.json"
);

/** Poll a signal file for a 6-digit OTP. Throws on timeout. */
async function waitForOtpFile(timeoutMs: number): Promise<string> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const raw = fs.readFileSync(OTP_SIGNAL_FILE, "utf8").trim();
      if (/^\d{6}$/.test(raw)) {
        try {
          fs.unlinkSync(OTP_SIGNAL_FILE);
        } catch {
          /* one-shot file — ignore */
        }
        return raw;
      }
    } catch {
      /* file not present yet */
    }
    await new Promise((r) => setTimeout(r, 500));
  }
  throw new Error(
    `OTP signal file ${OTP_SIGNAL_FILE} did not receive a 6-digit code within ${timeoutMs}ms`
  );
}

function decodeJwtEmail(token: string): string | undefined {
  const parts = token.split(".");
  if (parts.length !== 3) return undefined;
  const payload = JSON.parse(
    Buffer.from(
      parts[1].replace(/-/g, "+").replace(/_/g, "/"),
      "base64"
    ).toString("utf8")
  );
  return typeof payload.email === "string" ? payload.email : undefined;
}

test.describe("real login round-trip (Production Auth Station)", () => {
  test.skip(!PASSWORD, "set PUFFER_E2E_PASSWORD to run real-login E2E");

  /**
   * Cached-state path: storage file exists, so we already have a valid
   * Auth Station session + a puffer.authToken in localStorage. Just open
   * the app and assert the post-login state — no Auth Station round-trip.
   */
  test.describe("cached session", () => {
    test.skip(
      !fs.existsSync(STORAGE_STATE_FILE),
      "no cached storage state yet — run the 'first login' test once to populate it"
    );
    test.use({ storageState: STORAGE_STATE_FILE });

    test("loads /home with persisted token", async ({ page }) => {
      test.setTimeout(60_000);
      // The cached localStorage was captured under origin 127.0.0.1:1456 so
      // we must start at the same origin or Playwright won't apply it.
      await page.goto("/");
      // Either the gate routes us to /home (token present + onboarded) or to
      // /onboarding/where (token present + not onboarded). Both prove the
      // gate is honouring the cached session — accept either.
      await expect(page).toHaveURL(
        /localhost:1456\/#\/(home|onboarding\/where)/,
        {
          timeout: 15_000
        }
      );

      const stored = await page.evaluate(() =>
        localStorage.getItem("puffer.authToken")
      );
      expect(stored, "puffer.authToken should be in localStorage").not.toBeNull();
      const email = decodeJwtEmail(stored!);
      expect(email).toBe(EMAIL);

      await page.screenshot({
        path: "test-results/v2-login-cached-home.png",
        fullPage: false
      });
    });
  });

  /**
   * First-login path: no cached state yet. Drive the whole flow, ask the
   * orchestrator for the OTP, then dump storageState so future runs skip
   * the OTP wall.
   */
  test.describe("first login", () => {
    test.skip(
      fs.existsSync(STORAGE_STATE_FILE),
      "storage state already cached — first-login test is one-shot; delete the cache to re-run"
    );

    test("Sign in → Auth Station → callback → /home with token persisted", async ({
      page,
      context
    }) => {
      // Generous: must outlast the OTP wait (up to 5 min) + redirect.
      test.setTimeout(7 * 60_000);

      // First navigation: clear all auth-adjacent state. We do this via
      // page.evaluate AFTER goto rather than addInitScript, because the
      // init script would also re-run on the post-Auth-Station landing
      // and clobber the sessionStorage `state` value our own goToLogin
      // just stashed — causing a spurious state_mismatch.
      await page.goto("/");
      await page.evaluate(() => {
        try {
          localStorage.removeItem("puffer.authToken");
          localStorage.removeItem("puffer.authRefreshToken");
          localStorage.setItem("puffer.onboarded", "true");
          sessionStorage.clear();
        } catch {
          /* private mode — ignore */
        }
      });
      // Reload so the auth gate re-evaluates against the wiped storage.
      await page.reload();
      await expect(page).toHaveURL(/#\/login(\?|$)/);

      await Promise.all([
        page.waitForURL(/(authkit|auth)\.worldrouter\.ai/, { timeout: 30_000 }),
        page.getByRole("button", { name: "Sign in / Sign up" }).click()
      ]);

      const emailField = page.getByRole("textbox", { name: /email/i });
      await expect(emailField).toBeVisible({ timeout: 15_000 });
      await emailField.fill(EMAIL);
      await page.getByRole("button", { name: /continue/i }).click();

      const passwordField = page.locator('input[type="password"]').first();
      await expect(passwordField).toBeVisible({ timeout: 15_000 });
      await passwordField.fill(PASSWORD);
      await page
        .getByRole("button", { name: /sign in|log in|continue|submit/i })
        .first()
        .click();

      await page.waitForURL(
        (url: URL) =>
          url.host === "localhost:1456" ||
          url.pathname.includes("radar-challenge"),
        { timeout: 30_000 }
      );

      if (page.url().includes("radar-challenge")) {
        // eslint-disable-next-line no-console
        console.log(
          `[login.e2e] radar challenge hit — waiting for OTP at ${OTP_SIGNAL_FILE}`
        );
        const otp = await waitForOtpFile(5 * 60_000);
        for (const digit of otp) {
          await page.keyboard.type(digit, { delay: 50 });
        }
        await page.waitForURL(
          /localhost:1456\/#\/(home|onboarding|auth\/callback)/,
          { timeout: 30_000 }
        );
      }

      await expect(page).toHaveURL(/localhost:1456\/#\/home$/, {
        timeout: 30_000
      });

      const stored = await page.evaluate(() =>
        localStorage.getItem("puffer.authToken")
      );
      expect(stored).not.toBeNull();
      expect(decodeJwtEmail(stored!)).toBe(EMAIL);

      await page.screenshot({
        path: "test-results/v2-login-post-auth-home.png",
        fullPage: false
      });

      // Wait for the AuthCallback's fire-and-forget mintWorldRouterApiKey
      // to land an sk-worldrouter-… key in localStorage. Without this poll
      // we'd snapshot storage state mid-mint and downstream "key tracks
      // login" tests would see apiKey=null.
      await expect
        .poll(
          () =>
            page.evaluate(() =>
              localStorage.getItem("puffer.worldrouterApiKey")
            ),
          { timeout: 30_000 }
        )
        .toMatch(/^sk-/);

      // Persist the full browser state (cookies for auth.worldrouter.ai +
      // localStorage for 127.0.0.1:1456) so the cached-session test can
      // run without an OTP next time.
      fs.mkdirSync(path.dirname(STORAGE_STATE_FILE), { recursive: true });
      await context.storageState({ path: STORAGE_STATE_FILE });
      // eslint-disable-next-line no-console
      console.log(
        `[login.e2e] storage state cached → ${STORAGE_STATE_FILE} (delete to force re-login)`
      );
    });
  });
});

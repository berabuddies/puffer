/**
 * Real round-trip: take the cached Auth Station JWT and try to mint a
 * worldrouter `sk-worldrouter-…` API key via control-api's two-hop.
 *
 * Depends on tests/v2/.auth-storage.json (run login.e2e.spec.ts first).
 *
 * Run:
 *   PUFFER_E2E_PASSWORD=… ./node_modules/.bin/playwright test \
 *     tests/v2/apikey.e2e.spec.ts --reporter=line
 *
 * If CORS blocks the request, the error will include the exact failure
 * mode (TypeError for opaque CORS-blocked fetches, vs a 4xx body for
 * application errors). That signal tells us whether the browser-driven
 * approach is viable or whether we need a Tauri-side proxy.
 */
import { test, expect } from "@playwright/test";
import * as fs from "node:fs";
import * as path from "node:path";

const STORAGE_STATE_FILE = path.resolve(
  process.env.PUFFER_E2E_STORAGE_STATE ?? "tests/v2/.auth-storage.json"
);

test.describe("worldrouter API-key minting", () => {
  test.skip(
    !fs.existsSync(STORAGE_STATE_FILE),
    "no cached storage state — run login.e2e.spec.ts first to populate it"
  );
  test.use({ storageState: STORAGE_STATE_FILE });

  test("mints sk-worldrouter-… key from cached Auth Station JWT", async ({
    page
  }) => {
    test.setTimeout(60_000);
    await page.goto("/");

    // Drive the helper directly from the page context so we hit control-api
    // with the same origin (and same cookies, if any) as production usage.
    const result = await page.evaluate(async () => {
      // Dynamically import the helper so this test stays decoupled from
      // module-load timing.
      const mod = await import("/src-v2/lib/auth.svelte.ts");
      const token = mod.getAuthToken();
      if (!token) return { ok: false, reason: "no token in localStorage" };
      try {
        const key = await mod.mintWorldRouterApiKey(token);
        return { ok: true, key };
      } catch (err) {
        const e = err as { name?: string; message?: string; step?: string; status?: number; body?: unknown };
        return {
          ok: false,
          reason: e.message ?? String(err),
          name: e.name,
          step: e.step,
          status: e.status,
          body: e.body
        };
      }
    });

    // eslint-disable-next-line no-console
    console.log("[apikey.e2e] result:", JSON.stringify(result, null, 2));

    if (!result.ok) {
      // Surface the exact failure clearly. A TypeError ("Failed to fetch")
      // is the canonical browser-side CORS-blocked signal; a structured
      // ApiKeyMintError means the server responded — useful for debugging.
      throw new Error(
        `mint failed:\n${JSON.stringify(result, null, 2)}`
      );
    }
    expect(typeof result.key).toBe("string");
    expect(result.key).toMatch(/^sk-/);
    // eslint-disable-next-line no-console
    console.log(
      `[apikey.e2e] minted ${result.key!.slice(0, 18)}… (length ${result.key!.length})`
    );
  });
});

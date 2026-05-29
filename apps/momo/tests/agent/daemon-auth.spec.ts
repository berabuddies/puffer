/**
 * Task 1 — worldrouter key 接入 daemon.
 *
 * The minted `sk-worldrouter-…` key must no longer be registered with momo's
 * own 1431 backend under provider id "puffer". Instead `loginWorldRouter`
 * routes it through the puffer *daemon* (via `daemonClient`) as the built-in
 * `openai` provider, and points that provider's base_url at worldrouter's
 * OpenAI-compatible inference endpoint.
 *
 * The daemonClient round-trip is exercised end-to-end under test: the page
 * calls `daemon_handshake` on the backend `/ws` path, then dials the
 * FakeDaemon's `/daemon` path for `login_with_api_key` / `update_config`
 * (the FakeDaemon stands up both paths and already dispatches both methods).
 */
import { expect, test } from "@playwright/test";
import { FakeDaemon } from "../support/fakeDaemon";
import { bootOnboarded } from "../support/bootHelpers";

test("loginWorldRouter registers the key as the daemon's openai provider", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);

  // The DEV-only test bridge is installed when daemonAuth.ts loads. Import it
  // explicitly so the bridge is present regardless of which routes pulled the
  // module in already.
  await page.evaluate(async () => {
    await import("/src/lib/agent/daemonAuth.ts");
  });

  const loginPromise = daemon.waitForRequest("login_with_api_key");
  const configPromise = daemon.waitForRequest("update_config");

  await page.evaluate(async () => {
    await (window as unknown as {
      __loginWorldRouter: (key: string) => Promise<void>;
    }).__loginWorldRouter("sk-worldrouter-TEST");
  });

  const login = await loginPromise;
  expect(login.params).toMatchObject({
    providerId: "openai",
    apiKey: "sk-worldrouter-TEST"
  });

  const config = await configPromise;
  expect(config.params).toMatchObject({
    openaiBaseUrl: "https://inference-api.worldrouter.ai/v1",
    defaultProvider: "openai",
    defaultModel: "gpt-5.4"
  });

  // The key must NOT be registered under the legacy "puffer" provider id.
  const pufferLogin = daemon.requests.find(
    (req) => req.method === "login_with_api_key" && req.params.providerId === "puffer"
  );
  expect(pufferLogin).toBeUndefined();
});

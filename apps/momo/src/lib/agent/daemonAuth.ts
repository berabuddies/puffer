/**
 * Route the minted worldrouter API key to the puffer *daemon* (not momo's
 * own 1431 backend).
 *
 * The daemon has no "puffer" provider, so we register the key against the
 * built-in OpenAI-compatible `openai` provider and override that provider's
 * `base_url` to point at worldrouter's inference endpoint
 * (OpenAI-compatible chat/completions). `update_config` also makes `openai`
 * the default provider/model so chat turns pick up the worldrouter route.
 *
 * Connection plumbing lives in `daemonClient.ts`: `ensureDaemonClient()`
 * lazily handshakes (via momo's 1431 backend) and dials the daemon ws, and
 * `.request(method, params)` sends a daemon-protocol RPC.
 */
import { ensureDaemonClient } from "../daemonClient";

export const WORLDROUTER_BASE_URL = "https://inference-api.worldrouter.ai/v1";
export const WORLDROUTER_DEFAULT_MODEL = "gpt-5.4";

/** Register a minted worldrouter key with the daemon as the OpenAI-compatible
 *  provider, and point the openai provider's base_url at worldrouter. */
export async function loginWorldRouter(apiKey: string): Promise<void> {
  const client = await ensureDaemonClient();
  await client.request("login_with_api_key", { providerId: "openai", apiKey });
  await client.request("update_config", {
    openaiBaseUrl: WORLDROUTER_BASE_URL,
    defaultProvider: "openai",
    defaultModel: WORLDROUTER_DEFAULT_MODEL
  });
}

// Test bridge: lets Playwright drive the daemon round-trip without wiring a
// full login flow. DEV-only so it never ships in a production bundle.
if (import.meta.env.DEV) {
  (window as unknown as { __loginWorldRouter?: typeof loginWorldRouter }).__loginWorldRouter =
    loginWorldRouter;
}

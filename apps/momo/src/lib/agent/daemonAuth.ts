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
import { request as backendRequest } from "../wsClient";
import { getAuthToken } from "../auth.svelte";

// Inference endpoint the minted worldrouter key is sent to. MUST track the
// same environment as the control-api that mints the key
// (VITE_WORLDROUTER_CONTROL_URL): a test-minted key hitting the prod inference
// proxy fails with 401 "tokennotfoundindb" (the prod LiteLLM has no record of
// it). Env-driven so switching .env to test moves the endpoint too; defaults
// to prod when unset.
export const WORLDROUTER_BASE_URL =
  (import.meta.env.VITE_WORLDROUTER_INFERENCE_URL as string | undefined) ??
  "https://inference-api.worldrouter.ai/v1";
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

  // Also persist the key to ~/.wr/.creds (via momo's 1431 backend) so
  // WorldRouter-backed skills like book-by-phone can `source` it from the
  // agent's bash. NOTE: the base URL here is the *control-api* (where the
  // lifeclaw skill route lives), NOT the inference endpoint used for chat
  // above. Best-effort — don't block login if it fails.
  const controlUrl =
    (import.meta.env.VITE_WORLDROUTER_CONTROL_URL as string | undefined) ??
    "https://control-api.worldrouter.ai";
  try {
    await backendRequest("write_wr_creds", { apiKey, baseUrl: controlUrl });
  } catch (e) {
    console.warn("[wr] write_wr_creds failed (book-by-phone may lack creds)", e);
  }

  // Also persist the ucard backend base URL + the Auth-Station JWT to
  // ~/.ucard/.creds so the `momo-card` bin can reveal the card off-agent.
  // The base URL is the REAL backend (the bin isn't a browser → no CORS / no
  // Vite proxy), so we use VITE_BACKEND_BASE_URL directly. Best-effort.
  const ucardBase =
    (import.meta.env.VITE_BACKEND_BASE_URL as string | undefined) ??
    "http://127.0.0.1:8080";
  const jwt = getAuthToken();
  if (jwt) {
    try {
      await backendRequest("write_ucard_creds", { baseUrl: ucardBase, jwt });
      await backendRequest("install_card_agent_assets", {});
    } catch (e) {
      console.warn("[ucard] write_ucard_creds/install assets failed", e);
    }
  } else {
    console.warn("[ucard] no JWT at login — skipping ucard creds write");
  }
}

// Test bridge: lets Playwright drive the daemon round-trip without wiring a
// full login flow. DEV-only so it never ships in a production bundle.
if (import.meta.env.DEV) {
  (window as unknown as { __loginWorldRouter?: typeof loginWorldRouter }).__loginWorldRouter =
    loginWorldRouter;
}

/**
 * Auth state + helpers for the worldrouter (Auth Station) OIDC login flow.
 *
 * Wire-level summary:
 *   1. UI calls `goToLogin()` → window.location.href to
 *      `${AUTH_STATION_URL}/login?redirect_uri=…&client_state=…`.
 *   2. Auth Station authenticates the user and 302s back to
 *      `${redirect_uri}?token=<JWT>&refresh_token=<JWT>&state=<…>`.
 *   3. The /auth/callback page calls `handleAuthCallback(query)` which
 *      validates `state` against the value we stashed in sessionStorage,
 *      then persists the token to localStorage and populates `authState`.
 *
 * Trust model:
 *   We do NOT verify the JWT signature here — the backend is the only
 *   thing that can do that meaningfully (it has the JWKS / shared secret).
 *   The frontend treats the token as opaque except for decoding the
 *   payload to display `email` / `name` and to check `exp` for early
 *   expiry. We trust the token because (a) the redirect happened in the
 *   same webview (no third-party origin can write to our localStorage),
 *   and (b) the `state` round-trip rules out blind CSRF.
 *
 * The original onboarding flag helpers (isOnboarded / markOnboarded /
 * resetOnboarding) are kept verbatim so existing callers compile.
 *
 * SSR safety: every storage / window access is guarded so Vite's import
 * graph never throws.
 */

const ONBOARDED_KEY = "puffer.onboarded";
const TOKEN_KEY = "puffer.authToken";
const REFRESH_TOKEN_KEY = "puffer.authRefreshToken";
const STATE_KEY = "puffer.authState";
const RETURN_TO_KEY = "puffer.authReturnTo";
const API_KEY_KEY = "puffer.worldrouterApiKey";

const AUTH_STATION_URL = import.meta.env.VITE_AUTH_STATION_URL as string | undefined;
// control-api hosts the JWT→API-key two-hop. Mirrors donor's
// PUFFER_WORLDROUTER_CONTROL_URL env override.
const CONTROL_API_URL =
  (import.meta.env.VITE_WORLDROUTER_CONTROL_URL as string | undefined) ??
  "https://control-api.worldrouter.ai";

function safeLocalStorage(): Storage | null {
  if (typeof window === "undefined") return null;
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

function safeSessionStorage(): Storage | null {
  if (typeof window === "undefined") return null;
  try {
    return window.sessionStorage;
  } catch {
    return null;
  }
}

/* ───────── Onboarding flag (unchanged public API) ───────── */

/** True if the user has completed (or skipped past) the onboarding flow. */
export function isOnboarded(): boolean {
  const store = safeLocalStorage();
  if (!store) return false;
  return store.getItem(ONBOARDED_KEY) === "true";
}

/** Mark the user as onboarded. Idempotent — safe to call on every /home hit. */
export function markOnboarded(): void {
  const store = safeLocalStorage();
  if (!store) return;
  if (store.getItem(ONBOARDED_KEY) === "true") return;
  store.setItem(ONBOARDED_KEY, "true");
}

/** Clear the flag — useful for dev tooling / a future "reset" affordance. */
export function resetOnboarding(): void {
  const store = safeLocalStorage();
  if (!store) return;
  store.removeItem(ONBOARDED_KEY);
}

/* ───────── Auth state ───────── */

export interface AuthUser {
  sub: string;
  email?: string;
  name?: string;
  picture?: string;
  /** Unix epoch seconds — same units as the JWT `exp` claim. */
  exp: number;
}

export type AuthStatus = "unknown" | "signedIn" | "signedOut";

export interface AuthState {
  status: AuthStatus;
  user: AuthUser | null;
}

/**
 * Reactive auth store. Starts as "unknown" — App.svelte calls
 * `loadAuthFromStorage()` on mount to flip it to a definitive state.
 * Components should NOT render auth-dependent UI while status is
 * "unknown"; render a splash instead so we never flash signed-in chrome
 * before the local-storage check resolves.
 */
export const authState = $state<AuthState>({ status: "unknown", user: null });

/* ───────── JWT decoding (no signature verification) ───────── */

function base64UrlDecode(segment: string): string {
  // Restore standard base64 alphabet + padding so atob accepts it.
  const padded = segment.replace(/-/g, "+").replace(/_/g, "/");
  const padLen = (4 - (padded.length % 4)) % 4;
  const full = padded + "=".repeat(padLen);
  if (typeof atob === "function") {
    // atob returns binary string; decode UTF-8 by walking bytes.
    const raw = atob(full);
    try {
      const bytes = new Uint8Array(raw.length);
      for (let i = 0; i < raw.length; i++) bytes[i] = raw.charCodeAt(i);
      return new TextDecoder().decode(bytes);
    } catch {
      return raw;
    }
  }
  throw new Error("atob unavailable");
}

function decodeJwtPayload(token: string): AuthUser | null {
  if (typeof token !== "string" || token.length === 0) return null;
  const parts = token.split(".");
  if (parts.length !== 3) return null;
  let payload: Record<string, unknown>;
  try {
    payload = JSON.parse(base64UrlDecode(parts[1])) as Record<string, unknown>;
  } catch {
    return null;
  }
  const sub = typeof payload.sub === "string" ? payload.sub : null;
  const exp = typeof payload.exp === "number" ? payload.exp : null;
  if (!sub || !exp) return null;
  // Expired tokens count as malformed for our purposes — caller treats
  // them the same as missing tokens.
  if (exp * 1000 < Date.now()) return null;
  return {
    sub,
    exp,
    email: typeof payload.email === "string" ? payload.email : undefined,
    name: typeof payload.name === "string" ? payload.name : undefined,
    picture: typeof payload.picture === "string" ? payload.picture : undefined
  };
}

/* ───────── Public auth helpers ───────── */

/**
 * Re-hydrate auth state on app mount. Async because we may need to ask
 * Auth Station to refresh the access JWT (24h TTL) using the cached
 * refresh token (7d TTL) — that's what gives us the "7 days between
 * logins" guarantee the user asked for.
 *
 * State transitions:
 *   - access JWT valid              → signedIn
 *   - access expired, refresh works → mint new access, signedIn
 *   - both gone / refresh fails     → signedOut + wipe ALL cached
 *     auth data (JWT, refresh token, API key) so a stale sk-* key
 *     can't outlive its login.
 *
 * Side-effect on signedIn: if we have a cached worldrouter API key,
 * re-register it with the puffer Tauri host (idempotent; covers host
 * restarts that may have lost the in-memory credential).
 */
export async function loadAuthFromStorage(): Promise<void> {
  const store = safeLocalStorage();
  if (!store) {
    authState.status = "signedOut";
    authState.user = null;
    return;
  }

  const token = store.getItem(TOKEN_KEY);
  const refreshToken = store.getItem(REFRESH_TOKEN_KEY);

  // Happy path: access JWT still valid.
  let user = token ? decodeJwtPayload(token) : null;
  if (user) {
    authState.status = "signedIn";
    authState.user = user;
    syncApiKeyToHostIfCached();
    return;
  }

  // Access JWT missing / expired. Try refresh if we still have the
  // 7-day refresh token.
  if (refreshToken && AUTH_STATION_URL) {
    try {
      const fresh = await refreshAuthStationToken(refreshToken);
      store.setItem(TOKEN_KEY, fresh);
      user = decodeJwtPayload(fresh);
      if (user) {
        authState.status = "signedIn";
        authState.user = user;
        syncApiKeyToHostIfCached();
        return;
      }
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn("[auth] refresh failed — signing out", err);
    }
  }

  // Both gone (or refresh rejected). Sign out and wipe everything that
  // belongs to the previous session — including the API key, so it
  // can't outlive its login.
  store.removeItem(TOKEN_KEY);
  store.removeItem(REFRESH_TOKEN_KEY);
  store.removeItem(API_KEY_KEY);
  authState.status = "signedOut";
  authState.user = null;
}

/**
 * Exchange a refresh token for a new access JWT.
 * Per auth-docs/api-reference.md, refresh tokens are NOT rotated — same
 * token can be re-used until the 7-day TTL expires.
 */
async function refreshAuthStationToken(refreshToken: string): Promise<string> {
  if (!AUTH_STATION_URL) throw new Error("AUTH_STATION_URL not configured");
  const res = await fetch(`${AUTH_STATION_URL.replace(/\/$/, "")}/token/refresh`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ refresh_token: refreshToken })
  });
  if (!res.ok) {
    throw new Error(`token/refresh failed: HTTP ${res.status}`);
  }
  const body = (await res.json()) as { token?: string };
  if (typeof body.token !== "string" || body.token.length === 0) {
    throw new Error("token/refresh returned no token");
  }
  return body.token;
}

/**
 * Best-effort re-register: if we already have a cached API key (from a
 * prior login), push it back to the puffer host. Covers the case where
 * the host restarted and lost its in-memory credential map, while
 * leaving our localStorage intact. Idempotent on the host side.
 */
function syncApiKeyToHostIfCached(): void {
  const key = getWorldRouterApiKey();
  if (!key) return;
  void registerApiKeyWithPufferHost(key).catch((err) => {
    // eslint-disable-next-line no-console
    console.warn("[auth] re-register API key after restart failed", err);
  });
}

/** Return the currently-stored access token (or null). */
export function getAuthToken(): string | null {
  const store = safeLocalStorage();
  if (!store) return null;
  return store.getItem(TOKEN_KEY);
}

/** Return the cached worldrouter `sk-worldrouter-…` API key (or null). */
export function getWorldRouterApiKey(): string | null {
  const store = safeLocalStorage();
  if (!store) return null;
  return store.getItem(API_KEY_KEY);
}

/* ───────── worldrouter API-key minting (two-hop) ───────── */

interface ExchangeResponse {
  session_token: string;
  default_team_id: string;
}

interface MintKeyResponse {
  key: string;
  token_id?: string;
}

function buildKeyAlias(): string {
  const uuid =
    typeof crypto !== "undefined" && typeof crypto.randomUUID === "function"
      ? crypto.randomUUID()
      : `${Date.now()}-${Math.random().toString(36).slice(2)}`;
  return `puffer-desktop-${uuid.toUpperCase()}`;
}

export class ApiKeyMintError extends Error {
  constructor(
    message: string,
    public readonly step: "exchange" | "mint",
    public readonly status?: number,
    public readonly body?: unknown
  ) {
    super(message);
    this.name = "ApiKeyMintError";
  }
}

/**
 * Exchange the Auth Station JWT for an `sk-worldrouter-…` API key via
 * control-api's two-hop:
 *   1. POST /auth/exchange  { access_token } → { session_token, default_team_id }
 *   2. POST /platform/v1/teams/{team_id}/keys (Bearer session_token)
 *      { key_alias } → { key: "sk-worldrouter-…" }
 *
 * On success, persists the key to localStorage and returns it. Re-mints
 * on every call — caller should check getWorldRouterApiKey() first to
 * avoid the documented "duplicate key per login" leak the donor flagged.
 *
 * Throws ApiKeyMintError on any non-2xx; caller decides UX.
 *
 * CORS WARNING (untested as of port date 2026-05-26): control-api is
 * documented as backend-to-backend; browser fetch may be blocked by
 * preflight. If it fails with a network/CORS error, the desktop app
 * will need either a Tauri-side proxy (out of UI-scope) or worldrouter
 * to allow-list http://localhost:1456 on control-api's CORS policy.
 */
export async function mintWorldRouterApiKey(
  authStationToken: string
): Promise<string> {
  // Hop 1: exchange Auth Station JWT for an Infer-session token + team_id.
  const exchangeRes = await fetch(`${CONTROL_API_URL}/auth/exchange`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ access_token: authStationToken })
  });
  if (!exchangeRes.ok) {
    const body = await safeJson(exchangeRes);
    throw new ApiKeyMintError(
      `auth/exchange failed: HTTP ${exchangeRes.status}`,
      "exchange",
      exchangeRes.status,
      body
    );
  }
  const exchange = (await exchangeRes.json()) as ExchangeResponse;
  if (!exchange.session_token || !exchange.default_team_id) {
    throw new ApiKeyMintError(
      "auth/exchange returned incomplete payload",
      "exchange",
      exchangeRes.status,
      exchange
    );
  }

  // Hop 2: mint the inference key.
  const mintRes = await fetch(
    `${CONTROL_API_URL}/platform/v1/teams/${encodeURIComponent(
      exchange.default_team_id
    )}/keys`,
    {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${exchange.session_token}`
      },
      body: JSON.stringify({ key_alias: buildKeyAlias() })
    }
  );
  if (!mintRes.ok) {
    const body = await safeJson(mintRes);
    throw new ApiKeyMintError(
      `keys mint failed: HTTP ${mintRes.status}`,
      "mint",
      mintRes.status,
      body
    );
  }
  const mint = (await mintRes.json()) as MintKeyResponse;
  if (typeof mint.key !== "string" || !mint.key.startsWith("sk-")) {
    throw new ApiKeyMintError(
      "keys mint returned no key",
      "mint",
      mintRes.status,
      mint
    );
  }

  const store = safeLocalStorage();
  if (store) store.setItem(API_KEY_KEY, mint.key);

  // Register with the puffer Tauri host so chat actually picks it up.
  // We swallow errors here (key is still cached locally; retry will hit
  // the same path on next login) so a transient WS hiccup doesn't fail
  // the whole mint.
  try {
    await registerApiKeyWithPufferHost(mint.key);
  } catch (err) {
    // eslint-disable-next-line no-console
    console.warn(
      "[auth] minted API key but failed to register with puffer host; chat may not pick it up until retry",
      err
    );
  }

  return mint.key;
}

/**
 * Register the minted `sk-worldrouter-…` key with the puffer Tauri host
 * over WebSocket. The host stores it as the `puffer` provider's API key
 * (the only worldrouter-compatible provider id the backend whitelists)
 * and will inject it as `PUFFER_API_KEY` when it spawns the puffer CLI
 * for a chat turn — which is what makes chat actually work.
 *
 * Kept here (next to mintWorldRouterApiKey) instead of in wsClient.ts so
 * the auth module owns the whole "JWT → API key → registered" pipeline.
 */
export async function registerApiKeyWithPufferHost(apiKey: string): Promise<void> {
  // Lazy import keeps auth.svelte.ts importable in non-browser contexts
  // (e.g. unit tests) where wsClient pulls in `WebSocket`.
  const { request } = await import("./wsClient");
  await request("login_with_api_key", {
    providerId: "puffer",
    apiKey
  });
}

async function safeJson(res: Response): Promise<unknown> {
  try {
    return await res.json();
  } catch {
    try {
      return await res.text();
    } catch {
      return null;
    }
  }
}

/**
 * Build the Auth Station login URL and navigate the webview to it.
 * Persists a CSRF `state` and (optionally) a `returnTo` path so the
 * callback page can route the user back to where they intended.
 *
 * Never returns — the document is navigating away.
 */
export function goToLogin(returnTo?: string): void {
  if (typeof window === "undefined") return;
  if (!AUTH_STATION_URL) {
    // Fail loud in dev so the missing env var doesn't silently no-op.
    // eslint-disable-next-line no-console
    console.error("[auth] VITE_AUTH_STATION_URL is not set");
    return;
  }
  const session = safeSessionStorage();
  // crypto.randomUUID is available in all Tauri WebKit / Chromium webviews.
  const state =
    typeof crypto !== "undefined" && typeof crypto.randomUUID === "function"
      ? crypto.randomUUID()
      : `${Date.now()}-${Math.random().toString(36).slice(2)}`;
  if (session) {
    session.setItem(STATE_KEY, state);
    if (returnTo) session.setItem(RETURN_TO_KEY, returnTo);
    else session.removeItem(RETURN_TO_KEY);
  }
  // OAuth 2.0 (RFC 6749 §3.1.2) forbids fragments in redirect_uri, and Auth
  // Station silently treats fragment-bearing URIs as invalid → loses the user
  // on the auth domain. We use a plain pathname and let App.svelte's
  // bootstrap recognize the post-callback landing and bridge it back into
  // the hash router (see absorbOAuthCallbackInUrl).
  const redirectUri = `${window.location.origin}/auth/callback`;
  const params = new URLSearchParams({
    redirect_uri: redirectUri,
    client_state: state
  });
  window.location.href = `${AUTH_STATION_URL.replace(/\/$/, "")}/login?${params.toString()}`;
}

/**
 * Detects the post-Auth-Station landing — `window.location` looks like
 * `${origin}/auth/callback?token=…&state=…` because that's the redirect_uri
 * we sent — and converts it into a hash-routed `/auth/callback?…` so the
 * existing AuthCallback page (under the hash router) can pick up the query
 * the way it expects. Call from App.svelte onMount, BEFORE initRouter, so
 * the router sees the correct hash on first sync.
 *
 * Returns true if we rewrote the URL (caller should NOT also run the
 * normal "/" root-redirect logic — the AuthCallback page owns navigation
 * from here).
 */
export function absorbOAuthCallbackInUrl(): boolean {
  if (typeof window === "undefined") return false;
  const path = window.location.pathname;
  const search = window.location.search;
  if (path !== "/auth/callback") return false;
  if (!search || (!search.includes("token=") && !search.includes("error"))) {
    return false;
  }
  // Move the query under the hash route and reset path/search so a refresh
  // doesn't loop us through the callback again.
  const newHash = `#/auth/callback${search.startsWith("?") ? search : `?${search}`}`;
  window.history.replaceState(null, "", `/${newHash}`);
  return true;
}

/** Only allow returnTo values that look like local app paths. */
export function safeReturnTo(raw: string | null | undefined): string {
  if (!raw) return "/";
  if (!raw.startsWith("/")) return "/";
  if (raw.startsWith("//")) return "/"; // protocol-relative URL — reject
  return raw;
}

export type CallbackResult =
  | { ok: true; returnTo: string }
  | { ok: false; reason: string };

/**
 * Parse the Auth Station callback query, validate the `state` round-trip,
 * persist the token, and flip `authState` to signedIn.
 *
 * Pass the raw query string WITHOUT the leading `?`. With the hash router,
 * the query lives after the route inside `location.hash`, not in
 * `location.search` — the caller is responsible for extracting it.
 */
export function handleAuthCallback(searchString: string): CallbackResult {
  const session = safeSessionStorage();
  const expectedState = session?.getItem(STATE_KEY) ?? null;
  const returnTo = session?.getItem(RETURN_TO_KEY) ?? null;
  // One-shot: clear both regardless of outcome so a replay can't re-use them.
  if (session) {
    session.removeItem(STATE_KEY);
    session.removeItem(RETURN_TO_KEY);
  }

  const params = new URLSearchParams(searchString);
  const error = params.get("error");
  if (error) {
    const description = params.get("error_description");
    return { ok: false, reason: description || error };
  }

  const token = params.get("token");
  const refreshToken = params.get("refresh_token");
  const state = params.get("state");

  if (!token) {
    return { ok: false, reason: "missing_token" };
  }
  if (!expectedState || state !== expectedState) {
    return { ok: false, reason: "state_mismatch" };
  }
  const user = decodeJwtPayload(token);
  if (!user) {
    return { ok: false, reason: "token_invalid" };
  }

  const store = safeLocalStorage();
  if (store) {
    store.setItem(TOKEN_KEY, token);
    if (refreshToken) store.setItem(REFRESH_TOKEN_KEY, refreshToken);
  }
  authState.status = "signedIn";
  authState.user = user;

  return { ok: true, returnTo: safeReturnTo(returnTo) };
}

/**
 * Clear local auth state and bounce to Auth Station's logout endpoint.
 *
 * Reads `window.location.origin` so the post-logout redirect points at
 * whichever origin the webview is currently hosted at — works for dev
 * (`http://127.0.0.1:1420`) and any future packaged origin without code
 * changes. NOTE: packaged Tauri webviews historically use
 * `tauri://localhost` (macOS) / `https://tauri.localhost` (Windows),
 * which Auth Station will need to whitelist separately. See the README
 * TODO for that follow-up.
 */
export function signOut(): void {
  const store = safeLocalStorage();
  if (store) {
    store.removeItem(TOKEN_KEY);
    store.removeItem(REFRESH_TOKEN_KEY);
  }
  if (store) store.removeItem(API_KEY_KEY);
  // Best-effort: tell the puffer host to forget the key too, so a stale
  // PUFFER_API_KEY doesn't leak across signed-out sessions.
  void (async () => {
    try {
      const { request } = await import("./wsClient");
      await request("logout_provider", { providerId: "puffer" });
    } catch {
      /* host may be unavailable — local logout still proceeds */
    }
  })();
  authState.status = "signedOut";
  authState.user = null;

  if (typeof window === "undefined") return;
  if (!AUTH_STATION_URL) {
    // Without a configured Auth Station, just bounce to /login locally.
    window.location.hash = "#/login";
    return;
  }
  const postLogoutRedirect = `${window.location.origin}/#/login`;
  const params = new URLSearchParams({
    post_logout_redirect_uri: postLogoutRedirect
  });
  window.location.href = `${AUTH_STATION_URL.replace(/\/$/, "")}/logout?${params.toString()}`;
}

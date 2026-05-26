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

const AUTH_STATION_URL = import.meta.env.VITE_AUTH_STATION_URL as string | undefined;

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
 * Read token from localStorage and populate `authState`. Must be called
 * once on app mount, before the gate effect runs.
 */
export function loadAuthFromStorage(): void {
  const store = safeLocalStorage();
  if (!store) {
    // No storage means no possibility of a stored session — treat as
    // signed out so the gate routes the user to /login.
    authState.status = "signedOut";
    authState.user = null;
    return;
  }
  const token = store.getItem(TOKEN_KEY);
  const user = token ? decodeJwtPayload(token) : null;
  if (!user) {
    // Clean up any stale token so the next launch doesn't keep retrying it.
    if (token) {
      store.removeItem(TOKEN_KEY);
      store.removeItem(REFRESH_TOKEN_KEY);
    }
    authState.status = "signedOut";
    authState.user = null;
    return;
  }
  authState.status = "signedIn";
  authState.user = user;
}

/** Return the currently-stored access token (or null). */
export function getAuthToken(): string | null {
  const store = safeLocalStorage();
  if (!store) return null;
  return store.getItem(TOKEN_KEY);
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
  const redirectUri = `${window.location.origin}/#/auth/callback`;
  const params = new URLSearchParams({
    redirect_uri: redirectUri,
    client_state: state
  });
  window.location.href = `${AUTH_STATION_URL.replace(/\/$/, "")}/login?${params.toString()}`;
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

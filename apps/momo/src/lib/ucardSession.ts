/**
 * U-card backend session token.
 *
 * The ucard-backend (`/api/card/*`) does NOT accept the Auth Station access
 * JWT directly — it wants its own short-lived `sessionToken`, minted by
 * POSTing the JWT to `/api/auth/exchange`. This module owns that exchange:
 * it mints a sessionToken from the current JWT and caches it in-memory,
 * re-minting lazily when the cache is missing, expiring, or was minted from
 * a different JWT (token refresh / re-login / user switch).
 *
 * Ported from `worldclaw-app/lib/auth-session.ts`. The donor parked the
 * sessionToken in a zustand store seeded at login finalize; here we keep it
 * in-module and mint lazily off `getAuthToken()` (the JWT in localStorage,
 * key `puffer.authToken`). Binding the cache to the access token it was
 * minted from means we never have to hook sign-out: a new/absent JWT simply
 * misses the cache and re-exchanges (or fails closed).
 *
 * The exchange itself rides `backendFetch(..., { skipAuth: true })` so it
 * reuses the envelope decoding + the dev `/api` proxy base URL, and skipAuth
 * keeps it from recursing back into this module. The resulting import cycle
 * (backendFetch ⇄ ucardSession) is call-time only — neither side touches the
 * other at module-init — so it resolves cleanly under ESM.
 */

import { backendFetch, BackendError } from "./backendFetch";
import { getAuthToken } from "./auth.svelte";

interface ExchangeData {
  sessionToken: string;
  expiresIn: number; // seconds
}

interface CachedSession {
  sessionToken: string;
  expiresAt: number; // epoch ms
  /** The JWT this sessionToken was minted from — cache key for invalidation. */
  accessToken: string;
}

let cached: CachedSession | null = null;
/** Dedupes concurrent exchanges so a burst of card calls mints once. */
let inflight: Promise<string> | null = null;

/** True when the cached sessionToken has < `leewaySec` seconds of life left. */
function isExpiringSoon(expiresAt: number, leewaySec = 60): boolean {
  return expiresAt - Date.now() < leewaySec * 1000;
}

/** Mint a fresh sessionToken from `accessToken` and cache it. */
async function exchange(accessToken: string): Promise<string> {
  const data = await backendFetch<ExchangeData>("/auth/exchange", {
    method: "POST",
    body: { accessToken },
    skipAuth: true
  });
  if (!data?.sessionToken) {
    throw new BackendError(
      1005,
      "exchange returned no sessionToken",
      "session token required"
    );
  }
  cached = {
    sessionToken: data.sessionToken,
    expiresAt: Date.now() + (data.expiresIn ?? 0) * 1000,
    accessToken
  };
  return data.sessionToken;
}

/**
 * Return a valid ucard sessionToken, minting one if the cache is empty,
 * expiring, or was minted from a different JWT. Throws `BackendError(1005)`
 * when the user isn't signed in (no JWT to exchange).
 */
export async function ensureUcardSessionToken(): Promise<string> {
  const accessToken = getAuthToken();
  if (!accessToken) {
    throw new BackendError(1005, "Not signed in", "session token required");
  }
  if (
    cached &&
    cached.accessToken === accessToken &&
    !isExpiringSoon(cached.expiresAt)
  ) {
    return cached.sessionToken;
  }
  return refreshUcardSession();
}

/**
 * Force a fresh exchange from the current JWT, ignoring the cache.
 * Concurrent callers share a single in-flight request.
 */
export function refreshUcardSession(): Promise<string> {
  if (inflight) return inflight;
  const accessToken = getAuthToken();
  if (!accessToken) {
    return Promise.reject(
      new BackendError(1005, "Not signed in", "session token required")
    );
  }
  inflight = (async () => {
    try {
      return await exchange(accessToken);
    } finally {
      inflight = null;
    }
  })();
  return inflight;
}

/** Drop the cached sessionToken (e.g. after the backend rejects it). */
export function clearUcardSession(): void {
  cached = null;
}

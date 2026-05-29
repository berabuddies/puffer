/**
 * Thin fetch wrapper for the ucard-backend REST API.
 *
 * Ported from worldclaw-app/lib/backend.ts. Adapted for the desktop
 * environment:
 *   - Auth: the ucard-backend wants its own `sessionToken`, not the Auth
 *     Station JWT. `ucardSession.ts` mints + caches one (POST
 *     /api/auth/exchange) off the JWT; we attach it as the Bearer here.
 *     On any auth-failure code (1005/1007/1008/1009) we re-exchange once
 *     from the JWT and retry — recovering an expired/rotated sessionToken —
 *     before surfacing a terminal BackendError for the UI to act on.
 *   - Base URL: in a production build we hit
 *     `import.meta.env.VITE_BACKEND_BASE_URL` directly (falling back to
 *     `http://127.0.0.1:8080`, the same default the donor uses via
 *     `EXPO_PUBLIC_BACKEND_BASE_URL`). The packaged app runs at the
 *     `tauri://localhost` origin, which the backend's CORS allowlist
 *     accepts. In dev, though, the Tauri webview loads from
 *     `http://localhost:1466` — an origin the backend does NOT allowlist —
 *     so a direct cross-origin fetch is blocked by the WKWebView and surfaces
 *     as "Network error: Load failed". To dodge that we issue *same-origin*
 *     requests in dev (relative `/api/…`) and let the Vite dev server proxy
 *     them to the real backend server-side, where CORS never applies. The
 *     proxy target is configured from the same `VITE_BACKEND_BASE_URL` in
 *     `vite.config.ts`.
 *
 * Response envelope shape (matches ucard-backend):
 *   { code: number, message: string, field: string, data: T }
 * Non-zero `code` → BackendError. Network / JSON failures → plain Error.
 */

import {
  ensureUcardSessionToken,
  refreshUcardSession,
  clearUcardSession
} from './ucardSession';

// Dev → '' (same-origin /api, proxied by Vite to dodge the backend's CORS
// allowlist, which only admits the packaged tauri://localhost origin).
// Prod → absolute backend URL, hit directly.
const BASE_URL = import.meta.env.DEV
  ? ''
  : (import.meta.env as Record<string, string | undefined>)
      .VITE_BACKEND_BASE_URL ?? 'http://127.0.0.1:8080';
const API_PREFIX = '/api';

// Auth-failure codes. 1005 unauthorized, 1007 token expired, 1008 token
// invalid/malformed, 1009 session revoked. All four mean the attached ucard
// sessionToken is no good — we re-exchange from the JWT once and retry; if
// that still fails the error is terminal and the UI prompts re-login.
const AUTH_FAILURE_CODES = new Set([1005, 1007, 1008, 1009]);

export interface BackendEnvelope<T> {
  code: number;
  message: string;
  field: string;
  data: T;
}

export class BackendError extends Error {
  readonly code: number;
  readonly field: string;

  constructor(code: number, message: string, field: string) {
    super(message || `Backend error (code=${code})`);
    this.name = 'BackendError';
    this.code = code;
    this.field = field;
  }
}

/**
 * Best-effort user-facing copy for a backend error. Falls through to
 * `fallback` (or the raw message) when we don't have a specific recipe,
 * so unfamiliar errors stay visible instead of silently swallowed.
 */
export function describeBackendError(err: unknown, fallback?: string): string {
  if (err instanceof BackendError) {
    switch (err.code) {
      case 1000:
        if (!err.field) return 'Please check your input and try again.';
        if (/\s/.test(err.field)) return err.field;
        return `Please check the "${err.field}" field and try again.`;
      case 1001:
        return 'This item no longer exists. Please refresh and try again.';
      case 1002:
        return 'Your session has expired. Please log in again.';
      case 1003:
        return err.message || 'This action is not allowed in the current state.';
      case 1005:
      case 1007:
      case 1008:
      case 1009:
        return 'Your session has expired. Please log in again.';
      case 2000:
      case 2001:
        if (err.field && /\s/.test(err.field)) return err.field;
        return 'Server is having trouble. Please try again in a moment.';
      case 2003:
        return 'A third-party service is unavailable. Please try again shortly.';
      default:
        return err.message || fallback || 'Request failed.';
    }
  }
  if (err instanceof Error) {
    if (err.message.startsWith('Network error:')) {
      return 'Network unreachable. Please check your connection.';
    }
    return err.message || fallback || 'Something went wrong.';
  }
  return fallback || 'Something went wrong.';
}

export interface BackendFetchOptions extends Omit<RequestInit, 'body'> {
  body?: unknown;
  query?: Record<string, string | number | undefined>;
  /**
   * Skip auth header injection. Used by callers that intentionally
   * fire unauthenticated (e.g. health checks, future login bootstrap).
   */
  skipAuth?: boolean;
}

function buildUrl(path: string, query?: BackendFetchOptions['query']): string {
  const normalized = path.startsWith('/') ? path : `/${path}`;
  const url = `${BASE_URL}${API_PREFIX}${normalized}`;
  if (!query) return url;

  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(query)) {
    if (value === undefined) continue;
    params.append(key, String(value));
  }
  const qs = params.toString();
  return qs ? `${url}?${qs}` : url;
}

/**
 * Perform a single REST request, decode the envelope, and return `data`
 * on success or throw on failure. Callers should use the higher-level
 * `backendFetch` wrapper which also handles auth-token injection and
 * post-error bookkeeping.
 */
async function send<T>(
  path: string,
  opts: {
    body?: unknown;
    headers?: HeadersInit;
    query?: BackendFetchOptions['query'];
    rest: Omit<RequestInit, 'body' | 'headers'>;
    token: string | null;
  },
): Promise<T> {
  const finalHeaders: Record<string, string> = {
    'Content-Type': 'application/json',
    ...(opts.headers as Record<string, string> | undefined),
  };
  if (opts.token) finalHeaders.Authorization = `Bearer ${opts.token}`;

  const init: RequestInit = {
    ...opts.rest,
    headers: finalHeaders,
  };
  if (opts.body !== undefined) {
    init.body =
      typeof opts.body === 'string' ? opts.body : JSON.stringify(opts.body);
  }

  const method = (opts.rest as RequestInit).method ?? 'GET';

  let res: Response;
  try {
    res = await fetch(buildUrl(path, opts.query), init);
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err);
    console.warn(`[backend] ${path} network error: ${msg}`);
    throw new Error(`Network error: ${msg}`);
  }

  let json: unknown;
  try {
    json = await res.json();
  } catch {
    console.warn(`[backend] ${path} invalid JSON (HTTP ${res.status})`);
    throw new Error(`Invalid JSON response (HTTP ${res.status}) for ${path}`);
  }

  if (!json || typeof json !== 'object' || !('code' in json)) {
    console.warn(
      `[backend] ${path} unexpected envelope (HTTP ${res.status})`,
      json,
    );
    throw new Error(
      `Unexpected response shape (HTTP ${res.status}) for ${path}`,
    );
  }

  const envelope = json as BackendEnvelope<T>;
  if (envelope.code !== 0) {
    console.warn(
      `[backend] ${method} ${path} HTTP ${res.status} code=${envelope.code} field="${envelope.field ?? ''}" message="${envelope.message ?? ''}"`,
    );
    throw new BackendError(
      envelope.code,
      envelope.message || '',
      envelope.field || '',
    );
  }

  return envelope.data;
}

/**
 * Fetch from the ucard-backend at `${BASE_URL}/api${path}`. Unless
 * `skipAuth: true`, attaches a ucard `sessionToken` (minted from the Auth
 * Station JWT via `ucardSession`). Decodes the envelope and returns `data`
 * on success. Throws `BackendError` for any non-zero envelope code; throws
 * plain `Error` for network / JSON failures.
 *
 * On an auth-failure code (1005/1007/1008/1009) the cached sessionToken is
 * stale or rejected, so we drop it, re-exchange once from the JWT, and retry
 * the request. If the retry still fails the original error propagates and the
 * UI layer can route the user back to login.
 */
export async function backendFetch<T>(
  path: string,
  options: BackendFetchOptions = {},
): Promise<T> {
  const { body, query, headers, skipAuth, ...rest } = options;

  if (skipAuth) {
    return send<T>(path, { body, headers, query, rest, token: null });
  }

  // Mint/reuse the ucard sessionToken (proactively refreshed when expiring).
  const token = await ensureUcardSessionToken();

  try {
    return await send<T>(path, { body, headers, query, rest, token });
  } catch (err) {
    if (!(err instanceof BackendError) || !AUTH_FAILURE_CODES.has(err.code)) {
      throw err;
    }
    // The sessionToken was rejected — re-exchange from the JWT and retry once.
    console.warn(
      `[backend] auth code=${err.code} on ${path} → re-exchange + retry`,
    );
    clearUcardSession();
    let fresh: string;
    try {
      fresh = await refreshUcardSession();
    } catch {
      clearUcardSession();
      throw err; // surface the original auth failure (e.g. JWT also dead)
    }
    return send<T>(path, { body, headers, query, rest, token: fresh });
  }
}

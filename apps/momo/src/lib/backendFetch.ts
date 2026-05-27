/**
 * Thin fetch wrapper for the ucard-backend REST API.
 *
 * Ported from worldclaw-app/lib/backend.ts. Adapted for the desktop
 * environment:
 *   - Auth token is read from localStorage key `puffer.authToken`
 *     (the convention used by src/lib/auth.svelte.ts).
 *   - The RN-specific refresh-on-1007 flow from the donor is dropped:
 *     it depended on `useAuthStore` / `refreshBackendSession` which
 *     don't exist in the desktop app. On any of the auth-failure codes
 *     (1005, 1007, 1008, 1009) we surface a terminal BackendError and
 *     let the caller decide how to react (e.g. bounce to the login
 *     page). A future iteration can layer refresh on top once the
 *     desktop has a refresh-token endpoint wired up.
 *   - Base URL is read from `import.meta.env.VITE_BACKEND_BASE_URL`
 *     and falls back to `http://127.0.0.1:8080` — the same default the
 *     donor uses via `EXPO_PUBLIC_BACKEND_BASE_URL`.
 *
 * Response envelope shape (matches ucard-backend):
 *   { code: number, message: string, field: string, data: T }
 * Non-zero `code` → BackendError. Network / JSON failures → plain Error.
 */

const BASE_URL =
  (import.meta.env as Record<string, string | undefined>)
    .VITE_BACKEND_BASE_URL ?? 'http://127.0.0.1:8080';
const API_PREFIX = '/api';
const TOKEN_KEY = 'puffer.authToken';

// Backend codes that mean "this sessionToken is dead — go log in again".
// 1005 unauthorized, 1007 token expired, 1008 token invalid/malformed,
// 1009 session revoked. The donor split 1007 out as refreshable, but
// without a refresh hook here we treat the whole family as terminal.
const TERMINAL_AUTH_CODES = new Set([1005, 1007, 1008, 1009]);

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

function readAuthToken(): string | null {
  if (typeof window === 'undefined') return null;
  try {
    return window.localStorage.getItem(TOKEN_KEY);
  } catch {
    return null;
  }
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
 * Fetch from the ucard-backend at `${BASE_URL}/api${path}`. Injects the
 * cached auth token unless `skipAuth: true`. Decodes the envelope and
 * returns the `data` payload on success. Throws `BackendError` for any
 * non-zero envelope code; throws plain `Error` for network / JSON
 * failures.
 *
 * On a terminal auth-failure code (1005/1007/1008/1009) the stored
 * token is cleared so subsequent requests fail fast and the UI layer
 * can route the user back to login.
 */
export async function backendFetch<T>(
  path: string,
  options: BackendFetchOptions = {},
): Promise<T> {
  const { body, query, headers, skipAuth, ...rest } = options;
  const token = skipAuth ? null : readAuthToken();

  try {
    return await send<T>(path, { body, headers, query, rest, token });
  } catch (err) {
    if (err instanceof BackendError && TERMINAL_AUTH_CODES.has(err.code)) {
      console.warn(
        `[backend] terminal auth code=${err.code} on ${path} → clearing token`,
      );
      try {
        if (typeof window !== 'undefined') {
          window.localStorage.removeItem(TOKEN_KEY);
        }
      } catch {
        /* localStorage might be unavailable — already failing, nothing to do */
      }
    }
    throw err;
  }
}

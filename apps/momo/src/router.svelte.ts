/**
 * Puffer Desktop UI v2 — minimal hash router (Svelte 5 runes, zero deps).
 *
 * - currentRoute is a $state object that mirrors the current hash route.
 * - navigate(path) pushes the hash and updates currentRoute.
 * - matchRoute(definitions, path) is the pure matcher used by both navigate
 *   and the App to resolve a path to a route + params.
 *
 * Patterns use ':name' segments, e.g. '/agent/:taskId'. Trailing slashes are
 * stripped before matching. The matcher returns null when no pattern matches.
 */

export type RouteParams = Record<string, string>;

export interface RouteMatchInput {
  pattern: string;
}

export interface RouteMatch<T extends RouteMatchInput> {
  route: T;
  params: RouteParams;
}

export interface CurrentRoute {
  path: string;
  /** Raw query string after the route's `?`, without the leading `?`. Empty when none. */
  query: string;
  params: RouteParams;
}

const DEFAULT_PATH = "/home";

/**
 * Reactive store for the active route. Components read it directly:
 *
 *   import { currentRoute } from './router';
 *   $: console.log(currentRoute.path);
 */
export const currentRoute = $state<CurrentRoute>({
  path: DEFAULT_PATH,
  query: "",
  params: {}
});

/**
 * Parse the hash portion of the URL into a route path + raw query string.
 * Auth Station's OIDC redirect lands at `#/auth/callback?token=…&state=…`,
 * so we MUST split the query off the path — otherwise matchRoute can't
 * find `/auth/callback` and the gate effect would also miss `/login`.
 */
function readHash(): { path: string; query: string } {
  if (typeof window === "undefined") return { path: DEFAULT_PATH, query: "" };
  const raw = window.location.hash.replace(/^#/, "");
  if (!raw) return { path: DEFAULT_PATH, query: "" };
  // Split path from query first; `?` inside the hash separates them.
  const qIndex = raw.indexOf("?");
  const rawPath = qIndex >= 0 ? raw.slice(0, qIndex) : raw;
  const query = qIndex >= 0 ? raw.slice(qIndex + 1) : "";
  // Normalize path: ensure leading slash, strip trailing slash (except root).
  let path = rawPath.startsWith("/") ? rawPath : `/${rawPath}`;
  if (path.length > 1 && path.endsWith("/")) path = path.slice(0, -1);
  return { path, query };
}

/** Update currentRoute from window.location.hash. */
function syncFromHash(): void {
  const { path, query } = readHash();
  // Params are computed at App level against the route table; here we just
  // expose the raw path. App.svelte calls matchRoute() to derive params.
  currentRoute.path = path;
  currentRoute.query = query;
  currentRoute.params = {};
}

/** Programmatic navigation. Updates the hash; the hashchange listener will sync. */
export function navigate(path: string): void {
  if (typeof window === "undefined") return;
  const normalized = path.startsWith("/") ? path : `/${path}`;
  const target = `#${normalized}`;
  if (window.location.hash === target) {
    // Same hash → hashchange won't fire. Force a manual sync.
    syncFromHash();
    return;
  }
  window.location.hash = target;
}

/**
 * Match a path against a list of route definitions. Returns the first match
 * (most-specific-first ordering is the caller's responsibility) or null.
 */
export function matchRoute<T extends RouteMatchInput>(
  definitions: readonly T[],
  path: string
): RouteMatch<T> | null {
  const pathSegments = stripSegments(path);
  for (const def of definitions) {
    const patternSegments = stripSegments(def.pattern);
    if (patternSegments.length !== pathSegments.length) continue;
    const params: RouteParams = {};
    let ok = true;
    for (let i = 0; i < patternSegments.length; i++) {
      const ps = patternSegments[i];
      const xs = pathSegments[i];
      if (ps.startsWith(":")) {
        params[ps.slice(1)] = decodeURIComponent(xs);
      } else if (ps !== xs) {
        ok = false;
        break;
      }
    }
    if (ok) return { route: def, params };
  }
  return null;
}

function stripSegments(path: string): string[] {
  return path.split("/").filter((s) => s.length > 0);
}

/** Wire the global hashchange listener. Safe to call multiple times — idempotent. */
let wired = false;
export function initRouter(): void {
  if (wired || typeof window === "undefined") return;
  wired = true;
  window.addEventListener("hashchange", syncFromHash);
  // If nothing in the hash, set the default. Otherwise, sync.
  if (!window.location.hash) {
    window.location.hash = `#${DEFAULT_PATH}`;
  } else {
    syncFromHash();
  }
}

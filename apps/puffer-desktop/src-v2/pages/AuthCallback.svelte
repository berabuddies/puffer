<!--
  AuthCallback — handles the redirect back from Auth Station.

  Auth Station sends users to `<origin>/#/auth/callback?token=…&state=…`.
  Because we use a hash router, the query string lives inside
  `location.hash`, NOT `location.search`. Example:

    location.hash   = "#/auth/callback?token=…&state=…&refresh_token=…"
    location.search = ""   ← empty in hash-routed apps

  We extract the query off the hash, hand it to `handleAuthCallback`,
  then navigate either to the originally-intended path (if a `returnTo`
  was stashed during goToLogin) or to the root redirect target
  (`/home` for returning users, `/onboarding/where` for first-timers).

  On failure we route back to `/login?error=<reason>` so the login page
  can surface a friendly chip.
-->
<script lang="ts">
  import { onMount } from "svelte";

  import {
    getAuthToken,
    getWorldRouterApiKey,
    handleAuthCallback,
    mintWorldRouterApiKey,
    safeReturnTo
  } from "../lib/auth.svelte";
  import { navigate } from "../router.svelte";
  import { getRootRedirect } from "../routes";

  /**
   * Pull the query string out of `location.hash`. We prefer the hash form
   * because that's where Auth Station's redirect lands after the
   * single-page-app reload (the browser keeps the fragment intact). We
   * fall back to `location.search` so this still works under direct-load
   * test scenarios where a tool might rewrite the URL.
   */
  function extractQuery(): string {
    if (typeof window === "undefined") return "";
    const hash = window.location.hash || "";
    const qIndex = hash.indexOf("?");
    if (qIndex >= 0) return hash.slice(qIndex + 1);
    const search = window.location.search || "";
    return search.startsWith("?") ? search.slice(1) : search;
  }

  onMount(() => {
    const query = extractQuery();
    const result = handleAuthCallback(query);
    if (result.ok) {
      const target = safeReturnTo(result.returnTo);
      // Fire-and-forget the API-key mint AFTER navigation kicks off, so
      // the user lands on /home (or onboarding) immediately and the
      // ~15s control-api round-trip doesn't block the UI. The key only
      // matters for chat, which doesn't run until the user composes
      // something; by then the mint will have settled. We skip if a
      // key is already cached — donor flagged duplicate-key minting on
      // every login as a leak to avoid.
      if (!getWorldRouterApiKey()) {
        const jwt = getAuthToken();
        if (jwt) {
          mintWorldRouterApiKey(jwt).catch((err) => {
            // eslint-disable-next-line no-console
            console.warn(
              "[auth] worldrouter API key mint failed; chat will be unavailable until retried",
              err
            );
          });
        }
      }
      // safeReturnTo returns "/" when nothing was stashed; defer to the
      // root-redirect helper so signed-in users land on /home (or
      // onboarding if they haven't finished it).
      navigate(target === "/" ? getRootRedirect() : target);
    } else {
      navigate(`/login?error=${encodeURIComponent(result.reason)}`);
    }
  });
</script>

<main class="callback">
  <p class="callback__text">Signing you in…</p>
</main>

<style>
  .callback {
    min-height: 100vh;
    width: 100%;
    background: var(--color-surface-app);
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .callback__text {
    font-family: var(--font-system);
    font-size: var(--font-size-body);
    line-height: var(--line-height-body);
    color: var(--color-text-muted);
  }
</style>

<!--
  App — root component for v2.

  Reads the reactive `currentRoute` from the router, matches it against
  the route table, and either:
    1. Renders the matched component inside <Shell> (when hasShell === true),
    2. Renders it bare (when hasShell === false → onboarding flows, login),
    3. Falls back to <NotFound /> (still wrapped in Shell so the chrome stays
       consistent — matches the rest of the app).

  Auth gate:
    On mount we call `loadAuthFromStorage()` to flip `authState.status` from
    its initial "unknown" to a definitive signedIn/signedOut. Until that
    happens we render a minimal "Loading…" splash instead of the matched
    route — otherwise a signed-out user would briefly flash onboarding
    or home chrome before being kicked to /login.

    Once status is known, a $effect enforces the gate:
      - signedOut on any route that isn't /login or /auth/callback
        → navigate("/login")
      - signedIn on /login → navigate(getRootRedirect()) (i.e. land on
        /home or onboarding depending on the onboarded flag)

  Root redirect:
    '/' (or empty hash) is resolved against `getRootRedirect()`, which now
    consults auth state first and onboarding flag second. We only kick
    off the root-redirect resolution once auth is known so the decision
    is made with full information.

  The Toast renderer is mounted once at the root so any module can call
  `pushToast()` and the message will surface no matter which page is active.
-->
<script lang="ts">
  import { onMount } from "svelte";

  import { currentRoute, initRouter, matchRoute, navigate } from "./router.svelte";
  import { routes, getRootRedirect } from "./routes";
  import {
    absorbOAuthCallbackInUrl,
    authState,
    installDeepLinkListener,
    loadAuthFromStorage,
    markOnboarded
  } from "./lib/auth.svelte";
  import Shell from "./components/shell/Shell.svelte";
  import NotFound from "./pages/NotFound.svelte";
  import Toast from "./components/common/Toast.svelte";

  onMount(() => {
    // If Auth Station just redirected us back to /auth/callback (no fragment
    // — RFC 6749 forbids fragments in redirect_uri), rewrite the URL into
    // the hash form the rest of the app speaks BEFORE initRouter so the
    // first sync sees the correct hash route.
    const absorbed = absorbOAuthCallbackInUrl();
    initRouter();
    // Populate authState from localStorage BEFORE the root-redirect runs.
    // loadAuthFromStorage may need a network round-trip (refresh JWT
    // exchange), so it's async — we keep the gate showing the "Loading…"
    // splash via authState.status === "unknown" until it resolves.
    void loadAuthFromStorage().then(() => {
      // Root redirect — handle '/' or '' explicitly. Skip when we just
      // absorbed an OAuth callback: the AuthCallback page owns navigation
      // for that case.
      if (!absorbed && (currentRoute.path === "/" || currentRoute.path === "")) {
        navigate(getRootRedirect());
      }
    });

    // Tauri-only: register the OS deep-link listener. When the OS
    // routes `com.corbina.desktop://oauth/callback?…` back to us, we
    // navigate to /auth/callback?… in the hash router and let
    // AuthCallback.svelte run the same code path the web flow uses.
    let unlistenDeepLink: (() => void) | null = null;
    void installDeepLinkListener((url) => {
      const idx = url.indexOf("?");
      const query = idx >= 0 ? url.slice(idx + 1) : "";
      navigate(`/auth/callback?${query}`);
    }).then((unlisten) => {
      unlistenDeepLink = unlisten;
    });
    return () => {
      unlistenDeepLink?.();
    };
  });

  // Resolve current match each time the route changes.
  let match = $derived(matchRoute(routes, currentRoute.path));
  let Component = $derived(match?.route.component);
  let params = $derived(match?.params ?? {});
  let hasShell = $derived(match?.route.hasShell ?? true);
  /** Fullbleed pages (e.g. Agent) lock the column to viewport height and
   *  draw their own header/composer rows — see Shell.svelte. */
  let fullbleed = $derived(match?.route.fullbleed ?? false);

  // Whenever the user lands on /home (including the post-onboarding hop from
  // Done.svelte), persist the onboarded flag so next launch skips the flow.
  // This effect re-runs on every route change because it reads currentRoute.
  $effect(() => {
    if (currentRoute.path === "/home" || currentRoute.path.startsWith("/home/")) {
      markOnboarded();
    }
  });

  /**
   * Auth gate. Runs after every auth/route change.
   *
   * Public routes that don't require sign-in:
   *   /login            — the sign-in screen itself
   *   /auth/callback    — the redirect target from Auth Station
   *
   * Everything else demands a signedIn status.
   */
  const PUBLIC_PATHS = new Set(["/login", "/auth/callback"]);

  $effect(() => {
    // Don't gate while we're still figuring out auth state — the splash
    // below covers this window.
    if (authState.status === "unknown") return;

    const path = currentRoute.path;
    if (authState.status === "signedOut" && !PUBLIC_PATHS.has(path)) {
      navigate("/login");
      return;
    }
    if (authState.status === "signedIn" && path === "/login") {
      navigate(getRootRedirect());
    }
  });
</script>

{#if authState.status === "unknown"}
  <!-- Auth check is still running. Plain splash keeps us from flashing
       chrome that may not be appropriate once we know the answer. -->
  <main class="auth-splash">
    <p class="auth-splash__text">Loading…</p>
  </main>
{:else if !Component}
  <Shell>
    <NotFound />
  </Shell>
{:else if hasShell}
  <Shell {fullbleed}>
    <!-- Re-mount the page when path changes so $derived re-evaluates inside it. -->
    {#key currentRoute.path}
      <Component {...params} />
    {/key}
  </Shell>
{:else}
  {#key currentRoute.path}
    <Component {...params} />
  {/key}
{/if}

<!-- Global toast layer — must live outside the Shell so it overlays both
     shelled pages and the bare onboarding flow. -->
<Toast />

<style>
  .auth-splash {
    min-height: 100vh;
    width: 100%;
    background: var(--color-surface-app);
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .auth-splash__text {
    font-family: var(--font-system);
    font-size: var(--font-size-body);
    line-height: var(--line-height-body);
    color: var(--color-text-muted);
  }
</style>

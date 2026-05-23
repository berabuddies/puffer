<!--
  App — root component for v2.

  Reads the reactive `currentRoute` from the router, matches it against
  the route table, and either:
    1. Renders the matched component inside <Shell> (when hasShell === true),
    2. Renders it bare (when hasShell === false → onboarding flows),
    3. Falls back to <NotFound /> (still wrapped in Shell so the chrome stays
       consistent — matches the rest of the app).

  Root redirect:
    '/' (or empty hash) is resolved against `getRootRedirect()`, which
    consults the `puffer.onboarded` localStorage flag. First-run users land
    in `/onboarding/where`; returning users land in `/home`. Once a user
    reaches `/home` we set the flag so subsequent launches skip onboarding —
    `Done.svelte` already routes to `/home` after its 3-second hold, so this
    keeps the wiring entirely in App.svelte instead of touching the
    onboarding pages (which another agent owns).

  The Toast renderer is mounted once at the root so any module can call
  `pushToast()` and the message will surface no matter which page is active.
-->
<script lang="ts">
  import { onMount } from "svelte";

  import { currentRoute, initRouter, matchRoute, navigate } from "./router.svelte";
  import { routes, getRootRedirect } from "./routes";
  import { markOnboarded } from "./lib/auth.svelte";
  import Shell from "./components/shell/Shell.svelte";
  import NotFound from "./pages/NotFound.svelte";
  import Toast from "./components/common/Toast.svelte";

  onMount(() => {
    initRouter();
    // Root redirect — handle '/' or '' explicitly. Picks onboarding vs home
    // based on the persisted onboarded flag.
    if (currentRoute.path === "/" || currentRoute.path === "") {
      navigate(getRootRedirect());
    }
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
</script>

{#if !Component}
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

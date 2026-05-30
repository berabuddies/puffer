<!--
  Onboarding · Done (artboard 2E1-0)

  Final onboarding screen — mascot + "Thanks!" + "Filtering the chaos…".
  Holds for ~3 seconds, marks the user as onboarded, then routes to /home.
  Static layout matches the artboard (no decorative dots).
-->
<script lang="ts">
  import { onMount, onDestroy } from "svelte";

  import OnboardingShell from "../../components/onboarding/OnboardingShell.svelte";
  import { navigate } from "../../router.svelte";
  import { markOnboarded } from "../../lib/auth.svelte";
  import { commitProfile } from "../../lib/onboarding.svelte";

  let timer: ReturnType<typeof setTimeout> | null = null;

  onMount(() => {
    // Mark as onboarded immediately so a refresh during the 3s hold also
    // resolves to the main app, then navigate after the pause.
    markOnboarded();
    void commitProfile();
    timer = setTimeout(() => navigate("/home"), 3000);
  });

  onDestroy(() => {
    if (timer) clearTimeout(timer);
  });
</script>

<OnboardingShell title="Thanks!" subtitle="Filtering the chaos…" />

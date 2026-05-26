<!--
  Login — sign-in gate for v2.

  Minimal centered hero matching the Momo design language:
    - Mascot icon (the existing sun-spike artwork at /v2/mascot.png)
    - Serif heading "Momo"
    - Muted subtitle "Your proactive AI assistant for real life."
    - One black pill button "Sign in / Sign up"

  Click → `goToLogin()` navigates the webview out to Auth Station; the
  browser tab is replaced, so we never get a chance to render success
  state here. Once the click fires we just disable the button and show
  "Opening browser…" so a second click can't double-trigger the navigation.

  Error display: if the hash route contains `?error=<reason>` (set by
  AuthCallback on failure), we map known reasons to friendly copy and
  render a small chip above the button. The reason is read from the
  router's currentRoute so it stays reactive even if the user navigates
  back here without a full reload.
-->
<script lang="ts">
  import Mascot from "../components/common/Mascot.svelte";
  import { goToLogin } from "../lib/auth.svelte";
  import { currentRoute } from "../router.svelte";

  let submitting = $state(false);

  /**
   * Pull `?error=<reason>` off the hash route. The router splits the query
   * off the path and exposes it via `currentRoute.query`, so we just parse
   * that — reactive by construction.
   */
  let errorReason = $derived.by(() => {
    if (!currentRoute.query) return null;
    return new URLSearchParams(currentRoute.query).get("error");
  });

  /** Translate raw error reasons to copy that makes sense to a human. */
  function friendlyError(reason: string): string {
    switch (reason) {
      case "missing_token":
        return "Sign-in didn't complete. Please try again.";
      case "state_mismatch":
        return "Your sign-in session expired. Please try again.";
      case "token_invalid":
        return "We couldn't read the sign-in response. Please try again.";
      case "access_denied":
        return "Sign-in was canceled.";
      default:
        // Auth Station's error_description is human-readable; pass through.
        return reason;
    }
  }

  function onSignIn(): void {
    if (submitting) return;
    submitting = true;
    // goToLogin navigates the whole document; on the off-chance it returns
    // without actually navigating (missing env var), reset the spinner so
    // the user can retry.
    try {
      goToLogin();
    } finally {
      // Small delay before re-enabling — covers the brief moment between
      // the click and the browser starting to navigate.
      window.setTimeout(() => {
        submitting = false;
      }, 1500);
    }
  }
</script>

<main class="login">
  <div class="stack">
    <div class="headline">
      <Mascot size="lg" />
      <h1 class="login__title">Momo</h1>
      <p class="login__subtitle">Your proactive AI assistant for real life.</p>
    </div>

    {#if errorReason}
      <div class="login__error" role="alert">
        {friendlyError(errorReason)}
      </div>
    {/if}

    <button
      class="login__button"
      type="button"
      onclick={onSignIn}
      disabled={submitting}
    >
      {submitting ? "Opening browser…" : "Sign in / Sign up"}
    </button>
  </div>
</main>

<style>
  .login {
    min-height: 100vh;
    width: 100%;
    background: var(--color-surface-app);
    display: flex;
    align-items: center;
    justify-content: center;
    padding: var(--space-7) var(--space-5);
  }

  .stack {
    width: 100%;
    max-width: 480px;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0;
  }

  .headline {
    display: flex;
    flex-direction: column;
    align-items: center;
    /* Mirrors OnboardingShell: 12px gaps between mascot / title / subtitle. */
    gap: 12px;
  }

  /* Serif heading — same scale as the onboarding display title. */
  .login__title {
    font-family: var(--font-serif);
    font-size: 32px;
    line-height: 46px;
    letter-spacing: -0.5px;
    font-weight: var(--font-weight-regular);
    color: var(--color-text-primary);
    text-align: center;
  }

  .login__subtitle {
    font-family: var(--font-system);
    font-size: var(--font-size-body);
    line-height: var(--line-height-body);
    color: var(--color-text-muted);
    text-align: center;
  }

  .login__error {
    margin-top: 24px;
    padding: 8px 14px;
    border-radius: var(--radius-pill);
    background: rgba(220, 38, 38, 0.08);
    color: #9b1c1c;
    font-family: var(--font-system);
    font-size: var(--font-size-body);
    line-height: var(--line-height-body);
    text-align: center;
    max-width: 360px;
  }

  /* Solid-black pill — intentionally distinct from the cream primary used
     elsewhere in the app. Sign-in is a one-shot identity action, not a
     workflow command, so it reads as a discrete affordance instead of
     the warm "next step" cream. */
  .login__button {
    margin-top: 28px;
    height: 40px;
    padding: 0 24px;
    border-radius: var(--radius-pill);
    background: var(--color-text-primary);
    color: #ffffff;
    font-family: var(--font-sans);
    font-size: var(--font-size-button);
    line-height: var(--line-height-button);
    font-weight: var(--font-weight-medium);
    border: 0;
    cursor: pointer;
    transition: background-color 120ms ease, opacity 120ms ease;
  }
  .login__button:hover:not(:disabled) {
    background: #2a2a2a;
  }
  .login__button:disabled {
    opacity: 0.65;
    cursor: progress;
  }
</style>

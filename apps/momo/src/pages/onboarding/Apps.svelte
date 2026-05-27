<!--
  Onboarding · Apps (artboard 2A1-0)

  Mascot + display ("Student life can get busy fast…") + subtitle
  ("Let me handle some tasks for you") + three Connect rows (Gmail, Google
  Calendar, iMessage) + a centered "Browse 8 more integrations" pill +
  the shared skip link + a primary "Continue" CTA.

  Per spec: connecting an app logs to console and swaps the chip in-place
  ("Connect" → "Connected"); the explicit "Continue" CTA advances to
  /onboarding/done and writes the onboarded flag so the user lands in the
  full app on next visit.
-->
<script lang="ts">
  import { ArrowRight } from "lucide-svelte";

  import OnboardingShell from "../../components/onboarding/OnboardingShell.svelte";
  import AppConnectRow from "../../components/onboarding/AppConnectRow.svelte";
  import Button from "../../components/common/Button.svelte";
  import { navigate } from "../../router.svelte";
  import { pushToast } from "../../lib/toast.svelte";
  import { markOnboarded } from "../../lib/auth.svelte";

  // Avatar stack for the "browse more" pill — small initials with the
  // exact brand fills observed in the artboard (Amazon orange, etc).
  const browseAvatars: ReadonlyArray<{ letter: string; bg: string }> = [
    { letter: "A", bg: "#FF9900" },
    { letter: "D", bg: "#EB1700" },
    { letter: "T", bg: "#229ED9" },
    { letter: "N", bg: "#000000" },
    { letter: "L", bg: "#5E6AD2" },
    { letter: "B", bg: "#FF7E8A" }
  ];

  function onBrowse(): void {
    pushToast("Browse more integrations", "info");
  }

  function onContinue(): void {
    markOnboarded();
    navigate("/onboarding/done");
  }
</script>

<OnboardingShell
  title="Student life can get busy fast..."
  subtitle="Let me handle some tasks for you"
  skipTo="/onboarding/done"
>
  <div class="apps">
    <div class="connect-rows">
      <AppConnectRow
        logo="gmail"
        name="Gmail"
        description="Read, draft and reply on your behalf"
      />
      <AppConnectRow
        logo="google-calendar"
        name="Google Calendar"
        description="See your schedule, draft invites"
      />
      <AppConnectRow
        logo="imessage"
        name="iMessage"
        description="Summarize threads and draft replies"
      />
    </div>

    <button class="browse" type="button" onclick={onBrowse}>
      <span class="browse__avatars" aria-hidden="true">
        {#each browseAvatars as a, i (i)}
          <span class="browse__avatar" style:background={a.bg}>{a.letter}</span>
        {/each}
      </span>
      <span class="browse__label">Browse 8 more integrations</span>
      <ArrowRight size={13} strokeWidth={2} aria-hidden="true" />
    </button>

    <div class="continue">
      <Button variant="primary" size="md" label="Continue" onclick={onContinue} />
    </div>
  </div>
</OnboardingShell>

<style>
  .apps {
    width: 520px;
    max-width: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 20px;
  }

  .connect-rows {
    width: 100%;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  /* Browse pill — dashed neutral border, 40 tall, 16 padding, 10 gap.
     Border matches the artboard's #A8A8A8 dashed; placed under our muted
     fold so we don't have to introduce a new token for a single use. */
  .browse {
    margin-top: 0;
    height: 40px;
    display: inline-flex;
    align-items: center;
    gap: 10px;
    padding: 0 16px;
    border-radius: var(--radius-pill);
    border: 1px dashed #a8a8a8;
    background: var(--color-surface-app);
    color: var(--color-text-secondary);
    font-family: var(--font-system);
    font-size: var(--font-size-body);
    line-height: 16px;
    font-weight: var(--font-weight-medium);
    cursor: pointer;
    transition: background-color 120ms ease;
  }

  .browse:hover {
    background: var(--color-surface-rail);
  }

  .browse__avatars {
    display: inline-flex;
    align-items: center;
  }

  .browse__avatar {
    width: 18px;
    height: 18px;
    border-radius: 50%;
    border: 1.5px solid var(--color-surface-app);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    color: #ffffff;
    font-family: var(--font-serif);
    font-size: 9px;
    line-height: 12px;
    font-weight: var(--font-weight-medium);
    flex-shrink: 0;
  }

  .browse__avatar + .browse__avatar {
    margin-left: -6px;
  }

  .browse__label {
    color: var(--color-text-secondary);
  }

  .continue {
    margin-top: 4px;
    display: flex;
    justify-content: center;
  }
</style>

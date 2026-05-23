<!--
  Wallet · KYC empty state (Paper artboard 1LF-0).

  Layout: centered empty state inside the 760 page column.
    Inline line-art illustration (~250×130, coin stack + paper plane).
    Display title "Set Up Wallet to let Agent pay" (.text-display 28).
    Subtitle body "Complete KYC before adding crypto funds…" (muted, ~520 max).
    Two centered actions side-by-side: primary cream "Start KYC" + secondary
    "Learn more". Both 36px (size md).
    Three step cards horizontally beneath (Verify identity / Top up crypto /
    Agent can pay).
    Composer pinned at the bottom of the page column.
-->
<script lang="ts">
  import { onDestroy } from "svelte";

  import Composer from "../components/shell/Composer.svelte";
  import Button from "../components/common/Button.svelte";
  import KycIllustration from "../components/wallet/KycIllustration.svelte";
  import StepCard from "../components/wallet/StepCard.svelte";

  import { navigate } from "../router.svelte";
  import { pushToast } from "../lib/toast.svelte";

  let kycInFlight = $state<boolean>(false);
  let stepTimers: ReturnType<typeof setTimeout>[] = [];

  onDestroy(() => {
    for (const t of stepTimers) clearTimeout(t);
  });

  function handleStartKyc(): void {
    if (kycInFlight) return;
    kycInFlight = true;
    // 3-step mock flow with 600ms delay between toasts, then navigate home.
    stepTimers.push(
      setTimeout(() => pushToast("Step 1: Verifying identity…", "info"), 600)
    );
    stepTimers.push(
      setTimeout(() => pushToast("Step 2: Linking wallet…", "info"), 1200)
    );
    stepTimers.push(
      setTimeout(() => pushToast("Step 3: Funding setup…", "info"), 1800)
    );
    stepTimers.push(
      setTimeout(() => {
        pushToast("KYC complete", "success");
        navigate("/wallet");
      }, 2400)
    );
  }

  function handleLearnMore(): void {
    pushToast("Opening KYC docs…", "info");
  }

  function tapStep(label: string): void {
    pushToast(`Step: ${label}`, "info");
  }
</script>

<section class="kyc">
  <div class="kyc__hero">
    <KycIllustration />

    <div class="kyc__text">
      <h1 class="text-display kyc__title">Set Up Wallet to let Agent pay</h1>
      <p class="kyc__subtitle">
        Complete KYC before adding crypto funds and allowing Agent to place
        orders on your behalf.
      </p>
    </div>

    <div class="kyc__actions">
      <Button
        variant="primary"
        size="md"
        label={kycInFlight ? "Running KYC…" : "Start KYC"}
        disabled={kycInFlight}
        onclick={handleStartKyc}
      />
      <Button variant="secondary" size="md" label="Learn more" onclick={handleLearnMore} />
    </div>
  </div>

  <div class="kyc__steps">
    <button type="button" class="step-btn" onclick={() => tapStep("Verify identity")}>
      <StepCard eyebrow="1 · Verify identity" body="Required before wallet activation." />
    </button>
    <button type="button" class="step-btn" onclick={() => tapStep("Top up crypto")}>
      <StepCard eyebrow="2 · Top up crypto" body="Deposit USDC into your wallet." />
    </button>
    <button type="button" class="step-btn" onclick={() => tapStep("Agent can pay")}>
      <StepCard eyebrow="3 · Agent can pay" body="Approve orders and spending limits." />
    </button>
  </div>
</section>

<Composer placeholder="Ask Alice to pay, order, or top up…" />

<style>
  .kyc {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: var(--space-6);
    padding-bottom: var(--space-5);
  }

  .kyc__hero {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--space-5);
    max-width: 520px;
    width: 100%;
  }

  .kyc__text {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--space-2);
  }

  .kyc__title {
    text-align: center;
  }

  /* Body copy under the title. The shared .text-body utility doesn't exist —
     we render the muted secondary line inline at 14/20 to match the design. */
  .kyc__subtitle {
    font-family: var(--font-system);
    font-size: 14px;
    line-height: 20px;
    color: var(--color-text-secondary);
    text-align: center;
    max-width: 460px;
  }

  .kyc__actions {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: var(--space-2);
  }

  .kyc__steps {
    display: flex;
    gap: var(--space-4);
    width: 100%;
  }

  .step-btn {
    appearance: none;
    background: transparent;
    border: 0;
    padding: 0;
    margin: 0;
    flex: 1 1 0;
    min-width: 0;
    display: flex;
    cursor: pointer;
    text-align: left;
    border-radius: var(--radius-card);
  }
  .step-btn:focus-visible {
    outline: 2px solid var(--color-action-cream-border);
    outline-offset: 2px;
  }
</style>

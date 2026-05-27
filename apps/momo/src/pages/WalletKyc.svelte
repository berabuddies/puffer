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
  import Composer from "../components/shell/Composer.svelte";
  import Button from "../components/common/Button.svelte";
  import KycIllustration from "../components/wallet/KycIllustration.svelte";
  import StepCard from "../components/wallet/StepCard.svelte";

  import { navigate } from "../router.svelte";

  let kycInFlight = $state<boolean>(false);

  function handleStartKyc(): void {
    // Hand off to the new KYC form route. The legacy mock toast pipeline
    // is gone — the form submits to walletClient.startKyc() which drives
    // the UI state transitions.
    navigate("/wallet/kyc/form");
  }
</script>

<section class="kyc">
  <div class="kyc__hero">
    <KycIllustration />

    <div class="kyc__text">
      <h1 class="text-display kyc__title">Set Up Wallet to let Agent pay</h1>
      <p class="kyc__subtitle">
        Complete Identity verification before adding crypto funds and
        allowing Agent to place orders on your behalf.
      </p>
    </div>

    <div class="kyc__actions">
      <Button
        variant="primary"
        size="md"
        label="Verify identity"
        disabled={kycInFlight}
        onclick={handleStartKyc}
      />
    </div>
  </div>

  <div class="kyc__steps">
    <StepCard eyebrow="1 · Verify identity" body="Submit Identity Information" />
    <StepCard eyebrow="2 · Top up crypto" body="Deposit USDC into your wallet." />
    <StepCard eyebrow="3 · Agent can pay" body="Approve orders and spending limits." />
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

  /* Step cards beneath the hero are purely informational — no interaction
     affordance (no cursor change, no hover/focus styles). The StepCard
     component renders as <article>; we just lay them out in a row. */
  .kyc__steps {
    display: flex;
    gap: var(--space-4);
    width: 100%;
  }
</style>

<!--
  Wallet (empty state) — rendered inline by Wallet.svelte when the URL
  hash contains `empty=1`. Calm placeholder mirroring HomeEmpty/ContactEmpty.

  Routes are owned by another agent, so this page is not registered in
  routes.ts; the dev "Show empty" toggle on Wallet.svelte flips the hash.
-->
<script lang="ts">
  import { Wallet as WalletIcon } from "lucide-svelte";

  import { navigate } from "../router.svelte";
  import { pushToast } from "../lib/toast.svelte";

  function goKyc(): void {
    pushToast("Starting wallet setup…", "info");
    navigate("/wallet/kyc");
  }
</script>

<section class="empty">
  <div class="empty__art" aria-hidden="true">
    <WalletIcon size={48} strokeWidth={1.25} />
  </div>
  <h2 class="empty__title text-display">No wallet activity yet</h2>
  <p class="empty__body">
    Complete KYC and top up to let Momo make payments on your behalf.
  </p>
  <button type="button" class="empty__cta" onclick={goKyc}>
    Set up wallet
  </button>
</section>

<style>
  .empty {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: var(--space-3);
    text-align: center;
    padding: var(--space-7) var(--space-4);
  }

  .empty__art {
    width: 88px;
    height: 88px;
    border-radius: 50%;
    background: var(--color-surface-rail);
    color: var(--color-text-muted);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    margin-bottom: var(--space-3);
  }

  .empty__title {
    margin: 0;
  }

  .empty__body {
    max-width: 360px;
    font-family: var(--font-system);
    font-size: 14px;
    line-height: 20px;
    color: var(--color-text-secondary);
  }

  .empty__cta {
    margin-top: var(--space-3);
    height: var(--height-button-composer);
    padding: 0 var(--space-4);
    border-radius: var(--radius-pill);
    background: var(--color-text-primary);
    color: var(--color-surface-app);
    border: 0;
    font-family: var(--font-sans);
    font-size: var(--font-size-button);
    line-height: var(--line-height-button);
    font-weight: var(--font-weight-medium);
    cursor: pointer;
    transition: filter 120ms ease;
  }
  .empty__cta:hover {
    filter: brightness(1.1);
  }
</style>

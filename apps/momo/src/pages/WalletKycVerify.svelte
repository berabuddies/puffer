<!--
  Wallet · KYC verify (route /wallet/kyc/verify).

  Renders the eKYC provider URL in a full-bleed iframe inside the page
  column. The URL is handed in from WalletKycForm via walletState.ekycUrl
  (set inside walletClient.svelte.ts). If the user reaches this page
  without an URL (e.g. deep-linked), we bounce back to /wallet/kyc.

  Tauri CSP may block third-party iframes — this is a known follow-up.
  Out of scope here per the parent brief: do not add Tauri opener calls.
-->
<script lang="ts">
  import { onMount } from "svelte";
  import { navigate } from "../router.svelte";
  import { walletState } from "../lib/walletClient.svelte";
  import Button from "../components/common/Button.svelte";

  let url = $state<string | null>(walletState.ekycUrl);
  let frameError = $state<boolean>(false);

  const showMockShortcut =
    (import.meta.env as Record<string, string | undefined>).VITE_USE_MOCK_WALLET === "true";

  // Mock-only window surface — see Wallet.svelte for the same pattern.
  type WalletMock = { simulateApproveKyc(): void };
  function mock(): WalletMock | null {
    if (typeof window === "undefined") return null;
    return (window as unknown as { __wallet?: WalletMock }).__wallet ?? null;
  }

  onMount(() => {
    if (!url) {
      // No URL → user landed here directly. Bounce back to KYC entry.
      navigate("/wallet/kyc");
    }
  });

  function handleCancel(): void {
    walletState.ekycUrl = null;
    navigate("/wallet");
  }

  function handleFrameError(): void {
    frameError = true;
  }

  function simulateVerificationComplete(): void {
    // Mock-only escape hatch: didit isn't reachable in dev, so the iframe
    // blank-screens. Pretend the webhook just fired and bounce home.
    mock()?.simulateApproveKyc();
    navigate("/wallet");
  }
</script>

<section class="verify">
  <header class="verify__head">
    <div class="verify__copy">
      <h1 class="text-section">Verify identity</h1>
      <p class="verify__intro">
        Complete identity verification in the embedded provider window. You
        can close this page when finished — we'll pick up the status automatically.
      </p>
    </div>
    <div class="verify__actions">
      {#if showMockShortcut}
        <button
          type="button"
          class="verify__mock"
          onclick={simulateVerificationComplete}
        >
          Simulate verification complete (mock)
        </button>
      {/if}
      <Button variant="secondary" size="sm" label="Cancel" onclick={handleCancel} />
    </div>
  </header>

  {#if url}
    <div class="verify__frame">
      <iframe
        title="Identity verification"
        src={url}
        onerror={handleFrameError}
        allow="camera; microphone; clipboard-read; clipboard-write"
        referrerpolicy="no-referrer"
      ></iframe>
      {#if frameError}
        <div class="verify__fallback">
          <p>The verification window couldn't be embedded.</p>
          <a class="verify__link" href={url} target="_blank" rel="noreferrer noopener">
            Open verification
          </a>
        </div>
      {/if}
    </div>
  {/if}
</section>

<style>
  .verify {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    flex: 1;
    min-height: 0;
    padding-bottom: var(--space-5);
  }

  .verify__head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: var(--space-4);
  }

  .verify__copy {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    max-width: 540px;
  }

  .verify__intro {
    font-family: var(--font-system);
    font-size: 14px;
    line-height: 20px;
    color: var(--color-text-secondary);
  }

  .verify__frame {
    position: relative;
    display: flex;
    flex-direction: column;
    flex: 1 1 auto;
    min-height: 720px;
    background: var(--color-surface-rail);
    border-radius: var(--radius-card);
    border: 1px solid var(--color-card-border);
    overflow: hidden;
  }
  .verify__frame iframe {
    flex: 1 1 auto;
    width: 100%;
    min-height: 720px;
    border: 0;
    display: block;
  }

  .verify__fallback {
    position: absolute;
    inset: 0;
    background: var(--color-surface-rail);
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: var(--space-3);
    text-align: center;
    padding: var(--space-5);
  }
  .verify__fallback p {
    font-family: var(--font-system);
    font-size: 14px;
    color: var(--color-text-secondary);
  }
  .verify__link {
    font-family: var(--font-sans);
    font-size: 13px;
    color: var(--color-action-cream-text);
    background: var(--color-action-cream);
    padding: 8px 16px;
    border-radius: var(--radius-pill);
  }

  .verify__actions {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    flex-shrink: 0;
  }

  .verify__mock {
    appearance: none;
    background: transparent;
    border: 1px dashed var(--color-card-border);
    border-radius: var(--radius-control);
    padding: 6px 10px;
    color: var(--color-text-muted);
    font-family: var(--font-system);
    font-size: 11px;
    line-height: 14px;
    font-weight: var(--font-weight-medium);
    cursor: pointer;
    transition: background-color 120ms ease, color 120ms ease;
  }
  .verify__mock:hover {
    background: var(--color-surface-rail);
    color: var(--color-text-primary);
  }

  .verify__frame--mock {
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .verify__mock-placeholder {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--space-3);
    text-align: center;
    padding: var(--space-5);
    max-width: 480px;
  }
  .verify__mock-title {
    font-family: var(--font-system);
    font-size: 14px;
    font-weight: var(--font-weight-medium);
    color: var(--color-text-primary);
  }
  .verify__mock-body {
    font-family: var(--font-system);
    font-size: 13px;
    line-height: 20px;
    color: var(--color-text-secondary);
  }
  .verify__mock-url {
    font-family: var(--font-mono, ui-monospace, "SF Mono", Menlo, monospace);
    font-size: 12px;
    color: var(--color-text-muted);
    background: var(--color-surface-app);
    padding: 4px 8px;
    border-radius: var(--radius-control);
    border: 1px solid var(--color-card-border);
    word-break: break-all;
  }
</style>

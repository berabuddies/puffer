<!--
  TopUpModal — deposit address dialog (Paper artboard 3E6-1).

  Layout:
    Header: "Top up" (serif 20) + "Arrives ~1 min …" subtitle + close X.
    Two columns: Assets / Network.
    Deposit address section: gray rounded card with QR + address text,
      then a "Copy address" outline button, then the minimum-deposit note.
    Warning banner (cream-orange) with the network-specific guidance.
    Footer: outline "Cancel" + cream "Deposited" — both call `onClose`.

  On mount the modal fetches the first available deposit address from
  walletClient and renders a QR for `depositAddress`. The render is
  asynchronous (qrcode.toDataURL) — until it resolves the QR slot stays
  blank but the address is shown immediately.
-->
<script lang="ts">
  import QRCode from "qrcode";

  import Modal from "../common/Modal.svelte";
  import { walletClient } from "../../lib/walletClient.svelte";
  import type { DepositAddress } from "../../lib/walletTypes";
  import { pushToast } from "../../lib/toast.svelte";

  interface Props {
    open: boolean;
    onClose: () => void;
  }

  let { open, onClose }: Props = $props();

  let address = $state<DepositAddress | null>(null);
  let qrDataUrl = $state<string | null>(null);
  let loading = $state<boolean>(false);
  let loadError = $state<string | null>(null);

  // Refetch + regen the QR every time the modal opens. Closing wipes
  // state so a re-open starts from scratch (mirrors real production
  // behaviour where the address may rotate).
  $effect(() => {
    if (!open) {
      address = null;
      qrDataUrl = null;
      loadError = null;
      return;
    }
    void load();
  });

  async function load(): Promise<void> {
    loading = true;
    loadError = null;
    try {
      const res = await walletClient.getDepositAddress();
      const first = res.addresses[0];
      if (!first) {
        loadError = "No deposit address available.";
        return;
      }
      address = first;
      qrDataUrl = await QRCode.toDataURL(first.depositAddress, {
        margin: 1,
        width: 240,
        color: { dark: "#161616", light: "#ffffff" }
      });
    } catch (err) {
      loadError = err instanceof Error ? err.message : String(err);
    } finally {
      loading = false;
    }
  }

  async function copyAddress(): Promise<void> {
    if (!address) return;
    try {
      if (
        typeof navigator !== "undefined" &&
        navigator.clipboard &&
        typeof navigator.clipboard.writeText === "function"
      ) {
        await navigator.clipboard.writeText(address.depositAddress);
        pushToast("Address copied", "success");
      } else {
        pushToast("Clipboard unavailable", "error");
      }
    } catch {
      pushToast("Could not copy address", "error");
    }
  }
</script>

<Modal {open} {onClose} maxWidth="400px">
  <header class="topup-header">
    <div class="topup-header__copy">
      <h2 class="topup-title">Top up</h2>
      <p class="topup-subtitle">Arrives ~1 min after on-chain confirmation.</p>
    </div>
    <button
      type="button"
      class="topup-close"
      aria-label="Close"
      onclick={onClose}
    >
      <svg width="20" height="20" viewBox="0 0 20 20" aria-hidden="true">
        <path
          d="M5 5L15 15M15 5L5 15"
          stroke="currentColor"
          stroke-width="1.6"
          stroke-linecap="round"
        />
      </svg>
    </button>
  </header>

  <div class="topup-body">
    <div class="topup-meta">
      <div class="topup-meta__col">
        <span class="topup-label">Assets</span>
        <div class="topup-meta__row">
          <span class="asset-icon" aria-hidden="true">
            <span class="asset-icon__inner">$1</span>
          </span>
          <span class="topup-value">{address?.assetSymbol ?? "USD1"}</span>
        </div>
      </div>
      <div class="topup-meta__col">
        <span class="topup-label">Network</span>
        <div class="topup-meta__row">
          <span class="network-icon" aria-hidden="true">B</span>
          <span class="topup-value">{address?.chainName ?? "BNB Smart Chain"}</span>
        </div>
      </div>
    </div>

    <div class="topup-address">
      <span class="topup-label">Deposit address</span>
      <div class="address-card">
        <div class="qr-frame" aria-hidden="true">
          {#if qrDataUrl}
            <img src={qrDataUrl} alt="" />
          {:else if loading}
            <span class="qr-placeholder">Loading…</span>
          {:else if loadError}
            <span class="qr-placeholder qr-placeholder--error">!</span>
          {/if}
        </div>
        <p class="address-text">
          {address?.depositAddress ?? (loading ? "Loading address…" : loadError ?? "")}
        </p>
      </div>

      <button
        type="button"
        class="copy-button"
        onclick={copyAddress}
        disabled={!address}
      >
        Copy address
      </button>

      <p class="topup-note">Minimum deposit amount: 1 USD1</p>
    </div>

    <aside class="topup-warning">
      <span class="topup-warning__icon" aria-hidden="true">!</span>
      <div class="topup-warning__copy">
        <p class="topup-warning__title">Send only USD1 on BSC</p>
        <p class="topup-warning__body">
          Deposits on other networks may not be credited.
        </p>
      </div>
    </aside>
  </div>

  <footer class="topup-footer">
    <button type="button" class="footer-btn footer-btn--ghost" onclick={onClose}>
      Cancel
    </button>
    <button type="button" class="footer-btn footer-btn--cream" onclick={onClose}>
      Deposited
    </button>
  </footer>
</Modal>

<style>
  .topup-header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: var(--space-4);
    padding: 20px 20px 0 20px;
  }
  .topup-header__copy {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }
  .topup-title {
    font-family: var(--font-serif);
    font-size: 20px;
    line-height: 24px;
    font-weight: var(--font-weight-regular);
    color: var(--color-text-primary);
  }
  .topup-subtitle {
    font-family: var(--font-system);
    font-size: 12px;
    line-height: 16px;
    color: var(--color-text-secondary);
  }
  .topup-close {
    appearance: none;
    background: transparent;
    border: 0;
    width: 28px;
    height: 28px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border-radius: var(--radius-pill);
    color: var(--color-text-secondary);
    cursor: pointer;
  }
  .topup-close:hover {
    background: var(--color-surface-rail);
  }

  .topup-body {
    display: flex;
    flex-direction: column;
    gap: 20px;
    padding: 20px;
    overflow-y: auto;
  }

  .topup-label {
    font-family: var(--font-system);
    font-size: 12px;
    line-height: 16px;
    color: var(--color-text-secondary);
  }
  .topup-value {
    font-family: var(--font-system);
    font-size: 13px;
    line-height: 18px;
    color: var(--color-text-primary);
  }

  .topup-meta {
    display: flex;
    gap: 20px;
    align-items: flex-start;
  }
  .topup-meta__col {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    flex: 1;
    min-width: 0;
  }
  .topup-meta__row {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }

  .asset-icon {
    width: 22px;
    height: 22px;
    border-radius: 50%;
    background: var(--color-action-cream);
    color: var(--color-action-cream-text);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
  }
  .asset-icon__inner {
    font-family: var(--font-sans);
    font-size: 10px;
    font-weight: var(--font-weight-semibold);
    line-height: 1;
  }

  .network-icon {
    width: 22px;
    height: 22px;
    border-radius: 50%;
    background: #f3ba2f;
    color: #161616;
    font-family: var(--font-sans);
    font-size: 12px;
    font-weight: var(--font-weight-semibold);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
  }

  .topup-address {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .address-card {
    background: var(--color-surface-rail);
    border-radius: var(--radius-control);
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 12px;
    align-items: center;
  }

  .qr-frame {
    width: 140px;
    height: 140px;
    background: var(--color-surface-app);
    border-radius: 6px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    overflow: hidden;
  }
  .qr-frame img {
    width: 100%;
    height: 100%;
    display: block;
  }
  .qr-placeholder {
    font-family: var(--font-system);
    font-size: 12px;
    color: var(--color-text-muted);
  }
  .qr-placeholder--error {
    color: #b91c1c;
    font-weight: var(--font-weight-semibold);
  }

  .address-text {
    font-family: "IBM Plex Sans", var(--font-system);
    font-size: 14px;
    line-height: 18px;
    font-weight: var(--font-weight-medium);
    color: var(--color-text-muted);
    text-align: center;
    max-width: 220px;
    word-break: break-all;
  }

  .copy-button {
    appearance: none;
    background: var(--color-surface-app);
    border: 1px solid var(--color-input-border);
    border-radius: var(--radius-control);
    height: 36px;
    cursor: pointer;
    font-family: var(--font-sans);
    font-size: 13px;
    line-height: 16px;
    color: var(--color-text-primary);
    transition: background-color 120ms ease;
  }
  .copy-button:hover:not(:disabled) {
    background: var(--color-surface-rail);
  }
  .copy-button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .topup-note {
    font-family: var(--font-system);
    font-size: 12px;
    line-height: 16px;
    color: var(--color-text-muted);
  }

  .topup-warning {
    display: flex;
    gap: 10px;
    padding: 12px;
    border-radius: var(--radius-control);
    background: #fdf3ed;
    align-items: flex-start;
  }
  .topup-warning__icon {
    width: 16px;
    height: 16px;
    border-radius: 50%;
    background: #e77e33;
    color: #ffffff;
    font-family: var(--font-sans);
    font-size: 11px;
    font-weight: var(--font-weight-semibold);
    line-height: 1;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    margin-top: 2px;
  }
  .topup-warning__copy {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .topup-warning__title {
    font-family: var(--font-system);
    font-size: 13px;
    line-height: 18px;
    font-weight: var(--font-weight-medium);
    color: #c2410c;
  }
  .topup-warning__body {
    font-family: var(--font-system);
    font-size: 12px;
    line-height: 16px;
    color: #9a3412;
  }

  .topup-footer {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: var(--space-2);
    padding: 16px 20px 20px 20px;
  }

  .footer-btn {
    appearance: none;
    border-radius: var(--radius-control);
    height: 32px;
    padding: 0 16px;
    min-width: 88px;
    font-family: var(--font-sans);
    font-size: 13px;
    line-height: 16px;
    font-weight: var(--font-weight-medium);
    cursor: pointer;
    border: 0;
  }
  .footer-btn--ghost {
    background: var(--color-surface-input);
    border: 1px solid var(--color-input-border);
    color: var(--color-text-primary);
  }
  .footer-btn--ghost:hover {
    background: var(--color-surface-rail);
  }
  .footer-btn--cream {
    background: var(--color-action-cream);
    color: var(--color-action-cream-text);
  }
  .footer-btn--cream:hover {
    filter: brightness(0.97);
  }
</style>

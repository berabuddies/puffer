<!--
  TopUpModal — deposit USD1 into the U-card wallet (Paper artboard 3E6-1).

  Layout:
    Header: "Top up" (serif 20) + "Arrives ~1 min …" subtitle + close X.
    Two columns: Assets / Network.
    Deposit address section: gray rounded card with QR + address text,
      then a "Copy address" outline button, then the minimum-deposit note,
      then a live deposit-status line.
    Warning banner (cream-orange) with the network-specific guidance.
    Footer: outline "Cancel" + cream "Deposited" — both call `onClose`.

  Data flow (ucard-backend integration spec):
    1. On open, POST /card/deposit-intent → the per-card deposit `address`
       (NOT `addressOut`, which is the gateway's internal forwarding address)
       + the backend-enforced `minimumAmount`. We render `address` as a QR +
       copyable string and surface the minimum from the response (never
       hardcoded).
    2. The user sends USD1 on BNB Smart Chain to that address from their own
       wallet — we can't observe the transfer directly, so…
    3. …we poll GET /card/deposits while the modal is open and surface the
       credit lifecycle (depositStatus 0 waiting → 1 crediting → 2 credited).
       Only deposits that appear *after* the modal opened are shown, so an old
       deposit never reads as "your transfer landed". On credit we refresh the
       wallet snapshot so the new balance is ready behind the modal.

  Props:
    open    — whether the modal is visible.
    onClose — called when the user dismisses (Cancel / Deposited / ESC / backdrop).
    cardId  — the card to fund. Optional; omitted lets the backend pick the
              user's first depositable card.
-->
<script lang="ts">
  import { untrack } from "svelte";
  import QRCode from "qrcode";

  import Modal from "../common/Modal.svelte";
  import { walletClient } from "../../lib/walletClient.svelte";
  import { loadWallet } from "../../lib/walletStore.svelte";
  import type { DepositIntent, DepositRecord } from "../../lib/walletTypes";
  import { pushToast } from "../../lib/toast.svelte";

  interface Props {
    open: boolean;
    onClose: () => void;
    cardId?: number | null;
  }

  let { open, onClose, cardId = null }: Props = $props();

  // chainId 56 (BNB Smart Chain) + USD1 are fixed for now per the integration
  // spec; the minimum is read from the intent response, never hardcoded.
  const BSC_CHAIN_ID = 56;
  const DEPOSIT_ASSET = "USD1";
  const NETWORK_LABEL = "BNB Smart Chain";
  const USD1_DECIMALS = 18;
  const POLL_INTERVAL_MS = 6000;
  // Hard ceiling on the intent request so a hung/slow backend can't leave the
  // modal spinning forever — it flips to an error state the user can retry.
  const INTENT_TIMEOUT_MS = 15000;

  let intent = $state<DepositIntent | null>(null);
  let qrDataUrl = $state<string | null>(null);
  let loading = $state<boolean>(false);
  let loadError = $state<string | null>(null);

  // Deposit polling. `knownTxHashes` is the baseline captured at open, so we
  // only surface deposits that arrive afterwards. `latestDeposit` is the newest
  // such deposit; `credited` latches once any of them reaches depositStatus 2.
  let knownTxHashes = new Set<string>();
  let latestDeposit = $state<DepositRecord | null>(null);
  let credited = $state<boolean>(false);
  let pollTimer: ReturnType<typeof setInterval> | null = null;

  // Drive the lifecycle off `open` — and ONLY `open`. begin()/stop() read and
  // write `loading`/`intent` (for the template), and an async function's body
  // up to its first await runs synchronously inside the effect, so calling them
  // tracked would subscribe this effect to those state vars: begin() sets
  // loading=true → re-run → cleanup stop() sets loading=false → re-run →
  // begin() again …, an infinite loop that pegs the main thread (spinner never
  // resolves, every button unclickable). untrack() isolates those reads/writes
  // so the sole dependency is `open`; the cleanup then fires exactly once when
  // `open` flips false (or on unmount).
  $effect(() => {
    if (open) untrack(() => void begin());
    return () => untrack(() => stop());
  });

  function timeout(ms: number): Promise<never> {
    return new Promise((_resolve, reject) =>
      setTimeout(() => reject(new Error("Request timed out. Please try again.")), ms)
    );
  }

  async function begin(): Promise<void> {
    // Re-entrancy guard. Safe to read here because untrack() keeps it from
    // becoming an effect dependency (see the $effect note above).
    if (loading || intent) return;
    loading = true;
    loadError = null;
    try {
      const created = await Promise.race([
        walletClient.createDepositIntent({
          chainId: BSC_CHAIN_ID,
          assetSymbol: DEPOSIT_ASSET,
          ...(cardId != null ? { cardId } : {})
        }),
        timeout(INTENT_TIMEOUT_MS)
      ]);
      if (!open) return; // user closed mid-request
      if (!created.address) {
        loadError = "No deposit address available.";
        return;
      }
      intent = created;
      qrDataUrl = await QRCode.toDataURL(created.address, {
        margin: 1,
        width: 240,
        color: { dark: "#161616", light: "#ffffff" }
      });
      // Seed the baseline so pre-existing deposits aren't mistaken for this
      // session's transfer, then start polling for new ones.
      try {
        const existing = await walletClient.getDeposits({ limit: 50, offset: 0 });
        knownTxHashes = new Set(existing.map((d) => d.txHash).filter(Boolean));
      } catch {
        knownTxHashes = new Set();
      }
      startPolling();
    } catch (err) {
      loadError = err instanceof Error ? err.message : String(err);
    } finally {
      loading = false;
    }
  }

  function startPolling(): void {
    if (pollTimer) return;
    pollTimer = setInterval(() => void poll(), POLL_INTERVAL_MS);
  }

  async function poll(): Promise<void> {
    if (!open) return;
    let rows: DepositRecord[];
    try {
      rows = await walletClient.getDeposits({ limit: 20, offset: 0 });
    } catch {
      return; // transient — keep polling
    }
    const mine = cardId != null ? rows.filter((r) => r.cardId === cardId) : rows;
    const fresh = mine.filter((r) => r.txHash && !knownTxHashes.has(r.txHash));
    if (fresh.length === 0) return;
    latestDeposit = fresh[0]; // backend returns newest-first
    if (!credited && fresh.some((r) => r.depositStatus === 2)) {
      credited = true;
      pushToast("Deposit credited to your wallet", "success");
      // Refresh the snapshot behind the modal so the balance is current the
      // moment the user closes.
      void loadWallet();
    }
  }

  function stop(): void {
    if (pollTimer) {
      clearInterval(pollTimer);
      pollTimer = null;
    }
    intent = null;
    qrDataUrl = null;
    loading = false;
    loadError = null;
    knownTxHashes = new Set();
    latestDeposit = null;
    credited = false;
  }

  function formatDepositAmount(raw: string): string {
    const n = Number(raw) / 10 ** USD1_DECIMALS;
    if (!Number.isFinite(n)) return "";
    return n.toLocaleString("en-US", { maximumFractionDigits: 4 });
  }

  function depositStatusText(d: DepositRecord): string {
    switch (d.depositStatus) {
      case 2:
        return "credited";
      case 1:
        return "crediting…";
      case 3:
        return "credit failed — contact support";
      default:
        return "confirmed on-chain, crediting shortly…";
    }
  }

  async function copyAddress(): Promise<void> {
    if (!intent) return;
    try {
      if (
        typeof navigator !== "undefined" &&
        navigator.clipboard &&
        typeof navigator.clipboard.writeText === "function"
      ) {
        await navigator.clipboard.writeText(intent.address);
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
          <span class="topup-value">{intent?.assetSymbol ?? DEPOSIT_ASSET}</span>
        </div>
      </div>
      <div class="topup-meta__col">
        <span class="topup-label">Network</span>
        <div class="topup-meta__row">
          <span class="network-icon" aria-hidden="true">B</span>
          <span class="topup-value">{NETWORK_LABEL}</span>
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
          {intent?.address ?? (loading ? "Loading address…" : loadError ?? "")}
        </p>
      </div>

      <button
        type="button"
        class="copy-button"
        onclick={copyAddress}
        disabled={!intent}
      >
        Copy address
      </button>

      <p class="topup-note">
        Minimum deposit amount: {intent?.minimumAmount ?? "1"} {DEPOSIT_ASSET}
      </p>

      {#if intent}
        <p
          class="topup-status"
          class:topup-status--ok={credited}
          aria-live="polite"
        >
          {#if latestDeposit}
            Deposit of {formatDepositAmount(latestDeposit.amount)}
            {DEPOSIT_ASSET} — {depositStatusText(latestDeposit)}
          {:else}
            Waiting for your transfer… it appears here once confirmed on-chain.
          {/if}
        </p>
      {/if}
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

  .topup-status {
    font-family: var(--font-system);
    font-size: 12px;
    line-height: 16px;
    color: var(--color-text-secondary);
    padding: 8px 10px;
    border-radius: var(--radius-control);
    background: var(--color-surface-rail);
  }
  .topup-status--ok {
    color: #1a7f4b;
    background: rgba(39, 174, 96, 0.12);
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

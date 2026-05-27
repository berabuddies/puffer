<!--
  Wallet — main wallet page.

  Three faces of the same component:
    1. KYC empty (status === 'none')        — bounce to /wallet/kyc.
    2. In Review / Pending / Declined / etc. — Wallet shell with $0 balance,
                                                Identity verification KPI
                                                shows "In Review" (we never
                                                expose error text per spec).
    3. Approved                              — Wallet shell, real balance +
                                                transactions, Identity card
                                                shows the green check.

  The page calls into walletClient.* on mount:
    • getKycStatus()                       — drives the three branches above.
    • getCardList() + getBalance(cardId)   — only when approved.
    • getTransactions(cardId, …)           — only when approved.

  Top up CTA is disabled until status === 'approved'. Clicking it opens
  TopUpModal; closing the modal refreshes balance + transactions so the
  user sees their deposit credited as soon as the backend acknowledges it.
-->
<script lang="ts">
  import { onMount } from "svelte";
  import { CheckCircle2, Search } from "lucide-svelte";

  import Composer from "../components/shell/Composer.svelte";
  import Chip from "../components/common/Chip.svelte";
  import KpiCard from "../components/wallet/KpiCard.svelte";
  import ActivityRow from "../components/wallet/ActivityRow.svelte";
  import TopUpModal from "../components/wallet/TopUpModal.svelte";
  import {
    walletClient,
    setMockKycStatus,
    setMockHasFunds
  } from "../lib/walletClient.svelte";

  // Mock-only window surface exposed by walletClient.svelte.ts when
  // VITE_USE_MOCK_WALLET=true. Typed locally so we don't widen the
  // global Window type for production builds.
  type WalletMock = {
    simulateApproveKyc(): void;
    simulateDeclineKyc(opts?: { admin?: boolean }): void;
    simulateDeposit(amountUsd: number, opts?: { from?: string; label?: string }): void;
    simulateSpend(amountUsd: number, opts?: { merchant?: string; description?: string }): void;
    reset(): void;
  };
  function mock(): WalletMock | null {
    if (typeof window === "undefined") return null;
    return (window as unknown as { __wallet?: WalletMock }).__wallet ?? null;
  }
  import type {
    KycStatus,
    KycStatusResponse,
    Transaction
  } from "../lib/walletTypes";
  import type { WalletActivity } from "../data/types";
  import { navigate } from "../router.svelte";
  import { pushToast } from "../lib/toast.svelte";

  const showDevToggles =
    (import.meta.env as Record<string, string | undefined>).VITE_USE_MOCK_WALLET === "true";

  let kycStatus = $state<KycStatus>("none");
  let kycLoaded = $state<boolean>(false);
  let cardId = $state<number | null>(null);
  let balance = $state<number>(0);
  let txns = $state<Transaction[]>([]);
  let balanceHidden = $state<boolean>(false);
  let topUpOpen = $state<boolean>(false);

  async function refresh(): Promise<void> {
    let status: KycStatusResponse;
    try {
      status = await walletClient.getKycStatus();
    } catch (err) {
      pushToast(`Wallet load failed: ${err instanceof Error ? err.message : err}`, "error");
      kycLoaded = true;
      return;
    }
    kycStatus = status.status;
    kycLoaded = true;

    if (kycStatus === "none") {
      // No cardholder record yet → user has never started KYC. Bounce to
      // the empty-state page. We do this here rather than in App so the
      // route only redirects once data lands.
      navigate("/wallet/kyc");
      return;
    }

    if (kycStatus === "approved") {
      try {
        const cards = await walletClient.getCardList();
        const first = cards[0];
        if (first) {
          cardId = first.cardId;
          const [b, t] = await Promise.all([
            walletClient.getBalance(first.cardId),
            walletClient.getTransactions(first.cardId, { limit: 10 })
          ]);
          balance = b.availableBalance;
          txns = t.transactions;
        } else {
          // Approved but no card yet — treat the wallet as $0 + empty list.
          balance = 0;
          txns = [];
        }
      } catch (err) {
        pushToast(
          `Wallet load failed: ${err instanceof Error ? err.message : err}`,
          "error"
        );
      }
    } else {
      // In Review / Declined / etc. — show the wallet shell with $0 and
      // no transactions.
      balance = 0;
      txns = [];
    }
  }

  onMount(() => {
    void refresh();
  });

  function handleTopUp(): void {
    if (kycStatus !== "approved") return;
    topUpOpen = true;
  }

  function closeTopUp(): void {
    topUpOpen = false;
    void refresh();
  }

  function toggleBalance(): void {
    balanceHidden = !balanceHidden;
  }

  function tapKpi(label: string): void {
    pushToast(`KPI: ${label}`, "info");
  }

  function tapActivity(title: string): void {
    pushToast(`Activity: ${title}`, "info");
  }

  /* ───── Dev state toggles ─────────────────────────────────────
     Row 1 ("Quick jump") routes through the legacy shortcuts so the
     screenshot harness can teleport between canonical design states.
     Row 2 ("Simulate") drives the mock state machine the same way the
     real backend would — useful when validating click-through flows. */
  function setStatus(s: KycStatus, hasFunds: boolean): void {
    setMockKycStatus(s);
    setMockHasFunds(hasFunds);
    void refresh();
  }

  function simApproveKyc(): void {
    mock()?.simulateApproveKyc();
    void refresh();
  }
  function simDeclineKyc(): void {
    mock()?.simulateDeclineKyc();
    void refresh();
  }
  function simDeposit(): void {
    mock()?.simulateDeposit(500);
    void refresh();
  }
  function simSpend(): void {
    mock()?.simulateSpend(42);
    void refresh();
  }
  function simReset(): void {
    mock()?.reset();
    void refresh();
  }

  // Format helpers — the design uses comma-grouped two-decimal USD.
  let balanceDisplay = $derived(
    balanceHidden ? "●●●●●●" : formatCurrency(balance)
  );

  function formatCurrency(n: number): string {
    return n.toLocaleString("en-US", {
      style: "currency",
      currency: "USD",
      minimumFractionDigits: 2,
      maximumFractionDigits: 2
    });
  }

  function formatTxnAmount(t: Transaction): string {
    const sign = t.type === "credit" ? "+" : "-";
    return `${sign}${formatCurrency(t.amount)}`;
  }

  function txnToActivity(t: Transaction): WalletActivity {
    return {
      id: t.id,
      date: t.date,
      merchant: t.merchantName,
      amount: formatTxnAmount(t),
      category: t.description,
      direction: t.type === "credit" ? "in" : "out",
      description: t.description,
      time: t.date
    };
  }

  // Monthly spend = sum of debits, formatted. Mirrors the design's
  // "$0.00" empty state and "$326.80" with the sample data.
  let monthlySpend = $derived(
    formatCurrency(
      txns.filter((t) => t.type === "debit").reduce((s, t) => s + t.amount, 0)
    )
  );

  let activitiesDisplay = $derived(txns.map(txnToActivity));
  let isApproved = $derived(kycStatus === "approved");
  let identityLabel = $derived(isApproved ? "Verified" : "In Review");
</script>

<div class="wallet">
  <header class="wallet-header">
    <h1 class="text-section">Wallet</h1>
    {#if showDevToggles}
      <div class="dev-toggles" aria-label="Dev state toggles">
        <div class="dev-toggles__row" aria-label="Quick jump">
          <span class="dev-toggles__label">Jump:</span>
          <button type="button" class="dev-link" onclick={() => setStatus("none", false)}>None</button>
          <span class="dev-toggles__sep" aria-hidden="true">·</span>
          <button type="button" class="dev-link" onclick={() => setStatus("in_review", false)}>Review</button>
          <span class="dev-toggles__sep" aria-hidden="true">·</span>
          <button type="button" class="dev-link" onclick={() => setStatus("approved", false)}>Verified empty</button>
          <span class="dev-toggles__sep" aria-hidden="true">·</span>
          <button type="button" class="dev-link" onclick={() => setStatus("approved", true)}>Verified full</button>
        </div>
        <div class="dev-toggles__row" aria-label="Simulate">
          <span class="dev-toggles__label">Sim:</span>
          <button type="button" class="dev-link" onclick={simApproveKyc}>Approve KYC</button>
          <span class="dev-toggles__sep" aria-hidden="true">·</span>
          <button type="button" class="dev-link" onclick={simDeclineKyc}>Decline KYC</button>
          <span class="dev-toggles__sep" aria-hidden="true">·</span>
          <button type="button" class="dev-link" onclick={simDeposit}>+ Deposit $500</button>
          <span class="dev-toggles__sep" aria-hidden="true">·</span>
          <button type="button" class="dev-link" onclick={simSpend}>− Spend $42</button>
          <span class="dev-toggles__sep" aria-hidden="true">·</span>
          <button type="button" class="dev-link" onclick={simReset}>Reset</button>
        </div>
      </div>
    {/if}
  </header>

  {#if !kycLoaded}
    <p class="wallet-loading">Loading…</p>
  {:else}
    <section class="wallet-balance">
      <div class="wallet-balance__text">
        <span class="text-eyebrow">AVAILABLE BALANCE</span>
        <button
          type="button"
          class="wallet-balance__amount-btn"
          aria-label={balanceHidden ? "Show balance" : "Hide balance"}
          onclick={toggleBalance}
        >
          <span class="wallet-balance__amount">{balanceDisplay}</span>
        </button>
      </div>
      <div class="wallet-balance__action">
        <Chip
          label="Top up"
          variant="cream"
          onclick={isApproved ? handleTopUp : undefined}
          title={isApproved ? undefined : "Verify identity first"}
        />
      </div>
    </section>

    <section class="wallet-kpis">
      <button type="button" class="kpi-btn" onclick={() => tapKpi("Monthly spend")}>
        <KpiCard label="Monthly spend" value={monthlySpend} />
      </button>
      <button type="button" class="kpi-btn" onclick={() => tapKpi("Identity verification")}>
        <KpiCard label="Identity verification" value={identityLabel}>
          {#snippet trailing()}
            {#if isApproved}
              <CheckCircle2 size={16} strokeWidth={1.75} aria-hidden="true" />
            {:else}
              <Search size={16} strokeWidth={1.75} aria-hidden="true" />
            {/if}
          {/snippet}
        </KpiCard>
      </button>
    </section>

    <section class="wallet-activity">
      <header class="wallet-activity__header">
        <h2 class="text-section">Recent activity</h2>
        {#if activitiesDisplay.length > 0}
          <Chip label={String(activitiesDisplay.length)} variant="count" />
        {/if}
      </header>

      <div class="wallet-activity__scroll">
        {#if activitiesDisplay.length === 0}
          <p class="wallet-activity__empty">No activity</p>
        {:else}
          <ul class="wallet-activity__list">
            {#each activitiesDisplay as row (row.id)}
              <li>
                <button
                  type="button"
                  class="activity-btn"
                  onclick={() => tapActivity(row.merchant)}
                >
                  <ActivityRow activity={row} />
                </button>
              </li>
            {/each}
          </ul>
        {/if}
      </div>
    </section>
  {/if}

  <Composer placeholder="Ask Alice to pay, order, or top up…" />
</div>

<TopUpModal open={topUpOpen} onClose={closeTopUp} />

<style>
  /* Page-column claim. The /wallet route is `fullbleed: true` (see routes.ts),
   * so the Shell locks .page__column to viewport height and drops its top
   * padding. We rebuild that top padding here and lay out our own three-row
   * column (header rows + scrolling activity + pinned composer) so the
   * activity list scrolls internally instead of pushing the composer off
   * the viewport when the user has many transactions. */
  .wallet {
    display: flex;
    flex-direction: column;
    flex: 1;
    /* min-height: 0 lets the flex chain hand off correctly so .wallet-activity's
     * `flex: 1 1 0` can shrink below its content size. */
    min-height: 0;
    height: 100%;
    width: 100%;
    /* Mirrors the top padding Shell normally applies on non-fullbleed pages —
     * clears the macOS traffic-light strip. */
    padding-top: calc(var(--shell-traffic-spacer) + var(--space-2));
  }

  .wallet-header {
    flex-shrink: 0;
    padding-bottom: var(--space-4);
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
  }

  /* Loading branch occupies the activity slot so the composer still hugs the
   * bottom while we wait for KYC status. */
  .wallet-loading {
    flex: 1 1 0;
    min-height: 0;
    color: var(--color-text-muted);
    font-family: var(--font-system);
    font-size: var(--font-size-body);
    padding-top: var(--space-5);
  }

  .wallet-balance {
    flex-shrink: 0;
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: var(--space-4);
    padding-bottom: var(--space-5);
  }

  .wallet-balance__text {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    min-width: 0;
  }

  .wallet-balance__amount-btn {
    appearance: none;
    background: transparent;
    border: 0;
    padding: 0;
    margin: 0;
    text-align: left;
    cursor: pointer;
    border-radius: var(--radius-control);
  }
  .wallet-balance__amount-btn:focus-visible {
    outline: 2px solid var(--color-action-cream-border);
    outline-offset: 4px;
  }

  .wallet-balance__amount {
    font-family: var(--font-serif);
    font-size: 52px;
    line-height: 60px;
    font-weight: var(--font-weight-regular);
    color: var(--color-text-primary);
    letter-spacing: -0.01em;
  }

  .wallet-balance__action {
    flex-shrink: 0;
    padding-top: var(--space-2);
  }

  .wallet-kpis {
    flex-shrink: 0;
    display: flex;
    gap: var(--space-4);
    padding-bottom: var(--space-5);
  }

  .kpi-btn {
    appearance: none;
    background: transparent;
    border: 0;
    padding: 0;
    margin: 0;
    flex: 1 1 0;
    min-width: 0;
    display: flex;
    cursor: pointer;
    border-radius: var(--radius-card);
    text-align: left;
  }
  .kpi-btn:focus-visible {
    outline: 2px solid var(--color-action-cream-border);
    outline-offset: 2px;
  }

  /* The activity section is the flex-grow slot: it takes whatever vertical
   * space remains between the KPI cards and the composer. The section
   * header stays put (non-scrolling); the inner .wallet-activity__scroll
   * is the actual overflow region. min-height: 0 is the load-bearing rule
   * that lets this child shrink below its content size — without it the
   * section grows to fit the list and pushes the composer off-screen. */
  .wallet-activity {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    flex: 1 1 0;
    min-height: 0;
    padding-bottom: var(--space-5);
  }

  .wallet-activity__header {
    flex-shrink: 0;
    display: flex;
    align-items: center;
    gap: var(--space-3);
  }

  /* Scrolling viewport for the tx list. Holds either the empty state or
   * the <ul>. Owns the overflow so the section header above and the
   * composer below stay anchored. */
  .wallet-activity__scroll {
    flex: 1 1 0;
    min-height: 0;
    overflow-y: auto;
    overflow-x: hidden;
  }

  .wallet-activity__empty {
    text-align: center;
    color: var(--color-text-muted);
    font-family: var(--font-system);
    font-size: var(--font-size-body);
    padding: var(--space-6) 0;
  }

  .wallet-activity__list {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
  }

  .activity-btn {
    appearance: none;
    background: transparent;
    border: 0;
    padding: 0;
    margin: 0;
    width: 100%;
    display: flex;
    cursor: pointer;
    text-align: left;
    border-radius: var(--radius-card);
  }
  .activity-btn:focus-visible {
    outline: 2px solid var(--color-action-cream-border);
    outline-offset: 2px;
  }

  .dev-toggles {
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    gap: 2px;
  }

  .dev-toggles__row {
    display: flex;
    align-items: center;
    gap: 4px;
    flex-wrap: wrap;
    justify-content: flex-end;
  }

  .dev-toggles__label {
    color: var(--color-text-muted);
    font-family: var(--font-system);
    font-size: 10px;
    line-height: 14px;
    letter-spacing: 0.04em;
    text-transform: uppercase;
    padding-right: 2px;
  }

  .dev-toggles__sep {
    color: var(--color-text-muted);
    font-family: var(--font-system);
    font-size: 11px;
    line-height: 14px;
  }

  .dev-link {
    appearance: none;
    background: transparent;
    border: 0;
    padding: 2px 4px;
    color: var(--color-text-muted);
    font-family: var(--font-system);
    font-size: 11px;
    line-height: 14px;
    font-weight: var(--font-weight-medium);
    cursor: pointer;
    border-radius: var(--radius-control);
    transition: background-color 120ms ease, color 120ms ease;
  }
  .dev-link:hover {
    background: var(--color-surface-rail);
    color: var(--color-text-primary);
  }

  /* Pin the composer to the bottom of the .wallet flex column — it's the
   * last flex child, so as long as it doesn't shrink, the activity slot
   * above absorbs all leftover height. */
  .wallet :global(.composer) {
    flex-shrink: 0;
  }

  @media (max-width: 640px) {
    .dev-toggles__row {
      justify-content: flex-start;
    }
  }
</style>

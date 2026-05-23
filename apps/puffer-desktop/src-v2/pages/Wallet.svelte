<!--
  Wallet — main wallet page (Paper artboard 1GM-0).

  Layout (inside Shell, 760 content column):
    Section title "Wallet" (.text-section serif 24 — PageHeader's display 28
    is too big here; render an inline <h1> instead).
    Balance block:
      AVAILABLE BALANCE eyebrow + huge serif balance number
      Cream "Top up crypto" chip pinned to the top-right of the row.
    3 KPI cards horizontal (Monthly spend / Pending holds / KYC status).
    Recent activity:
      "Recent activity" serif 24 + count chip
      ActivityRow stack (icon + title + meta on the left, amount + time right).
    Composer pinned at the bottom of the page column.

  Dev toggle: `#empty=1` swaps in WalletEmpty inline (route table is owned
  by another agent, so we read the hash directly).
-->
<script lang="ts">
  import { onMount } from "svelte";
  import { CheckCircle2 } from "lucide-svelte";

  import Composer from "../components/shell/Composer.svelte";
  import Chip from "../components/common/Chip.svelte";
  import KpiCard from "../components/wallet/KpiCard.svelte";
  import ActivityRow from "../components/wallet/ActivityRow.svelte";
  import WalletEmpty from "./WalletEmpty.svelte";
  import { wallet } from "../data/wallet";
  import { navigate } from "../router.svelte";
  import { pushToast } from "../lib/toast.svelte";

  // Dev-only empty-state preview. routes.ts is owned by another agent, so
  // we use a sessionStorage flag instead of adding a /wallet/empty route.
  const EMPTY_KEY = "puffer.dev.wallet.empty";

  let isEmpty = $state<boolean>(false);
  let balanceHidden = $state<boolean>(false);

  function readEmptyFlag(): void {
    if (typeof window === "undefined") return;
    try {
      isEmpty = window.sessionStorage.getItem(EMPTY_KEY) === "1";
    } catch {
      isEmpty = false;
    }
  }

  function writeEmptyFlag(value: boolean): void {
    if (typeof window === "undefined") return;
    try {
      if (value) window.sessionStorage.setItem(EMPTY_KEY, "1");
      else window.sessionStorage.removeItem(EMPTY_KEY);
    } catch {
      /* no-op */
    }
    isEmpty = value;
  }

  onMount(() => {
    readEmptyFlag();
  });

  function handleTopUp(): void {
    pushToast("Top up flow coming soon", "info");
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

  // Dev toggles
  function showEmpty(): void {
    writeEmptyFlag(true);
  }
  function showKyc(): void {
    writeEmptyFlag(false);
    navigate("/wallet/kyc");
  }
  function showFull(): void {
    writeEmptyFlag(false);
    navigate("/wallet");
  }

  let balanceDisplay = $derived(balanceHidden ? "●●●●●●" : wallet.balance);
</script>

<header class="wallet-header">
  <h1 class="text-section">Wallet</h1>
  <div class="dev-toggles" aria-label="Dev state toggles">
    <button type="button" class="dev-link" onclick={showEmpty}>Show empty</button>
    <button type="button" class="dev-link" onclick={showKyc}>Show KYC</button>
    <button type="button" class="dev-link" onclick={showFull}>Show full</button>
  </div>
</header>

{#if isEmpty}
  <WalletEmpty />
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
      <Chip label="Top up crypto" variant="cream" onclick={handleTopUp} />
    </div>
  </section>

  <section class="wallet-kpis">
    <button type="button" class="kpi-btn" onclick={() => tapKpi("Monthly spend")}>
      <KpiCard label="Monthly spend" value={wallet.monthlySpend} />
    </button>
    <button type="button" class="kpi-btn" onclick={() => tapKpi("Pending holds")}>
      <KpiCard label="Pending holds" value={wallet.pendingHolds} />
    </button>
    <button type="button" class="kpi-btn" onclick={() => tapKpi("KYC status")}>
      <KpiCard label="KYC status" value={wallet.kycStatus}>
        {#snippet trailing()}
          <CheckCircle2 size={16} strokeWidth={1.75} aria-hidden="true" />
        {/snippet}
      </KpiCard>
    </button>
  </section>

  <section class="wallet-activity">
    <header class="wallet-activity__header">
      <h2 class="text-section">Recent activity</h2>
      <!-- Count chip matches the design verbatim (artboard 1KJ-0 → "6"); -->
      <!-- the visible list shows the 3 most recent rows from data/wallet.ts. -->
      <Chip label="6" variant="count" />
    </header>

    <ul class="wallet-activity__list">
      {#each wallet.recentActivity as row (row.id)}
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
  </section>
{/if}

<Composer placeholder="Ask Alice to pay, order, or top up…" />

<style>
  .wallet-header {
    padding-bottom: var(--space-4);
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
  }

  .wallet-balance {
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

  .wallet-activity {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    flex: 1;
    padding-bottom: var(--space-5);
  }

  .wallet-activity__header {
    display: flex;
    align-items: center;
    gap: var(--space-3);
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
    align-items: center;
    gap: var(--space-2);
  }

  .dev-link {
    appearance: none;
    background: transparent;
    border: 0;
    padding: 2px 6px;
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
</style>

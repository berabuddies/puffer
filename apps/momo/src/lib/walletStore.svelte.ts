/**
 * Wallet page store (Svelte 5 runes).
 *
 * Owns the wallet snapshot — KYC status + the approved card's balance and
 * recent transactions — at *module* scope so it outlives the Wallet page
 * component. The page reads from here instead of holding its own `$state`,
 * which is what makes the "loading only the first time" behaviour possible.
 *
 * Stale-while-revalidate (mirrors creditStore.svelte.ts): only the cold
 * first load — when we have no snapshot yet (`kyc === null`) — flips `status`
 * to `loading`, so the page shows its placeholder. Every later visit renders
 * the previous snapshot immediately and `loadWallet()` refreshes it in the
 * background, leaving `status` at `ready`; re-opening /wallet therefore never
 * flashes "Loading…" again. A background-refresh failure keeps the last good
 * snapshot on screen and just hands the error back to the caller (the page
 * toasts it) — only a failed *first* load surfaces `status = "error"`.
 *
 * Surface:
 *   - `walletState` ($state) — `{ status, kyc, cardId, balance, txns }`.
 *   - `loadWallet()` — fetch once, folding the result into `walletState`;
 *     resolves to an error message string (for the toast) or `null`.
 */

import { walletClient } from "./walletClient.svelte";
import { describeBackendError } from "./backendFetch";
import { isCardLimitReachedError } from "./walletApi";
import type { KycStatus, Transaction } from "./walletTypes";

export type WalletStatus = "loading" | "ready" | "error";

export const walletState = $state<{
  status: WalletStatus;
  /** `null` until the first load resolves the real status — the cold-load flag. */
  kyc: KycStatus | null;
  cardId: number | null;
  balance: number;
  txns: Transaction[];
}>({ status: "loading", kyc: null, cardId: null, balance: 0, txns: [] });

function errMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/**
 * Fetch a card's balance + recent transactions and fold them into
 * `walletState`. Shared by `loadWallet` (approved-with-card path) and
 * `activateCard` (post-issue refresh) so the two stay in lockstep. Throws on
 * fetch failure — callers decide how to surface it.
 */
async function loadCardBalanceAndTxns(cardId: number): Promise<void> {
  const [b, t] = await Promise.all([
    walletClient.getBalance(cardId),
    walletClient.getTransactions(cardId, { limit: 10 })
  ]);
  walletState.balance = b.availableBalance;
  walletState.txns = t.transactions;
}

/**
 * Fetch the wallet snapshot once and fold it into `walletState`. Returns an
 * error message when the load failed (so the page can toast it), else `null`.
 *
 * Only the cold first load (`kyc === null`) shows the `loading` placeholder;
 * subsequent calls refresh in the background without regressing the snapshot.
 */
export async function loadWallet(): Promise<string | null> {
  const firstLoad = walletState.kyc === null;
  if (firstLoad) walletState.status = "loading";

  let kyc;
  try {
    kyc = await walletClient.getKycStatus();
  } catch (err) {
    // Cold load → surface the error placeholder; background refresh → keep the
    // last snapshot on screen. Either way, hand the message back to toast.
    if (firstLoad) walletState.status = "error";
    return errMessage(err);
  }
  walletState.kyc = kyc.status;

  if (kyc.status === "approved") {
    try {
      const cards = await walletClient.getCardList();
      const first = cards[0];
      if (first) {
        walletState.cardId = first.cardId;
        await loadCardBalanceAndTxns(first.cardId);
      } else {
        // Approved but no card yet — treat the wallet as $0 + empty list.
        walletState.cardId = null;
        walletState.balance = 0;
        walletState.txns = [];
      }
    } catch (err) {
      // KYC is known; only the card snapshot failed. Mark ready (we have a
      // status to render) and let the page toast the card-fetch error.
      walletState.status = "ready";
      return errMessage(err);
    }
  } else {
    // In Review / Declined / etc. — show the wallet shell with $0 + no txns.
    walletState.cardId = null;
    walletState.balance = 0;
    walletState.txns = [];
  }

  walletState.status = "ready";
  return null;
}

/**
 * Issue (activate) the user's card, then refresh the snapshot so the new
 * card's balance + transactions fold into `walletState` and the page drops the
 * activate view. Returns an error message when issuing failed (for the toast),
 * else `null`.
 *
 * This does NOT itself de-dupe concurrent calls — the page disables the button
 * while it's pending. The backend is idempotent and 3-card-capped regardless,
 * so a stray double-fire can never over-issue.
 */
export async function activateCard(): Promise<string | null> {
  let issued;
  try {
    issued = await walletClient.issueCard();
  } catch (err) {
    // Map to friendly copy: the 3-card cap surfaces as a backend code-1000
    // BackendError whose raw field text we don't want to show verbatim.
    if (isCardLimitReachedError(err)) {
      return "Wallet activation limit reached.";
    }
    return describeBackendError(err, errMessage(err));
  }
  // The card now exists on the backend. Reflect it from the AUTHORITATIVE issue
  // result before the balance fetch — so a transient balance/txn failure (or a
  // read-after-write lag in a full getCardList reload) can't strand the page on
  // the activate prompt with a card that was actually created. (`needsCard`
  // keys on `cardId`.) We also skip the redundant getKycStatus/getCardList that
  // a full loadWallet() would re-run — approval is a precondition of this call.
  walletState.cardId = issued.cardId;
  try {
    await loadCardBalanceAndTxns(issued.cardId);
  } catch (err) {
    walletState.status = "ready";
    return errMessage(err);
  }
  walletState.status = "ready";
  return null;
}

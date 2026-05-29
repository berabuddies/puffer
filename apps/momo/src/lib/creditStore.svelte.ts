/**
 * Credit balance store (Svelte 5 runes).
 *
 * Owns the WorldRouter billing balance shown in the sidebar Credits pill.
 * The balance is fetched from control-api's billing-account endpoint using
 * the worldrouter session resolved by `ensureWrSession` (auth.svelte.ts).
 *
 * Surface:
 *   - `creditState` ($state) — `{ creditsUsd, status }`.
 *   - `loadCredits()` — fetch once, updating `creditState`.
 */

import { ensureWrSession, type WrSession } from "./auth.svelte";
import { fetchBillingAccount, BillingError } from "./billingApi";

const CONTROL_API_URL =
  (import.meta.env.VITE_WORLDROUTER_CONTROL_URL as string | undefined) ??
  "https://control-api.worldrouter.ai";

export type CreditStatus = "idle" | "loading" | "ready" | "error";

export const creditState = $state<{
  creditsUsd: number | null;
  status: CreditStatus;
}>({ creditsUsd: null, status: "idle" });

/** Fetch the balance for a resolved session, returning the USD figure. */
async function fetchBalance(session: WrSession): Promise<number> {
  const data = await fetchBillingAccount(
    CONTROL_API_URL,
    session.teamId,
    session.sessionToken
  );
  return data.credit_balance_usd;
}

/**
 * Fetch the billing balance once and fold the result into `creditState`.
 * Never throws: any failure lands as `status = "error"` (the pill renders
 * `—`) so a credit hiccup never breaks the rest of the rail. A 401/403 —
 * typically an expired session token — triggers a single forced
 * re-exchange + retry before giving up.
 */
export async function loadCredits(): Promise<void> {
  creditState.status = "loading";
  const session = await ensureWrSession();
  if (!session) {
    creditState.status = "error";
    return;
  }
  try {
    creditState.creditsUsd = await fetchBalance(session);
    creditState.status = "ready";
    return;
  } catch (err) {
    const expired =
      err instanceof BillingError &&
      (err.status === 401 || err.status === 403);
    if (expired) {
      const fresh = await ensureWrSession({ forceRefresh: true });
      if (fresh) {
        try {
          creditState.creditsUsd = await fetchBalance(fresh);
          creditState.status = "ready";
          return;
        } catch (retryErr) {
          // eslint-disable-next-line no-console
          console.warn("[credits] retry after 401 failed", retryErr);
        }
      }
    } else {
      // eslint-disable-next-line no-console
      console.warn("[credits] failed to load billing balance", err);
    }
    creditState.status = "error";
  }
}

/* ─── Polling lifecycle ───────────────────────────────────────────────
 * The sidebar owns the polling window: startCreditPolling() while signed
 * in, stopCreditPolling() on teardown / sign-out.
 */

let pollTimer: ReturnType<typeof setTimeout> | null = null;
let focusHandler: (() => void) | null = null;

/** Schedule the next poll. Mirrors worldagent: 30s normally, 5s while the
 *  balance is negative (fast recovery from overdraft). */
function scheduleNext(): void {
  if (pollTimer) clearTimeout(pollTimer);
  const interval = (creditState.creditsUsd ?? 0) < 0 ? 5_000 : 30_000;
  pollTimer = setTimeout(() => {
    void loadCredits().then(scheduleNext);
  }, interval);
}

/**
 * Begin polling the billing balance: load immediately, then on a 30s/5s
 * cadence, plus a refetch whenever the window regains focus. Idempotent —
 * a fresh start resets state and cancels any prior timer/listener.
 */
export function startCreditPolling(): void {
  stopCreditPolling();
  creditState.creditsUsd = null;
  creditState.status = "idle";
  void loadCredits().then(scheduleNext);
  if (typeof window !== "undefined") {
    focusHandler = () => {
      void loadCredits();
    };
    window.addEventListener("focus", focusHandler);
  }
}

/** Stop polling and detach the focus listener. */
export function stopCreditPolling(): void {
  if (pollTimer) {
    clearTimeout(pollTimer);
    pollTimer = null;
  }
  if (focusHandler && typeof window !== "undefined") {
    window.removeEventListener("focus", focusHandler);
    focusHandler = null;
  }
}

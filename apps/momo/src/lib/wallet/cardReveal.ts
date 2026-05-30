/**
 * cardReveal — the "fetch a card's plaintext PAN + CVV at the moment of use,
 * hand it to one consumer, then drop it" primitive.
 *
 * This is the *capability* half of the chat-time card handoff. It deliberately
 * does NOT know where the card is going (chat message, a WorldRouter payment
 * sink, a tool argument) — that injection point is still undecided. What it
 * guarantees is the part that must be right regardless of the sink:
 *
 *   1. **Live, never cached.** Every reveal hits the backend through the
 *      injected `CardSource`. The CVV in particular is meant to be read fresh
 *      each time — we never stash it in a variable that outlives the call.
 *   2. **Scoped lifetime.** `revealCardForUse` fetches the card, passes it to a
 *      single `use` callback, and best-effort scrubs its own reference in a
 *      `finally`. The card object's lifetime is the callback's lifetime; once
 *      `use` resolves the reference held here is gone.
 *   3. **No ambient storage.** Nothing in this module retains card data between
 *      calls — there is no module-level cache, no `$state`, no window handle.
 *
 * ── Why dependency injection (CardSource) ──────────────────────────────────
 * The production card client (`walletClient` in walletClient.svelte.ts) is a
 * Svelte-runes module that can't load in a plain-node test runner. This file
 * stays runes-free and takes its data source as a parameter, so the lifetime /
 * no-cache guarantees can be unit-tested with a fake source (see
 * tests/cardReveal.spec.ts). Production callers pass `walletCardSource()` from
 * cardReveal.svelte.ts, which adapts the real walletClient.
 */

import type { CardDetails } from "../walletTypes";

/**
 * The minimal slice of a wallet client this module needs: fetch the live,
 * plaintext details for one card. Implemented in production by walletClient
 * (REST or mock) and in tests by a fake.
 */
export interface CardSource {
  getCardDetails(cardId: number): Promise<CardDetails>;
}

/**
 * A reveal request: which card, and (for diagnostics / future sink routing)
 * an optional human-readable reason the card is being revealed. The reason is
 * never sent anywhere sensitive — it's for logging the *fact* of a reveal, not
 * the card.
 */
export interface RevealRequest {
  cardId: number;
  /** Free-text label for why the reveal happened, e.g. "book-by-phone:task_42". */
  reason?: string;
}

/**
 * Fetch a card's live PAN + CVV and hand it to exactly one consumer, then drop
 * our reference to it.
 *
 * The card object is valid only for the duration of `use`. `use` should do its
 * work (POST to a sink, build a one-shot payload, …) and return; it must not
 * stash the card for later. Whatever `use` returns is passed through as the
 * result — make that a *non-sensitive* summary (e.g. `{ last4 }`), never the
 * card itself.
 *
 * Errors from the fetch or from `use` propagate to the caller unchanged; the
 * `finally` scrub still runs. Returns whatever `use` resolves to.
 */
export async function revealCardForUse<T>(
  req: RevealRequest,
  source: CardSource,
  use: (card: CardDetails) => T | Promise<T>
): Promise<T> {
  let card: CardDetails | null = null;
  try {
    card = await source.getCardDetails(req.cardId);
    return await use(card);
  } finally {
    // Best-effort scrub: overwrite the fields we held and drop the reference.
    // JS can't guarantee the GC zeroes the backing store, but this ensures THIS
    // module retains nothing readable after the call, and makes the intent
    // ("the card does not outlive the use") explicit and testable.
    if (card) {
      card.cardNumber = "";
      card.cvv = "";
      card.expMonth = "";
      card.expYear = "";
      card = null;
    }
  }
}

/** Last 4 digits of a PAN, for the non-sensitive summary a consumer returns to
 *  the agent / UI in place of the real number. Returns "" for a too-short PAN. */
export function last4(cardNumber: string): string {
  const digits = cardNumber.replace(/\D/g, "");
  return digits.length >= 4 ? digits.slice(-4) : "";
}

/**
 * Convenience: reveal a card and return ONLY its non-sensitive summary. This is
 * the shape most call sites want — proof a live card exists / was used, with no
 * PAN or CVV escaping. The full card never leaves `revealCardForUse`'s scope.
 */
export interface CardSummary {
  last4: string;
  /** MM/YY, composed from the live expMonth/expYear. */
  expiry: string;
}

export async function revealCardSummary(
  req: RevealRequest,
  source: CardSource
): Promise<CardSummary> {
  return revealCardForUse(req, source, (card) => ({
    last4: last4(card.cardNumber),
    expiry: card.expMonth && card.expYear ? `${card.expMonth}/${card.expYear}` : ""
  }));
}

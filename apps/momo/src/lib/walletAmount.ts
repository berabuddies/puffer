/**
 * Parse a ucard-backend transaction amount string into major-unit dollars.
 *
 * Why this is not a plain `parseInt(s) / 100` (the original assumption):
 *   The backend's GET /api/card/transactions serves rows from TWO paths with
 *   DIFFERENT amount encodings (verified against ucard-backend
 *   api/router/cards/handler.go):
 *     • broker / StradaCarte (the PRIMARY path) passes the processor amount
 *       through verbatim — a major-unit USD string WITH a decimal point,
 *       e.g. "0.99", "1.00", "2418.32".  (handler.go convertBrokerTransaction:
 *       `tx.Amount = *brokerTx.Amount`)
 *     • the local DB fallback formats integer cents via strconv.FormatInt —
 *       a plain integer string, e.g. "99", "100".  (handler.go
 *       fetchTransactionsFromDB: `strconv.FormatInt(dbTx.Amount, 10)`)
 *
 *   The old `parseInt(raw.amount, 10) / 100` only handled the cents form, so
 *   every Strada row ("0.99") parsed to `0` → transactions showed "$0.00".
 *
 * We distinguish the two by the decimal point: Strada always emits a fixed
 * 2-dp string (so it always contains "."), FormatInt never emits one. There is
 * no ambiguous overlap — "1.00" (Strada dollars) and "100" (DB cents) both map
 * to 1.00 — so this is exact for both paths.
 *
 * Returns 0 for empty / unparseable input (matches the previous `|| 0`).
 */
export function parseTransactionAmount(raw: string | undefined | null): number {
  const s = (raw ?? "").trim();
  if (!s) return 0;
  if (s.includes(".")) {
    // Already major-unit dollars (Strada / broker path).
    const dollars = Number.parseFloat(s);
    return Number.isFinite(dollars) ? dollars : 0;
  }
  // Integer cents (DB fallback path).
  const cents = Number.parseInt(s, 10);
  return Number.isFinite(cents) ? cents / 100 : 0;
}

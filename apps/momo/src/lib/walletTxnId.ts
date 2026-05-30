/**
 * Stable, guaranteed-unique key for a wallet transaction row.
 *
 * Why this exists (regression guard):
 *   The ucard-backend returns `id: 0` for EVERY transaction — the real
 *   per-transaction reference is `authRefNum`. The previous normalizer used
 *   `String(raw.id ?? raw.authRefNum ?? …)`, and because `??` only falls
 *   through on null/undefined, `0 ?? authRefNum` evaluates to `0`. So every
 *   transaction collapsed to the key `"0"`. The wallet's keyed `{#each … (row.id)}`
 *   then saw duplicate keys and Svelte 5 threw `each_key_duplicate` while
 *   rendering the wallet shell — which aborts the loading→shell transition and
 *   leaves the page stuck on "Loading…" (only reproducible against the real
 *   backend with ≥2 transactions; the mock client mints unique ids so tests
 *   never hit it).
 *
 * The fix: prefer `authRefNum` (the meaningful reference) but ALWAYS suffix the
 * list index, so the key is unique even when two rows share an authRefNum
 * (an authorization and its later settlement / reversal legitimately can) or
 * when both id and authRefNum are missing. `index` is unique within the
 * rendered list and stable across re-renders (the stored array order is fixed),
 * which is exactly what a keyed `{#each}` needs.
 *
 * Kept in its own dependency-free module so it can be unit-tested without
 * pulling in the backendFetch → ucardSession → auth.svelte (Svelte runes)
 * import chain, which can't load outside a Svelte/browser context.
 */
export function transactionKey(
  raw: { id?: number; authRefNum?: string },
  index: number,
): string {
  const ref = raw.authRefNum?.trim() || (raw.id ? String(raw.id) : "");
  return ref ? `${ref}-${index}` : `txn-${index}`;
}

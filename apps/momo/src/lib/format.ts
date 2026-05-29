/**
 * Display formatting helpers.
 *
 * `formatCredits` mirrors worldagent-frontend's `lib/format.ts`: the
 * WorldRouter billing balance is stored in USD, but the product surfaces
 * it as "Credits" where 1 USD = 100 Credits. No currency symbol — the
 * literal word " Credits" is the unit.
 */

const creditsFormatter = new Intl.NumberFormat("en-US", {
  minimumFractionDigits: 0,
  maximumFractionDigits: 2
});

/** Convert a USD amount to its "Credits" magnitude (1 USD = 100 Credits). */
export function usdToCredits(usd: number): number {
  return usd * 100;
}

/**
 * Format a USD balance as a "Credits" string, e.g. `12.34` → `"1,234
 * Credits"`. A tiny but non-zero balance collapses to `"< 0.01 Credits"`
 * so it never renders as a misleading `"0 Credits"`.
 */
export function formatCredits(usd: number): string {
  const credits = usdToCredits(usd);
  if (credits > 0 && credits < 0.01) return "< 0.01 Credits";
  return `${creditsFormatter.format(credits)} Credits`;
}

/**
 * Thin client for the WorldRouter control-api billing endpoint.
 *
 * Mirrors worldagent-frontend's `lib/wr-api.client.ts` / `wr-types.ts`,
 * trimmed to the single read momo needs: the team billing account, whose
 * `credit_balance_usd` drives the sidebar Credits pill.
 *
 *   GET {controlUrl}/platform/v1/teams/{teamId}/billing-account
 *   Authorization: Bearer <session_token>
 *
 * Unlike `backendFetch` (ucard-backend, `{code,message,data}` envelope),
 * control-api returns the JSON object directly.
 */

/** Subset of worldagent's `BillingAccount` — only the fields we surface. */
export interface BillingAccount {
  available_balance_usd: number;
  spend_usd: number;
  fund_usd: number;
}

export interface TeamBillingAccountResponse {
  team_id: string;
  billing_account?: BillingAccount;
  /** Available balance in USD. Displayed as USD × 100 "Credits". */
  credit_balance_usd: number;
}

/** HTTP-level failure carrying the status so callers can react to 401/403. */
export class BillingError extends Error {
  readonly status: number;
  constructor(status: number, message?: string) {
    super(message || `billing-account failed: HTTP ${status}`);
    this.name = "BillingError";
    this.status = status;
  }
}

/**
 * Fetch the team's billing account. Throws `BillingError` on any non-2xx
 * (status preserved) and a plain `Error` on network / JSON failure.
 */
export async function fetchBillingAccount(
  controlUrl: string,
  teamId: string,
  sessionToken: string
): Promise<TeamBillingAccountResponse> {
  const url = `${controlUrl.replace(/\/$/, "")}/platform/v1/teams/${encodeURIComponent(
    teamId
  )}/billing-account`;

  let res: Response;
  try {
    res = await fetch(url, {
      headers: { Authorization: `Bearer ${sessionToken}` }
    });
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err);
    throw new Error(`Network error: ${msg}`);
  }

  if (!res.ok) throw new BillingError(res.status);
  return (await res.json()) as TeamBillingAccountResponse;
}

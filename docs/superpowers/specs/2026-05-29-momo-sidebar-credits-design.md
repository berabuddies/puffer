# momo Sidebar — Real Credits Balance

**Date:** 2026-05-29 · **Owner:** sean · **App:** `apps/momo`

## Goal

Replace the hard-coded `currentUser.credits` mock (1,000,000) in the
sidebar's bottom-left Credits pill with the user's real WorldRouter
account balance, loaded the same way `worldagent-frontend` does.

## Confirmed decisions

| Topic | Decision |
|---|---|
| Data source | WorldRouter billing-account `credit_balance_usd` (LLM usage balance, **not** the ucard wallet balance shown on the Wallet page) |
| Display format | `credit_balance_usd × 100` formatted `en-US` (≤2 decimals) + `" Credits"`, no `$`. 1 USD = 100 Credits. Identical to worldagent. |
| Refresh | Load after sign-in; poll every 30s (5s while balance `< 0`); refetch on window focus. |
| Click | Pill/badge opens the external top-up page in the system browser. |

## Data flow (key insight)

Querying billing needs `session_token` + `team_id`. momo **already obtains
both** in `mintWorldRouterApiKey`'s first hop (`POST /auth/exchange`) — it
just discards them. We reuse that exchange instead of adding new login code.

```
JWT ──POST {CONTROL_API}/auth/exchange──▶ { session_token, default_team_id }
        GET {CONTROL_API}/platform/v1/teams/{team_id}/billing-account
                          (Authorization: Bearer session_token)
                              ▼
                  credit_balance_usd (USD float)  ──×100──▶  "1,234 Credits"
```

`CONTROL_API` = `VITE_WORLDROUTER_CONTROL_URL ?? https://control-api.worldrouter.ai`.

## Changes (5 units)

1. **`lib/auth.svelte.ts` — reuse the exchange.** Extract hop-1 into an
   internal helper; cache its result (`session_token` + `team_id` + owner
   `sub`) in a **module-level in-memory var** (not localStorage — more
   sensitive than the sk- key, and we poll anyway so a re-exchange after
   app restart is cheap). Export `ensureWrSession(): Promise<{ sessionToken;
   teamId } | null>` (returns cache if owner matches current user, else
   re-exchanges with current JWT). `mintWorldRouterApiKey` reuses the
   extracted exchange. `signOut()` clears the cache.

2. **`lib/billingApi.ts` (new) — thin HTTP layer.** Types `BillingAccount` /
   `TeamBillingAccountResponse` (mirror worldagent `wr-types.ts`, only used
   fields). `fetchBillingAccount(controlUrl, teamId, sessionToken)` →
   `GET .../billing-account` with Bearer header. Direct call to control-api
   (same host as existing mint; verified reachable per momo CLAUDE.md).

3. **`lib/creditStore.svelte.ts` (new) — state + polling.**
   `creditState = $state<{ creditsUsd: number | null; status:
   "idle"|"loading"|"ready"|"error" }>`. `loadCredits()` →
   `ensureWrSession()` → `fetchBillingAccount` → store `credit_balance_usd`;
   on 401/403 re-exchange once via JWT and retry (no assumption about
   session_token TTL). `startCreditPolling()` / `stopCreditPolling()`:
   30s interval (5s when `creditsUsd < 0`), `window` focus refetch; reset
   then load on start.

4. **`lib/format.ts` (new) — `formatCredits(usd)`.** `usd * 100` via
   `Intl.NumberFormat("en-US", { maximumFractionDigits: 2 })` + `" Credits"`.

5. **`components/shell/Sidebar.svelte` — wire real data + click.**
   `$effect` on `authState.status === "signedIn"` → `startCreditPolling()`,
   cleanup → `stopCreditPolling()`. Expanded pill + collapsed badge show
   `formatCredits(creditsUsd)` when `status === "ready"`, else `—`. Both
   become `<button>` that calls `openUrl(${VITE_WR_DASHBOARD_URL ??
   "https://www.worldrouter.ai"}/dashboard/credits)` via
   `@tauri-apps/plugin-opener` (system browser, like `goToLogin`). Drop the
   `currentUser.credits` reference (leave the mock field in `user.ts`).

**New env:** `VITE_WR_DASHBOARD_URL` (default `https://www.worldrouter.ai`),
documented in `.env.example`.

## Failure handling

Any billing failure (CORS / network / 401) → `status = "error"` → render
`—` + `console.warn`; never blocks other features. Next poll self-heals.
Mirrors worldagent's degraded behavior.

## Assumptions / risks

- Direct browser-side fetch to control-api works for `GET billing-account`
  (the POST exchange/mint already do, same host + Bearer). If CORS blocks
  it, fall back to a Tauri Rust-side proxy request.
- `session_token` TTL unknown → handled by re-exchange-on-401.

## Verification

`npm run check` (types) · manual: sign in → real number appears bottom-left
→ click opens the top-up page. No Rust changes.

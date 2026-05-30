/**
 * REST WalletClient — hits the ucard-backend at /api/card/* and adapts
 * its raw shapes to the shared wallet contract in `./walletTypes.ts`.
 *
 * Ported from worldclaw-app/lib/card-api.ts. Out-of-scope endpoints
 * (closeCard / bindAddress / getUserInfo / getDeposits) are intentionally
 * not implemented here — the desktop surface needs the read-side + KYC entry
 * points, issueCard (the Activate-card button), and getCardDetails for the
 * chat-time card-number + CVV handoff.
 */

import { backendFetch, BackendError } from './backendFetch';
import { transactionKey } from './walletTxnId';
import { parseTransactionAmount } from './walletAmount';
import type {
  CardBalance,
  CardDetails,
  CardListEntry,
  CreateCardHolderParams,
  IssueCardResult,
  DepositAddress,
  KycStatus,
  KycStatusResponse,
  StartKycResult,
  Transaction,
  WalletClient,
} from './walletTypes';

export { BackendError };

// ---------------------------------------------------------------------------
// Backend KYC status code → KycStatus
// ---------------------------------------------------------------------------
//
// Mirrors Strada's lifecycle. Verified against ucard-backend
// 2026-04-29 (see worldclaw-app/lib/card-api.ts lines 172-182).

type BackendKycStatusCode = 0 | 1 | 2 | 3 | 4 | 6 | 7;

const STATUS_MAP: Record<BackendKycStatusCode, KycStatus> = {
  0: 'pending',
  1: 'approved',
  2: 'declined',
  3: 'in_review',
  4: 'draft',
  6: 'error',
  7: 'admin_decline',
};

// ---------------------------------------------------------------------------
// Transaction normalizer
// ---------------------------------------------------------------------------

interface RawBackendTransaction {
  id?: number;
  cardId?: number;
  authRefNum?: string;
  amount?: string;
  description?: string;
  dateCreated?: string;
  dateSettled?: string;
  transType?: string;
  transStatus?: string;
  merchantName?: string;
  merchantCurrency?: string;
  merchantAmount?: string;
}

// Strada's transStatus strings we expect to see. "Completion" is the
// common happy path. Anything unknown becomes pending so an
// unrecognized value never silently disappears from the UI.
function mapTransStatus(raw: string | undefined): Transaction['status'] {
  const s = (raw ?? '').toLowerCase();
  if (s === 'completion' || s === 'completed' || s === 'captured') return 'settled';
  if (s === 'declined' || s === 'reversal' || s === 'reversed') return 'declined';
  return 'pending';
}

// Strada transType strings. "Refund" and "Credit" credit the card; every
// other type (Authorization / Purchase / Capture / etc.) is a debit.
// Treat unknowns as debit so a positive amount never renders as income.
function mapTransType(raw: string | undefined): Transaction['type'] {
  const t = (raw ?? '').toLowerCase();
  return t === 'refund' || t === 'credit' ? 'credit' : 'debit';
}

function normalizeTransaction(raw: RawBackendTransaction, index: number): Transaction {
  // Amounts arrive in two encodings depending on the backend path that served
  // the row: Strada/broker (primary) passes a major-unit USD string ("0.99")
  // while the DB fallback formats integer cents ("99"). parseTransactionAmount
  // normalizes both to major-unit dollars — see walletAmount.ts. (The old
  // `parseInt(amount) / 100` turned every Strada row into $0.00.)
  const amount = parseTransactionAmount(raw.amount);
  const merchantAmount = parseTransactionAmount(raw.merchantAmount);

  const status = mapTransStatus(raw.transStatus);
  const type = mapTransType(raw.transType);

  // We track merchantCurrency internally (handy for diagnostics) but
  // the shared `Transaction` contract drops it — see file footer note.
  let merchantCurrency = raw.merchantCurrency || 'USD';
  if (merchantCurrency === '840') merchantCurrency = 'USD';
  else if (merchantCurrency.startsWith('392')) merchantCurrency = 'JPY';

  void merchantCurrency;

  return {
    // The backend returns `id: 0` for every row — keying the UI list off it
    // collapses every transaction to "0" and crashes Svelte's keyed `{#each}`
    // (each_key_duplicate), stranding the wallet on "Loading…". Derive a
    // unique, stable key from authRefNum + index instead. See walletTxnId.ts.
    id: transactionKey(raw, index),
    date: raw.dateCreated || raw.dateSettled || '',
    description: (raw.description || '').trim(),
    amount,
    merchantAmount,
    type,
    status,
    // Strada returns merchantName padded with trailing spaces.
    merchantName: (raw.merchantName || '').trim(),
  };
}

// ---------------------------------------------------------------------------
// RestWalletClient
// ---------------------------------------------------------------------------

export class RestWalletClient implements WalletClient {
  async startKyc(params: CreateCardHolderParams): Promise<StartKycResult> {
    // ucard-backend's KycStart chains CreateCardHolder + SubmitForReview
    // + eKycUrl and returns the URL in a single response. An empty
    // `ekycUrl` should be treated as an error by the caller (didit.me
    // is supposed to always return one).
    const data = await backendFetch<{ cardHolderId: number; ekycUrl: string }>(
      '/card/kyc/start',
      { method: 'POST', body: params },
    );
    return {
      cardHolderId: data.cardHolderId,
      ekycUrl: data.ekycUrl,
    };
  }

  async getKycStatus(): Promise<KycStatusResponse> {
    let data: {
      status: BackendKycStatusCode;
      statusText?: string;
      message?: string;
      ekycUrl?: string;
      cardId?: number;
      maskedCardNumber?: string;
    };
    try {
      data = await backendFetch('/card/kyc/status');
    } catch (err) {
      // No CardHolder yet → the backend returns ResourceNotFound (1001).
      // That's the "never started KYC" state, not a failure: surface it as
      // `none` (matching mockWalletClient) so the UI shows the KYC intro
      // instead of an error toast. Mirrors worldclaw-app/app/card.tsx's
      // CODE_RESOURCE_NOT_FOUND handling.
      if (err instanceof BackendError && err.code === 1001) {
        return { status: 'none', message: null };
      }
      throw err;
    }

    return {
      status: STATUS_MAP[data.status] ?? 'error',
      message: data.message || null,
      ekycUrl: data.ekycUrl || undefined,
      cardId: data.cardId,
      maskedCardNumber: data.maskedCardNumber,
    };
  }

  async getCardList(): Promise<CardListEntry[]> {
    const data = await backendFetch<{
      userId: number;
      totalCards: number;
      cards?: Array<{
        id: string | number;
        maskedCardNumber: string;
        status: string;
        nameOnCard: string;
        createdAt: string;
      }>;
    }>('/card/list');
    // Wire format uses `id`; shared contract uses `cardId`. Map here.
    return (data.cards ?? []).map((c) => ({
      cardId: typeof c.id === 'number' ? c.id : parseInt(c.id, 10),
      maskedCardNumber: c.maskedCardNumber,
      status: c.status,
    }));
  }

  async issueCard(): Promise<IssueCardResult> {
    // POST /card/issue — create + activate a virtual card. Idempotent on the
    // backend (returns the existing active card if one exists) and capped at
    // 3 cards/user, enforced under a distributed lock. We send an empty body;
    // nameOnCard / alias are optional and the backend derives the name from
    // the cardholder record. A "card limit reached" rejection comes back as
    // BackendError(1000) — see isCardLimitReachedError below.
    const data = await backendFetch<{ cardId: number; maskedCardNumber: string }>(
      '/card/issue',
      { method: 'POST', body: {} },
    );
    return {
      cardId: data.cardId,
      maskedCardNumber: data.maskedCardNumber,
    };
  }

  async getCardDetails(cardId: number): Promise<CardDetails> {
    // Sensitive: returns the plaintext PAN + CVV. The endpoint is plural
    // (`/card/details`); the backend splits expiry into expMonth / expYear
    // (ucard-backend jsonmodel.go → ViewCardDetails). We deliberately do NOT
    // cache the result — every call hits the backend so the chat-time handoff
    // always sends a live CVV.
    const data = await backendFetch<{
      cardNumber?: string;
      expMonth?: string;
      expYear?: string;
      cvv?: string;
    }>('/card/details', { query: { cardId } });
    return {
      cardNumber: data.cardNumber ?? '',
      expMonth: data.expMonth ?? '',
      expYear: data.expYear ?? '',
      cvv: data.cvv ?? '',
    };
  }

  async getBalance(cardId: number): Promise<CardBalance> {
    const data = await backendFetch<{
      cardId: number;
      availableBalance: number;
      ledgerBalance?: number;
      currency: string;
    }>('/card/balance', { query: { cardId } });
    return {
      availableBalance: data.availableBalance / 100,
      currency: data.currency === '840' ? 'USD' : data.currency,
    };
  }

  async getTransactions(
    cardId: number,
    opts?: { offset?: number; limit?: number },
  ): Promise<{ transactions: Transaction[] }> {
    const data = await backendFetch<{
      transactions?: RawBackendTransaction[];
    }>('/card/transactions', {
      query: {
        cardId,
        offset: opts?.offset,
        limit: opts?.limit,
      },
    });
    const list = data.transactions ?? [];
    return { transactions: list.map(normalizeTransaction) };
  }

  async getDepositAddress(): Promise<{ addresses: DepositAddress[] }> {
    const data = await backendFetch<{ addresses: DepositAddress[] }>(
      '/card/deposit-address',
    );
    return { addresses: data.addresses ?? [] };
  }
}

/**
 * Recognise the "card limit reached" rejection from POST /card/issue — the
 * backend caps users at 3 cards (BackendError code 1000, field "card limit
 * reached: maximum 3 cards per user"). The Activate-card flow uses this to
 * show a friendly message instead of the raw backend text.
 */
export function isCardLimitReachedError(err: unknown): boolean {
  return (
    err instanceof BackendError &&
    err.code === 1000 &&
    err.field.includes('card limit reached')
  );
}

// ---------------------------------------------------------------------------
// Singleton export
// ---------------------------------------------------------------------------

export const walletApi: WalletClient = new RestWalletClient();

// ---------------------------------------------------------------------------
// Contract note
// ---------------------------------------------------------------------------
//
// The donor's `Transaction` type includes a `merchantCurrency: string`
// field. The shared contract in `walletTypes.ts` drops it. We follow
// the shared contract; if the UI ever needs the original currency the
// field can be added back to both this normalizer and the contract.

/**
 * REST WalletClient — hits the ucard-backend at /api/card/* and adapts
 * its raw shapes to the shared wallet contract in `./walletTypes.ts`.
 *
 * Ported from worldclaw-app/lib/card-api.ts. Out-of-scope endpoints
 * (createCard / closeCard / getCardDetails / bindAddress / getUserInfo /
 * getDeposits) are intentionally not implemented here — the desktop
 * surface only needs the read-side + KYC entry points.
 */

import { backendFetch, BackendError } from './backendFetch';
import type {
  CardBalance,
  CardListEntry,
  CreateCardHolderParams,
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

function normalizeTransaction(raw: RawBackendTransaction): Transaction {
  // Backend returns amount / merchantAmount as integer cents in string
  // form. UI works in major-unit dollars / yen / etc.
  const amountCents = parseInt(raw.amount ?? '0', 10) || 0;
  const merchantCents = parseInt(raw.merchantAmount ?? '0', 10) || 0;

  const status = mapTransStatus(raw.transStatus);
  const type = mapTransType(raw.transType);

  // We track merchantCurrency internally (handy for diagnostics) but
  // the shared `Transaction` contract drops it — see file footer note.
  let merchantCurrency = raw.merchantCurrency || 'USD';
  if (merchantCurrency === '840') merchantCurrency = 'USD';
  else if (merchantCurrency.startsWith('392')) merchantCurrency = 'JPY';

  void merchantCurrency;

  return {
    id: String(raw.id ?? raw.authRefNum ?? Math.random()),
    date: raw.dateCreated || raw.dateSettled || '',
    description: (raw.description || '').trim(),
    amount: amountCents / 100,
    merchantAmount: merchantCents / 100,
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
    const data = await backendFetch<{
      status: BackendKycStatusCode;
      statusText?: string;
      message?: string;
      ekycUrl?: string;
      cardId?: number;
      maskedCardNumber?: string;
    }>('/card/kyc/status');

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
 * Recognise the "card limit reached" rejection from POST /card/issue.
 * Kept here for parity with the donor even though the desktop UI does
 * not call /card/issue today — leave callers a single place to import
 * from when the time comes.
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

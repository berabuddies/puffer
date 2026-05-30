/**
 * Shared wallet types — the contract between the UI layer and any
 * WalletClient implementation (REST, mock, or future variants).
 *
 * Owned jointly with the UI agent: this file is written idempotently to
 * the exact contract negotiated for the U-card port. If the UI agent
 * overwrites it later with the same content, no diff. If you need to
 * change the shape, coordinate with the UI agent first.
 */

export type KycStatus =
  | 'none'
  | 'draft'
  | 'pending'
  | 'in_review'
  | 'approved'
  | 'declined'
  | 'error'
  | 'admin_decline';

export interface CreateCardHolderParams {
  firstName: string;
  lastName: string;
  dob: string;
  email: string;
  callingCode: string;
  countryCallingCode: string;
  phoneNum: string;
  cellNum: string;
  address: string;
  city: string;
  state: string;
  country: string;
  zipCode: string;
}

export interface StartKycResult {
  cardHolderId: number;
  ekycUrl: string;
}

export interface KycStatusResponse {
  status: KycStatus;
  message: string | null;
  ekycUrl?: string;
  cardId?: number;
  maskedCardNumber?: string;
}

export interface CardBalance {
  availableBalance: number;
  currency: string;
}

export interface Transaction {
  id: string;
  date: string;
  description: string;
  amount: number;
  merchantAmount: number;
  type: 'credit' | 'debit';
  status: 'success' | 'settled' | 'declined' | 'pending';
  merchantName: string;
}

export interface DepositAddress {
  chainId: number;
  chainName: string;
  assetSymbol: string;
  depositAddress: string;
}

export interface CardListEntry {
  cardId: number;
  maskedCardNumber: string;
  status: string;
}

/**
 * Parameters for `POST /card/deposit-intent`. `chainId` is fixed at 56 (BNB
 * Smart Chain) and `assetSymbol` at "USD1" for now. `cardId` is optional —
 * omitting it lets the backend pick the user's first depositable card.
 * `expectedAmount` is the amount the user intends to send (optional but
 * recommended); the backend echoes back the enforced `minimumAmount` either
 * way.
 */
export interface CreateDepositIntentParams {
  chainId: number;
  assetSymbol: string;
  cardId?: number;
  expectedAmount?: string;
}

/**
 * Result of `POST /card/deposit-intent`. `address` is the address the user
 * must send funds to — surface THIS one in the UI. `addressOut` is the
 * gateway's final forwarding address (debug only — never present it as the
 * pay-to). `minimumAmount` is the backend-enforced floor the UI must respect.
 */
export interface DepositIntent {
  intentId: number;
  cardId: number;
  chainId: number;
  assetSymbol: string;
  address: string;
  addressOut: string;
  minimumAmount: string;
  callbackUrl: string;
  status: string;
}

/**
 * On-chain deposit lifecycle, from `GET /card/deposits`. A record only appears
 * here once the webhook has verified the transfer and written it to
 * user_deposit_records — a freshly created intent with no transfer yet won't
 * show up. `depositStatus` is the field to poll:
 *   0 waiting to credit · 1 crediting · 2 credited · 3 credit failed
 */
export type DepositStatus = 0 | 1 | 2 | 3;

export interface DepositRecord {
  txHash: string;
  /** Raw on-chain amount as a base-unit string (USD1 has 18 decimals). */
  amount: string;
  /** 1 = on-chain success. */
  txStatus: number;
  /** 1 = confirmed. */
  confirmStatus: number;
  loadingTxId?: number;
  /** 3 = auto-audit approved. */
  auditStatus: number;
  cardId: number;
  depositStatus: DepositStatus;
}

/**
 * Result of issuing (activating) a card via `POST /card/issue`. The backend
 * echoes just the new card's id + masked number (ucard-backend
 * IssueCardResponse). The endpoint is idempotent: a user who already has an
 * active card gets that card back rather than a second one.
 */
export interface IssueCardResult {
  cardId: number;
  maskedCardNumber: string;
}

/**
 * Sensitive card credentials returned by `getCardDetails` (the backend's
 * `GET /card/details` — note the plural). The plaintext PAN + CVV are meant
 * to be fetched fresh at the moment of use (e.g. the chat-time handoff) and
 * never cached. Expiry arrives split as `expMonth` / `expYear`.
 */
export interface CardDetails {
  cardNumber: string;
  expMonth: string;
  expYear: string;
  cvv: string;
}

export interface WalletClient {
  startKyc(p: CreateCardHolderParams): Promise<StartKycResult>;
  getKycStatus(): Promise<KycStatusResponse>;
  getCardList(): Promise<CardListEntry[]>;
  /**
   * Issue (create + activate) a virtual card for an approved user. Idempotent
   * on the backend (returns the existing active card if one exists) and capped
   * at 3 cards per user. Surfaced in the UI behind the explicit "Activate
   * card" button — never called automatically.
   */
  issueCard(): Promise<IssueCardResult>;
  /**
   * Fetch the plaintext PAN + CVV + expiry for a card. Sensitive: callers
   * must fetch on demand and never persist the result — the CVV in
   * particular is intended to be read live each time it's needed.
   */
  getCardDetails(cardId: number): Promise<CardDetails>;
  getBalance(cardId: number): Promise<CardBalance>;
  getTransactions(
    cardId: number,
    opts?: { offset?: number; limit?: number },
  ): Promise<{ transactions: Transaction[] }>;
  getDepositAddress(): Promise<{ addresses: DepositAddress[] }>;
  /**
   * Create a crypto deposit intent and get back the address to fund plus the
   * backend-enforced minimum. Drives the Top up modal.
   */
  createDepositIntent(p: CreateDepositIntentParams): Promise<DepositIntent>;
  /**
   * List recent on-chain deposits, newest first — polled while the Top up
   * modal is open to surface credit status. Only webhook-verified deposits
   * appear here.
   */
  getDeposits(opts?: { offset?: number; limit?: number }): Promise<DepositRecord[]>;
}

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
}

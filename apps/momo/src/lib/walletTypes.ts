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

export interface WalletClient {
  startKyc(p: CreateCardHolderParams): Promise<StartKycResult>;
  getKycStatus(): Promise<KycStatusResponse>;
  getCardList(): Promise<CardListEntry[]>;
  getBalance(cardId: number): Promise<CardBalance>;
  getTransactions(
    cardId: number,
    opts?: { offset?: number; limit?: number },
  ): Promise<{ transactions: Transaction[] }>;
  getDepositAddress(): Promise<{ addresses: DepositAddress[] }>;
}

/**
 * WalletClient binding for the UI.
 *
 * The exported `walletClient` defaults to the real REST implementation
 * (`walletApi` from ./walletApi). Set `VITE_USE_MOCK_WALLET=true` at build
 * time to swap in `mockWalletClient` instead — used by the Playwright
 * harness and by hand-driven design walks to drive deterministic states.
 *
 * ─── Mock design ──────────────────────────────────────────────────────
 * The mock behaves like a stateful backend, not a state setter. Real
 * users drive it via:
 *
 *   1. `walletClient.startKyc(form)` — the form submits and the mock
 *      flips into `in_review`, generates an ekyc URL + cardHolderId,
 *      stores the submitted params.
 *   2. External "backend events" — fired explicitly from the UI dev bar
 *      or devtools console via `window.__wallet.simulate*`:
 *        - simulateApproveKyc()  — eKYC webhook approved → card minted.
 *        - simulateDeclineKyc()  — eKYC webhook declined.
 *        - simulateDeposit(usd)  — on-chain deposit confirmed.
 *        - simulateSpend(usd)    — agent paid a vendor.
 *        - reset()               — full reset to initial state.
 *
 * `setMockKycStatus` / `setMockHasFunds` remain as legacy shortcuts for
 * Playwright tests that want to teleport to a specific state without
 * walking the form. They are wrappers over the state machine.
 *
 * Mock latency: every call resolves after a random 200–400 ms delay so
 * the UI feels real and loading states are exercised in dev.
 */

import type {
  CardBalance,
  CardDetails,
  CardListEntry,
  CreateCardHolderParams,
  CreateDepositIntentParams,
  IssueCardResult,
  DepositAddress,
  DepositIntent,
  DepositRecord,
  KycStatus,
  KycStatusResponse,
  StartKycResult,
  Transaction,
  WalletClient
} from "./walletTypes";
import { walletApi } from "./walletApi";
import { BackendError } from "./backendFetch";

type WalletMethod = keyof WalletClient;

/**
 * The mock's "backend" state. Every WalletClient method reads or mutates
 * this object — there is no separate `hasFunds` flag or status-only
 * shortcut; the state machine is the source of truth.
 */
interface MockState {
  kycStatus: KycStatus;
  ekycUrl: string | null;
  cardHolderId: number | null;
  submittedParams: CreateCardHolderParams | null;
  cards: CardListEntry[];
  /** Available balance in dollars. */
  balance: number;
  transactions: Transaction[];
  /** On-chain deposit records surfaced by getDeposits (Top up polling). */
  deposits: DepositRecord[];
}

function initialState(): MockState {
  return {
    kycStatus: "none",
    ekycUrl: null,
    cardHolderId: null,
    submittedParams: null,
    cards: [],
    balance: 0,
    transactions: [],
    deposits: []
  };
}

const mockState: MockState = initialState();

/** Per-method call counter — exposed via window.__wallet.getCallCount for
 *  tests that need to assert "exactly N calls fired" (e.g. double-submit
 *  guards, balance-refetch-on-modal-close). */
const callCounts: Partial<Record<WalletMethod, number>> = {};

/** When set, the next call to the matching method rejects once with `message`
 *  and the failure auto-clears so subsequent calls succeed normally. Mirrors
 *  the FakeDaemon.failNext idiom from the V1 chat tests. */
let nextFailure: { method: WalletMethod; message: string } | null = null;

/** When non-null, overrides the random 200–400ms mock latency. 0 makes mock
 *  calls resolve synchronously — useful for tests that want deterministic
 *  ordering without arbitrary waits. */
let latencyOverride: number | null = null;

/* ─── Mocked card constants ────────────────────────────────────────── */

const MOCK_CARD_ID = 7777;
const MOCK_MASKED = "•••• 4242";

const SAMPLE_DEPOSIT: DepositAddress = {
  chainId: 56,
  chainName: "BNB Smart Chain",
  assetSymbol: "USD1",
  // Solana-style placeholder to mirror the design; real client will return
  // a BSC hex address. The UI is asset-agnostic, so this is purely cosmetic.
  depositAddress: "HdDN8BEJXgDFhjVBpfh4hkKjFCYa97ryDneHyRy7tHRf"
};

/* ─── Latency / failure helpers ────────────────────────────────────── */

function delay<T>(value: T): Promise<T> {
  const ms =
    latencyOverride !== null
      ? Math.max(0, latencyOverride)
      : 200 + Math.floor(Math.random() * 200);
  return new Promise((resolve) => setTimeout(() => resolve(value), ms));
}

/** Common pre-amble for every mock method: bump the call counter, then
 *  consume a queued failure if one is targeted at this method. Returns a
 *  rejected promise (with the configured message) when a failure fires;
 *  callers should `if (fail) return fail` before doing real work. */
function consumeCallSlot<T>(method: WalletMethod): Promise<T> | null {
  callCounts[method] = (callCounts[method] ?? 0) + 1;
  if (nextFailure && nextFailure.method === method) {
    const msg = nextFailure.message;
    nextFailure = null;
    const ms =
      latencyOverride !== null
        ? Math.max(0, latencyOverride)
        : 200 + Math.floor(Math.random() * 200);
    return new Promise<T>((_resolve, reject) =>
      setTimeout(() => reject(new Error(msg)), ms)
    );
  }
  return null;
}

/* ─── Public mock knobs (latency / failure / counters) ─────────────── */

/** Make the next call to `method` reject. Pass `null` to clear a pending
 *  failure without firing. The failure auto-clears after firing once. */
export function setMockFailure(
  method: WalletMethod | null,
  message: string = "Network error"
): void {
  nextFailure = method ? { method, message } : null;
}

/** Override the mock latency globally. `null` restores the default 200–400ms
 *  random delay; `0` makes everything resolve on the next microtask. */
export function setMockLatency(ms: number | null): void {
  latencyOverride = ms;
}

/** Read the current call count for a given mock method. Tests use this to
 *  assert double-submit guards and refetch-on-close behaviour. */
export function getCallCount(method: WalletMethod): number {
  return callCounts[method] ?? 0;
}

/** Read-only snapshot of the mock state, primarily for tests + devtools. */
export function getMockState(): Readonly<MockState> {
  return { ...mockState };
}

/** Reset every mock knob (state, counters, queued failure, latency override)
 *  back to defaults. Call in test beforeEach for clean isolation. */
export function resetMockState(): void {
  Object.assign(mockState, initialState());
  nextFailure = null;
  latencyOverride = null;
  for (const k of Object.keys(callCounts) as WalletMethod[]) {
    delete callCounts[k];
  }
}

/* ─── Simulated backend events ─────────────────────────────────────── */

/** eKYC webhook approved → flip status to approved. Does NOT issue a card and
 *  does NOT credit balance — both happen via explicit user actions, mirroring
 *  the real backend: approving KYC leaves the user card-less until they POST
 *  /card/issue (the "Activate card" button). Tests that want an approved+card
 *  state without walking the activate flow use setMockKycStatus("approved"). */
function simulateApproveKyc(): void {
  mockState.kycStatus = "approved";
  // The ekyc URL is one-shot; the verify page is done with it.
  mockState.ekycUrl = null;
  walletState.ekycUrl = null;
}

/** eKYC webhook declined. UI still maps non-approved → "In Review" for
 *  privacy; the underlying status reflects the truth so tests can assert
 *  the leakage guard. `admin: true` mirrors the admin_decline edge case. */
function simulateDeclineKyc(opts?: { admin?: boolean }): void {
  mockState.kycStatus = opts?.admin ? "admin_decline" : "declined";
  mockState.ekycUrl = null;
  walletState.ekycUrl = null;
}

/** On-chain USD1 deposit confirmed → credit balance + push a credit txn. */
function simulateDeposit(
  amountUsd: number = 500,
  opts?: { from?: string; label?: string }
): void {
  const from = opts?.from ?? "0x91…4A2C";
  const label = opts?.label ?? "Crypto top up";
  mockState.balance += amountUsd;
  mockState.transactions = [
    {
      id: `tx-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 6)}`,
      date: nowLabel(),
      description: `USDC deposit from ${from} · Confirmed`,
      amount: amountUsd,
      merchantAmount: amountUsd,
      type: "credit",
      status: "success",
      merchantName: label
    },
    ...mockState.transactions
  ];
  // Also surface it as a credited on-chain deposit record so the Top up modal's
  // getDeposits polling shows the same money landing (depositStatus 2).
  pushDepositRecord(amountUsd, 2);
}

/** Build + prepend a deposit record to mock state. Amount is encoded in base
 *  units (USD1 has 18 decimals) like the real backend; status drives the Top
 *  up modal's poll display (0 waiting · 1 crediting · 2 credited · 3 failed). */
function pushDepositRecord(amountUsd: number, status: 0 | 1 | 2 | 3): void {
  const baseUnits = (BigInt(Math.round(amountUsd * 100)) * 10n ** 16n).toString();
  const cardId = mockState.cards[0]?.cardId ?? MOCK_CARD_ID;
  mockState.deposits = [
    {
      txHash: `0x${Date.now().toString(16)}${Math.random().toString(16).slice(2, 10)}`,
      amount: baseUnits,
      txStatus: 1,
      confirmStatus: 1,
      loadingTxId: Math.floor(Math.random() * 100000),
      auditStatus: 3,
      cardId,
      depositStatus: status
    },
    ...mockState.deposits
  ];
}

/** Push a still-pending on-chain deposit (depositStatus 0) WITHOUT crediting
 *  the balance — lets dev/tests exercise the modal's "waiting → credited"
 *  transition before calling simulateDeposit to settle it. */
function simulatePendingDeposit(amountUsd: number = 5): void {
  pushDepositRecord(amountUsd, 0);
}

/** Agent paid a vendor → debit balance + push a debit txn. */
function simulateSpend(
  amountUsd: number = 42,
  opts?: { merchant?: string; description?: string }
): void {
  const merchant = opts?.merchant ?? "Narisawa";
  const description =
    opts?.description ?? "Restaurant booking · Approved by Hanzhi";
  mockState.balance -= amountUsd;
  mockState.transactions = [
    {
      id: `tx-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 6)}`,
      date: nowLabel(),
      description,
      amount: amountUsd,
      merchantAmount: amountUsd,
      type: "debit",
      status: "settled",
      merchantName: `Agent paid ${merchant}`
    },
    ...mockState.transactions
  ];
}

function nowLabel(): string {
  const d = new Date();
  const hh = d.getHours().toString().padStart(2, "0");
  const mm = d.getMinutes().toString().padStart(2, "0");
  return `Today ${hh}:${mm}`;
}

/* ─── Legacy shortcuts (prefer simulate* APIs) ─────────────────────── */

/**
 * Legacy shortcut — prefer the simulate* APIs.
 *
 * Teleports the mock straight into `s`. For `approved`, this is the
 * (reset + simulateApproveKyc) shorthand; for `in_review`, it just sets
 * the status flag (no submitted-form needed for the jump). `none` resets.
 */
export function setMockKycStatus(s: KycStatus): void {
  if (s === "none") {
    resetMockState();
    return;
  }
  if (s === "approved") {
    // Wipe to a clean baseline first so a previous "Verified full" run
    // doesn't bleed balance / txns into a fresh "Verified empty" jump.
    Object.assign(mockState, initialState());
    mockState.kycStatus = "approved";
    // Legacy teleport keeps a card present so existing has-card tests
    // (A3/A4/D/E/F/G) stay green. The realistic webhook path
    // (simulateApproveKyc) no longer auto-issues — that gap is exactly what
    // the Activate-card button exercises.
    mockState.cards = [
      { cardId: MOCK_CARD_ID, maskedCardNumber: MOCK_MASKED, status: "active" }
    ];
    return;
  }
  // pending / draft / in_review / declined / error / admin_decline:
  // just flip the flag — the UI's privacy mapping is what's under test.
  mockState.kycStatus = s;
}

/**
 * Legacy shortcut — prefer simulateDeposit / simulateSpend.
 *
 * `true`  → seed the classic "Verified full" state (deposit + two spends,
 *           net balance ≈ $2,247.82, three transactions).
 * `false` → clear balance and transactions, leave KYC alone.
 */
export function setMockHasFunds(v: boolean): void {
  if (!v) {
    mockState.balance = 0;
    mockState.transactions = [];
    return;
  }
  // Clear first so repeated true→true doesn't double-credit.
  mockState.balance = 0;
  mockState.transactions = [];
  // Order: spends first so the deposit shows up as the most recent txn,
  // matching the donor sample ordering (deposit at the top).
  simulateSpend(128.5, {
    merchant: "Rakuten",
    description: "Rakuten order · Gift for Kim"
  });
  simulateSpend(42, {
    merchant: "Narisawa",
    description: "Restaurant booking · Approved by Hanzhi"
  });
  simulateDeposit(2418.32, { from: "0x91…4A2C", label: "Crypto top up" });
}

/* ─── The mock WalletClient ────────────────────────────────────────── */

export const mockWalletClient: WalletClient = {
  async startKyc(p: CreateCardHolderParams): Promise<StartKycResult> {
    const fail = consumeCallSlot<StartKycResult>("startKyc");
    if (fail) return fail;
    // Already-submitted guard — the real backend rejects a second
    // cardholder create with a BackendError. Treat anything other than
    // "none" / "draft" as already in flight.
    if (mockState.kycStatus !== "none" && mockState.kycStatus !== "draft") {
      const ms =
        latencyOverride !== null
          ? Math.max(0, latencyOverride)
          : 200 + Math.floor(Math.random() * 200);
      return new Promise<StartKycResult>((_resolve, reject) =>
        setTimeout(
          () =>
            reject(
              new BackendError(
                1003,
                "Identity verification already submitted.",
                "kyc"
              )
            ),
          ms
        )
      );
    }
    const cardHolderId = 100_000 + Math.floor(Math.random() * 900_000);
    const ekycUrl = "https://verify.didit.me/session/IHj0MwWBY1Qw";
    mockState.kycStatus = "in_review";
    mockState.cardHolderId = cardHolderId;
    mockState.ekycUrl = ekycUrl;
    mockState.submittedParams = { ...p };
    return delay({ cardHolderId, ekycUrl });
  },

  async getKycStatus(): Promise<KycStatusResponse> {
    const fail = consumeCallSlot<KycStatusResponse>("getKycStatus");
    if (fail) return fail;
    const first = mockState.cards[0];
    const base: KycStatusResponse = {
      status: mockState.kycStatus,
      message: null
    };
    if (mockState.ekycUrl) base.ekycUrl = mockState.ekycUrl;
    if (first) {
      base.cardId = first.cardId;
      base.maskedCardNumber = first.maskedCardNumber;
    }
    return delay(base);
  },

  async getCardList(): Promise<CardListEntry[]> {
    const fail = consumeCallSlot<CardListEntry[]>("getCardList");
    if (fail) return fail;
    return delay(mockState.cards.slice());
  },

  async issueCard(): Promise<IssueCardResult> {
    const fail = consumeCallSlot<IssueCardResult>("issueCard");
    if (fail) return fail;
    // Enforce the backend's 3-card cap (closed cards excluded; the mock has no
    // closed state, so every card counts). Mirrors BackendError(1000) +
    // "card limit reached" so isCardLimitReachedError matches in the UI.
    if (mockState.cards.length >= 3) {
      const ms =
        latencyOverride !== null
          ? Math.max(0, latencyOverride)
          : 200 + Math.floor(Math.random() * 200);
      return new Promise<IssueCardResult>((_resolve, reject) =>
        setTimeout(
          () =>
            reject(
              new BackendError(
                1000,
                "card limit reached: maximum 3 cards per user",
                "card limit reached: maximum 3 cards per user"
              )
            ),
          ms
        )
      );
    }
    // First card uses the canonical MOCK ids so getCardDetails / masked-number
    // assertions line up; later cards get distinct ids.
    const n = mockState.cards.length;
    const cardId = MOCK_CARD_ID + n;
    mockState.cards = [
      ...mockState.cards,
      { cardId, maskedCardNumber: MOCK_MASKED, status: "active" }
    ];
    return delay({ cardId, maskedCardNumber: MOCK_MASKED });
  },

  async getCardDetails(cardId: number): Promise<CardDetails> {
    const fail = consumeCallSlot<CardDetails>("getCardDetails");
    if (fail) return fail;
    const known = mockState.cards.some((c) => c.cardId === cardId);
    if (!known) {
      // Mirror the backend's ResourceNotFound for an unknown card so callers
      // exercise the same error path the real client would hit.
      const ms =
        latencyOverride !== null
          ? Math.max(0, latencyOverride)
          : 200 + Math.floor(Math.random() * 200);
      return new Promise<CardDetails>((_resolve, reject) =>
        setTimeout(
          () => reject(new BackendError(1001, "Card not found", "card")),
          ms
        )
      );
    }
    // Plaintext sample matching MOCK_MASKED ("•••• 4242"). Stable values so
    // tests can assert exactly what the chat-time handoff would send.
    return delay({
      cardNumber: "4242424242424242",
      expMonth: "12",
      expYear: "29",
      cvv: "123"
    });
  },

  async getBalance(cardId: number): Promise<CardBalance> {
    const fail = consumeCallSlot<CardBalance>("getBalance");
    if (fail) return fail;
    const known = mockState.cards.some((c) => c.cardId === cardId);
    return delay({
      availableBalance: known ? mockState.balance : 0,
      currency: "USD"
    });
  },

  async getTransactions(
    cardId: number,
    opts?: { offset?: number; limit?: number }
  ): Promise<{ transactions: Transaction[] }> {
    const fail = consumeCallSlot<{ transactions: Transaction[] }>(
      "getTransactions"
    );
    if (fail) return fail;
    const known = mockState.cards.some((c) => c.cardId === cardId);
    if (!known) return delay({ transactions: [] });
    const offset = opts?.offset ?? 0;
    const limit = opts?.limit ?? 50;
    return delay({
      transactions: mockState.transactions.slice(offset, offset + limit)
    });
  },

  async getDepositAddress(): Promise<{ addresses: DepositAddress[] }> {
    const fail = consumeCallSlot<{ addresses: DepositAddress[] }>(
      "getDepositAddress"
    );
    if (fail) return fail;
    return delay({ addresses: [SAMPLE_DEPOSIT] });
  },

  async createDepositIntent(
    p: CreateDepositIntentParams
  ): Promise<DepositIntent> {
    const fail = consumeCallSlot<DepositIntent>("createDepositIntent");
    if (fail) return fail;
    // Mirror the real backend: echo a stable pay-to `address` (the same one
    // getDepositAddress hands out, so tests assert one constant) plus an
    // `addressOut` (debug-only gateway address) and the enforced minimum.
    return delay({
      intentId: 3,
      cardId: p.cardId ?? mockState.cards[0]?.cardId ?? MOCK_CARD_ID,
      chainId: p.chainId,
      assetSymbol: p.assetSymbol,
      address: SAMPLE_DEPOSIT.depositAddress,
      addressOut: "0x5bad62a1918ea07f598503a62fcb59480507ece0",
      minimumAmount: "1",
      callbackUrl:
        "https://ucard-test.tomo.services/api/card/deposit-webhooks/cryptapi/mock",
      status: "pending"
    });
  },

  async getDeposits(opts?: {
    offset?: number;
    limit?: number;
  }): Promise<DepositRecord[]> {
    const fail = consumeCallSlot<DepositRecord[]>("getDeposits");
    if (fail) return fail;
    const offset = opts?.offset ?? 0;
    const limit = opts?.limit ?? 20;
    return delay(mockState.deposits.slice(offset, offset + limit));
  }
};

/** The canonical mock deposit address — re-exported so tests can assert the
 *  TopUpModal renders exactly what the mock returned (no encoding bugs). */
export const MOCK_DEPOSIT_ADDRESS: string = SAMPLE_DEPOSIT.depositAddress;

/**
 * Tiny ambient store for the in-flight eKYC URL — the form page hands it
 * off to the verify page via this reactive object instead of touching the
 * URL hash (we already use the hash for routing). Wiped by the verify
 * page on unmount.
 */
export const walletState = $state<{ ekycUrl: string | null }>({ ekycUrl: null });

// Swap to the mock when explicitly opted-in. Real REST client otherwise.
const useMock =
  (import.meta.env as Record<string, string | undefined>)
    .VITE_USE_MOCK_WALLET === "true";
export const walletClient: WalletClient = useMock ? mockWalletClient : walletApi;

// Expose dev knobs on `window.__wallet` so the user can walk every design
// state from the dev bar or devtools console:
//
//   __wallet.simulateApproveKyc();      // eKYC webhook approved
//   __wallet.simulateDeposit(500);      // on-chain deposit confirmed
//   __wallet.simulateSpend(42);         // agent paid a vendor
//   __wallet.reset();                   // full reset to initial state
//
// The legacy setMockKycStatus / setMockHasFunds shortcuts remain for
// Playwright tests; new callers should prefer the simulate* APIs.
if (typeof window !== "undefined") {
  (window as unknown as { __wallet?: unknown }).__wallet = {
    // simulate* — the real backend would do these autonomously.
    simulateApproveKyc,
    simulateDeclineKyc,
    simulateDeposit,
    simulatePendingDeposit,
    simulateSpend,
    reset: resetMockState,
    getState: getMockState,
    // mock knobs
    setMockFailure,
    setMockLatency,
    getCallCount,
    resetMockState,
    MOCK_DEPOSIT_ADDRESS,
    // legacy shortcuts — prefer simulate* APIs above
    setMockKycStatus,
    setMockHasFunds,
    getMockState
  };
}

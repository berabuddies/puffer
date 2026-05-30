/**
 * cardReveal unit spec — guards the "live, scoped, never-cached" contract of
 * the chat-time card reveal primitive (src/lib/wallet/cardReveal.ts).
 *
 * Pure-function spec (no `page`): cardReveal.ts is runes-free and takes its
 * data source by injection, so a fake CardSource exercises every guarantee
 * without a browser or the real walletClient.
 */
import { test, expect } from "@playwright/test";
import {
  revealCardForUse,
  revealCardSummary,
  last4,
  type CardSource
} from "../src/lib/wallet/cardReveal";
import type { CardDetails } from "../src/lib/walletTypes";

function sampleCard(): CardDetails {
  return {
    cardNumber: "4242424242424242",
    expMonth: "12",
    expYear: "29",
    cvv: "123"
  };
}

/** A CardSource that counts calls and returns a fresh card each time, so a
 *  test can prove every reveal re-fetches (no caching). */
function countingSource(make: () => CardDetails = sampleCard): {
  source: CardSource;
  calls: () => number;
} {
  let calls = 0;
  return {
    source: {
      async getCardDetails(_cardId: number): Promise<CardDetails> {
        calls += 1;
        return make();
      }
    },
    calls: () => calls
  };
}

test("revealCardForUse hands the live card to the use callback", async () => {
  const { source } = countingSource();
  const seen = await revealCardForUse({ cardId: 1 }, source, (card) => ({
    cardNumber: card.cardNumber,
    cvv: card.cvv
  }));
  expect(seen.cardNumber).toBe("4242424242424242");
  expect(seen.cvv).toBe("123");
});

test("every reveal re-fetches from the source (no caching)", async () => {
  const { source, calls } = countingSource();
  await revealCardSummary({ cardId: 1 }, source);
  await revealCardSummary({ cardId: 1 }, source);
  await revealCardSummary({ cardId: 1 }, source);
  expect(calls()).toBe(3);
});

test("the card reference is scrubbed after use resolves", async () => {
  const { source } = countingSource();
  // Capture the exact object the source returned by leaking it out of `use`.
  // (Tests are allowed to peek; production callers must NOT do this.)
  let leaked: CardDetails | null = null;
  await revealCardForUse({ cardId: 1 }, source, (card) => {
    leaked = card;
  });
  // The finally-scrub overwrites the fields on the object cardReveal held.
  expect(leaked).not.toBeNull();
  expect(leaked!.cardNumber).toBe("");
  expect(leaked!.cvv).toBe("");
  expect(leaked!.expMonth).toBe("");
  expect(leaked!.expYear).toBe("");
});

test("the card is still scrubbed when use throws", async () => {
  const { source } = countingSource();
  let leaked: CardDetails | null = null;
  await expect(
    revealCardForUse({ cardId: 1 }, source, (card) => {
      leaked = card;
      throw new Error("sink failed");
    })
  ).rejects.toThrow("sink failed");
  // Even on the error path, the finally ran and scrubbed the reference.
  expect(leaked).not.toBeNull();
  expect(leaked!.cardNumber).toBe("");
  expect(leaked!.cvv).toBe("");
});

test("a fetch failure propagates and never reaches the use callback", async () => {
  let used = false;
  const source: CardSource = {
    async getCardDetails(): Promise<CardDetails> {
      throw new Error("backend boom");
    }
  };
  await expect(
    revealCardForUse({ cardId: 1 }, source, () => {
      used = true;
    })
  ).rejects.toThrow("backend boom");
  expect(used).toBe(false);
});

test("revealCardSummary returns only non-sensitive fields (no PAN/CVV)", async () => {
  const { source } = countingSource();
  const summary = await revealCardSummary({ cardId: 1 }, source);
  expect(summary).toEqual({ last4: "4242", expiry: "12/29" });
  // Defensive: the summary object must not carry the full number or cvv.
  const serialized = JSON.stringify(summary);
  expect(serialized).not.toContain("4242424242424242");
  expect(serialized).not.toContain("123");
});

test("last4 extracts the trailing 4 digits, stripping non-digits", () => {
  expect(last4("4242424242424242")).toBe("4242");
  expect(last4("4242 4242 4242 4242")).toBe("4242");
  expect(last4("•••• 1234")).toBe("1234");
  expect(last4("12")).toBe("");
  expect(last4("")).toBe("");
});

test("revealCardSummary leaves blank expiry when the source omits it", async () => {
  const { source } = countingSource(() => ({
    cardNumber: "4000000000000002",
    expMonth: "",
    expYear: "",
    cvv: "999"
  }));
  const summary = await revealCardSummary({ cardId: 7 }, source);
  expect(summary).toEqual({ last4: "0002", expiry: "" });
});

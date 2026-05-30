/**
 * Regression: GET /api/card/transactions returns amounts in TWO encodings —
 * Strada/broker rows are major-unit USD strings with a decimal ("0.99"), the
 * DB fallback returns integer-cents strings ("99"). The old
 * `parseInt(amount) / 100` turned every Strada amount into $0.00. This spec
 * pins both paths to the same dollar value.
 *
 * Pure-function spec (no `page`): the mock wallet client never exercises the
 * REST normalizer, so only a unit test can guard this.
 */
import { test, expect } from "@playwright/test";
import { parseTransactionAmount } from "../src/lib/walletAmount";

test("Strada decimal-dollar strings parse as dollars (not /100)", () => {
  expect(parseTransactionAmount("0.99")).toBeCloseTo(0.99, 5);
  expect(parseTransactionAmount("1.00")).toBeCloseTo(1, 5);
  expect(parseTransactionAmount("1.25")).toBeCloseTo(1.25, 5);
  expect(parseTransactionAmount("2418.32")).toBeCloseTo(2418.32, 5);
  expect(parseTransactionAmount("0.05")).toBeCloseTo(0.05, 5);
});

test("DB integer-cents strings parse as cents/100", () => {
  expect(parseTransactionAmount("99")).toBeCloseTo(0.99, 5);
  expect(parseTransactionAmount("100")).toBeCloseTo(1, 5);
  expect(parseTransactionAmount("241832")).toBeCloseTo(2418.32, 5);
  expect(parseTransactionAmount("0")).toBe(0);
});

test("the two encodings agree on the same dollar value (no ambiguity)", () => {
  expect(parseTransactionAmount("1.00")).toBe(parseTransactionAmount("100"));
  expect(parseTransactionAmount("0.99")).toBe(parseTransactionAmount("99"));
});

test("empty / missing input is 0 (matches previous `|| 0`)", () => {
  expect(parseTransactionAmount("")).toBe(0);
  expect(parseTransactionAmount(undefined)).toBe(0);
  expect(parseTransactionAmount(null)).toBe(0);
});

test("regression: real Strada '0.99' is no longer 0", () => {
  // The exact value the wallet showed as -$0.00 before the fix.
  expect(parseTransactionAmount("0.99")).not.toBe(0);
});

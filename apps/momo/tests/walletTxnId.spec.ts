/**
 * Regression: real ucard-backend transactions all carry `id: 0`, so the
 * transaction list key MUST come from something else (authRefNum + index),
 * never from `id` alone. A non-unique key collapses every row to the same
 * value, and the wallet's keyed `{#each … (row.id)}` then throws Svelte's
 * `each_key_duplicate` mid-render → the page is stuck on "Loading…".
 *
 * This is a pure-function spec (no `page`): the mock wallet client mints
 * unique ids, so the in-browser wallet.spec can't surface this — only the
 * REST normalizer can, and its key derivation lives in walletTxnId.ts.
 */
import { test, expect } from "@playwright/test";
import { transactionKey } from "../src/lib/walletTxnId";

// Verbatim shape of /api/card/transactions from ucard-test: every row's
// numeric `id` is 0; the only distinguishing field is `authRefNum`.
const REAL_PAYLOAD: Array<{ id: number; authRefNum: string }> = [
  { id: 0, authRefNum: "282421" },
  { id: 0, authRefNum: "282397" },
  { id: 0, authRefNum: "000000071109" },
  { id: 0, authRefNum: "82773036135500254900008" },
  { id: 0, authRefNum: "000000068727" },
];

test("all-id:0 backend rows produce unique keys (no each_key_duplicate)", () => {
  const keys = REAL_PAYLOAD.map((raw, i) => transactionKey(raw, i));
  expect(new Set(keys).size).toBe(REAL_PAYLOAD.length);
  // And the key must NOT be the bare "0" the old `?? ` logic produced.
  expect(keys.every((k) => k !== "0")).toBe(true);
});

test("repeated authRefNum across rows still yields unique keys", () => {
  // An authorization and its later settlement/reversal can share authRefNum.
  const rows = [
    { id: 0, authRefNum: "555" },
    { id: 0, authRefNum: "555" },
  ];
  const keys = rows.map((raw, i) => transactionKey(raw, i));
  expect(new Set(keys).size).toBe(2);
});

test("missing id and authRefNum falls back to a unique index key", () => {
  const rows = [{}, {}, {}];
  const keys = rows.map((raw, i) => transactionKey(raw, i));
  expect(new Set(keys).size).toBe(3);
});

test("a real authRefNum is preserved in the key (stable, meaningful)", () => {
  expect(transactionKey({ id: 0, authRefNum: "282421" }, 3)).toBe("282421-3");
});

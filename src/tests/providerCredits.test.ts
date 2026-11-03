import { expect, test } from "bun:test";
import type { QuotaSnapshot } from "../src/features/relay/api/types";
import { providerCreditsSummary } from "../src/features/relay/providerCredits";

function account(amount: number, ledgerAt: number | null, windowAt: number, key = "ledger-a") {
  const quota: QuotaSnapshot = {
    primary: null,
    secondary: null,
    limitReached: false,
    resetCreditsAvailable: null,
    availableCreditsMicroUnits: amount * 1_000_000,
    providerCreditsObservedAtMs: ledgerAt,
    updatedAtMs: windowAt,
    error: null,
  };
  return { creditBalanceKey: key, quota };
}

test("shared credits use ledger freshness instead of passive window updates", () => {
  for (const accounts of [
    [account(100, 1_000, 9_000), account(20, 2_000, 2_000)],
    [account(20, 2_000, 2_000), account(100, 1_000, 9_000)],
  ]) {
    expect(providerCreditsSummary(accounts)).toEqual({ kind: "finite", availableCredits: 20 });
  }
});

test("a new zero ledger is not replaced by an older positive connection", () => {
  expect(providerCreditsSummary([account(0, 2_000, 2_000), account(100, 1_000, 9_000)])).toBeNull();
});

test("legacy balances stay visible without outranking dated ledger observations", () => {
  expect(providerCreditsSummary([account(100, null, 9_000)])).toEqual({ kind: "finite", availableCredits: 100 });
  expect(providerCreditsSummary([account(100, null, 9_000), account(20, 1_000, 1_000)])).toEqual({ kind: "finite", availableCredits: 20 });
  expect(providerCreditsSummary([account(10, 1_000, 1_000), account(20, 1_000, 1_000, "ledger-b")])).toEqual({ kind: "finite", availableCredits: 30 });
});

import type { AccountSummary } from "./api/types";

export type ProviderCreditsSummary =
  | { kind: "finite"; availableCredits: number }
  | { kind: "unlimited" };

const PROVIDER_CREDIT_MICRO_UNITS = 1_000_000;

type CreditAccount = Pick<AccountSummary, "quota" | "creditBalanceKey">;

function hasCreditBalance(account: CreditAccount): boolean {
  const amount = account.quota.availableCreditsMicroUnits;
  return account.quota.providerCreditsUnlimited === true
    || (amount != null && Number.isSafeInteger(amount) && amount >= 0);
}

/** Count each provider ledger once, using its newest reported balance. */
export function providerCreditsSummary(
  accounts: readonly CreditAccount[],
): ProviderCreditsSummary | null {
  const sharedBalances = new Map<string, CreditAccount>();
  const balances: CreditAccount[] = [];
  for (const account of accounts) {
    if (!hasCreditBalance(account)) continue;
    const key = account.creditBalanceKey?.trim();
    if (!key) {
      balances.push(account);
      continue;
    }
    const previous = sharedBalances.get(key);
    const updatedAt = account.quota.updatedAtMs ?? 0;
    const previousUpdatedAt = previous?.quota.updatedAtMs ?? 0;
    if (!previous || updatedAt > previousUpdatedAt) {
      sharedBalances.set(key, account);
    }
  }

  let totalMicroUnits = 0;
  let hasFiniteCredits = false;
  for (const account of [...balances, ...sharedBalances.values()]) {
    if (account.quota.providerCreditsUnlimited === true) return { kind: "unlimited" };
    const microUnits = account.quota.availableCreditsMicroUnits;
    if (microUnits == null || !Number.isSafeInteger(microUnits) || microUnits < 0) continue;
    totalMicroUnits += microUnits;
    hasFiniteCredits = true;
  }
  return hasFiniteCredits && totalMicroUnits > 0
    ? { kind: "finite", availableCredits: totalMicroUnits / PROVIDER_CREDIT_MICRO_UNITS }
    : null;
}

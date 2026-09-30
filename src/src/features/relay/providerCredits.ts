import type { AccountSummary } from "./api/types";

export type ProviderCreditsSummary =
  | { kind: "finite"; availableCredits: number }
  | { kind: "unlimited" };

const PROVIDER_CREDIT_MICRO_UNITS = 1_000_000;

/** Sums only provider-reported credits from the supplied account set. */
export function providerCreditsSummary(
  accounts: readonly Pick<AccountSummary, "quota">[],
): ProviderCreditsSummary | null {
  let totalMicroUnits = 0;
  let hasFiniteCredits = false;
  for (const account of accounts) {
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

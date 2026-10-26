import { buildAccountValueProjection, type RemainingApiEquivalentEstimate } from "../../accountEconomics";
import type { PoolMember } from "../../poolHelpers";

/** Display-only lifetime totals for current pool members, not routing inputs. */
export function poolEconomics(members: readonly PoolMember[], nowMs = Date.now()) {
  const pooled = members.filter((member) => member.inPool && (member.kind === "source" || !member.routeInactive));
  const used = { microUsd: 0, pricedTokens: 0, unpricedTokens: 0 };
  let accountCount = 0;
  const ledgers = new Set<string>();
  const remaining = new Map<string, { value: RemainingApiEquivalentEstimate; observedAt: number }>();
  let purchaseMicroUsd = 0;
  let recoveredMicroUsd = 0;
  let purchaseCount = 0;
  let paybackApproximate = false;
  let recoveredPricedTokens = 0;
  let recoveredUnpricedTokens = 0;

  for (const member of pooled) {
    used.microUsd += member.apiEquivalent.microUsd;
    used.pricedTokens += member.apiEquivalent.pricedTokens ?? 0;
    used.unpricedTokens += member.apiEquivalent.unpricedTokens;
    if (member.kind !== "account") continue;
    accountCount += 1;
    const value = buildAccountValueProjection(member.apiEquivalent, member.quota, member.quotaWindowUsage, member.purchaseCostMicroUsd, nowMs);
    const ledger = member.quotaWindowUsage?.ledgerKey ?? member.creditBalanceKey;
    const key = ledger ? `ledger:${ledger}` : `account:${member.id}`;
    ledgers.add(key);
    if (value.remainingApiEquivalent) {
      const observedAt = value.remainingApiEquivalent.calibration
        ? member.quotaWindowUsage!.observedAtMs : member.quota.updatedAtMs ?? 0;
      const previous = remaining.get(key);
      if (!previous || observedAt > previous.observedAt) {
        remaining.set(key, { value: value.remainingApiEquivalent, observedAt });
      }
    }
    const cost = value.purchaseCostMicroUsd;
    if (cost == null || !Number.isSafeInteger(cost) || cost < 0) continue;
    purchaseMicroUsd += cost;
    recoveredMicroUsd += member.apiEquivalent.microUsd;
    recoveredPricedTokens += member.apiEquivalent.pricedTokens ?? 0;
    recoveredUnpricedTokens += member.apiEquivalent.unpricedTokens;
    purchaseCount += 1;
    paybackApproximate ||= value.approximate;
  }

  const estimates = [...remaining.values()];
  const remainingCount = estimates.length;
  const remainingMicroUsd = estimates.reduce((sum, { value }) => sum + value.microUsd, 0);
  return {
    used, accountCount, remainingCount, remainingTotalCount: ledgers.size,
    remainingMicroUsd: remainingCount ? remainingMicroUsd : null,
    remainingApproximate: estimates.some(({ value }) => value.approximate) || remainingCount < ledgers.size,
    purchaseCount, purchaseMicroUsd, recoveredMicroUsd,
    // Use matched account usage and purchase costs, never average percentages
    // or inflate payback with API sources that have no account purchase cost.
    payback: purchaseMicroUsd > 0 && (recoveredPricedTokens > 0 || recoveredUnpricedTokens === 0)
      ? recoveredMicroUsd / purchaseMicroUsd : null,
    paybackApproximate: paybackApproximate || purchaseCount < accountCount,
  };
}

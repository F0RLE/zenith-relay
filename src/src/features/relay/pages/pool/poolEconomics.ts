import { buildAccountValueProjection } from "../../accountEconomics";
import type { PoolMember } from "../../poolHelpers";

/** Display-only lifetime totals for current pool members, not routing inputs. */
export function poolEconomics(members: readonly PoolMember[]) {
  const pooled = members.filter((member) => member.inPool && (member.kind === "source" || !member.routeInactive));
  const used = { microUsd: 0, pricedTokens: 0, unpricedTokens: 0 };
  let accountCount = 0;
  let remainingMicroUsd = 0;
  let remainingCount = 0;
  let remainingApproximate = false;
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
    const value = buildAccountValueProjection(member.apiEquivalent, member.quota, member.quotaWindowUsage, member.purchaseCostMicroUsd);
    if (value.remainingApiEquivalent) {
      remainingMicroUsd += value.remainingApiEquivalent.microUsd;
      remainingApproximate ||= value.remainingApiEquivalent.approximate;
      remainingCount += 1;
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

  return {
    used, accountCount, remainingCount,
    remainingMicroUsd: remainingCount ? remainingMicroUsd : null,
    remainingApproximate: remainingApproximate || remainingCount < accountCount,
    purchaseCount, purchaseMicroUsd, recoveredMicroUsd,
    // Use matched account usage and purchase costs, never average percentages
    // or inflate payback with API sources that have no account purchase cost.
    payback: purchaseMicroUsd > 0 && (recoveredPricedTokens > 0 || recoveredUnpricedTokens === 0)
      ? recoveredMicroUsd / purchaseMicroUsd : null,
    paybackApproximate: paybackApproximate || purchaseCount < accountCount,
  };
}

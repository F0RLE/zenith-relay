import type { ApiEquivalentSummary, QuotaSnapshot, QuotaWindowUsage } from "./api/types";

export type RemainingApiEquivalentEstimate = {
  microUsd: number;
  approximate: boolean;
  windowKind: "primary" | "secondary" | null;
};

export type AccountValueProjection = {
  purchaseCostMicroUsd: number | null;
  remainingApiEquivalent: RemainingApiEquivalentEstimate | null;
  payback: number | null;
  approximate: boolean;
};

/**
 * Builds the shared display projection for account usage cards.
 *
 * These values do not affect quota state, routing, provider cost, or customer
 * billing. Remaining API equivalent is available only when the local or
 * user-managed server ledger can isolate usage inside the active quota window.
 */
export function buildAccountValueProjection(
  usage: Pick<ApiEquivalentSummary, "microUsd" | "unpricedTokens">,
  quota?: Pick<QuotaSnapshot, "primary" | "secondary" | "directBalanceMicroUsd">,
  quotaWindowUsage?: QuotaWindowUsage | null,
  purchaseCostMicroUsd?: number | null,
): AccountValueProjection {
  const purchaseCost = purchaseCostMicroUsd ?? null;
  return {
    purchaseCostMicroUsd: purchaseCost,
    remainingApiEquivalent: estimateRemainingApiEquivalent(quota, quotaWindowUsage),
    payback: purchaseCost && purchaseCost > 0 ? usage.microUsd / purchaseCost : null,
    approximate: usage.unpricedTokens > 0,
  };
}

export function estimateRemainingApiEquivalent(
  quota?: Pick<QuotaSnapshot, "primary" | "secondary" | "directBalanceMicroUsd">,
  quotaWindowUsage?: QuotaWindowUsage | null,
): RemainingApiEquivalentEstimate | null {
  if (!quota) return null;
  const directBalance = quota.directBalanceMicroUsd;
  if (directBalance != null && Number.isFinite(directBalance) && directBalance >= 0) {
    return { microUsd: Math.round(directBalance), approximate: false, windowKind: null };
  }
  if (!quotaWindowUsage) return null;
  const window = quota[quotaWindowUsage.kind];
  const available = window?.availableBasisPoints;
  if (
    window?.windowStartMs !== quotaWindowUsage.windowStartMs
    || available == null
    || !Number.isFinite(available)
    || available < 0
    || available > 10_000
    || quotaWindowUsage.apiEquivalent.microUsd <= 0
    || quotaWindowUsage.apiEquivalent.unpricedTokens > 0
  ) return null;
  const consumed = 10_000 - available;
  if (consumed <= 0) return null;
  return {
    microUsd: Math.round(quotaWindowUsage.apiEquivalent.microUsd * available / consumed),
    approximate: true,
    windowKind: quotaWindowUsage.kind,
  };
}

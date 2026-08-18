import type { ApiEquivalentSummary, QuotaSnapshot } from "./api/types";

export type PotentialEstimate = {
  microUsd: number;
  approximate: boolean;
};

export type AccountValueProjection = {
  purchaseCostMicroUsd: number | null;
  potential: PotentialEstimate | null;
  payback: number | null;
  approximate: boolean;
};

/**
 * Estimates remaining API-equivalent capacity from observed usage and the
 * limiting provider-reported quota window.
 *
 * This is a display estimate only. It does not change quota state, routing,
 * provider cost, or customer billing.
 */
export function estimateAccountPotential(
  usage: Pick<ApiEquivalentSummary, "microUsd" | "unpricedTokens">,
  quota: Pick<QuotaSnapshot, "primary" | "secondary" | "directBalanceMicroUsd">,
): PotentialEstimate | null {
  if (usage.microUsd <= 0 || !Number.isFinite(usage.microUsd)) return null;

  if (quota.directBalanceMicroUsd != null && Number.isFinite(quota.directBalanceMicroUsd) && quota.directBalanceMicroUsd >= 0) {
    return { microUsd: Math.round(quota.directBalanceMicroUsd), approximate: false };
  }
  if (usage.unpricedTokens > 0) return null;

  const windows = [quota.primary, quota.secondary].filter((window): window is NonNullable<typeof window> => {
    return window != null
      && window.availableBasisPoints != null
      && Number.isFinite(window.availableBasisPoints)
      && window.availableBasisPoints >= 0
      && window.availableBasisPoints <= 10_000
      && window.providerCycleId != null
      && window.providerCycleId.trim().length > 0
      && window.windowStartMs != null
      && window.resetAtMs != null
      && window.windowMinutes != null;
  });

  const matchingWindows = windows.filter((window, index) => {
    if (index === 0) return true;
    const first = windows[0];
    return window.providerCycleId === first.providerCycleId
      && window.windowStartMs === first.windowStartMs
      && window.resetAtMs === first.resetAtMs
      && window.windowMinutes === first.windowMinutes;
  });

  const estimates = matchingWindows
    .map((window) => {
      const available = window?.availableBasisPoints;
      if (available == null) return null;
      const consumed = 10_000 - available;
      if (consumed <= 0) return null;
      return Math.round((usage.microUsd * available) / consumed);
    })
    .filter((value): value is number => value != null);

  if (!estimates.length) return null;
  return {
    microUsd: Math.min(...estimates),
    approximate: usage.unpricedTokens > 0,
  };
}

/**
 * Builds the shared display projection for account usage cards.
 *
 * These values are estimates only. The projection deliberately has no effect
 * on quota state, routing, provider cost, or customer billing.
 */
export function buildAccountValueProjection(
  usage: Pick<ApiEquivalentSummary, "microUsd" | "unpricedTokens">,
  quota: Pick<QuotaSnapshot, "primary" | "secondary" | "directBalanceMicroUsd">,
  purchaseCostMicroUsd?: number | null,
): AccountValueProjection {
  const purchaseCost = purchaseCostMicroUsd ?? null;
  return {
    purchaseCostMicroUsd: purchaseCost,
    potential: estimateAccountPotential(usage, quota),
    payback: purchaseCost && purchaseCost > 0 ? usage.microUsd / purchaseCost : null,
    approximate: usage.unpricedTokens > 0,
  };
}

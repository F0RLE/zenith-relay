import type { ApiEquivalentSummary, QuotaSnapshot } from "./api/types";

export type PotentialEstimate = {
  microUsd: number;
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
  quota: Pick<QuotaSnapshot, "primary" | "secondary">,
): PotentialEstimate | null {
  if (usage.microUsd <= 0 || !Number.isFinite(usage.microUsd)) return null;

  const estimates = [quota.primary, quota.secondary]
    .map((window) => {
      const available = window?.availableBasisPoints;
      if (available == null || !Number.isFinite(available) || available < 0 || available > 10_000) return null;
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

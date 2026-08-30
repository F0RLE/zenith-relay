import type { ApiEquivalentSummary, QuotaSnapshot, QuotaWindowUsage } from "./api/types";

const MIN_OBSERVED_USAGE_BASIS_POINTS = 500;
const MIN_PRICING_COVERAGE_BASIS_POINTS = 8_000;
const MIN_WEEKLY_WINDOW_MINUTES = 6 * 24 * 60;
const MAX_WEEKLY_WINDOW_MINUTES = 8 * 24 * 60;

export type RemainingApiEquivalentEstimate = {
  microUsd: number;
  approximate: boolean;
  windowKind: "primary" | "secondary" | null;
  windowMinutes: number | null;
};

export type AccountValueProjection = {
  purchaseCostMicroUsd: number | null;
  remainingApiEquivalent: RemainingApiEquivalentEstimate | null;
  payback: number | null;
  approximate: boolean;
};

export function formatAccountPayback(payback: number | null, locale: string, approximate = false) {
  if (payback == null) return "—";
  const formatted = new Intl.NumberFormat(locale, { style: "percent", maximumFractionDigits: 0 }).format(payback);
  return `${approximate ? "≈" : ""}${formatted}`;
}

/**
 * Builds the shared display projection for account usage cards.
 *
 * These values do not affect quota state, routing, provider cost, or customer
 * billing.
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

/**
 * Estimates the API-price equivalent still represented by the provider's
 * longest quota window. The estimate is intentionally withheld until Relay
 * observed enough of that exact window and priced at least 80% of its tokens.
 */
export function estimateRemainingApiEquivalent(
  quota?: Pick<QuotaSnapshot, "primary" | "secondary" | "directBalanceMicroUsd">,
  evidence?: QuotaWindowUsage | null,
): RemainingApiEquivalentEstimate | null {
  if (!quota) return null;
  const directBalance = quota.directBalanceMicroUsd;
  if (directBalance != null && Number.isFinite(directBalance) && directBalance >= 0) {
    return {
      microUsd: Math.round(directBalance),
      approximate: false,
      windowKind: null,
      windowMinutes: null,
    };
  }
  if (!evidence) return null;

  const window = quota[evidence.kind];
  const available = window?.availableBasisPoints;
  const totalTokens = evidence.apiEquivalent.pricedTokens + evidence.apiEquivalent.unpricedTokens;
  if (
    window?.windowStartMs !== evidence.windowStartMs
    || window?.observedAtMs !== evidence.observedAtMs
    || window?.windowMinutes !== evidence.windowMinutes
    || available == null
    || !Number.isFinite(available)
    || available < 0
    || available > 10_000
    || evidence.windowMinutes < MIN_WEEKLY_WINDOW_MINUTES
    || evidence.windowMinutes > MAX_WEEKLY_WINDOW_MINUTES
    || evidence.apiEquivalent.microUsd <= 0
    || totalTokens <= 0
    || evidence.apiEquivalent.pricedTokens * 10_000
      < totalTokens * MIN_PRICING_COVERAGE_BASIS_POINTS
  ) return null;

  const consumed = 10_000 - available;
  if (consumed < MIN_OBSERVED_USAGE_BASIS_POINTS) return null;
  const microUsd = Math.round(evidence.apiEquivalent.microUsd * available / consumed);
  if (!Number.isFinite(microUsd) || microUsd < 0) return null;
  return {
    microUsd,
    approximate: true,
    windowKind: evidence.kind,
    windowMinutes: evidence.windowMinutes,
  };
}

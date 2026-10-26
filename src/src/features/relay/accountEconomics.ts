import type { ApiEquivalentSummary, QuotaSnapshot, QuotaWindowUsage } from "./api/types";
import type { TFunction } from "i18next";
import { formatNumber } from "./numberFormatting";
import { formatAccountValueMicroUsd } from "./poolFormatting";

const MIN_WEEKLY_WINDOW_MINUTES = 6 * 24 * 60;
const MAX_WEEKLY_WINDOW_MINUTES = 8 * 24 * 60;
const MAX_ESTIMATE_AGE_MS = 3 * 24 * 60 * 60 * 1_000;

export type RemainingApiEquivalentEstimate = {
  microUsd: number;
  approximate: boolean;
  windowKind: "primary" | "secondary" | null;
  windowMinutes: number | null;
  calibration?: QuotaWindowUsage["calibration"];
  lowMicroUsd?: number;
  highMicroUsd?: number;
};

export type AccountValueProjection = {
  purchaseCostMicroUsd: number | null;
  remainingApiEquivalent: RemainingApiEquivalentEstimate | null;
  payback: number | null;
  approximate: boolean;
};

export function formatAccountPayback(payback: number | null, locale: string, approximate = false) {
  if (payback == null) return "—";
  const formatted = formatNumber(payback, locale, { style: "percent", maximumFractionDigits: 0 });
  return `${approximate ? "≈" : ""}${formatted}`;
}

export function accountPaybackHint(usage: Pick<ApiEquivalentSummary, "microUsd">, purchaseCost: number | null, locale: string, t: TFunction) {
  return purchaseCost == null
    ? t("accounts.accountValue.purchaseMissing")
    : t("accounts.accountValue.paybackHint", {
      used: formatAccountValueMicroUsd(usage.microUsd, locale),
      purchase: formatAccountValueMicroUsd(purchaseCost, locale),
    });
}

export function formatRemainingApiEquivalent(value: Pick<RemainingApiEquivalentEstimate, "microUsd" | "approximate"> | null, locale: string) {
  return value ? formatAccountValueMicroUsd(value.microUsd, locale, value.approximate) : "—";
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
  nowMs = Date.now(),
): AccountValueProjection {
  const purchaseCost = purchaseCostMicroUsd ?? null;
  return {
    purchaseCostMicroUsd: purchaseCost,
    remainingApiEquivalent: estimateRemainingApiEquivalent(quota, quotaWindowUsage, nowMs),
    payback: purchaseCost && purchaseCost > 0 ? usage.microUsd / purchaseCost : null,
    approximate: usage.unpricedTokens > 0,
  };
}

/**
 * Applies the same persisted workload calibration to comparable subscriptions.
 * Native code owns pairing, pricing coverage, cohort selection and ledger identity.
 * Old per-account cumulative/recent evidence is deliberately not a fallback.
 */
export function estimateRemainingApiEquivalent(
  quota?: Pick<QuotaSnapshot, "primary" | "secondary" | "directBalanceMicroUsd">,
  evidence?: QuotaWindowUsage | null,
  nowMs = Date.now(),
): RemainingApiEquivalentEstimate | null {
  const directBalance = quota?.directBalanceMicroUsd;
  if (directBalance != null && Number.isSafeInteger(directBalance) && directBalance >= 0) {
    return {
      microUsd: Math.round(directBalance),
      approximate: false,
      windowKind: null,
      windowMinutes: null,
    };
  }
  const calibration = evidence?.calibration;
  if (!evidence || !calibration) return null;
  const available = evidence.availableBasisPoints;
  const profiles = calibration.profileCount ?? 1;
  const coverage = calibration.pricingCoverageBasisPoints ?? 10_000;
  const median = calibration.medianMicroUsd;
  if (
    !evidence.ledgerKey || !calibration.profileId
    || !Number.isSafeInteger(profiles) || profiles < 1
    || !Number.isSafeInteger(coverage) || coverage < 8_000 || coverage > 10_000
    || (profiles === 1 && !calibration.workload)
    || median == null || !Number.isSafeInteger(median) || median <= 0
    || !Number.isSafeInteger(nowMs)
    || !Number.isSafeInteger(evidence.windowStartMs) || evidence.windowStartMs < 0
    || evidence.windowStartMs > evidence.observedAtMs
    || !Number.isSafeInteger(evidence.observedAtMs)
    || evidence.observedAtMs > nowMs
    || nowMs - evidence.observedAtMs >= MAX_ESTIMATE_AGE_MS
    || !Number.isSafeInteger(evidence.resetAtMs) || evidence.resetAtMs <= nowMs
    || !Number.isSafeInteger(available)
    || available < 0
    || available > 10_000
    || evidence.windowMinutes < MIN_WEEKLY_WINDOW_MINUTES
    || evidence.windowMinutes > MAX_WEEKLY_WINDOW_MINUTES
    || !Number.isSafeInteger(calibration.observedAtMs) || calibration.observedAtMs > nowMs
    || nowMs - calibration.observedAtMs >= MAX_ESTIMATE_AGE_MS
    || !Number.isSafeInteger(calibration.validUntilMs) || calibration.validUntilMs <= nowMs
    || !Number.isSafeInteger(calibration.accountCount) || calibration.accountCount < 1
    || ![calibration.lowMicroUsd, calibration.highMicroUsd]
      .every((value) => Number.isSafeInteger(value) && value > 0)
    || calibration.lowMicroUsd > calibration.highMicroUsd
    || calibration.lowMicroUsd > median || median > calibration.highMicroUsd
  ) return null;
  const remaining = (capacity: number) => Number((BigInt(capacity) * BigInt(available) + 5_000n) / 10_000n);
  return {
    microUsd: remaining(median),
    approximate: true,
    windowKind: evidence.kind,
    windowMinutes: evidence.windowMinutes,
    calibration,
    lowMicroUsd: remaining(calibration.lowMicroUsd),
    highMicroUsd: remaining(calibration.highMicroUsd),
  };
}

export function remainingValueHint(value: RemainingApiEquivalentEstimate | null, locale: string, t: TFunction) {
  if (!value) return t("accounts.accountValue.remainingMissing");
  if (!value.calibration) return t("accounts.accountValue.remainingDirectHint");
  const profile = value.calibration;
  return t((profile.profileCount ?? 1) > 1 ? "accounts.accountValue.remainingMixedHint" : "accounts.accountValue.remainingHint", {
    workload: profile.workload,
    cache: `${profile.cacheBand}–${Math.min(100, (profile.cacheBand ?? 0) + 25)}%`,
    output: `${profile.outputBand}–${Math.min(100, (profile.outputBand ?? 0) + 10)}%`,
    samples: profile.accountCount,
    profiles: profile.profileCount ?? 1,
    coverage: formatNumber((profile.pricingCoverageBasisPoints ?? 10_000) / 10_000, locale, { style: "percent", maximumFractionDigits: 1 }),
  });
}

export function remainingValueDeadlines(evidence?: QuotaWindowUsage | null) {
  return evidence ? [evidence.resetAtMs, evidence.observedAtMs + MAX_ESTIMATE_AGE_MS, evidence.calibration?.validUntilMs] : [];
}

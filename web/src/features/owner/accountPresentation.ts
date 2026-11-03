// Pure Desktop presentation logic only. Never import its store, commands or IPC.
import { buildAccountValueProjection, estimateRemainingApiEquivalent, formatAccountPayback, formatRemainingApiEquivalent } from "../../../../src/src/features/relay/accountEconomics";
import { formatAccountValueMicroUsd } from "../../../../src/src/features/relay/poolFormatting";
import { formatDetailedRemainingTime, formatWindowDuration, isFastSupplementalQuota } from "../../../../src/src/features/relay/quotaFormatting";
import type { ApiEquivalentSummary, QuotaSnapshot, QuotaWindow, QuotaWindowUsage } from "../../../../src/src/features/relay/api/types";
import { defaultLocale, translate } from "../../i18n/catalog";
import type { Locale } from "../../i18n/catalog";
import type { MessageKey } from "../../i18n/types";

/** Allowlisted Hosted projection: no account credentials or local runtime objects. */
export interface OwnerAccountPresentation {
  plan: string | null;
  route: string | null;
  quota: Pick<QuotaSnapshot, "primary" | "secondary" | "supplemental" | "directBalanceMicroUsd" | "availableCreditsMicroUnits" | "providerCreditsUnlimited">;
  quotaStatus: "pending" | "updated" | "refreshing" | "failed" | "requires_reauth" | "not_available";
  // Absent diagnostics omit the row; known diagnostics with no expiry show a dash.
  tokenExpiresAtMs?: number | null;
  subscriptionUntilMs: number | null;
  apiEquivalent: Pick<ApiEquivalentSummary, "microUsd" | "unpricedTokens"> | null;
  quotaWindowUsage: QuotaWindowUsage | null;
  purchaseCostMicroUsd: number | null;
}

const timeUnits: Record<string, MessageKey> = { "timeShort.days": "duration.days", "timeShort.hours": "duration.hours", "timeShort.minutes": "duration.minutes", "timeShort.seconds": "duration.seconds" };

export function ownerRemainingTime(targetMs: number | null, nowMs: number, locale: Locale = defaultLocale) {
  const timeTranslation = ((key: string, options: { count: number }) => translate(locale, timeUnits[key]!, { count: options.count.toLocaleString(locale) })) as Parameters<typeof formatDetailedRemainingTime>[2];
  return targetMs == null || !Number.isFinite(targetMs) || Math.abs(targetMs) > 8.64e15 ? "—" : formatDetailedRemainingTime(targetMs, nowMs, timeTranslation);
}

export function ownerQuotaWindows(account: OwnerAccountPresentation | undefined, locale: Locale = defaultLocale) {
  if (!account || account.quotaStatus === "requires_reauth") return [];
  const label = (window: QuotaWindow) => window.windowMinutes === 10080 ? translate(locale, "account.week") : formatWindowDuration(window.windowMinutes, locale, translate(locale, window.kind === "primary" ? "account.primary" : "account.additional"));
  return [
    ...(["primary", "secondary"] as const).flatMap(key => {
      const window = account.quota[key];
      return window ? [{ id: key, label: label(window), window }] : [];
    }),
    ...(account.quota.supplemental ?? []).filter(item => !isFastSupplementalQuota(item)).map(item => ({ id: item.id, label: `${item.label} · ${label(item.window)}`, window: item.window })),
  ];
}

export function ownerQuotaPercent(basisPoints: number | null) {
  return basisPoints == null || !Number.isInteger(basisPoints) || basisPoints < 0 || basisPoints > 10000 ? null : Math.round(basisPoints / 100);
}

export function ownerAccountValue(account: OwnerAccountPresentation | undefined, nowMs: number, locale: Locale = defaultLocale) {
  if (!account?.apiEquivalent) {
    const remaining = estimateRemainingApiEquivalent(account?.quota, account?.quotaWindowUsage, nowMs);
    return { used: "—", remaining: formatRemainingApiEquivalent(remaining, locale), payback: "—", paid: false, direct: remaining?.approximate === false };
  }
  const projection = buildAccountValueProjection(account.apiEquivalent, account.quota, account.quotaWindowUsage, account.purchaseCostMicroUsd, nowMs);
  return {
    used: formatAccountValueMicroUsd(account.apiEquivalent.microUsd, locale, projection.approximate),
    remaining: formatRemainingApiEquivalent(projection.remainingApiEquivalent, locale),
    payback: formatAccountPayback(projection.payback, locale, projection.approximate),
    paid: projection.payback != null && projection.payback >= 1,
    direct: projection.remainingApiEquivalent?.approximate === false,
  };
}

/** Account-only display totals; unknown telemetry is not a zero or a customer charge. */
export function ownerAccountTotals(accounts: readonly { id: string; account?: OwnerAccountPresentation }[], nowMs: number, locale: Locale = defaultLocale) {
  let used = 0, usageCount = 0, usageApproximate = false;
  let purchase = 0, recovered = 0, purchaseCount = 0, paybackApproximate = false;
  const ledgers = new Set<string>();
  const remaining = new Map<string, { microUsd: number; approximate: boolean; observedAt: number }>();
  for (const { id, account } of accounts) {
    const ledger = account?.quotaWindowUsage?.ledgerKey;
    const key = ledger ? `ledger:${ledger}` : `account:${id}`;
    ledgers.add(key);
    const estimate = estimateRemainingApiEquivalent(account?.quota, account?.quotaWindowUsage, nowMs);
    if (estimate) {
      const observedAt = account?.quotaWindowUsage?.observedAtMs ?? 0;
      if (!remaining.has(key) || observedAt > remaining.get(key)!.observedAt) remaining.set(key, { ...estimate, observedAt });
    }
    if (!account?.apiEquivalent) continue;
    usageCount += 1;
    used += account.apiEquivalent.microUsd;
    usageApproximate ||= account.apiEquivalent.unpricedTokens > 0;
    const value = buildAccountValueProjection(account.apiEquivalent, account.quota, account.quotaWindowUsage, account.purchaseCostMicroUsd, nowMs);
    if (value.purchaseCostMicroUsd == null || !Number.isSafeInteger(value.purchaseCostMicroUsd) || value.purchaseCostMicroUsd < 0 || value.payback == null) continue;
    purchase += value.purchaseCostMicroUsd;
    recovered += account.apiEquivalent.microUsd;
    purchaseCount += 1;
    paybackApproximate ||= value.approximate;
  }
  const estimates = [...remaining.values()];
  const remainingEstimate = estimates.length ? {
    microUsd: estimates.reduce((sum, value) => sum + value.microUsd, 0),
    approximate: estimates.length < ledgers.size || estimates.some(value => value.approximate),
  } : null;
  const payback = purchase > 0 ? recovered / purchase : null;
  return {
    used: usageCount ? formatAccountValueMicroUsd(used, locale, usageApproximate || usageCount < accounts.length) : "—",
    remaining: formatRemainingApiEquivalent(remainingEstimate, locale),
    payback: formatAccountPayback(payback, locale, paybackApproximate || purchaseCount < accounts.length),
    paid: payback != null && payback >= 1,
    usageCount, remainingCount: estimates.length, remainingTotalCount: ledgers.size, purchaseCount, accountCount: accounts.length,
  };
}

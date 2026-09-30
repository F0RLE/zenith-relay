import type { TFunction } from "i18next";
import type { DefaultServiceTier, QuotaWindow } from "./api/types";
import { formatNumber } from "./numberFormatting";

export function formatRemainingTime(targetMs: number, nowMs: number, t: TFunction) {
  const totalSeconds = Math.max(0, Math.floor((targetMs - nowMs) / 1_000));
  if (totalSeconds >= 86_400) return t("timeShort.days", { count: Math.floor(totalSeconds / 86_400) });
  if (totalSeconds >= 3_600) return `${t("timeShort.hours", { count: Math.floor(totalSeconds / 3_600) })} ${t("timeShort.minutes", { count: Math.floor(totalSeconds % 3_600 / 60) })}`;
  if (totalSeconds >= 60) return `${t("timeShort.minutes", { count: Math.floor(totalSeconds / 60) })} ${t("timeShort.seconds", { count: totalSeconds % 60 })}`;
  return t("timeShort.seconds", { count: totalSeconds });
}

export function formatDetailedRemainingTime(targetMs: number, nowMs: number, t: TFunction) {
  const totalMinutes = Math.max(0, Math.floor((targetMs - nowMs) / 60_000));
  if (totalMinutes < 1_440) return formatRemainingTime(targetMs, nowMs, t);
  return `${t("timeShort.days", { count: Math.floor(totalMinutes / 1_440) })} ${t("timeShort.hours", { count: Math.floor(totalMinutes % 1_440 / 60) })} ${t("timeShort.minutes", { count: totalMinutes % 60 })}`;
}

export function formatWindowDuration(minutes: number | null, locale: string, fallback: string) {
  if (!minutes) return fallback;
  const units: Array<[number, Intl.NumberFormatOptions["unit"]]> = [[10_080, "week"], [1_440, "day"], [60, "hour"], [1, "minute"]];
  const matching = units.find(([size]) => minutes % size === 0) ?? [1, "minute" as Intl.NumberFormatOptions["unit"]] as const;
  const [size, unit] = matching;
  return formatNumber(minutes / size, locale, { style: "unit", unit, unitDisplay: "long", maximumFractionDigits: 1 });
}

export function formatQuotaRemaining(basisPoints: number | null, locale: string) {
  return basisPoints == null ? "—" : formatNumber(basisPoints / 10_000, locale, { style: "percent", maximumFractionDigits: 1 });
}

export function isFastSupplementalQuota(item: { label: string; serviceTier?: DefaultServiceTier | null }) {
  return item.serviceTier === "fast"
    || item.serviceTier === "ultrafast"
    || /\b(priority|fast|ultrafast)\b/i.test(item.label);
}

export function quotaWindowLabel(window: QuotaWindow | null, kind: "primary" | "secondary", t: TFunction) {
  const minutes = window?.windowMinutes;
  if (!minutes) return t(`quota.${kind}`);
  if (minutes % 10_080 === 0) {
    const weeks = minutes / 10_080;
    return weeks === 1 ? t("quota.week") : t("quota.weeks", { count: weeks });
  }
  if (minutes % 1_440 === 0) return t("quota.days", { count: minutes / 1_440 });
  if (minutes % 60 === 0) return t("quota.hours", { count: minutes / 60 });
  return t("quota.minutes", { count: minutes });
}

export function formatSupplementalQuotaLabel(
  label: string,
  serviceTier: DefaultServiceTier | null | undefined,
  t: TFunction,
) {
  const normalized = label.trim();
  if (serviceTier !== "fast" && serviceTier !== "ultrafast") return normalized;
  const tierPattern = serviceTier === "ultrafast" ? /\bultrafast\b/gi : /\b(priority|fast)\b/gi;
  const baseLabel = normalized
    .replace(tierPattern, "")
    .replace(/\s{2,}/g, " ")
    .replace(/\s*[·-]\s*$/u, "")
    .trim();
  const tierLabel = t(serviceTier === "ultrafast" ? "quota.ultrafastTier" : "quota.fastTier");
  return baseLabel ? `${baseLabel} · ${tierLabel}` : tierLabel;
}

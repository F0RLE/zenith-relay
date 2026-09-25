import type { SourceStats, SourceStatsAmount, SourceStatsStatus } from "./api/types";
import { formatNumber } from "./numberFormatting";

export type SourceStatsState = {
  value: SourceStats | null;
  loading: boolean;
  failed: boolean;
  error?: SourceStatsStatus;
};

export function sourceStatsStatus(stats: SourceStats): SourceStatsStatus {
  const status = stats.status ?? "available";
  return status === "available" && stats.provider === "unsupported" ? "unsupported" : status;
}

export function settledSourceStats(previous: SourceStats | null, result: SourceStats): SourceStatsState {
  const status = sourceStatsStatus(result);
  const failed = status !== "available" && status !== "unsupported";
  const retained = failed && previous && sourceStatsStatus(previous) === "available";
  return {
    value: retained ? { ...previous, stale: true, refreshError: status } : result,
    loading: false,
    failed: failed || Boolean(result.refreshError),
    ...(result.refreshError || failed ? { error: result.refreshError ?? status } : {}),
  };
}

/** A delayed host snapshot must not replace a newer explicit read. Snapshot
 * projection also cannot complete an independent in-flight UI operation. */
export function projectedSourceStats(previous: SourceStatsState | undefined, result: SourceStats): SourceStatsState {
  if (previous?.value?.asOfMs != null && result.asOfMs != null && previous.value.asOfMs > result.asOfMs) return previous;
  return { ...settledSourceStats(previous?.value ?? null, result), loading: previous?.loading ?? false };
}

export function sourceStatsAmounts(stats: SourceStats | null | undefined): SourceStatsAmount[] {
  if (!stats || sourceStatsStatus(stats) !== "available") return [];
  if (stats.amounts?.length) return stats.amounts;
  return [{ currency: "USD", balanceMicros: stats.balanceMicroUsd, spentMicros: stats.spentMicroUsd }];
}

export function formatSourceAmount(micros: number, currency: SourceStatsAmount["currency"], locale: string): string {
  return formatNumber(micros / 1_000_000, locale, currency === "CREDITS"
    ? { maximumFractionDigits: 2 }
    : { style: "currency", currency, minimumFractionDigits: 2, maximumFractionDigits: 2 });
}

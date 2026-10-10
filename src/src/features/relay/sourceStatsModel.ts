import type { SourceStats, SourceStatsAmount, SourceStatsStatus } from "./api/types";
import { formatNumber } from "./numberFormatting";

export type SourceStatsState = {
  stats: SourceStats | null;
  loading: boolean;
  failed: boolean;
  error?: SourceStatsStatus;
};

export function sourceStatsStatus(stats: SourceStats): SourceStatsStatus {
  const status = stats.status ?? "available";
  return status === "available" && stats.provider === "unsupported" ? "unsupported" : status;
}

export function settledSourceStats(previousStats: SourceStats | null, nextStats: SourceStats): SourceStatsState {
  const status = sourceStatsStatus(nextStats);
  const failed = status !== "available" && status !== "unsupported";
  const retained = failed && previousStats && sourceStatsStatus(previousStats) === "available";
  return {
    stats: retained ? { ...previousStats, stale: true, refreshError: status } : nextStats,
    loading: false,
    failed: failed || Boolean(nextStats.refreshError),
    ...(nextStats.refreshError || failed ? { error: nextStats.refreshError ?? status } : {}),
  };
}

/** A delayed host snapshot must not replace a newer explicit read. Snapshot
 * projection also cannot complete an independent in-flight UI operation. */
export function projectedSourceStats(previousState: SourceStatsState | undefined, nextStats: SourceStats): SourceStatsState {
  if (previousState?.stats?.asOfMs != null && nextStats.asOfMs != null && previousState.stats.asOfMs > nextStats.asOfMs) return previousState;
  return { ...settledSourceStats(previousState?.stats ?? null, nextStats), loading: previousState?.loading ?? false };
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

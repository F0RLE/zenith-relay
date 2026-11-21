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

function olderSourceObservation(previousStats: SourceStats | null | undefined, nextStats: SourceStats): boolean {
  if (previousStats?.asOfMs == null) return false;
  if (nextStats.asOfMs != null) return nextStats.asOfMs < previousStats.asOfMs;
  return sourceStatsStatus(nextStats) === "available" && !nextStats.refreshError;
}

export function settledSourceStats(previousStats: SourceStats | null, nextStats: SourceStats): SourceStatsState {
  if (previousStats && olderSourceObservation(previousStats, nextStats)) nextStats = previousStats;
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
  if (previousState && olderSourceObservation(previousState.stats, nextStats)) return previousState;
  const newer = nextStats.asOfMs != null && (previousState?.stats?.asOfMs == null || nextStats.asOfMs > previousState.stats.asOfMs);
  const healthy = sourceStatsStatus(nextStats) === "available" && !nextStats.stale && !nextStats.refreshError;
  if (previousState && (previousState.failed || previousState.stats?.stale || previousState.stats?.refreshError) && healthy && !newer) {
    return previousState;
  }
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

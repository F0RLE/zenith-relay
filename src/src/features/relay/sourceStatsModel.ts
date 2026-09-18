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
  return {
    value: failed && previous && sourceStatsStatus(previous) === "available" ? previous : result,
    loading: false,
    failed,
    ...(failed ? { error: status } : {}),
  };
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

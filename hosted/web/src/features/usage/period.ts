export type UsagePeriod = "today" | "24h" | "week" | "month" | "all";

export const usagePeriods = [
  { value: "today", label: "period.today" },
  { value: "24h", label: "period.last24Hours", shortLabel: "period.24Hours" },
  { value: "week", label: "period.last7Days", shortLabel: "period.7Days" },
  { value: "month", label: "period.last30Days", shortLabel: "period.30Days" },
  { value: "all", label: "period.allTime" },
] as const;

// Snapshot-relative rolling windows; "today" remains a UTC calendar day.
export function inUsagePeriod(at: string, asOf: string, period: UsagePeriod): boolean {
  const time = Date.parse(at), end = Date.parse(asOf);
  if (!Number.isFinite(time) || !Number.isFinite(end) || time > end) return false;
  const today = new Date(end); today.setUTCHours(0, 0, 0, 0);
  const start = period === "all" ? -Infinity : period === "today" ? today.getTime()
    : end - (period === "24h" ? 1 : period === "week" ? 7 : 30) * 86_400_000;
  return time >= start;
}

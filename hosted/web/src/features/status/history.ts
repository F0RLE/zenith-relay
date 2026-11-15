export type StatusPeriod = "day" | "week";
export interface StatusInterval { startMs: number; endMs: number }

/** Empty monitoring windows, not synthesized measurements or request aggregates. */
export function statusIntervals(period: StatusPeriod, endMs: number): StatusInterval[] {
  if (!Number.isFinite(endMs)) return [];
  const count = period === "day" ? 48 : 28;
  const duration = period === "day" ? 30 * 60_000 : 6 * 60 * 60_000;
  return Array.from({ length: count }, (_, index) => ({
    startMs: endMs - (count - index) * duration,
    endMs: endMs - (count - index - 1) * duration,
  }));
}

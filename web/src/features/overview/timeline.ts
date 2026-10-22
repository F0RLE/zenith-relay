import type { RequestRecord } from "../usage/model";

export type OverviewRange = "today" | "week" | "month";
export interface OverviewBucket {
  startMs: number;
  endMs: number;
  label: string;
  interval: string;
  partial: boolean;
  rows: RequestRecord[];
}

export function formatOverviewInterval(startMs: number, endMs: number): string {
  const format = (value: number) => new Date(value).toLocaleString("ru-RU", {
    timeZone: "UTC", day: "2-digit", month: "2-digit", year: "numeric", hour: "2-digit", minute: "2-digit",
  });
  return `${format(startMs)} — ${format(endMs)} UTC`;
}

export function overviewTimeline(records: readonly RequestRecord[], range: OverviewRange, asOf: string, model = "all") {
  const endMs = Date.parse(asOf);
  if (!Number.isFinite(endMs)) throw new RangeError("Invalid overview snapshot");
  const midnight = new Date(endMs); midnight.setUTCHours(0, 0, 0, 0);
  const days = range === "today" ? 1 : range === "week" ? 7 : 30;
  const startMs = midnight.getTime() - (days - 1) * 86400000;
  const stepMs = range === "today" ? 3600000 : 86400000;
  const rows = records.filter(record => {
    const time = Date.parse(record.startedAt);
    return time >= startMs && time < endMs && (model === "all" || record.model.id === model);
  });
  const count = range === "today" ? Math.max(1, Math.ceil((endMs - startMs) / stepMs)) : days;
  const buckets: OverviewBucket[] = Array.from({ length: count }, (_, index) => {
    const start = startMs + index * stepMs;
    const end = Math.min(start + stepMs, endMs);
    const date = new Date(start);
    return {
      startMs: start, endMs: end, partial: end < start + stepMs,
      label: range === "today" ? `${String(date.getUTCHours()).padStart(2, "0")}:00`
        : date.toLocaleDateString("ru-RU", { timeZone: "UTC", day: "2-digit", month: "2-digit" }),
      interval: formatOverviewInterval(start, end), rows: [],
    };
  });
  for (const row of rows) buckets[Math.floor((Date.parse(row.startedAt) - startMs) / stepMs)]!.rows.push(row);
  return { rows, buckets, startMs, endMs, stepMs };
}

import type { RequestRecord } from "../usage/model";
import { defaultLocale } from "../../i18n/catalog";
import type { Locale } from "../../i18n/catalog";

export type OverviewRange = "24h" | "week" | "month" | "all";
export interface OverviewBucket {
  startMs: number;
  endMs: number;
  interval: string;
  rows: RequestRecord[];
}

export function formatOverviewInterval(startMs: number, endMs: number, locale: Locale = defaultLocale): string {
  const format = (value: number) => new Date(value).toLocaleString(locale, {
    timeZone: "UTC", day: "2-digit", month: "2-digit", year: "numeric", hour: "2-digit", minute: "2-digit",
  });
  return `${format(startMs)} — ${format(endMs)} UTC`;
}

export function overviewTimeline(records: readonly RequestRecord[], range: OverviewRange, asOf: string, model = "all", locale: Locale = defaultLocale) {
  const endMs = Date.parse(asOf);
  if (!Number.isFinite(endMs)) throw new RangeError("Invalid overview snapshot");
  const days = range === "24h" ? 1 : range === "week" ? 7 : 30;
  let stepMs = range === "24h" ? 3600000 : 86400000;
  let count = range === "24h" ? 24 : days;
  if (range === "all") {
    let earliestMs = endMs;
    for (const record of records) {
      const time = Date.parse(record.startedAt);
      if (time < earliestMs) earliestMs = time;
    }
    const durationMs = Math.max(3600000, endMs - earliestMs);
    // Keep the complete supplied history, with at most 30 readable buckets.
    stepMs = durationMs <= 86400000 ? 3600000 : Math.ceil(durationMs / (30 * 86400000)) * 86400000;
    count = Math.ceil(durationMs / stepMs);
  }
  const startMs = endMs - count * stepMs;
  const rows = records.filter(record => {
    const time = Date.parse(record.startedAt);
    return time >= startMs && time < endMs && (model === "all" || record.model.id === model);
  });
  const buckets: OverviewBucket[] = Array.from({ length: count }, (_, index) => {
    const start = startMs + index * stepMs;
    const end = start + stepMs;
    return {
      startMs: start, endMs: end,
      interval: formatOverviewInterval(start, end, locale), rows: [],
    };
  });
  for (const row of rows) buckets[Math.floor((Date.parse(row.startedAt) - startMs) / stepMs)]!.rows.push(row);
  return { rows, buckets, startMs, endMs, stepMs };
}

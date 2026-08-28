import type { LocalUsage, RemoteUsage, UsageBucket, UsageTotals } from "../../api/types";
import { measureTokenSpeed } from "../../usageSpeed";
import { emptyUsageTotals } from "../../usageTotals";

export type Range = "today" | "week" | "month";
export type AnalyticsScope = "" | `source:${string}` | `account:${string}`;
export type WindowBucket = { startMs: number; endMs: number; label: string; fullLabel: string; showLabel: boolean };
export type Analytics = { totals: UsageTotals; buckets: UsageBucket[] };
export type UsageSample = {
  createdAtMs: number;
  success: boolean;
  latencyMs: number;
  ttftMs: number | null;
  generationMs: number | null;
  inputTokens: number | null;
  cachedInputTokens: number | null;
  cacheWriteInputTokens?: number | null;
  reasoningTokens: number | null;
  outputTokens: number | null;
  totalTokens: number | null;
  apiEquivalent?: UsageTotals["apiEquivalent"] | null;
};

export const HOUR_MS = 60 * 60 * 1_000;
export const DAY_MS = 24 * HOUR_MS;

export function chartWindows(range: Range, locale: string, now = new Date()): WindowBucket[] {
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const count = range === "today" ? 24 : range === "week" ? 7 : 30;
  const bucketMs = range === "today" ? HOUR_MS : DAY_MS;
  const startMs = today.getTime() - (range === "today" ? 0 : (count - 1) * DAY_MS);
  const hour = new Intl.DateTimeFormat(locale, { hour: "2-digit", hourCycle: "h23" });
  const weekday = new Intl.DateTimeFormat(locale, { weekday: "short" });
  const day = new Intl.DateTimeFormat(locale, { day: "numeric" });
  const full = new Intl.DateTimeFormat(locale, range === "today"
    ? { day: "numeric", month: "short", hour: "2-digit", minute: "2-digit", hourCycle: "h23" }
    : { day: "numeric", month: "long" });
  return Array.from({ length: count }, (_, index) => {
    const bucketStart = startMs + index * bucketMs;
    const date = new Date(bucketStart);
    return {
      startMs: bucketStart,
      endMs: bucketStart + bucketMs - 1,
      label: range === "today" ? hour.format(date) : range === "week" ? weekday.format(date) : day.format(date),
      fullLabel: full.format(date),
      showLabel: range === "week" || index % (range === "today" ? 4 : 5) === 0 || index === count - 1,
    };
  });
}

export function analyticsFromPage(
  totals: UsageTotals | undefined,
  buckets: UsageBucket[] | undefined,
  samples: UsageSample[],
  windows: WindowBucket[],
): Analytics {
  return { totals: totals ?? totalsFromSamples(samples), buckets: buckets?.length ? buckets : bucketsFromSamples(windows, samples) };
}

export function bucketsFromSamples(windows: WindowBucket[], samples: UsageSample[]) {
  return windows.map((window) => ({
    startMs: window.startMs,
    totals: totalsFromSamples(samples.filter((sample) => sample.createdAtMs >= window.startMs && sample.createdAtMs <= window.endMs)),
  }));
}

export function totalsFromSamples(samples: UsageSample[]) {
  return samples.reduce<UsageTotals>((totals, sample) => {
    const outputTokens = sample.success ? Math.max(0, sample.outputTokens ?? 0) : 0;
    totals.requests += 1;
    totals.successfulRequests += Number(sample.success);
    totals.latencyMs += sample.latencyMs;
    if (sample.ttftMs != null) { totals.ttftMs += sample.ttftMs; totals.ttftSamples += 1; }
    const generation = measureTokenSpeed({ success: sample.success, outputTokens: sample.outputTokens, reasoningTokens: sample.reasoningTokens, durationMs: sample.generationMs });
    if (generation) {
      totals.generationMs += generation.durationMs;
      totals.generationSamples += 1;
      totals.generationOutputTokens += generation.outputTokens;
    }
    totals.inputTokens += sample.inputTokens ?? 0;
    if (sample.cachedInputTokens != null) { totals.cachedInputTokens += sample.cachedInputTokens; totals.cachedInputSamples += 1; }
    if (sample.cacheWriteInputTokens != null) { totals.cacheWriteInputTokens = (totals.cacheWriteInputTokens ?? 0) + sample.cacheWriteInputTokens; totals.cacheWriteInputSamples = (totals.cacheWriteInputSamples ?? 0) + 1; }
    totals.reasoningTokens += sample.reasoningTokens ?? 0;
    totals.outputTokens += sample.outputTokens ?? 0;
    totals.totalTokens += sample.totalTokens ?? 0;
    if (sample.apiEquivalent) {
      totals.apiEquivalent.microUsd += sample.apiEquivalent.microUsd;
      totals.apiEquivalent.pricedTokens += sample.apiEquivalent.pricedTokens;
      totals.apiEquivalent.unpricedTokens += sample.apiEquivalent.unpricedTokens;
    } else {
      totals.apiEquivalent.unpricedTokens += sample.totalTokens ?? 0;
    }
    if (outputTokens > 0 && sample.latencyMs > 0) { totals.speedOutputTokens += outputTokens; totals.speedDurationMs += sample.latencyMs; }
    return totals;
  }, emptyUsageTotals());
}

export function localSamples(events: LocalUsage[]): UsageSample[] {
  return events.map((item) => ({ ...item, createdAtMs: Date.parse(item.createdAt) }));
}

export function remoteSamples(events: RemoteUsage[]): UsageSample[] {
  return events.map((item) => ({ ...item, ttftMs: item.ttftMs ?? null, generationMs: item.generationMs ?? null }));
}

export function fillBuckets(windows: WindowBucket[], buckets: UsageBucket[]) {
  const byStart = new Map(buckets.map((bucket) => [bucket.startMs, bucket.totals]));
  return windows.map((window) => byStart.get(window.startMs) ?? emptyUsageTotals());
}

export function lineSegments(values: Array<number | null>, max: number) {
  const segments: string[] = [];
  let current = "";
  values.forEach((value, index) => {
    if (value == null) {
      if (current) segments.push(current);
      current = "";
      return;
    }
    const x = (index + 0.5) / values.length * 100;
    const y = (1 - value / max) * 100;
    current += `${current ? " L" : "M"}${x.toFixed(2)} ${y.toFixed(2)}`;
  });
  if (current) segments.push(current);
  return segments;
}

export function formatApiEquivalent(value: number | null, locale: string) {
  return value == null ? "—" : `≈${formatUsd(value, locale)}`;
}

export function formatUsd(value: number, locale: string) {
  return new Intl.NumberFormat(locale, { style: "currency", currency: "USD", minimumFractionDigits: 2, maximumFractionDigits: value < 0.01 ? 6 : value < 1 ? 4 : 2 }).format(value);
}

export function sourceHost(value: string) {
  try {
    return new URL(value).host;
  } catch {
    return value;
  }
}

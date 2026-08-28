import { describe, expect, test } from "bun:test";
import {
  analyticsFromPage,
  chartWindows,
  DAY_MS,
  fillBuckets,
  HOUR_MS,
  lineSegments,
  totalsFromSamples,
  type UsageSample,
} from "../src/features/relay/pages/overview/overviewAnalyticsModel";

const sample = (overrides: Partial<UsageSample> = {}): UsageSample => ({
  createdAtMs: 1_000,
  success: true,
  latencyMs: 200,
  ttftMs: 50,
  generationMs: 100,
  inputTokens: 100,
  cachedInputTokens: 25,
  cacheWriteInputTokens: 10,
  reasoningTokens: 1,
  outputTokens: 11,
  totalTokens: 111,
  apiEquivalent: { microUsd: 2_000_000, pricedTokens: 100, unpricedTokens: 11 },
  ...overrides,
});

describe("overview analytics model", () => {
  test("creates stable local-time windows for each range", () => {
    const now = new Date(2026, 7, 28, 12, 30);
    const today = chartWindows("today", "en-US", now);
    const week = chartWindows("week", "en-US", now);
    const month = chartWindows("month", "en-US", now);

    expect(today).toHaveLength(24);
    expect(today[0].startMs).toBe(new Date(2026, 7, 28).getTime());
    expect(today[23].endMs - today[0].startMs + 1).toBe(24 * HOUR_MS);
    expect(week).toHaveLength(7);
    expect(week[1].startMs - week[0].startMs).toBe(DAY_MS);
    expect(month).toHaveLength(30);
  });

  test("aggregates usage, cache counters, speed, and priced tokens", () => {
    const totals = totalsFromSamples([
      sample(),
      sample({ success: false, outputTokens: 20, totalTokens: 20, apiEquivalent: null }),
    ]);

    expect(totals).toMatchObject({
      requests: 2,
      successfulRequests: 1,
      inputTokens: 200,
      cachedInputTokens: 50,
      cachedInputSamples: 2,
      cacheWriteInputTokens: 20,
      cacheWriteInputSamples: 2,
      outputTokens: 31,
      totalTokens: 131,
      generationOutputTokens: 9,
      generationSamples: 1,
      speedOutputTokens: 11,
      speedDurationMs: 200,
      apiEquivalent: { microUsd: 2_000_000, pricedTokens: 100, unpricedTokens: 31 },
    });
  });

  test("fills missing buckets and preserves explicit server buckets", () => {
    const windows = chartWindows("week", "en-US", new Date(2026, 7, 28, 12));
    const fallback = analyticsFromPage(undefined, undefined, [sample({ createdAtMs: windows[0].startMs })], windows);
    expect(fallback.buckets).toHaveLength(7);
    expect(fallback.buckets[0].totals.requests).toBe(1);

    const explicit = { startMs: windows[2].startMs, totals: totalsFromSamples([]) };
    const result = analyticsFromPage(undefined, [explicit], [sample()], windows);
    expect(result.buckets).toEqual([explicit]);
    expect(fillBuckets(windows, [explicit])[2]).toEqual(explicit.totals);
    expect(fillBuckets(windows, [explicit])[0].requests).toBe(0);
  });

  test("breaks chart paths at missing measurements", () => {
    expect(lineSegments([1, 2, null, 3], 3)).toEqual([
      "M12.50 66.67 L37.50 33.33",
      "M87.50 0.00",
    ]);
  });
});

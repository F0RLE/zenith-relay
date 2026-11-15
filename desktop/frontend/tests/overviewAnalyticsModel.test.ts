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
import { sourceHost, sourcePort } from "../src/features/relay/sourceUrl";
import { observedTokensPerSecond } from "../src/features/relay/usageSpeed";
import { cacheHitRate } from "../src/features/relay/usageTotals";

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
  test("computes observed cache rate only from valid paired input measurements", () => {
    expect(cacheHitRate(totalsFromSamples([sample()]))).toBe(25);
    expect(cacheHitRate(totalsFromSamples([sample({ cachedInputTokens: 0 })]))).toBe(0);
    expect(cacheHitRate(totalsFromSamples([]))).toBeNull();
    expect(cacheHitRate(totalsFromSamples([sample({ cachedInputTokens: null })]))).toBeNull();
    const mixed = totalsFromSamples([
      sample(),
      sample({ inputTokens: 200, cachedInputTokens: null }),
    ]);
    expect(mixed.inputTokens).toBe(300);
    expect(mixed.cachedInputTokens).toBe(25);
    expect(mixed.cacheRateInputTokens).toBe(100);
    expect(mixed.cacheRateCachedInputTokens).toBe(25);
    expect(cacheHitRate(mixed)).toBe(25);
    expect(cacheHitRate(totalsFromSamples([sample({ cachedInputTokens: 101 })]))).toBeNull();
  });

  test("does not compare physical cache measurements with logical request counts", () => {
    const totals = totalsFromSamples([
      sample(),
      sample({ inputTokens: 300, cachedInputTokens: 225 }),
      sample({ success: false, inputTokens: null, cachedInputTokens: null }),
    ]);
    expect(cacheHitRate(totals)).toBe(62.5);
    expect(cacheHitRate({ ...totals, requests: 1, successfulRequests: 1 })).toBe(62.5);
    expect(cacheHitRate({ ...totals, requests: 5, successfulRequests: 1 })).toBe(62.5);
  });

  test("does not let invalid or unpaired cache usage distort valid measurements", () => {
    for (const unknown of [
      sample({ inputTokens: null }), sample({ inputTokens: 0 }),
      sample({ cachedInputTokens: -1 }), sample({ cachedInputTokens: 101 }),
      sample({ inputTokens: Number.NaN }), sample({ cachedInputTokens: Number.POSITIVE_INFINITY }),
    ]) {
      expect(cacheHitRate(totalsFromSamples([unknown]))).toBeNull();
      expect(cacheHitRate(totalsFromSamples([sample(), unknown]))).toBe(25);
    }
  });

  test("creates stable local-time windows for each range", () => {
    const now = new Date(2026, 7, 28, 12, 30);
    const today = chartWindows("today", "en-US", now);
    const week = chartWindows("week", "en-US", now);
    const month = chartWindows("month", "en-US", now);

    expect(today).toHaveLength(24);
    expect(today[0].startMs).toBe(new Date(2026, 7, 27, 13).getTime());
    expect(today[23].startMs).toBe(new Date(2026, 7, 28, 12).getTime());
    expect(today[23].endMs - today[0].startMs + 1).toBe(24 * HOUR_MS);
    expect(week).toHaveLength(7);
    expect(week[1].startMs - week[0].startMs).toBe(DAY_MS);
    expect(month).toHaveLength(30);
  });

  test("image usage contributes tokens and duration but never text speed or first-token timing", () => {
    const text = sample();
    const image = sample({
      latencyMs: 10_000, ttftMs: 25, generationMs: 4_500,
      inputTokens: 45, outputTokens: 4_000, totalTokens: 4_045,
      imageUsage: { generatedImages: 1, outputImageTokens: 4_000 },
    });
    const baseline = totalsFromSamples([text]);
    const mixed = totalsFromSamples([text, image]);
    expect(mixed.requests).toBe(2);
    expect(mixed.totalTokens).toBe(4_156);
    expect(mixed.latencyMs).toBe(10_200);
    for (const field of ["ttftMs", "ttftSamples", "generationMs", "generationSamples",
      "generationOutputTokens", "speedOutputTokens", "speedDurationMs"] as const) {
      expect(mixed[field]).toBe(baseline[field]);
      expect(totalsFromSamples([image])[field]).toBe(0);
    }
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
      cacheRateInputTokens: 200,
      cacheRateCachedInputTokens: 50,
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

  test("weights generation speed by observed duration and excludes failures and buffered outliers", () => {
    const totals = totalsFromSamples([
      sample({ outputTokens: 30, reasoningTokens: 0, generationMs: 500 }),
      sample({ outputTokens: 20, reasoningTokens: 0, generationMs: 1_500 }),
      sample({ success: false, outputTokens: 100, generationMs: 100 }),
      sample({ outputTokens: 52, reasoningTokens: 0, generationMs: 50 }),
    ]);
    expect(totals).toMatchObject({ generationOutputTokens: 48, generationMs: 2_000, generationSamples: 2 });
    expect(observedTokensPerSecond(totals.generationOutputTokens, totals.generationMs)).toBe(24);
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

  test("uses a readable source host without rejecting manual addresses", () => {
    expect(sourceHost("https://api.example.test/v1")).toBe("api.example.test");
    expect(sourceHost("manual source")).toBe("manual source");
  });

  test("reads an explicit port without rejecting a malformed address", () => {
    expect(sourcePort("http://127.0.0.1:4317/v1")).toBe("4317");
    expect(sourcePort("partial gateway address")).toBe("");
  });
});

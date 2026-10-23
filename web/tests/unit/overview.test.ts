import { describe, expect, test } from "bun:test";
import { overviewTimeline } from "../../src/features/overview/timeline";
import type { OverviewRange } from "../../src/features/overview/timeline";
import { usageTotals } from "../../src/features/usage/model";
import { previewRequests } from "../../src/preview/fixtures";

const asOf = "2026-10-20T14:32:00Z";
const sample = previewRequests.find(row => row.accounting === "posted")!;
const at = (startedAt: string) => ({ ...sample, startedAt });

describe("overview UTC timeline", () => {
  test("today excludes yesterday, the snapshot boundary and future requests", () => {
    const result = overviewTimeline([
      at("2026-10-19T23:59:59.999Z"), at("2026-10-20T00:00:00Z"),
      at("2026-10-20T01:00:00Z"), at("2026-10-20T14:31:59.999Z"),
      at(asOf), at("2026-10-20T15:00:00Z"), at("2026-10-21T00:00:00Z"),
    ], "today", asOf);
    expect(result.rows).toHaveLength(3);
    expect(result.buckets).toHaveLength(15);
    expect(result.buckets[0]!.rows).toHaveLength(1);
    expect(result.buckets[1]!.rows).toHaveLength(1);
    expect(result.buckets[0]!.partial).toBe(false);
    expect(result.buckets.at(-1)!.rows).toHaveLength(1);
    expect(result.buckets.at(-1)!.endMs).toBe(Date.parse(asOf));
    expect(result.buckets.at(-1)!.partial).toBe(true);
    expect(result.buckets.at(-1)!.interval).toBe("20.10.2026, 14:00 — 20.10.2026, 14:32 UTC");
  });

  test("7 and 30 days retain exact UTC calendar windows across month and DST boundaries", () => {
    const snapshot = "2026-11-02T04:30:00-05:00";
    for (const [range, start, count] of [
      ["week", "2026-10-27T00:00:00Z", 7], ["month", "2026-10-04T00:00:00Z", 30],
    ] as const) {
      const result = overviewTimeline([at(start), at(snapshot), at("2026-11-02T09:29:59Z")], range, snapshot);
      expect(result.startMs).toBe(Date.parse(start));
      expect(result.endMs).toBe(Date.parse("2026-11-02T09:30:00Z"));
      expect(result.rows).toHaveLength(2);
      expect(result.buckets).toHaveLength(count);
      expect(result.stepMs).toBe(86400000);
      expect(result.buckets.at(-1)!.partial).toBe(true);
      expect(result.buckets.slice(0, -1).every(bucket => bucket.endMs - bucket.startMs === 86400000)).toBe(true);
    }
  });

  test("an exact hour and midnight do not add future observations", () => {
    const exact = overviewTimeline([at("2026-10-20T13:59:59Z"), at("2026-10-20T14:00:00Z")], "today", "2026-10-20T14:00:00Z");
    expect(exact.buckets).toHaveLength(14);
    expect(exact.buckets.at(-1)!.partial).toBe(false);
    expect(exact.rows).toHaveLength(1);
    const midnight = overviewTimeline([at("2026-10-20T00:00:00Z")], "today", "2026-10-20T00:00:00Z");
    expect(midnight.rows).toHaveLength(0);
    expect(midnight.buckets).toHaveLength(1);
    expect(midnight.buckets[0]!.rows).toHaveLength(0);
  });

  test("all filtered records occur once and bucket charges reconcile with the period", () => {
    for (const range of ["today", "week", "month"] satisfies OverviewRange[]) {
      for (const model of ["all", "demo-gpt", "missing"]) {
        const { rows, buckets } = overviewTimeline(previewRequests, range, asOf, model);
        expect(buckets.flatMap(bucket => bucket.rows).map(row => row.id).sort()).toEqual(rows.map(row => row.id).sort());
        expect(buckets.reduce((sum, bucket) => sum + usageTotals(bucket.rows).charge!, 0n)).toBe(usageTotals(rows).charge!);
        expect(rows.every(row => model === "all" || row.model.id === model)).toBe(true);
      }
    }
  });

  test("empty time buckets stay unknown for speed and speeds are duration-weighted", () => {
    const first = { ...at("2026-10-20T08:00:00Z"), tokens: { ...sample.tokens, output: 100 }, timing: { ...sample.timing, generationMs: 1000, totalMs: 2000 } };
    const second = { ...at("2026-10-20T08:30:00Z"), tokens: { ...sample.tokens, output: 100 }, timing: { ...sample.timing, generationMs: 9000, totalMs: 10000 } };
    const { buckets } = overviewTimeline([first, second], "today", asOf);
    expect(usageTotals(buckets[7]!.rows).generation).toBeNull();
    expect(usageTotals(buckets[8]!.rows).generation).toBe(20);
    expect(usageTotals(buckets[8]!.rows).e2e).toBeCloseTo(200 / 12);
  });

  test("invalid snapshots are rejected", () => {
    expect(() => overviewTimeline([], "today", "invalid")).toThrow(RangeError);
  });
});

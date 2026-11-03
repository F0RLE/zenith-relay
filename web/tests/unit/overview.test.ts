import { describe, expect, test } from "bun:test";
import { overviewTimeline } from "../../src/features/overview/timeline";
import type { OverviewRange } from "../../src/features/overview/timeline";
import { usageTotals } from "../../src/features/usage/model";
import { previewRequests } from "../../src/preview/fixtures";

const asOf = "2026-10-20T14:32:00Z";
const sample = previewRequests.find(row => row.accounting === "posted")!;
const at = (startedAt: string) => ({ ...sample, startedAt });

describe("overview rolling timeline", () => {
  test("24 hours includes yesterday at the exact cutoff, not calendar midnight", () => {
    const result = overviewTimeline([
      at("2026-10-19T14:31:59.999Z"), at("2026-10-19T14:32:00Z"),
      at("2026-10-19T15:32:00Z"), at("2026-10-20T14:31:59.999Z"),
      at(asOf), at("2026-10-20T15:00:00Z"), at("invalid"),
    ], "24h", asOf);
    expect(result.rows).toHaveLength(3);
    expect(result.startMs).toBe(Date.parse("2026-10-19T14:32:00Z"));
    expect(result.endMs - result.startMs).toBe(86400000);
    expect(result.buckets).toHaveLength(24);
    expect(result.buckets[0]!.rows).toHaveLength(1);
    expect(result.buckets[1]!.rows).toHaveLength(1);
    expect(result.buckets.at(-1)!.rows).toHaveLength(1);
    expect(result.buckets.at(-1)!.endMs).toBe(Date.parse(asOf));
    expect(result.buckets.every(bucket => bucket.endMs - bucket.startMs === 3600000)).toBe(true);
    expect(result.buckets.at(-1)!.interval).toBe("10/20/2026, 01:32 PM — 10/20/2026, 02:32 PM UTC");
  });

  test("7 and 30 days retain exact elapsed windows across month and DST boundaries", () => {
    const snapshot = "2026-11-02T04:30:00-05:00";
    for (const [range, start, count] of [
      ["week", "2026-10-26T09:30:00Z", 7], ["month", "2026-10-03T09:30:00Z", 30],
    ] as const) {
      const result = overviewTimeline([at(start), at(snapshot), at("2026-11-02T09:29:59Z")], range, snapshot);
      expect(result.startMs).toBe(Date.parse(start));
      expect(result.endMs).toBe(Date.parse("2026-11-02T09:30:00Z"));
      expect(result.rows).toHaveLength(2);
      expect(result.buckets).toHaveLength(count);
      expect(result.stepMs).toBe(86400000);
      expect(result.endMs - result.startMs).toBe(count * 86400000);
      expect(result.buckets.every(bucket => bucket.endMs - bucket.startMs === 86400000)).toBe(true);
    }
  });

  test("an exact hour and midnight do not add future observations", () => {
    const exact = overviewTimeline([at("2026-10-20T13:59:59Z"), at("2026-10-20T14:00:00Z")], "24h", "2026-10-20T14:00:00Z");
    expect(exact.buckets).toHaveLength(24);
    expect(exact.rows).toHaveLength(1);
    const midnight = overviewTimeline([at("2026-10-19T23:59:59Z"), at("2026-10-20T00:00:00Z")], "24h", "2026-10-20T00:00:00Z");
    expect(midnight.rows).toHaveLength(1);
    expect(midnight.buckets).toHaveLength(24);
    expect(midnight.buckets.at(-1)!.rows).toHaveLength(1);
    expect(midnight.startMs).toBe(Date.parse("2026-10-19T00:00:00Z"));
  });

  test("all filtered records occur once and bucket charges reconcile with the period", () => {
    for (const range of ["24h", "week", "month", "all"] satisfies OverviewRange[]) {
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
    const { buckets } = overviewTimeline([first, second], "24h", asOf);
    expect(usageTotals(buckets[0]!.rows).generation).toBeNull();
    const measured = buckets.find(bucket => bucket.rows.length)!;
    expect(usageTotals(measured.rows).generation).toBe(20);
    expect(usageTotals(measured.rows).e2e).toBeCloseTo(200 / 12);
  });

  test("invalid snapshots are rejected", () => {
    expect(() => overviewTimeline([], "24h", "invalid")).toThrow(RangeError);
  });

  test("all time includes old history once and keeps a bounded, continuous chart", () => {
    const endMs = Date.parse(asOf);
    for (const durationMs of [1, 3600000, 86400000, 86400001, 30 * 86400000, 3650 * 86400000]) {
      const earliest = new Date(endMs - durationMs).toISOString();
      const records = [at(earliest), at(new Date(endMs - 1).toISOString()), at(asOf), at("2027-01-01T00:00:00Z"), at("invalid")];
      const result = overviewTimeline(records, "all", asOf);
      expect(result.rows).toHaveLength(2);
      expect(result.startMs).toBeLessThanOrEqual(Date.parse(earliest));
      expect(result.endMs).toBe(endMs);
      expect(result.buckets.length).toBeGreaterThanOrEqual(1);
      expect(result.buckets.length).toBeLessThanOrEqual(30);
      expect(result.buckets.flatMap(bucket => bucket.rows)).toEqual(result.rows);
      expect(result.buckets.reduce((sum, bucket) => sum + usageTotals(bucket.rows).charge!, 0n)).toBe(usageTotals(result.rows).charge!);
      result.buckets.forEach((bucket, index) => {
        expect(bucket.endMs - bucket.startMs).toBe(result.stepMs);
        expect(bucket.endMs).toBeLessThanOrEqual(endMs);
        if (index) expect(bucket.startMs).toBe(result.buckets[index - 1]!.endMs);
      });
      expect(result.buckets.at(-1)!.endMs).toBe(endMs);
      const filtered = overviewTimeline(records, "all", asOf, "missing");
      expect(filtered.startMs).toBe(result.startMs);
      expect(filtered.rows).toEqual([]);
    }
  });

  test("all time with empty, invalid or future-only history remains finite and empty", () => {
    for (const records of [[], [at("invalid")], [at(asOf), at("2027-01-01T00:00:00Z")]]) {
      const result = overviewTimeline(records, "all", asOf);
      expect(result.rows).toEqual([]);
      expect(result.buckets).toHaveLength(1);
      expect(result.startMs).toBe(Date.parse(asOf) - 3600000);
      expect(result.buckets[0]!.rows).toEqual([]);
    }
    expect(() => overviewTimeline([], "all", "invalid")).toThrow(RangeError);
  });
});

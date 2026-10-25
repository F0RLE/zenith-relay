import { describe, expect, test } from "bun:test";
import type { WeeklyCapacityGroup, WeeklyCapacityReport } from "../src/features/relay/api/types/weeklyCapacity";
import { cachedCapacity, capacityCurrent, capacityGroupOptions, loadCapacity, nextCapacityExpiry } from "../src/features/relay/pages/overview/weeklyCapacityModel";

const DAY = 86_400_000;
const NOW = 2_000_000_000_000;
function report(id = "synthetic"): WeeklyCapacityReport {
  const group: WeeklyCapacityGroup = {
    id, plan: "plus", workload: "model · default", cacheBand: 50, outputBand: 10,
    pricingRevision: "synthetic", points: [], stale: false,
    current: { atMs: NOW, observedAtMs: NOW, sampleStartMs: NOW - 60_000,
      validUntilMs: NOW + 3 * DAY, medianMicroUsd: 100_000_000,
      lowMicroUsd: 90_000_000, highMicroUsd: 110_000_000, accountCount: 3, consumedBasisPoints: 1500 },
  };
  return { generatedAtMs: NOW, events: [], groups: [group], summaries: [{
    plan: group.plan, pricingRevision: group.pricingRevision,
    points: group.points, current: group.current, stale: group.stale,
  }] };
}

describe("weekly capacity", () => {
  test("model choices stay short without merging distinct measurements", () => {
    const group = report().groups[0]!;
    const options = capacityGroupOptions([
      { ...group, id: "short", workload: "model · standard · short · high" },
      { ...group, id: "long", workload: "model · standard · long · high" },
      { ...group, id: "other", workload: "other-model · standard · short · high" },
      { ...group, id: "mixed", workload: "model · standard · short · high + model · priority · long · high + other-model · standard · short · high" },
    ]);
    expect(options).toEqual([
      { value: "short", label: "model · 1" },
      { value: "long", label: "model · 2" },
      { value: "other", label: "other-model" },
      { value: "mixed", label: "model + other-model" },
    ]);
    expect(capacityGroupOptions([group])).toEqual([{ value: group.id, label: "model" }]);
    expect(capacityGroupOptions([])).toEqual([]);
  });

  test("marks 24-hour evidence stale and hides expired/future estimates", () => {
    const group = report().groups[0]!;
    expect(capacityCurrent(group, NOW)?.stale).toBe(false);
    expect(capacityCurrent(group, NOW + DAY)?.stale).toBe(true);
    expect(capacityCurrent(group, NOW + 3 * DAY)).toBeNull();
    expect(capacityCurrent(group, NOW - 1)).toBeNull();
    const expired: WeeklyCapacityGroup = { ...group, current: { ...group.current!, validUntilMs: NOW } };
    expect(capacityCurrent(expired, NOW)).toBeNull();
    expect(capacityCurrent(undefined, NOW)).toBeNull();
  });

  test("schedules only freshness/reset deadlines, with no provider polling", () => {
    expect(nextCapacityExpiry(report(), NOW)).toBe(NOW + DAY - 60_000);
    expect(nextCapacityExpiry(report(), NOW + DAY)).toBe(NOW + 3 * DAY);
    expect(nextCapacityExpiry(report(), NOW + 3 * DAY)).toBeNull();
    const summaryOnly = { ...report(), groups: [] };
    expect(nextCapacityExpiry(summaryOnly, NOW)).toBe(NOW + DAY - 60_000);
    expect(capacityCurrent(summaryOnly.summaries[0], NOW)?.point.medianMicroUsd).toBe(100_000_000);
    expect(capacityCurrent(summaryOnly.summaries[0], NOW + 3 * DAY)).toBeNull();
  });

  test("deduplicates requests and caches navigation but refreshes a new revision", async () => {
    let calls = 0;
    const load = async () => { calls += 1; return report(); };
    const [first, second] = await Promise.all([loadCapacity("dedup", 1, load, NOW), loadCapacity("dedup", 1, load, NOW)]);
    expect(first).toBe(second);
    expect(calls).toBe(1);
    expect(await loadCapacity("dedup", 1, load, NOW + 1)).toBe(first);
    expect(cachedCapacity("dedup", 2)).toBeNull();
    await loadCapacity("dedup", 2, load, NOW + 2);
    expect(calls).toBe(2);
    await loadCapacity("dedup", 2, load, NOW + 301_000);
    expect(calls).toBe(3);
  });

  test("older in-flight results cannot replace a newer or restarted native session", async () => {
    let release!: (value: WeeklyCapacityReport) => void;
    const old = loadCapacity("race", 8, () => new Promise((resolve) => { release = resolve; }), NOW);
    const fresh = report("restarted");
    await loadCapacity("race", 0, async () => fresh, NOW);
    release(report("old"));
    await old;
    expect(cachedCapacity("race", 0)).toBe(fresh);
    expect(cachedCapacity("race", 8)).toBeNull();
  });

  test("failed loads can retry and cache size stays bounded", async () => {
    await expect(loadCapacity("retry", 0, async () => { throw new Error("synthetic"); }, NOW)).rejects.toThrow("synthetic");
    await loadCapacity("retry", 0, async () => report(), NOW);
    for (let i = 0; i < 9; i += 1) await loadCapacity(`bounded-${i}`, 0, async () => report(), NOW);
    expect(cachedCapacity("bounded-0", 0)).toBeNull();
    expect(cachedCapacity("bounded-8", 0)).not.toBeNull();
  });
});

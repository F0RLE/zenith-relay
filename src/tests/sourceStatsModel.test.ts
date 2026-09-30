import { describe, expect, test } from "bun:test";
import type { SourceStats } from "../src/features/relay/api/types";
import { formatSourceAmount, projectedSourceStats, settledSourceStats, sourceStatsAmounts, sourceStatsStatus } from "../src/features/relay/sourceStatsModel";

const legacy: SourceStats = { provider: "zenith", balanceMicroUsd: 0, spentMicroUsd: 1_000_000, requests: null, totalTokens: null };

describe("provider statistics presentation", () => {
  test("cached snapshots update the value without settling a pending manual read", () => {
    const current = { value: { ...legacy, asOfMs: 20 }, loading: true, failed: false };
    const newer = { ...legacy, balanceMicroUsd: 17_000_000, asOfMs: 30 };
    expect(projectedSourceStats(current, newer)).toEqual({ value: newer, loading: true, failed: false });
    expect(projectedSourceStats(current, { ...legacy, asOfMs: 10 })).toBe(current);
  });
  test("old servers retain real zero balances", () => {
    expect(sourceStatsStatus(legacy)).toBe("available");
    expect(sourceStatsAmounts(legacy)).toEqual([{ currency: "USD", balanceMicros: 0, spentMicros: 1_000_000 }]);
  });
  test("provider units take precedence over legacy USD", () => {
    const amounts = [{ currency: "CNY" as const, balanceMicros: 2_000_000, spentMicros: null }];
    expect(sourceStatsAmounts({ ...legacy, amounts })).toEqual(amounts);
    expect(formatSourceAmount(2_000_000, "CNY", "en-US")).toBe("CN¥2.00");
    expect(formatSourceAmount(2_000_000, "CREDITS", "en-US")).toBe("2");
  });
  test("refresh failure retains successful values and marks them stale", () => {
    const failure: SourceStats = { ...legacy, balanceMicroUsd: null, status: "rate_limited" };
    expect(settledSourceStats(legacy, failure)).toEqual({ value: { ...legacy, stale: true, refreshError: "rate_limited" }, loading: false, failed: true, error: "rate_limited" });
    expect(sourceStatsAmounts(failure)).toEqual([]);
  });
  test("server-retained last success remains stale after remount without a new provider read", () => {
    const retained: SourceStats = { ...legacy, asOfMs: 123, stale: true, refreshError: "unavailable" };
    expect(settledSourceStats(null, retained)).toEqual({ value: retained, loading: false, failed: true, error: "unavailable" });
  });
  test("unsupported response clears previous values", () => {
    const unsupported: SourceStats = { ...legacy, provider: "unsupported" };
    expect(sourceStatsStatus(unsupported)).toBe("unsupported");
    expect(settledSourceStats(legacy, unsupported)).toEqual({ value: unsupported, loading: false, failed: false });
  });
  test("desktop defaults for old unsupported responses retain their meaning", () => {
    const unsupported: SourceStats = { ...legacy, provider: "unsupported", status: "available" };
    expect(sourceStatsStatus(unsupported)).toBe("unsupported");
    expect(sourceStatsAmounts(unsupported)).toEqual([]);
    expect(settledSourceStats(legacy, unsupported)).toEqual({ value: unsupported, loading: false, failed: false });
    expect(sourceStatsStatus({ ...unsupported, status: "unauthorized" })).toBe("unauthorized");
  });
});

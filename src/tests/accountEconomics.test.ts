import { describe, expect, test } from "bun:test";
import { buildAccountValueProjection, estimateRemainingApiEquivalent, formatAccountPayback } from "../src/features/relay/accountEconomics";
import type { QuotaWindowUsage } from "../src/features/relay/api/types";
import { calibratedWeek } from "./accountEconomics.fixture";

describe("account value projection", () => {
  test("keeps the same display projection for Connections and Usage", () => {
    const usage = { microUsd: 10_000_000, unpricedTokens: 0 };
    const projection = buildAccountValueProjection(usage, undefined, undefined, 5_000_000);

    expect(projection).toEqual({
      purchaseCostMicroUsd: 5_000_000,
      remainingApiEquivalent: null,
      payback: 2,
      approximate: false,
    });
    expect(buildAccountValueProjection(usage, undefined, undefined, 5_000_000)).toEqual(projection);
  });

  test("marks payback as approximate when some tokens are unpriced", () => {
    expect(buildAccountValueProjection({ microUsd: 10_000_000, unpricedTokens: 3 }, undefined, undefined, 5_000_000)).toEqual({
      purchaseCostMicroUsd: 5_000_000,
      remainingApiEquivalent: null,
      payback: 2,
      approximate: true,
    });
  });

  test("formats the shared payback display without changing its approximation marker", () => {
    expect(formatAccountPayback(null, "en-US")).toBe("—");
    expect(formatAccountPayback(1.25, "en-US")).toBe("125%");
    expect(formatAccountPayback(1.25, "en-US", true)).toBe("≈125%");
  });

  test("preserves missing and zero purchase-cost behavior", () => {
    const usage = { microUsd: 10_000_000, unpricedTokens: 0 };
    expect(buildAccountValueProjection(usage)).toMatchObject({ purchaseCostMicroUsd: null, remainingApiEquivalent: null, payback: null });
    expect(buildAccountValueProjection(usage, undefined, undefined, 0)).toMatchObject({ purchaseCostMicroUsd: 0, remainingApiEquivalent: null, payback: null });
  });

  const now = Date.now();
  const quota = {
    primary: null,
    secondary: {
      kind: "secondary" as const,
      availableBasisPoints: 5_800,
      explicitlyFull: false,
      resetAtMs: now + 6 * 86_400_000,
      windowStartMs: now - 86_400_000,
      windowMinutes: 10_080,
      observedAtMs: now,
    },
  };
  const evidence = calibratedWeek(now);

  test("applies the shared median to the remaining weekly fraction", () => {
    expect(estimateRemainingApiEquivalent(quota, evidence)).toMatchObject({
      microUsd: 5_800_000,
      lowMicroUsd: 4_640_000, highMicroUsd: 6_960_000,
      approximate: true,
      windowKind: "secondary",
      windowMinutes: 10_080,
    });
  });

  test("comparable accounts have equal forecasts regardless of lifetime usage", () => {
    const first = buildAccountValueProjection({ microUsd: 1_000_000, unpricedTokens: 0 }, quota, evidence, 1_000_000);
    const second = buildAccountValueProjection({ microUsd: 90_000_000, unpricedTokens: 0 }, quota, { ...evidence, ledgerKey: "other" }, 1_000_000);
    expect(first.remainingApiEquivalent).toEqual(second.remainingApiEquivalent);
    expect(first.payback).toBe(1);
    expect(second.payback).toBe(90);
  });

  test("a fresh or unused account can use an established shared calibration", () => {
    expect(estimateRemainingApiEquivalent(quota, { ...evidence, availableBasisPoints: 10_000 })?.microUsd).toBe(10_000_000);
    expect(estimateRemainingApiEquivalent(quota, { ...evidence, availableBasisPoints: 0 })?.microUsd).toBe(0);
  });

  test("does not substitute a monthly window for the weekly estimate", () => {
    const monthlyMinutes = 30 * 24 * 60;
    expect(estimateRemainingApiEquivalent({
      ...quota,
      secondary: { ...quota.secondary, windowMinutes: monthlyMinutes },
    }, { ...evidence, windowMinutes: monthlyMinutes })).toBeNull();
  });

  test("rejects malformed and stale native window evidence", () => {
    expect(estimateRemainingApiEquivalent(quota, { ...evidence, observedAtMs: 9_999 })).toBeNull();
    expect(estimateRemainingApiEquivalent(quota, { ...evidence, windowStartMs: now + 1 })).toBeNull();
    for (const availableBasisPoints of [-1, 10_001, Number.NaN, 100.5]) {
      expect(estimateRemainingApiEquivalent(quota, { ...evidence, availableBasisPoints })).toBeNull();
    }
  });

  test("expires forecasts at reset or after 72 hours and rejects future evidence", () => {
    expect(estimateRemainingApiEquivalent(quota, evidence, now + 3 * 86_400_000)).toBeNull();
    expect(estimateRemainingApiEquivalent(quota, evidence, now - 1)).toBeNull();
    expect(estimateRemainingApiEquivalent(quota, { ...evidence, resetAtMs: now }, now)).toBeNull();
    expect(estimateRemainingApiEquivalent(quota, evidence, evidence.calibration.validUntilMs)).toBeNull();
    expect(estimateRemainingApiEquivalent(quota, evidence, now + 60_000)).not.toBeNull();
  });

  test("uses a provider-reported direct balance without extrapolation", () => {
    expect(estimateRemainingApiEquivalent({
      ...quota,
      directBalanceMicroUsd: 42_000_000,
    }, evidence)).toEqual({
      microUsd: 42_000_000,
      approximate: false,
      windowKind: null,
      windowMinutes: null,
    });
  });

  test("linked SIWC application ceilings cannot replace the shared subscription evidence", () => {
    expect(estimateRemainingApiEquivalent({ ...quota, secondary: { ...quota.secondary, availableBasisPoints: 100, windowMinutes: 300 } }, evidence)?.microUsd).toBe(5_800_000);
  });

  test("does not fall back to legacy per-account usage when calibration is missing", () => {
    const legacy = { kind: "secondary", windowStartMs: evidence.windowStartMs, observedAtMs: now, windowMinutes: 10_080,
      apiEquivalent: { microUsd: 5_210_000, pricedTokens: 90, unpricedTokens: 10 },
      recentConsumedBasisPoints: 1_000, recentApiEquivalent: { microUsd: 500_000, pricedTokens: 100, unpricedTokens: 0 } };
    expect(estimateRemainingApiEquivalent(quota, legacy as unknown as QuotaWindowUsage)).toBeNull();
    expect(estimateRemainingApiEquivalent(quota, undefined)).toBeNull();
  });

  test("rejects empty, invalid, future or expired calibration", () => {
    for (const patch of [{ accountCount: 0 }, { medianMicroUsd: Number.NaN }, { lowMicroUsd: 12_000_001 },
      { highMicroUsd: 1 }, { profileId: "" }, { observedAtMs: now + 1 }, { validUntilMs: now }]) {
      expect(estimateRemainingApiEquivalent(quota, { ...evidence, calibration: { ...evidence.calibration, ...patch } }, now)).toBeNull();
    }
  });

  test("scales integer microUSD without losing precision in the intermediate product", () => {
    const cost = Number.MAX_SAFE_INTEGER;
    const value = estimateRemainingApiEquivalent(quota, { ...evidence, availableBasisPoints: 9_999,
      calibration: { ...evidence.calibration, lowMicroUsd: cost, medianMicroUsd: cost, highMicroUsd: cost } });
    expect(value?.microUsd).toBe(Number((BigInt(cost) * 9_999n + 5_000n) / 10_000n));
  });
});

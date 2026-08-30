import { describe, expect, test } from "bun:test";
import { buildAccountValueProjection, estimateRemainingApiEquivalent, formatAccountPayback } from "../src/features/relay/accountEconomics";

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

  const quota = {
    primary: null,
    secondary: {
      kind: "secondary" as const,
      availableBasisPoints: 5_800,
      explicitlyFull: false,
      resetAtMs: 20_000,
      windowStartMs: 1_000,
      windowMinutes: 10_080,
      observedAtMs: 10_000,
    },
  };
  const evidence = {
    kind: "secondary" as const,
    windowStartMs: 1_000,
    observedAtMs: 10_000,
    windowMinutes: 10_080,
    apiEquivalent: { microUsd: 5_210_000, pricedTokens: 90, unpricedTokens: 10 },
  };

  test("estimates the remaining long-window value from aligned Relay usage", () => {
    expect(estimateRemainingApiEquivalent(quota, evidence)).toEqual({
      microUsd: 7_194_762,
      approximate: true,
      windowKind: "secondary",
      windowMinutes: 10_080,
    });
  });

  test("withholds the estimate below five percent observed usage", () => {
    expect(estimateRemainingApiEquivalent({
      ...quota,
      secondary: { ...quota.secondary, availableBasisPoints: 9_600 },
    }, evidence)).toBeNull();
  });

  test("withholds the estimate below eighty percent price coverage", () => {
    expect(estimateRemainingApiEquivalent(quota, {
      ...evidence,
      apiEquivalent: { ...evidence.apiEquivalent, pricedTokens: 79, unpricedTokens: 21 },
    })).toBeNull();
  });

  test("does not substitute a monthly window for the weekly estimate", () => {
    const monthlyMinutes = 30 * 24 * 60;
    expect(estimateRemainingApiEquivalent({
      ...quota,
      secondary: { ...quota.secondary, windowMinutes: monthlyMinutes },
    }, { ...evidence, windowMinutes: monthlyMinutes })).toBeNull();
  });

  test("withholds evidence from a different observation or quota cycle", () => {
    expect(estimateRemainingApiEquivalent(quota, { ...evidence, observedAtMs: 9_999 })).toBeNull();
    expect(estimateRemainingApiEquivalent(quota, { ...evidence, windowStartMs: 999 })).toBeNull();
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
});

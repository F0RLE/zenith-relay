import { describe, expect, test } from "bun:test";
import { buildAccountValueProjection } from "../src/features/relay/accountEconomics";

describe("account value projection", () => {
  const quota = {
    primary: null,
    secondary: {
      kind: "secondary" as const,
      availableBasisPoints: 5_700,
      explicitlyFull: false,
      resetAtMs: 2_000,
      windowStartMs: 1_000,
      windowMinutes: 10_080,
      observedAtMs: 1_500,
    },
  };
  const windowUsage = {
    kind: "secondary" as const,
    windowStartMs: 1_000,
    apiEquivalent: { microUsd: 10_000_000, pricedTokens: 100, unpricedTokens: 0 },
  };

  test("keeps the same display projection for Connections and Usage", () => {
    const usage = { microUsd: 10_000_000, unpricedTokens: 0 };
    const projection = buildAccountValueProjection(usage, quota, windowUsage, 5_000_000);

    expect(projection).toEqual({
      purchaseCostMicroUsd: 5_000_000,
      remainingApiEquivalent: { microUsd: 13_255_814, approximate: true, windowKind: "secondary" },
      payback: 2,
      approximate: false,
    });
    expect(buildAccountValueProjection(usage, quota, windowUsage, 5_000_000)).toEqual(projection);
  });

  test("does not estimate remaining quota when the current window has unpriced tokens", () => {
    expect(buildAccountValueProjection({ microUsd: 10_000_000, unpricedTokens: 3 }, quota, {
      ...windowUsage,
      apiEquivalent: { ...windowUsage.apiEquivalent, unpricedTokens: 3 },
    }, 5_000_000)).toEqual({
      purchaseCostMicroUsd: 5_000_000,
      remainingApiEquivalent: null,
      payback: 2,
      approximate: true,
    });
  });

  test("preserves missing and zero purchase-cost behavior", () => {
    const usage = { microUsd: 10_000_000, unpricedTokens: 0 };
    expect(buildAccountValueProjection(usage)).toMatchObject({ purchaseCostMicroUsd: null, remainingApiEquivalent: null, payback: null });
    expect(buildAccountValueProjection(usage, undefined, undefined, 0)).toMatchObject({ purchaseCostMicroUsd: 0, remainingApiEquivalent: null, payback: null });
  });

  test("uses a provider-reported direct balance without extrapolation", () => {
    expect(buildAccountValueProjection({ microUsd: 0, unpricedTokens: 0 }, {
      ...quota,
      directBalanceMicroUsd: 42_000_000,
    })).toMatchObject({
      remainingApiEquivalent: { microUsd: 42_000_000, approximate: false, windowKind: null },
    });
  });
});

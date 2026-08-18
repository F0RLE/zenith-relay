import { describe, expect, test } from "bun:test";
import { buildAccountValueProjection, estimateAccountPotential } from "../src/features/relay/accountEconomics";

function quota(primary: number | null, secondary: number | null) {
  const window = (kind: "primary" | "secondary", availableBasisPoints: number | null) => availableBasisPoints == null ? null : {
    kind,
    availableBasisPoints,
    explicitlyFull: false,
    providerCycleId: "cycle-1",
    resetAtMs: 18_000_001,
    windowMinutes: 300,
    windowStartMs: 1,
    observedAtMs: 1,
  };
  return { primary: window("primary", primary), secondary: window("secondary", secondary), directBalanceMicroUsd: null };
}

describe("account potential", () => {
  test("estimates remaining value from one reported quota window", () => {
    expect(estimateAccountPotential({ microUsd: 10_000_000, unpricedTokens: 0 }, quota(5_000, null))).toEqual({
      microUsd: 10_000_000,
      approximate: false,
    });
  });

  test("uses the smallest estimate from primary and secondary windows", () => {
    expect(estimateAccountPotential({ microUsd: 14_100_000, unpricedTokens: 0 }, quota(7_200, 6_400))).toEqual({
      microUsd: 25_066_667,
      approximate: false,
    });
  });

  test("keeps an exhausted window at zero and ignores a full or unknown window", () => {
    expect(estimateAccountPotential({ microUsd: 10_000_000, unpricedTokens: 0 }, quota(0, 10_000))).toEqual({
      microUsd: 0,
      approximate: false,
    });
    expect(estimateAccountPotential({ microUsd: 10_000_000, unpricedTokens: 0 }, quota(10_000, null))).toBeNull();
    expect(estimateAccountPotential({ microUsd: 10_000_000, unpricedTokens: 0 }, quota(null, null))).toBeNull();
  });

  test("fails closed when some usage is unpriced", () => {
    expect(estimateAccountPotential({ microUsd: 10_000_000, unpricedTokens: 42 }, quota(5_000, null))).toBeNull();
  });

  test("does not invent potential without priced usage", () => {
    expect(estimateAccountPotential({ microUsd: 0, unpricedTokens: 100 }, quota(5_000, null))).toBeNull();
  });

  test("prefers a direct provider balance over percentage math", () => {
    expect(estimateAccountPotential({ microUsd: 10_000_000, unpricedTokens: 42 }, {
      ...quota(null, null),
      directBalanceMicroUsd: 2_500_000,
    })).toEqual({ microUsd: 2_500_000, approximate: false });
  });
});

describe("account value projection", () => {
  test("keeps the same display projection for Connections and Usage", () => {
    const usage = { microUsd: 10_000_000, unpricedTokens: 0 };
    const limits = quota(5_000, null);
    const projection = buildAccountValueProjection(usage, limits, 5_000_000);

    expect(projection).toEqual({
      purchaseCostMicroUsd: 5_000_000,
      potential: { microUsd: 10_000_000, approximate: false },
      payback: 2,
      approximate: false,
    });
    expect(buildAccountValueProjection(usage, limits, 5_000_000)).toEqual(projection);
  });

  test("marks values approximate when usage contains unpriced tokens", () => {
    expect(buildAccountValueProjection({ microUsd: 10_000_000, unpricedTokens: 3 }, quota(5_000, null), 5_000_000)).toEqual({
      purchaseCostMicroUsd: 5_000_000,
      potential: null,
      payback: 2,
      approximate: true,
    });
  });

  test("preserves missing and zero purchase-cost behavior", () => {
    const usage = { microUsd: 10_000_000, unpricedTokens: 0 };
    const limits = quota(5_000, null);

    expect(buildAccountValueProjection(usage, limits)).toMatchObject({ purchaseCostMicroUsd: null, payback: null });
    expect(buildAccountValueProjection(usage, limits, 0)).toMatchObject({ purchaseCostMicroUsd: 0, payback: null });
  });

  test("keeps quota-derived potential absent when all windows are full or unknown", () => {
    const usage = { microUsd: 10_000_000, unpricedTokens: 0 };
    expect(buildAccountValueProjection(usage, quota(10_000, null), 5_000_000).potential).toBeNull();
    expect(buildAccountValueProjection(usage, quota(null, null), 5_000_000).potential).toBeNull();
  });
});

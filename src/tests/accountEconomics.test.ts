import { describe, expect, test } from "bun:test";
import { estimateAccountPotential } from "../src/features/relay/accountEconomics";

function quota(primary: number | null, secondary: number | null) {
  const window = (kind: "primary" | "secondary", availableBasisPoints: number | null) => availableBasisPoints == null ? null : {
    kind,
    availableBasisPoints,
    explicitlyFull: false,
    resetAtMs: null,
    windowMinutes: 300,
    observedAtMs: 1,
  };
  return { primary: window("primary", primary), secondary: window("secondary", secondary) };
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

  test("marks the estimate approximate when some usage is unpriced", () => {
    expect(estimateAccountPotential({ microUsd: 10_000_000, unpricedTokens: 42 }, quota(5_000, null))).toEqual({
      microUsd: 10_000_000,
      approximate: true,
    });
  });

  test("does not invent potential without priced usage", () => {
    expect(estimateAccountPotential({ microUsd: 0, unpricedTokens: 100 }, quota(5_000, null))).toBeNull();
  });
});

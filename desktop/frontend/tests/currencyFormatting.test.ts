import { describe, expect, test } from "bun:test";
import { pricedUsd } from "../src/features/relay/currencyFormatting";
import { formatUsageApiEquivalent } from "../src/features/relay/pages/usage/usageFormatting";

describe("priced usage display", () => {
  test("keeps a measured per-image cost even without priced tokens", () => {
    for (const unpricedTokens of [0, 120]) {
      const summary = { microUsd: 40_000, pricedTokens: 0, unpricedTokens };
      expect(pricedUsd(summary)).toBe(0.04);
      expect(formatUsageApiEquivalent(summary, "en")).toBe("≈$0.04");
    }
  });

  test("distinguishes unpriced usage from explicitly free token usage", () => {
    const unknown = { microUsd: 0, pricedTokens: 0, unpricedTokens: 120 };
    expect(pricedUsd(unknown)).toBeNull();
    expect(formatUsageApiEquivalent(unknown, "en")).toBe("—");
    expect(pricedUsd({ microUsd: 0, pricedTokens: 120 })).toBe(0);
    expect(pricedUsd({ microUsd: 0, pricedTokens: 0 })).toBeNull();
  });
});

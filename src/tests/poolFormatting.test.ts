import { describe, expect, test } from "bun:test";
import { formatAccountValueMicroUsd, formatApiEquivalent, normalizeReasoningEffort, sortReasoningEfforts } from "../src/features/relay/poolFormatting";

describe("pool currency formatting", () => {
  test("formats account value with optional approximation marker", () => {
    expect(formatAccountValueMicroUsd(1_234_567, "en-US")).toBe("$1.23");
    expect(formatAccountValueMicroUsd(1_234_567, "en-US", true)).toBe("≈$1.23");
    expect(formatAccountValueMicroUsd(12_000, "en-US")).toBe("$0.01");
  });

  test("keeps API-equivalent source values compact enough for pool cards", () => {
    expect(formatApiEquivalent(1_234_567, "en-US")).toBe("≈$1.23");
    expect(formatApiEquivalent(12_000, "en-US")).toBe("≈$0.01");
    expect(formatApiEquivalent(2_000_000, "en-US")).toBe("≈$2.00");
    expect(formatApiEquivalent(3_682_678_150, "en-US")).toBe("≈$3,682.68");
  });
});

describe("reasoning effort identities", () => {
  test("deduplicates very-high aliases without changing other provider modes", () => {
    expect(sortReasoningEfforts(["max", "extra-high", "high", "VERY_HIGH", "xhigh", "VeryHigh", "ExtraHigh", "low", "vendor-mode"])).toEqual([
      "low", "high", "xhigh", "max", "vendor-mode",
    ]);
    expect(normalizeReasoningEffort(" X-HIGH ")).toBe("xhigh");
    expect(normalizeReasoningEffort("Vendor-Mode")).toBe("vendor-mode");
  });
});

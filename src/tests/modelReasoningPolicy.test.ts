import { describe, expect, test } from "bun:test";
import {
  initialReasoningLevels,
  normalizeReasoningLevel,
  toggleReasoningLevel,
} from "../src/features/relay/pages/pool/modelReasoningPolicy";

describe("model reasoning policy", () => {
  test("normalizes detected levels but preserves an explicit empty policy", () => {
    expect(initialReasoningLevels(undefined, ["HIGH", "low"])).toEqual(["low", "high"]);
    expect(initialReasoningLevels([], ["medium"])).toEqual([]);
    expect(initialReasoningLevels(["high", "low"], ["medium"])).toEqual(["low", "high"]);
  });

  test("toggles normalized levels immutably", () => {
    const current = ["low"];
    expect(toggleReasoningLevel(current, " HIGH ")).toEqual(["low", "high"]);
    expect(toggleReasoningLevel(current, "low")).toEqual([]);
    expect(normalizeReasoningLevel("  X_HIGH ")).toBe("x_high");
  });

});

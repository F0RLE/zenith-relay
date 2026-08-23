import { describe, expect, test } from "bun:test";
import type { ModelReasoningProbeResult } from "../src/features/relay/api/types";
import {
  addReasoningLevel,
  initialReasoningLevels,
  mergeSuccessfulProbe,
  normalizeReasoningLevel,
  toggleReasoningLevel,
} from "../src/features/relay/pages/pool/modelReasoningPolicy";

function probe(overrides: Partial<ModelReasoningProbeResult> = {}): ModelReasoningProbeResult {
  return {
    modelId: "model",
    level: "high",
    sourceCount: 1,
    availableCount: 1,
    appliedToSettings: true,
    sources: [],
    ...overrides,
  };
}

describe("model reasoning policy", () => {
  test("normalizes detected levels but preserves an explicit empty policy", () => {
    expect(initialReasoningLevels(undefined, ["HIGH", "low"])).toEqual(["low", "high"]);
    expect(initialReasoningLevels([], ["medium"])).toEqual([]);
    expect(initialReasoningLevels(["high", "low"], ["medium"])).toEqual(["low", "high"]);
  });

  test("toggles and adds normalized levels immutably", () => {
    const current = ["low"];
    expect(toggleReasoningLevel(current, " HIGH ")).toEqual(["low", "high"]);
    expect(toggleReasoningLevel(current, "low")).toEqual([]);
    expect(addReasoningLevel(current, " LOW ")).toBe(current);
    expect(normalizeReasoningLevel("  X_HIGH ")).toBe("x_high");
  });

  test("merges only probes that were applied to settings", () => {
    expect(mergeSuccessfulProbe(["low"], probe())).toEqual(["low", "high"]);
    expect(mergeSuccessfulProbe(["low"], probe({ appliedToSettings: false }))).toEqual(["low"]);
  });
});

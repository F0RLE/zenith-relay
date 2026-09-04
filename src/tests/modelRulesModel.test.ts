import { describe, expect, test } from "bun:test";
import type { ModelSummary } from "../src/features/relay/api/types";
import {
  formatModelDisplayName,
  modelSignature,
  normalizeReasoningSelection,
  reorderById,
  reorderModelGroups,
  supportedReasoningLevels,
  type ModelRuleGroup,
} from "../src/features/relay/pages/pool/modelRulesModel";

const model = (id: string, overrides: Partial<ModelSummary> = {}): ModelSummary => ({
  id,
  enabled: true,
  memberCount: 1,
  codexVisible: true,
  codexDisplayName: id,
  catalogRank: null,
  inputMicroUsdPerMillion: null,
  outputMicroUsdPerMillion: null,
  customPrice: false,
  ...overrides,
});

describe("model rules model", () => {
  test("builds a stable signature from render-affecting model metadata", () => {
    const current = [model("a", { reasoningLevels: ["low"] })];
    expect(modelSignature(current)).toBe(modelSignature(current));
    expect(modelSignature(current)).not.toBe(modelSignature([model("a", { reasoningLevels: ["high"] })]));
  });

  test("reorders rows immutably and rejects missing or equal targets", () => {
    const current = [model("a"), model("b"), model("c")];
    const next = reorderById(current, "a", "c");
    expect(next?.map((item) => item.id)).toEqual(["b", "c", "a"]);
    expect(current.map((item) => item.id)).toEqual(["a", "b", "c"]);
    expect(reorderById(current, "a", "a")).toBeNull();
    expect(reorderById(current, "missing", "a")).toBeNull();
  });

  test("moves complete groups while preserving each group's model order", () => {
    const groups: ModelRuleGroup[] = [
      { id: "one", label: "One", items: [model("a"), model("b")] },
      { id: "two", label: "Two", items: [model("c")] },
      { id: "three", label: "Three", items: [model("d"), model("e")] },
    ];
    const next = reorderModelGroups(groups, "one", "three");
    expect(next?.map((item) => item.id)).toEqual(["c", "d", "e", "a", "b"]);
    expect(groups[0]?.items.map((item) => item.id)).toEqual(["a", "b"]);
  });

  test("formats common provider display names without changing identifiers", () => {
    expect(formatModelDisplayName("gpt 5 4")).toBe("GPT-5.4");
    expect(formatModelDisplayName("claude opus 4 8")).toBe("Claude opus 4.8");
    expect(formatModelDisplayName("o3")).toBe("O3");
  });

  test("normalizes advertised reasoning levels and preserves provider order", () => {
    const current = model("reasoning", {
      reasoningLevels: ["legacy"],
      reasoningSupportedLevels: [" HIGH ", "low", "HIGH", ""],
    });
    const supported = supportedReasoningLevels(current);
    expect(supported).toEqual(["high", "low"]);
    expect(normalizeReasoningSelection(supported, ["low", "stale", "HIGH"])).toEqual(["high", "low"]);
  });

  test("offers manual candidates only when the runtime explicitly permits unknown-model discovery", () => {
    expect(supportedReasoningLevels(model("claude-fable-5-1", {
      reasoningManualFallback: true,
    }))).toEqual(["low", "medium", "high", "xhigh", "max"]);
    expect(supportedReasoningLevels(model("known-non-reasoning", {
      reasoningSupportedLevels: [],
      reasoningLevels: [],
    }))).toEqual([]);
  });
});

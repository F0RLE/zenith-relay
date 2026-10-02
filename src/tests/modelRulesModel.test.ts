import { describe, expect, test } from "bun:test";
import type { ModelSummary } from "../src/features/relay/api/types";
import {
  completeModelDisplayOrder,
  modelSignature,
  modelSpeedTiers,
  modelShowsReasoningControl,
  normalizeReasoningSelection,
  reorderById,
  reorderModelGroups,
  supportedReasoningLevels,
  pendingModelEnabled,
  reconcilePendingModelEnabled,
  clearPendingModelEnabled,
  type ModelRuleGroup,
} from "../src/features/relay/pages/pool/modelRulesModel";

const model = (id: string, overrides: Partial<ModelSummary> = {}): ModelSummary => ({
  id,
  enabled: true,
  memberCount: 1,
  codexVisible: true,
  codexDisplayName: id,
  catalogProvider: "openai",
  catalogFamily: "gpt",
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

  test("keeps the three speed modes for a configurable family", () => {
    expect(modelSpeedTiers(model("gpt", { speedSupported: true, speedTiers: ["standard"] }))).toEqual(["standard", "fast", "ultrafast"]);
    expect(modelSpeedTiers(model("gpt", { speedSupported: true, speedTiers: ["fast", "standard"] }))).toEqual(["standard", "fast"]);
    expect(modelSpeedTiers(model("other", { speedSupported: false }))).toEqual(["standard"]);
  });

  test("hides reasoning when a model has no levels and never offers it for image generation", () => {
    expect(modelShowsReasoningControl(model("gpt-5.4", { reasoningSupportedLevels: ["low", "high"] }))).toBe(true);
    expect(modelShowsReasoningControl(model("gpt-reserve"))).toBe(false);
    expect(modelShowsReasoningControl(model("gpt-image-2", { reasoningSupportedLevels: ["low", "high"], catalogFamily: "gpt-image" }))).toBe(false);
    expect(modelShowsReasoningControl(model("vision", { catalogOutputModalities: ["text"], reasoningLevels: ["medium"] }))).toBe(true);
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

  test("keeps unavailable catalog models when saving a reordered visible group", () => {
    const order = completeModelDisplayOrder(
      [model("gpt-b"), model("gpt-a")],
      [model("gpt-a"), model("gpt-b"), model("gpt-unavailable")],
    );
    expect(order).toEqual(["gpt-b", "gpt-a", "gpt-unavailable"]);
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

  test("does not invent candidates when an automatic catalog has no levels", () => {
    expect(supportedReasoningLevels(model("claude-fable-5-1", {
      reasoningManualFallback: true,
    }))).toEqual([]);
    expect(supportedReasoningLevels(model("known-non-reasoning", {
      reasoningSupportedLevels: [],
      reasoningLevels: [],
    }))).toEqual([]);
  });
});

describe("optimistic model switches", () => {
  test("shows the pending value and clears it only after the runtime agrees", () => {
    const pending = { "gpt-5.4": false, "gpt-5.5": true };
    expect(pendingModelEnabled(pending, { id: "gpt-5.4", enabled: true })).toBeFalse();
    expect(pendingModelEnabled(pending, { id: "other", enabled: true })).toBeTrue();
    const waiting = reconcilePendingModelEnabled(pending, [
      { id: "gpt-5.4", enabled: true },
      { id: "gpt-5.5", enabled: false },
    ]);
    expect(waiting).toBe(pending);
    expect(reconcilePendingModelEnabled(pending, [
      { id: "gpt-5.4", enabled: false },
      { id: "gpt-5.5", enabled: false },
    ])).toEqual({ "gpt-5.5": true });
  });

  test("rolls back only the failed attempt", () => {
    const pending = { "gpt-5.4": false, "gpt-5.5": true };
    expect(clearPendingModelEnabled(pending, "gpt-5.4", true)).toBe(pending);
    expect(clearPendingModelEnabled(pending, "gpt-5.4", false)).toEqual({ "gpt-5.5": true });
  });
});


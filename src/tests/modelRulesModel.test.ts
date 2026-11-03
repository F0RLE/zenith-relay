import { describe, expect, test } from "bun:test";
import type { ModelSummary } from "../src/features/relay/api/types";
import { orderModelIdsBySnapshot } from "../src/features/relay/modelGroups";
import {
  completeModelDisplayOrder,
  modelSignature,
  modelSpeedTiers,
  modelShowsContextControl,
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
  test("catalog order consumes duplicate IDs once and preserves first spelling and unknown inventory order", () => {
    const models = ["unknown-B", " GPT-TEST ", "unknown-a", "gpt-test", "", "claude-test", "UNKNOWN-A", "extra"];
    const summaries = [model("unknown-a"), model("absent"), model("gpt-test"), model("UNKNOWN-A")];
    const identity = (catalogOrder: number) => ({ catalogProvider: null, catalogFamily: null, catalogOrder });
    const catalog = {
      "claude-test": identity(0),
      missing: identity(1),
      "gpt-test": identity(4),
      "CLAUDE-TEST": identity(7),
      extra: identity(1.5),
      "unknown-B": identity(-1),
    };
    const before = structuredClone({ models, summaries, catalog });
    expect(orderModelIdsBySnapshot(models, summaries, catalog)).toEqual([
      "claude-test", " GPT-TEST ", "unknown-a", "unknown-B", "extra",
    ]);
    expect({ models, summaries, catalog }).toEqual(before);
  });

  test("catalog ordering keeps tied backend positions stable and accepts empty inventories", () => {
    const catalog = {
      b: { catalogProvider: null, catalogFamily: null, catalogOrder: 2 },
      a: { catalogProvider: null, catalogFamily: null, catalogOrder: 2 },
    };
    expect(orderModelIdsBySnapshot(["a", "b", "c"], [model("c")], catalog)).toEqual(["b", "a", "c"]);
    expect(orderModelIdsBySnapshot([], [model("a")], catalog)).toEqual([]);
    expect(orderModelIdsBySnapshot(["b", "B", "a"], [])).toEqual(["b", "a"]);
  });

  test("builds a stable signature from render-affecting model metadata", () => {
    const current = [model("a", { reasoningLevels: ["low"] })];
    expect(modelSignature(current)).toBe(modelSignature(current));
    expect(modelSignature(current)).not.toBe(modelSignature([model("a", { reasoningLevels: ["high"] })]));
    expect(modelSignature([model("a", { contextPolicy: { mode: "short" } })]))
      .not.toBe(modelSignature([model("a", { contextPolicy: { mode: "extended" } })]));
    expect(modelSignature([model("a", { contextPolicy: { mode: "custom", tokens: 128000 } })]))
      .not.toBe(modelSignature([model("a", { contextPolicy: { mode: "custom", tokens: 256000 } })]));
    expect(modelSignature([model("a", { contextConfigurable: false })]))
      .not.toBe(modelSignature([model("a", { contextConfigurable: true })]));
    expect(modelSignature([model("a")]))
      .not.toBe(modelSignature([model("a", { contextWindows: {} })]));
    expect(modelSignature([model("a", { contextWindows: { defaultTokens: 200000, maximumTokens: 900000 } })]))
      .not.toBe(modelSignature([model("a", { contextWindows: { defaultTokens: 272000, maximumTokens: 900000 } })]));
  });

  test("reorders rows immutably and rejects missing or equal targets", () => {
    const current = [model("a"), model("b"), model("c")];
    const next = reorderById(current, "a", "c");
    expect(next?.map((item) => item.id)).toEqual(["b", "c", "a"]);
    expect(current.map((item) => item.id)).toEqual(["a", "b", "c"]);
    expect(reorderById(current, "a", "a")).toBeNull();
    expect(reorderById(current, "missing", "a")).toBeNull();
  });

  test("keeps only the speed modes supplied by the runtime", () => {
    expect(modelSpeedTiers(model("gpt", { speedSupported: true, speedTiers: ["standard"] }))).toEqual(["standard"]);
    expect(modelSpeedTiers(model("gpt", { speedSupported: true, speedTiers: ["fast", "standard"] }))).toEqual(["standard", "fast"]);
    expect(modelSpeedTiers(model("gpt", { speedSupported: true }))).toEqual(["standard"]);
    expect(modelSpeedTiers(model("other", { speedSupported: false }))).toEqual(["standard"]);
  });

  test("returns independent speed selections without mutating runtime tiers", () => {
    const current = model("gpt", { speedTiers: ["ultrafast", "fast", "standard", "fast"] });
    Object.freeze(current.speedTiers);
    const tiers = modelSpeedTiers(current);
    expect(tiers).toEqual(["standard", "fast", "ultrafast"]);
    tiers.pop();
    expect(modelSpeedTiers(current)).toEqual(["standard", "fast", "ultrafast"]);
    expect(current.speedTiers).toEqual(["ultrafast", "fast", "standard", "fast"]);
  });

  test("hides reasoning when a model has no levels and never offers it for image generation", () => {
    expect(modelShowsReasoningControl(model("gpt-5.4", { reasoningSupportedLevels: ["low", "high"] }))).toBe(true);
    expect(modelShowsReasoningControl(model("gpt-reserve"))).toBe(false);
    expect(modelShowsReasoningControl(model("gpt-image-2", { reasoningSupportedLevels: ["low", "high"], catalogFamily: "gpt-image" }))).toBe(false);
    expect(modelShowsReasoningControl(model("vision", { catalogOutputModalities: ["text"], reasoningLevels: ["medium"] }))).toBe(true);
  });

  test("shows context only for text-output models", () => {
    expect(modelShowsContextControl(model("gpt-5.4", { contextConfigurable: true }))).toBe(true);
    expect(modelShowsContextControl(model("vision", {
      catalogInputModalities: ["text", "image"],
      catalogOutputModalities: ["text"],
      contextConfigurable: true,
    }))).toBe(true);
    expect(modelShowsContextControl(model("gpt-image-2", {
      catalogFamily: "gpt-image",
      catalogOutputModalities: ["text"],
      contextConfigurable: true,
    }))).toBe(false);
    expect(modelShowsContextControl(model("dall-e-4", {
      catalogOutputModalities: ["image"],
      contextConfigurable: true,
    }))).toBe(false);
    expect(modelShowsContextControl(model("gemini-video", {
      catalogOutputModalities: ["video"],
      contextConfigurable: true,
    }))).toBe(false);
    expect(modelShowsContextControl(model("text-embedding-3-large", {
      catalogFamily: "embedding",
      catalogOutputModalities: [],
      contextConfigurable: true,
    }))).toBe(false);
    expect(modelShowsContextControl(model("unknown-audio", {
      catalogOutputModalities: ["audio"],
      contextConfigurable: false,
    }))).toBe(false);
  });

  test("moves complete groups while preserving each group's model order", () => {
    const groups: ModelRuleGroup[] = [
      { id: "one", label: "One", models: [model("a"), model("b")] },
      { id: "two", label: "Two", models: [model("c")] },
      { id: "three", label: "Three", models: [model("d"), model("e")] },
    ];
    for (const group of groups) {
      for (const item of group.models) Object.freeze(item);
      Object.freeze(group.models);
      Object.freeze(group);
    }
    Object.freeze(groups);
    const next = reorderModelGroups(groups, "one", "three");
    expect(next?.map((item) => item.id)).toEqual(["c", "d", "e", "a", "b"]);
    expect(next?.[0]).toBe(groups[1]?.models[0]);
    expect(next?.[3]).toBe(groups[0]?.models[0]);
    expect(reorderModelGroups(groups, "three", "one")?.map((item) => item.id))
      .toEqual(["d", "e", "a", "b", "c"]);
    expect(groups.map((group) => group.models.map((item) => item.id)))
      .toEqual([["a", "b"], ["c"], ["d", "e"]]);
  });

  test("rejects invalid group moves and accepts empty groups", () => {
    const groups: ModelRuleGroup[] = [
      { id: "one", label: "One", models: [model("a")] },
      { id: "empty", label: "Empty", models: [] },
      { id: "two", label: "Two", models: [model("b")] },
    ];
    expect(reorderModelGroups(groups, "one", "one")).toBeNull();
    expect(reorderModelGroups(groups, "missing", "one")).toBeNull();
    expect(reorderModelGroups(groups, "one", "missing")).toBeNull();
    expect(reorderModelGroups([], "one", "two")).toBeNull();
    expect(reorderModelGroups(groups, "two", "empty")?.map((item) => item.id)).toEqual(["a", "b"]);
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

  test("keeps very high selectable across catalog and saved-policy aliases", () => {
    const current = model("reasoning", {
      reasoningSupportedLevels: ["low", "very_high", "xhigh", "extra-high", "max"],
    });
    const supported = supportedReasoningLevels(current);
    expect(supported).toEqual(["low", "xhigh", "max"]);
    expect(normalizeReasoningSelection(supported, ["VERY_HIGH", "max"])).toEqual(["xhigh", "max"]);
    expect(normalizeReasoningSelection(supported, ["extra_high"])).toEqual(["xhigh"]);
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


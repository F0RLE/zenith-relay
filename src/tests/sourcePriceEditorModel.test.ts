import { describe, expect, test } from "bun:test";
import type { SourceSummary } from "../src/features/relay/api/types";
import {
  parseSourcePriceDrafts,
  removeSourcePriceDraft,
  sourcePriceDrafts,
  sourcePriceModels,
  updateSourcePriceDraft,
  type SourcePriceDrafts,
} from "../src/features/relay/components/sourcePriceEditorModel";

const source = (overrides: Partial<SourceSummary> = {}): SourceSummary => ({
  id: "source",
  name: "Source",
  enabled: true,
  inPool: true,
  draining: false,
  operationalStatus: "rotation",
  baseUrl: "https://example.test/v1",
  wireApi: "responses",
  models: ["gpt-5.4", "claude-opus"],
  allowedModels: [],
  excludedModels: [],
  priority: 1,
  weight: 1,
  recoveryDelaySeconds: 0,
  apiEquivalent: { microUsd: 0, unpricedTokens: 0 },
  secretAvailable: true,
  lastErrorCode: null,
  ...overrides,
});

describe("source price editor model", () => {
  test("merges price, detected, allow, deny, and catalog models once", () => {
    expect(sourcePriceModels(source({
      models: ["GPT-5.4", "custom"],
      allowedModels: ["gpt-5.4"],
      excludedModels: ["CUSTOM"],
      modelPriceOverrides: { "model-x": { inputMicroUsdPerMillion: 1, outputMicroUsdPerMillion: 2 } },
      detectedModelPrices: { "MODEL-X": { inputMicroUsdPerMillion: 3, outputMicroUsdPerMillion: 4 } },
    }))).toEqual(["MODEL-X", "GPT-5.4", "custom"]);
  });

  test("updates and removes a draft immutably using a normalized model key", () => {
    const current: SourcePriceDrafts = {};
    const updated = updateSourcePriceDraft(current, "GPT-5.4", "input", "1.25");
    expect(updated["gpt-5.4"]).toMatchObject({ input: "1.25", output: "" });
    expect(current).toEqual({});
    expect(removeSourcePriceDraft(updated, "GPT-5.4")).toEqual({});
  });

  test("round-trips valid prices and omits optional blank values", () => {
    const drafts = sourcePriceDrafts({ model: {
      inputMicroUsdPerMillion: 1_250_000,
      outputMicroUsdPerMillion: 7_500_000,
      cachedInputMicroUsdPerMillion: 125_000,
    } });
    expect(drafts).toMatchObject({ model: { input: "1.25", output: "7.5", cached: "0.125" } });
    expect(parseSourcePriceDrafts(drafts)).toEqual({ model: {
      inputMicroUsdPerMillion: 1_250_000,
      outputMicroUsdPerMillion: 7_500_000,
      cachedInputMicroUsdPerMillion: 125_000,
    } });
  });

  test("rejects malformed required prices and malformed optional values", () => {
    expect(parseSourcePriceDrafts({ model: { input: "bad", output: "1", cached: "", cacheWrite5m: "", cacheWrite1h: "" } })).toBeNull();
    expect(parseSourcePriceDrafts({ model: { input: "1", output: "1", cached: "", cacheWrite5m: "bad", cacheWrite1h: "" } })).toBeNull();
  });
});

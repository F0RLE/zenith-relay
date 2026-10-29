import { describe, expect, test } from "bun:test";
import type { SourceSummary } from "../src/features/relay/api/types";
import {
  parseSourcePriceDrafts,
  removeSourcePriceDraft,
  sourcePriceDrafts,
  sourcePriceModels,
  updateSourcePriceDraft,
  updateSourcePriceScheduleDraft,
  selectedPriceRates,
  publishedPriceBands,
  publishedPriceTiers,
  priceContextRanges,
  sourcePriceQuote,
  sourcePriceOverride,
  sourcePriceCacheWrites,
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
  test("context overlays preserve same-tier values for absent fields but keep explicit zeroes", () => {
    const price = {
      input: 1_000_000, output: 2_000_000, cacheRead: 100_000,
      priority: { input: 3_000_000, output: 4_000_000, cacheRead: 200_000 },
      above200k: { priority: { input: undefined, output: 0, cacheRead: undefined } },
    };
    expect(selectedPriceRates(price, "above200k", "priority")).toEqual({ input: 3_000_000, output: 0, cacheRead: 200_000 });
    expect(selectedPriceRates(price, "above200k", "flex")).toEqual({});
  });
  test("price schedules and thresholds require evidence for the selected tier", () => {
    expect(publishedPriceTiers(undefined)).toEqual(["standard"]);
    const price = {
      input: 1_000_000, output: 2_000_000,
      flex: {}, priority: { input: 0 },
      above200k: { standard: { input: 2_000_000 } },
      above272k: { priority: { output: 4_000_000 } },
      above512k: { ultrafast: { input: 0 } },
    };
    expect(publishedPriceTiers(price)).toEqual(["standard", "priority", "ultrafast"]);
    expect(publishedPriceBands(price)).toEqual(["base", "above200k"]);
    expect(publishedPriceBands(price, undefined, "priority")).toEqual(["base", "above272k"]);
    expect(publishedPriceBands(price, undefined, "flex")).toEqual(["base"]);
    const draft = sourcePriceDrafts({ model: {
      inputMicroUsdPerMillion: 1, outputMicroUsdPerMillion: 2,
      flex: { input: 0 }, above100k: { flex: { output: 3 } },
    } }).model;
    expect(publishedPriceTiers(undefined, draft)).toEqual(["standard", "flex"]);
    expect(publishedPriceBands(undefined, draft, "flex")).toEqual(["base", "above100k"]);
    expect(publishedPriceBands(undefined, draft)).toEqual(["base"]);
  });

  test("source-scoped resolution is authoritative and manual reset never becomes automatic", () => {
    const detected = { inputMicroUsdPerMillion: 80_000, outputMicroUsdPerMillion: 400_000 };
    const evidence = { source: "liteLlmExact" as const, quote: { input: 1_000_000, output: 2_000_000 },
      catalogRevision: "synthetic", catalogFetchedAtMs: 1, stale: false };
    expect(sourcePriceQuote(detected, evidence)).toEqual(evidence.quote);
    expect(sourcePriceQuote(undefined, { ...evidence, source: "manual" })).toBeUndefined();
    expect(sourcePriceQuote(detected, { ...evidence, source: "unpriced", quote: null })).toBeUndefined();
    expect(sourcePriceQuote(undefined, undefined)).toBeUndefined();
    expect(sourcePriceQuote(detected, undefined)).toMatchObject({ input: 80_000, output: 400_000 });
  });
  test("generic creation prices round-trip without a manufactured TTL or multiplier", () => {
    const price = {
      inputMicroUsdPerMillion: 1_000_000, outputMicroUsdPerMillion: 2_000_000,
      cacheWriteMicroUsdPerMillion: 870_000,
      flex: { cacheWrite: 0 },
    };
    const quote = sourcePriceQuote(price, undefined);
    expect(quote?.cacheWrite).toBe(870_000);
    expect(quote?.cacheWrite5m).toBeUndefined();
    expect(sourcePriceOverride(quote)).toMatchObject(price);
    expect(parseSourcePriceDrafts(sourcePriceDrafts({ model: price }))).toEqual({ model: price });
    expect(sourcePriceCacheWrites(quote)).toEqual({ generic: true, fiveMinutes: false, oneHour: false });
    expect(selectedPriceRates(quote, "base", "flex")).toEqual({ cacheWrite: 0 });
  });
  test("context choices come only from published or saved model prices", () => {
    expect(publishedPriceBands(undefined)).toEqual(["base"]);
    expect(publishedPriceBands({ input: 1, output: 2, above200k: { standard: {} } })).toEqual(["base"]);
    expect(publishedPriceBands({ input: 1, output: 2, above272k: { flex: { input: 0 } } })).toEqual(["base"]);
    expect(publishedPriceBands({ input: 1, output: 2, above272k: { flex: { input: 0 } } }, undefined, "flex")).toEqual(["base", "above272k"]);
    const draft = sourcePriceDrafts({ model: {
      inputMicroUsdPerMillion: 1, outputMicroUsdPerMillion: 2,
      above128k: { priority: { cacheRead: 0 } },
    } }).model;
    expect(publishedPriceBands(undefined, draft)).toEqual(["base"]);
    expect(publishedPriceBands(undefined, draft, "priority")).toEqual(["base", "above128k"]);
    expect(priceContextRanges(["base", "above100k", "above512k"])).toEqual([
      { band: "base", min: undefined, max: 100_000 },
      { band: "above100k", min: 100_000, max: 512_000 },
      { band: "above512k", min: 512_000, max: undefined },
    ]);
  });

  test("editing one base field preserves automatic prices and every model-specific schedule", () => {
    const price = {
      inputMicroUsdPerMillion: 1_000_000, outputMicroUsdPerMillion: 2_000_000,
      above100k: { standard: { input: 2_000_000 } },
      above128k: { flex: { input: 0 } },
      above512k: { priority: { output: 8_000_000, cacheWrite1h: 0 } },
    };
    const draft = updateSourcePriceScheduleDraft({}, "Model", "base", "standard", "input", "1.5", price);
    expect(parseSourcePriceDrafts(draft)).toEqual({ model: { ...price, inputMicroUsdPerMillion: 1_500_000 } });
    const quote = sourcePriceQuote(price, undefined);
    expect(sourcePriceOverride(quote)).toMatchObject(price);
    expect(selectedPriceRates(quote, "above512k", "standard")).toMatchObject({ input: 2_000_000, output: 2_000_000 });
    expect(selectedPriceRates(quote, "above512k", "flex")).toEqual({ input: 0 });
    expect(sourcePriceCacheWrites(quote)).toEqual({ fiveMinutes: false, oneHour: true, generic: false });
    expect(publishedPriceBands(quote)).toEqual(["base", "above100k"]);
    expect(publishedPriceBands(quote, undefined, "flex")).toEqual(["base", "above128k"]);
    expect(publishedPriceBands(quote, undefined, "priority")).toEqual(["base", "above512k"]);
  });

  test("editing long Flex preserves the base, published tiers, and exact zero cache prices", () => {
    const price = {
      inputMicroUsdPerMillion: 1_000_000, outputMicroUsdPerMillion: 2_000_000,
      flex: { input: 500_000, output: 1_000_000, cacheRead: 0 },
      above272k: { standard: { input: 2_000_000, output: 4_000_000 } },
    };
    const drafts = updateSourcePriceScheduleDraft({}, "Model", "above272k", "flex", "input", "1.25", price);
    expect(parseSourcePriceDrafts(drafts)).toEqual({ model: {
      ...price, above272k: { ...price.above272k, flex: { input: 1_250_000 } },
    } });
    expect(selectedPriceRates({ input: 1, output: 2 }, "above272k", "flex")).toEqual({});
    expect(selectedPriceRates({ input: 1, output: 2, above200k: { standard: { input: 3 } } }, "above272k", "standard"))
      .toMatchObject({ input: 3, output: 2 });
  });
  test("merges price, detected, allow, deny, and catalog models once", () => {
    expect(sourcePriceModels(source({
      models: ["GPT-5.4", "custom"],
      allowedModels: ["gpt-5.4"],
      excludedModels: ["CUSTOM"],
      modelPriceOverrides: { "model-x": { inputMicroUsdPerMillion: 1, outputMicroUsdPerMillion: 2 } },
      detectedModelPrices: { "MODEL-X": { inputMicroUsdPerMillion: 3, outputMicroUsdPerMillion: 4 } },
    }))).toEqual(["GPT-5.4", "custom", "model-x"]);
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

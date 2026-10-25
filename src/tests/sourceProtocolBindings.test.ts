import { describe, expect, test } from "bun:test";
import {
  effectiveSourceProtocolBindings,
  sourceModelsWithCacheWritePricing,
  sourceSupportsNativeProtocol,
  sourceSupportsNativeResponses,
} from "../src/features/relay/sourceProtocolBindings";
import type { SourceSummary } from "../src/features/relay/api/types";

describe("source protocol bindings", () => {
  test("Interactions is native-only and never supplies a Responses or Messages bridge", () => {
    const source = { wireApi:"gemini_interactions", models:["synthetic"], protocolBindings:[{wireApi:"gemini_interactions",adapter:"native",modelIds:["synthetic"]}] } satisfies Pick<SourceSummary,"wireApi"|"models"|"protocolBindings">;
    expect(effectiveSourceProtocolBindings(source)).toHaveLength(1);
    expect(sourceSupportsNativeProtocol(source,"gemini_interactions")).toBe(true);
    expect(sourceSupportsNativeResponses(source)).toBe(false);
    expect(sourceModelsWithCacheWritePricing(source)).toEqual([]);
  });
  test("cache-write pricing follows Messages upstream routes", () => {
    const source = {
      wireApi: "responses",
      models: ["gpt-native", "claude-bridge", "claude-native"],
      protocolBindings: [
        { wireApi: "responses", adapter: "native", modelIds: ["gpt-native"] },
        { wireApi: "responses", adapter: "responses_to_messages", modelIds: ["claude-bridge"] },
        { wireApi: "messages", adapter: "native", modelIds: ["claude-native"] },
      ],
    } satisfies Pick<SourceSummary, "wireApi" | "models" | "protocolBindings">;

    expect(sourceModelsWithCacheWritePricing(source)).toEqual([
      "claude-bridge",
      "claude-native",
    ]);
  });

  test("preserves two Responses connector routes for one source", () => {
    const source = {
      wireApi: "responses",
      models: ["gpt-native", "claude-bridge"],
      protocolBindings: [
        {
          wireApi: "responses",
          adapter: "native",
          reasoningMode: "disabled",
          modelIds: ["gpt-native"],
        },
        {
          wireApi: "responses",
          adapter: "responses_to_messages",
          reasoningMode: "adaptive",
          modelIds: ["claude-bridge"],
        },
      ],
    } satisfies Pick<SourceSummary, "wireApi" | "models" | "protocolBindings">;

    expect(effectiveSourceProtocolBindings(source)).toEqual([
      {
        wireApi: "responses",
        adapter: "native",
        reasoningMode: "disabled",
        cacheWriteTtl: "provider",
        modelIds: ["gpt-native"],
      },
      {
        wireApi: "responses",
        adapter: "responses_to_messages",
        reasoningMode: "adaptive",
        cacheWriteTtl: "provider",
        modelIds: ["claude-bridge"],
      },
    ]);
    expect(sourceSupportsNativeResponses(source)).toBe(true);
  });

  test("does not treat a bridge-only source as a direct Responses endpoint", () => {
    const source = {
      wireApi: "responses",
      models: ["claude-bridge"],
      protocolBindings: [{
        wireApi: "responses",
        adapter: "responses_to_messages",
        reasoningMode: "disabled",
        modelIds: ["claude-bridge"],
      }],
    } satisfies Pick<SourceSummary, "wireApi" | "models" | "protocolBindings">;

    expect(sourceSupportsNativeResponses(source)).toBe(false);
  });

  test("normalizes legacy bridge reasoning to the internal adaptive translator", () => {
    const source = {
      wireApi: "responses",
      models: ["claude-bridge"],
      protocolBindings: [{
        wireApi: "responses",
        adapter: "responses_to_messages",
        reasoningMode: "disabled",
        modelIds: ["claude-bridge"],
      }],
    } satisfies Pick<SourceSummary, "wireApi" | "models" | "protocolBindings">;

    expect(effectiveSourceProtocolBindings(source)[0]?.reasoningMode).toBe("adaptive");
  });

  test("keeps an explicit Gemini bridge available only through Responses", () => {
    const source = {
      wireApi: "responses",
      models: ["gemini-3-pro"],
      protocolBindings: [{
        wireApi: "responses",
        adapter: "responses_to_gemini",
        reasoningMode: "disabled",
        modelIds: ["gemini-3-pro"],
      }],
    } satisfies Pick<SourceSummary, "wireApi" | "models" | "protocolBindings">;

    expect(effectiveSourceProtocolBindings(source)[0].adapter).toBe("responses_to_gemini");
    expect(sourceSupportsNativeResponses(source)).toBe(false);
  });

  test("does not expand an empty Gemini bridge to the source catalog", () => {
    const source = {
      wireApi: "responses",
      models: ["gemini-3-pro"],
      protocolBindings: [{
        wireApi: "responses",
        adapter: "responses_to_gemini",
        reasoningMode: "disabled",
        modelIds: [],
      }],
    } satisfies Pick<SourceSummary, "wireApi" | "models" | "protocolBindings">;

    expect(effectiveSourceProtocolBindings(source)[0]?.modelIds).toEqual([]);
  });

  test("keeps native Messages separate from the Responses route", () => {
    const source = {
      wireApi: "responses",
      models: ["gpt-native", "claude-messages"],
      protocolBindings: [
        {
          wireApi: "responses",
          adapter: "native",
          reasoningMode: "disabled",
          modelIds: ["gpt-native"],
        },
        {
          wireApi: "messages",
          adapter: "native",
          reasoningMode: "disabled",
          cacheWriteTtl: "1h",
          modelIds: ["claude-messages"],
        },
      ],
    } satisfies Pick<SourceSummary, "wireApi" | "models" | "protocolBindings">;

    expect(sourceSupportsNativeResponses(source)).toBe(true);
    expect(sourceModelsWithCacheWritePricing(source)).toEqual(["claude-messages"]);
    expect(effectiveSourceProtocolBindings(source)[1]?.cacheWriteTtl).toBe("1h");
  });

  test("uses the source catalog for a sole legacy-compatible empty binding", () => {
    const source = {
      wireApi: "responses",
      models: [" GPT-Legacy ", "gpt-legacy"],
      protocolBindings: [{
        wireApi: "responses",
        adapter: "native",
        reasoningMode: "disabled",
        modelIds: [],
      }],
    } satisfies Pick<SourceSummary, "wireApi" | "models" | "protocolBindings">;

    expect(effectiveSourceProtocolBindings(source)[0]?.modelIds).toEqual(["GPT-Legacy"]);
    expect(sourceSupportsNativeResponses(source)).toBe(true);
  });

  test("does not expand an empty binding in a multi-route source", () => {
    const source = {
      wireApi: "responses",
      models: ["gpt-native", "claude-messages"],
      protocolBindings: [
        {
          wireApi: "responses",
          adapter: "native",
          reasoningMode: "disabled",
          modelIds: [],
        },
        {
          wireApi: "messages",
          adapter: "native",
          reasoningMode: "disabled",
          modelIds: ["claude-messages"],
        },
      ],
    } satisfies Pick<SourceSummary, "wireApi" | "models" | "protocolBindings">;

    expect(sourceSupportsNativeResponses(source)).toBe(false);
    expect(sourceModelsWithCacheWritePricing(source)).toEqual(["claude-messages"]);
  });

  for (const [wireApi, modelIds] of [
    ["responses", ["retired-model"]],
    ["messages", ["retired-model"]],
    ["responses", ["current-model", "retired-model"]],
  ] as const) {
    test(`does not expose ${wireApi} when a binding references unavailable models: ${modelIds.join(",")}`, () => {
      const source = {
        wireApi,
        models: ["current-model"],
        protocolBindings: [{ wireApi, adapter: "native", modelIds: [...modelIds] }],
      } satisfies Pick<SourceSummary, "wireApi" | "models" | "protocolBindings">;

      expect(sourceSupportsNativeProtocol(source)).toBe(false);
      expect(sourceModelsWithCacheWritePricing(source)).toEqual([]);
    });
  }

  for (const resolvedProtocolBindings of [[], [{ wireApi: "responses", adapter: "native", modelIds: [] }]] as const) {
    test(`does not expand the backend's empty resolved routes (${resolvedProtocolBindings.length} bindings)`, () => {
      const source = {
        wireApi: "responses",
        models: ["current-model"],
        protocolBindings: [{ wireApi: "responses", adapter: "native", modelIds: [] }],
        resolvedProtocolBindings: resolvedProtocolBindings.map(binding => ({ ...binding, modelIds: [...binding.modelIds] })),
      } satisfies Pick<SourceSummary, "wireApi" | "models" | "protocolBindings" | "resolvedProtocolBindings">;

      expect(sourceSupportsNativeResponses(source)).toBe(false);
      expect(sourceModelsWithCacheWritePricing(source)).toEqual([]);
    });
  }

  test("does not turn an adapter for another client protocol into a native route", () => {
    const source = {
      wireApi: "responses",
      models: ["current-model"],
      protocolBindings: [{ wireApi: "responses", adapter: "messages_to_responses", modelIds: ["current-model"] }],
    } satisfies Pick<SourceSummary, "wireApi" | "models" | "protocolBindings">;

    expect(effectiveSourceProtocolBindings(source)).toEqual([]);
    expect(sourceSupportsNativeResponses(source)).toBe(false);
  });

  test("does not expose duplicate routes rejected by the backend", () => {
    const source = {
      wireApi: "responses",
      models: ["current-model"],
      protocolBindings: [
        { wireApi: "responses", adapter: "native", modelIds: ["current-model"] },
        { wireApi: "responses", adapter: "native", modelIds: ["current-model"] },
      ],
    } satisfies Pick<SourceSummary, "wireApi" | "models" | "protocolBindings">;

    expect(effectiveSourceProtocolBindings(source)).toEqual([]);
    expect(sourceSupportsNativeResponses(source)).toBe(false);
  });

  test("does not expose Messages cache TTL on a native Responses route", () => {
    const source = {
      wireApi: "responses",
      models: ["current-model"],
      protocolBindings: [{ wireApi: "responses", adapter: "native", cacheWriteTtl: "1h", modelIds: ["current-model"] }],
    } satisfies Pick<SourceSummary, "wireApi" | "models" | "protocolBindings">;

    expect(effectiveSourceProtocolBindings(source)).toEqual([]);
    expect(sourceSupportsNativeResponses(source)).toBe(false);
  });

  test("keeps explicit manual models when the source catalog is not yet known", () => {
    const source = {
      wireApi: "responses",
      models: [],
      protocolBindings: [{ wireApi: "responses", adapter: "native", modelIds: [" Manual-Model ", "manual-model"] }],
    } satisfies Pick<SourceSummary, "wireApi" | "models" | "protocolBindings">;

    expect(effectiveSourceProtocolBindings(source)[0]?.modelIds).toEqual(["Manual-Model"]);
    expect(sourceSupportsNativeResponses(source)).toBe(true);
  });
});

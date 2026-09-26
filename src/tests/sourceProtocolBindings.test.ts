import { describe, expect, test } from "bun:test";
import {
  effectiveSourceProtocolBindings,
  runtimeSourceProtocolBindings,
  sourceModelsForWireApi,
  sourceModelsWithCacheWritePricing,
  sourceSupportsNativeResponses,
  sourceSupportsWireApi,
} from "../src/features/relay/sourceProtocolBindings";
import type { SourceSummary } from "../src/features/relay/api/types";

describe("source protocol bindings", () => {
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
    expect(sourceModelsForWireApi(source, "responses")).toEqual([
      "gpt-native",
      "claude-bridge",
    ]);
    expect(sourceSupportsWireApi(source, "responses")).toBe(true);
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

    expect(sourceSupportsWireApi(source, "responses")).toBe(true);
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
    expect(sourceModelsForWireApi(source, "responses")).toEqual(["gemini-3-pro"]);
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

    expect(sourceModelsForWireApi(source, "responses")).toEqual([]);
    expect(sourceSupportsWireApi(source, "responses")).toBe(false);
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

    expect(sourceModelsForWireApi(source, "responses")).toEqual(["gpt-native"]);
    expect(sourceModelsForWireApi(source, "messages")).toEqual(["claude-messages"]);
    expect(runtimeSourceProtocolBindings(source)).toEqual(effectiveSourceProtocolBindings(source));
  });

  test("uses the source catalog for a sole legacy-compatible empty binding", () => {
    const source = {
      wireApi: "responses",
      models: ["gpt-legacy"],
      protocolBindings: [{
        wireApi: "responses",
        adapter: "native",
        reasoningMode: "disabled",
        modelIds: [],
      }],
    } satisfies Pick<SourceSummary, "wireApi" | "models" | "protocolBindings">;

    expect(sourceModelsForWireApi(source, "responses")).toEqual(["gpt-legacy"]);
    expect(sourceSupportsWireApi(source, "responses")).toBe(true);
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

    expect(sourceModelsForWireApi(source, "responses")).toEqual([]);
    expect(sourceSupportsNativeResponses(source)).toBe(false);
    expect(sourceModelsForWireApi(source, "messages")).toEqual(["claude-messages"]);
  });

});

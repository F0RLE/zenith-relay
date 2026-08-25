import { describe, expect, test } from "bun:test";
import {
  effectiveSourceProtocolBindings,
  runtimeSourceProtocolBindings,
  sourceModelsForWireApi,
  sourceSupportsNativeResponses,
  sourceSupportsWireApi,
} from "../src/features/relay/sourceProtocolBindings";
import {
  apiProviderSourceInput,
  type ApiProviderValue,
} from "../src/features/relay/components/ApiProviderForm";
import type { SourceSummary } from "../src/features/relay/api/types";

describe("source protocol bindings", () => {
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

  test("links confirmed Messages models into the Responses route", () => {
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
          modelIds: ["claude-messages"],
        },
      ],
    } satisfies Pick<SourceSummary, "wireApi" | "models" | "protocolBindings">;

    expect(sourceModelsForWireApi(source, "responses")).toEqual([
      "gpt-native",
      "claude-messages",
    ]);
    expect(sourceModelsForWireApi(source, "messages")).toEqual(["claude-messages"]);
    expect(runtimeSourceProtocolBindings(source).at(-1)).toEqual({
      wireApi: "responses",
      adapter: "responses_to_messages",
      reasoningMode: "adaptive",
      cacheWriteTtl: "provider",
      modelIds: ["claude-messages"],
    });
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

    expect(sourceModelsForWireApi(source, "responses")).toEqual(["claude-messages"]);
    expect(sourceSupportsNativeResponses(source)).toBe(false);
    expect(sourceModelsForWireApi(source, "messages")).toEqual(["claude-messages"]);
  });

  test("advanced bindings remain provider-neutral after normalization", () => {
    const value = {
      kind: "openai",
      name: "OpenAI",
      baseUrl: "https://api.openai.com/v1",
      wireApi: "responses",
      apiKey: "sk-synthetic",
      protocolBindings: [
        { wireApi: "responses", modelIds: [] },
        { wireApi: "messages", modelIds: [] },
        { wireApi: "chat_completions", modelIds: [] },
      ],
    } satisfies ApiProviderValue;

    expect(apiProviderSourceInput(value).protocolBindings).toEqual([
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
        modelIds: [],
      },
      {
        wireApi: "chat_completions",
        adapter: "native",
        reasoningMode: "disabled",
        modelIds: [],
      },
    ]);
  });

  test("simple source route choices preserve native Messages and Gemini adapters", () => {
    const anthropic = {
      kind: "custom",
      name: "Anthropic endpoint",
      baseUrl: "https://api.anthropic.com/v1",
      wireApi: "messages" as const,
      apiKey: "sk-anthropic",
      protocolBindings: [{ wireApi: "messages" as const, adapter: "native" as const, reasoningMode: "disabled" as const, modelIds: [] }],
    } satisfies ApiProviderValue;
    const google = {
      ...anthropic,
      name: "Google endpoint",
      baseUrl: "https://generativelanguage.googleapis.com/v1beta",
      wireApi: "gemini" as const,
      protocolBindings: [{ wireApi: "gemini" as const, adapter: "native" as const, reasoningMode: "disabled" as const, modelIds: [] }],
    } satisfies ApiProviderValue;

    expect(apiProviderSourceInput(anthropic).protocolBindings[0]).toMatchObject({ wireApi: "messages", adapter: "native" });
    expect(apiProviderSourceInput(google).protocolBindings[0]).toMatchObject({ wireApi: "gemini", adapter: "native" });
  });

  test("manual model catalogs are assigned to the selected adapter route", () => {
    const value = {
      kind: "custom",
      name: "No catalog endpoint",
      baseUrl: "https://api.example.test/v1",
      wireApi: "responses" as const,
      apiKey: "sk-synthetic",
      models: ["gpt-5.6", "gpt-5.5"],
      protocolBindings: [{
        wireApi: "responses" as const,
        adapter: "responses_to_gemini" as const,
        reasoningMode: "disabled" as const,
        modelIds: [],
      }],
    } satisfies ApiProviderValue;

    expect(apiProviderSourceInput(value)).toMatchObject({
      models: ["gpt-5.6", "gpt-5.5"],
      protocolBindings: [{
        wireApi: "responses",
        adapter: "responses_to_gemini",
        modelIds: ["gpt-5.6", "gpt-5.5"],
      }],
    });
  });

  test("manual mode keeps entered models unassigned until the operator chooses a route", () => {
    const value = {
      kind: "custom",
      name: "Manual route selection",
      baseUrl: "https://api.example.test/v1beta",
      wireApi: "responses" as const,
      apiKey: "sk-synthetic",
      modelCatalogMode: "manual" as const,
      models: ["gemini-2.5-pro"],
      autoAssignModels: false,
      protocolBindings: [{
        wireApi: "responses" as const,
        adapter: "responses_to_gemini" as const,
        reasoningMode: "disabled" as const,
        modelIds: [],
      }],
    } satisfies ApiProviderValue;

    expect(apiProviderSourceInput(value)).toMatchObject({
      models: ["gemini-2.5-pro"],
      protocolBindings: [{
        wireApi: "responses",
        adapter: "responses_to_gemini",
        modelIds: [],
      }],
    });
  });

  test("explicit automatic mode ignores stale manual models and triggers discovery", () => {
    const value = {
      kind: "custom",
      name: "Provider with discovery",
      baseUrl: "https://api.example.test/v1",
      wireApi: "responses" as const,
      apiKey: "sk-synthetic",
      modelCatalogMode: "automatic" as const,
      models: ["stale-model"],
      protocolBindings: [{
        wireApi: "responses" as const,
        adapter: "native" as const,
        reasoningMode: "disabled" as const,
        modelIds: [],
      }],
    } satisfies ApiProviderValue;

    expect(apiProviderSourceInput(value)).toMatchObject({
      models: [],
      protocolBindings: [{ modelIds: [] }],
    });
  });

  test("keeps cache-write TTL separate for each Messages upstream binding", () => {
    const value = {
      kind: "custom",
      name: "Compatible API",
      baseUrl: "https://api.example.test/v1",
      wireApi: "responses",
      apiKey: "sk-synthetic",
      protocolBindings: [
        {
          wireApi: "messages",
          adapter: "native",
          reasoningMode: "disabled",
          cacheWriteTtl: "5m",
          modelIds: ["claude-native"],
        },
        {
          wireApi: "responses",
          adapter: "responses_to_messages",
          reasoningMode: "adaptive",
          cacheWriteTtl: "1h",
          modelIds: ["claude-bridge"],
        },
      ],
    } satisfies ApiProviderValue;

    expect(apiProviderSourceInput(value).protocolBindings).toEqual(value.protocolBindings);
  });
});

import { describe, expect, test } from "bun:test";
import {
  effectiveSourceProtocolBindings,
  sourceSupportsNativeResponses,
  sourceSupportsWireApi,
} from "../src/features/relay/components/SourceProtocolBindingsEditor";
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
        modelIds: ["gpt-native"],
      },
      {
        wireApi: "responses",
        adapter: "responses_to_messages",
        reasoningMode: "adaptive",
        modelIds: ["claude-bridge"],
      },
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
});

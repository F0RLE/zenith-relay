import { describe, expect, test } from "bun:test";
import type { SourceProtocolBinding } from "../src/features/relay/api/types";
import {
  updateBridgeModel,
  updateCacheWriteTtl,
  updateModelRoute,
  updateNativeProtocol,
} from "../src/features/relay/components/sourceProtocolBindingsEditorModel";

const models = ["gpt-5.4", "claude-opus"];

const native = (wireApi: SourceProtocolBinding["wireApi"], modelIds: string[] = models): SourceProtocolBinding => ({
  wireApi,
  adapter: "native",
  reasoningMode: "disabled",
  modelIds,
  cacheWriteTtl: "provider",
});

describe("source protocol bindings editor model", () => {
  test("adds a secondary native route without assigning models", () => {
    const existing = [native("responses")];
    expect(updateNativeProtocol({
      bindings: existing,
      models,
      autoAssignModels: true,
      wireApi: "messages",
      selected: true,
    })).toEqual([
      existing[0],
      native("messages", []),
    ]);
    expect(updateNativeProtocol({
      bindings: existing,
      models,
      autoAssignModels: true,
      wireApi: "responses",
      selected: false,
    })).toEqual([]);
  });

  test("assigns a model independently to multiple routes while preserving a legacy final route", () => {
    const bindings = [native("responses", ["gpt-5.4"]), native("messages", ["claude-opus"])] as const;
    const moved = updateModelRoute({
      bindings,
      models,
      autoAssignModels: true,
      wireApi: "responses",
      adapter: "native",
      model: "claude-opus",
      selected: true,
    });
    expect(moved.map((binding) => binding.modelIds)).toEqual([["gpt-5.4", "claude-opus"], ["claude-opus"]]);

    const legacy = [native("responses", ["gpt-5.4"])] as const;
    expect(updateModelRoute({
      bindings: legacy,
      models,
      autoAssignModels: true,
      wireApi: "responses",
      adapter: "native",
      model: "gpt-5.4",
      selected: false,
    })).toEqual(legacy);
  });

  test("allows native Messages and Responses-to-Messages to share a model", () => {
    const bindings = [native("messages", ["claude-opus"]), {
      wireApi: "responses",
      adapter: "responses_to_messages" as const,
      reasoningMode: "adaptive" as const,
      cacheWriteTtl: "5m" as const,
      modelIds: ["gpt-5.4"],
    }];
    expect(updateBridgeModel({
      bindings,
      models,
      autoAssignModels: true,
      adapter: "responses_to_messages",
      model: "claude-opus",
      selected: true,
      cacheWriteTtl: "5m",
  })).toEqual([
      bindings[0],
      {
        ...bindings[1],
        modelIds: ["gpt-5.4", "claude-opus"],
      },
    ]);
  });

  test("creates and removes a Gemini bridge route without changing native routes", () => {
    const nativeResponses = native("responses", ["gpt-5.4"]);
    const withGemini = updateBridgeModel({
      bindings: [nativeResponses],
      models,
      autoAssignModels: true,
      adapter: "responses_to_gemini",
      model: "claude-opus",
      selected: true,
      cacheWriteTtl: "provider",
    });
    expect(withGemini).toEqual([
      { ...nativeResponses, modelIds: ["gpt-5.4"] },
      {
        wireApi: "responses",
        adapter: "responses_to_gemini",
        reasoningMode: "adaptive",
        modelIds: ["claude-opus"],
      },
    ]);
    expect(updateBridgeModel({
      bindings: withGemini,
      models,
      autoAssignModels: true,
      adapter: "responses_to_gemini",
      model: "claude-opus",
      selected: false,
      cacheWriteTtl: "provider",
    })).toEqual([nativeResponses]);
  });

  test("updates cache TTL only for Messages-capable routes", () => {
    const next = updateCacheWriteTtl([
      native("responses"),
      native("messages"),
      {
        wireApi: "responses",
        adapter: "responses_to_messages",
        reasoningMode: "adaptive",
        modelIds: models,
      },
    ], "5m");
    expect(next.map((binding) => binding.cacheWriteTtl)).toEqual(["provider", "5m", "5m"]);
  });

  test("keeps native and two converted paths for the same input and model", () => {
    let bindings = [native("responses", ["gpt-5.4"])];
    for (const adapter of ["responses_to_messages", "responses_to_gemini"] as const) {
      bindings = updateModelRoute({ bindings, models, autoAssignModels: true,
        wireApi: "responses", adapter, model: "gpt-5.4", selected: true });
    }
    expect(bindings.map((binding) => binding.modelIds)).toEqual([
      ["gpt-5.4"], ["gpt-5.4"], ["gpt-5.4"],
    ]);
    const removed = updateModelRoute({ bindings, models, autoAssignModels: true,
      wireApi: "responses", adapter: "responses_to_messages", model: "gpt-5.4", selected: false });
    expect(removed.map((binding) => binding.adapter)).toEqual(["native", "responses_to_gemini"]);
  });
});

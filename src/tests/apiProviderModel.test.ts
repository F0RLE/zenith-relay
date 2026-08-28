import { describe, expect, test } from "bun:test";
import {
  addApiProviderModel,
  apiProviderReady,
  apiProviderSourceInput,
  clearApiProviderSelection,
  defaultApiProviderValue,
  removeApiProviderModel,
  selectApiProvider,
  setApiProviderAutoAssignModels,
  setApiProviderModelCatalogMode,
  type ApiProviderValue,
} from "../src/features/relay/components/apiProviderModel";

const provider = (overrides: Partial<ApiProviderValue> = {}): ApiProviderValue => ({
  ...defaultApiProviderValue(),
  kind: "custom",
  name: "Example",
  baseUrl: "https://example.test/v1",
  apiKey: "key",
  protocolBindings: [
    { wireApi: "responses", adapter: "native", reasoningMode: "disabled", modelIds: ["existing"] },
    { wireApi: "messages", adapter: "native", reasoningMode: "disabled", modelIds: [] },
  ],
  ...overrides,
});

describe("API provider model", () => {
  test("selects a provider without mutating the previous value or losing its key", () => {
    const current = provider({ apiKey: "preserve-me", models: ["model-a"] });
    const selected = selectApiProvider(current, "openai");

    expect(selected).toMatchObject({ kind: "openai", name: "OpenAI", apiKey: "preserve-me", models: ["model-a"] });
    expect(selected.protocolBindings).not.toBe(current.protocolBindings);
    expect(selected.protocolBindings[0]).not.toBe(current.protocolBindings[0]);
    expect(current.kind).toBe("custom");
  });

  test("clears provider selection while retaining entered secret and model preferences", () => {
    const current = provider({ apiKey: "keep", models: ["model-a"], modelCatalogMode: "manual", autoAssignModels: false });
    expect(clearApiProviderSelection(current)).toMatchObject({
      kind: null,
      apiKey: "keep",
      models: ["model-a"],
      modelCatalogMode: "manual",
      autoAssignModels: false,
    });
  });

  test("adds manual models case-insensitively and assigns only the first route", () => {
    const current = provider({ modelCatalogMode: "manual", protocolBindings: [
      { wireApi: "responses", adapter: "native", reasoningMode: "disabled", modelIds: [] },
      { wireApi: "messages", adapter: "native", reasoningMode: "disabled", modelIds: [] },
    ] });
    const added = addApiProviderModel(current, "  Model-A  ");

    expect(added).toMatchObject({ models: ["Model-A"] });
    expect(added?.protocolBindings.map((binding) => binding.modelIds)).toEqual([["Model-A"], []]);
    expect(addApiProviderModel(added!, "model-a")).toBeNull();
  });

  test("removes a model from the catalog and every route without mutating input", () => {
    const current = provider({ models: ["Model-A", "Model-B"], protocolBindings: [
      { wireApi: "responses", adapter: "native", reasoningMode: "disabled", modelIds: ["model-a"] },
      { wireApi: "messages", adapter: "native", reasoningMode: "disabled", modelIds: ["MODEL-A", "model-b"] },
    ] });
    const next = removeApiProviderModel(current, "MODEL-a");

    expect(next.models).toEqual(["Model-B"]);
    expect(next.protocolBindings.map((binding) => binding.modelIds)).toEqual([[], ["model-b"]]);
    expect(current.models).toEqual(["Model-A", "Model-B"]);
  });

  test("clears route assignments when switching back to automatic catalog mode", () => {
    const current = provider({ modelCatalogMode: "manual", models: ["model-a"] });
    const next = setApiProviderModelCatalogMode(current, "automatic");

    expect(next.modelCatalogMode).toBe("automatic");
    expect(next.protocolBindings.every((binding) => binding.modelIds.length === 0)).toBe(true);
  });

  test("auto-assigns only unassigned models to the first route", () => {
    const current = provider({ models: ["existing", "new-model"], autoAssignModels: false });
    const next = setApiProviderAutoAssignModels(current, true);

    expect(next.protocolBindings[0].modelIds).toEqual(["existing", "new-model"]);
    expect(next.protocolBindings[1].modelIds).toEqual([]);
    expect(current.autoAssignModels).toBe(false);
  });

  test("validates provider identity and builds a trimmed source payload", () => {
    expect(apiProviderReady(provider({ apiKey: " " }))).toBe(false);
    expect(apiProviderReady(provider())).toBe(true);
    expect(apiProviderReady(provider({ name: "", baseUrl: "" }))).toBe(false);

    const input = apiProviderSourceInput(provider({ name: "  Example  ", baseUrl: " https://example.test/v1 ", apiKey: " key " }));
    expect(input).toMatchObject({ name: "Example", baseUrl: "https://example.test/v1", apiKey: "key", models: [] });
    expect(input.protocolBindings).toHaveLength(2);
  });
});

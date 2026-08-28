import type { SourceProtocolBinding, SourceWireApi } from "../api/types";
import { normalizedAdapter, normalizedReasoningMode } from "../sourceProtocolBindings";

export type ApiProviderKind = "zenith" | "openai" | "openrouter" | "custom";
export type ApiProviderValue = {
  kind: ApiProviderKind | null;
  name: string;
  baseUrl: string;
  wireApi: SourceWireApi;
  protocolBindings: SourceProtocolBinding[];
  apiKey: string;
  /** Optional manual catalog for providers that do not expose GET /models. */
  models?: string[];
  /** Whether the model catalog comes from discovery or manual entry. */
  modelCatalogMode?: "automatic" | "manual";
  /** Automatically attach newly entered models to the selected protocol. */
  autoAssignModels?: boolean;
};

export type ApiProviderDefinition = Omit<ApiProviderValue, "apiKey">;
export type ModelCatalogMode = "automatic" | "manual";

export const providerOrder: ApiProviderKind[] = ["openai", "openrouter", "zenith", "custom"];

export const providerDefaults: Record<ApiProviderKind, ApiProviderDefinition> = {
  zenith: {
    kind: "zenith",
    name: "Zenith API",
    baseUrl: "https://api.zenithmarket.dev/v1",
    wireApi: "responses",
    protocolBindings: [{ wireApi: "responses", adapter: "native", reasoningMode: "disabled", modelIds: [] }],
  },
  openai: {
    kind: "openai",
    name: "OpenAI",
    baseUrl: "https://api.openai.com/v1",
    wireApi: "responses",
    protocolBindings: [{ wireApi: "responses", adapter: "native", reasoningMode: "disabled", modelIds: [] }],
  },
  openrouter: {
    kind: "openrouter",
    name: "OpenRouter",
    baseUrl: "https://openrouter.ai/api/v1",
    wireApi: "responses",
    protocolBindings: [{ wireApi: "responses", adapter: "native", reasoningMode: "disabled", modelIds: [] }],
  },
  custom: {
    kind: "custom",
    name: "",
    baseUrl: "",
    wireApi: "responses",
    protocolBindings: [{ wireApi: "responses", adapter: "native", reasoningMode: "disabled", modelIds: [] }],
  },
};

export function defaultApiProviderValue(): ApiProviderValue {
  return {
    kind: null,
    name: "",
    baseUrl: "",
    wireApi: "responses",
    protocolBindings: [{ wireApi: "responses", adapter: "native", reasoningMode: "disabled", modelIds: [] }],
    apiKey: "",
    models: [],
    modelCatalogMode: "automatic",
    autoAssignModels: true,
  };
}

export function selectApiProvider(value: ApiProviderValue, kind: ApiProviderKind): ApiProviderValue {
  const definition = providerDefaults[kind];
  return {
    ...definition,
    protocolBindings: definition.protocolBindings.map(cloneBinding),
    apiKey: value.apiKey,
    models: value.models ?? [],
    modelCatalogMode: value.modelCatalogMode ?? "automatic",
    autoAssignModels: value.autoAssignModels !== false,
  };
}

export function clearApiProviderSelection(value: ApiProviderValue): ApiProviderValue {
  return {
    ...defaultApiProviderValue(),
    // Changing the provider should not make the user retype an API key they
    // have already entered. The key is still never rendered in the selector.
    apiKey: value.apiKey,
    models: value.models ?? [],
    modelCatalogMode: value.modelCatalogMode ?? "automatic",
    autoAssignModels: value.autoAssignModels !== false,
  };
}

export function setApiProviderModelCatalogMode(value: ApiProviderValue, mode: ModelCatalogMode): ApiProviderValue {
  return {
    ...value,
    modelCatalogMode: mode,
    protocolBindings: mode === "manual"
      ? value.protocolBindings
      : value.protocolBindings.map((binding) => ({ ...binding, modelIds: [] })),
  };
}

export function addApiProviderModel(value: ApiProviderValue, rawModel: string): ApiProviderValue | null {
  if ((value.modelCatalogMode ?? ((value.models ?? []).length ? "manual" : "automatic")) !== "manual") return null;
  const model = rawModel.trim();
  if (!model) return null;
  const models = value.models ?? [];
  if (models.some((candidate) => candidate.toLowerCase() === model.toLowerCase())) return null;
  const nextModels = [...models, model];
  const autoAssignModels = value.autoAssignModels !== false;
  const protocolBindings = autoAssignModels && value.protocolBindings.length
    ? value.protocolBindings.map((binding, index) => index === 0 && !binding.modelIds.some((candidate) => candidate.toLowerCase() === model.toLowerCase())
      ? { ...binding, modelIds: [...binding.modelIds, model] }
      : binding)
    : value.protocolBindings;
  return { ...value, models: nextModels, protocolBindings };
}

export function removeApiProviderModel(value: ApiProviderValue, model: string): ApiProviderValue {
  const normalized = model.toLowerCase();
  return {
    ...value,
    models: (value.models ?? []).filter((candidate) => candidate.toLowerCase() !== normalized),
    protocolBindings: value.protocolBindings.map((binding) => ({
      ...binding,
      modelIds: binding.modelIds.filter((candidate) => candidate.toLowerCase() !== normalized),
    })),
  };
}

export function setApiProviderAutoAssignModels(value: ApiProviderValue, autoAssignModels: boolean): ApiProviderValue {
  if (!autoAssignModels || !value.protocolBindings.length) return { ...value, autoAssignModels };
  const models = value.models ?? [];
  const assigned = new Set(value.protocolBindings.flatMap((binding) => binding.modelIds.map((model) => model.toLowerCase())));
  return {
    ...value,
    autoAssignModels,
    protocolBindings: value.protocolBindings.map((binding, index) => index === 0
      ? { ...binding, modelIds: [...binding.modelIds, ...models.filter((model) => !assigned.has(model.toLowerCase()))] }
      : binding),
  };
}

export function apiProviderReady(value: ApiProviderValue) {
  return Boolean(
    value.kind
      && value.apiKey.trim()
      && providerProtocolBindings(value).length
      && (value.kind === "zenith" || (value.name.trim() && value.baseUrl.trim())),
  );
}

export function apiProviderSourceInput(value: ApiProviderValue) {
  const manualMode = value.modelCatalogMode === "manual"
    || (value.modelCatalogMode === undefined && (value.models ?? []).length > 0);
  const models = manualMode ? (value.models ?? []).map((model) => model.trim()).filter(Boolean) : [];
  const autoAssignModels = manualMode && value.autoAssignModels !== false;
  let manualCatalogAssigned = false;
  const protocolBindings = providerProtocolBindings(value).map((binding) => {
    const assignManualCatalog = autoAssignModels && models.length > 0
      && binding.modelIds.length === 0
      && !manualCatalogAssigned;
    if (assignManualCatalog) manualCatalogAssigned = true;
    return {
      ...binding,
      // The simple setup picker has one selected route. Assigning the optional
      // manual catalog to only the first empty route keeps later routes
      // intentionally unassigned instead of creating an overlap.
      modelIds: assignManualCatalog ? models : binding.modelIds,
    };
  });
  return {
    name: value.name.trim(),
    baseUrl: value.baseUrl.trim(),
    apiKey: value.apiKey.trim(),
    wireApi: protocolBindings[0]?.wireApi ?? value.wireApi,
    protocolBindings,
    models,
    allowedModels: [],
    excludedModels: [],
    draining: false,
    priority: 0,
    weight: 1,
    recoveryDelaySeconds: 0,
  };
}

function providerProtocolBindings(value: ApiProviderValue) {
  return value.protocolBindings.map((binding) => {
    const adapter = normalizedAdapter(binding);
    return {
      wireApi: binding.wireApi,
      modelIds: [...binding.modelIds],
      adapter,
      reasoningMode: normalizedReasoningMode(binding, adapter),
      ...(binding.cacheWriteTtl ? { cacheWriteTtl: binding.cacheWriteTtl } : {}),
    };
  });
}

function cloneBinding(binding: SourceProtocolBinding): SourceProtocolBinding {
  return {
    wireApi: binding.wireApi,
    modelIds: [...binding.modelIds],
    adapter: binding.adapter ?? "native",
    reasoningMode: binding.reasoningMode ?? "disabled",
    ...(binding.cacheWriteTtl ? { cacheWriteTtl: binding.cacheWriteTtl } : {}),
  };
}

import type { SourceWireApi } from "../api/types";

export type ApiProviderKind = "zenith" | "openai" | "openrouter" | "custom";
export type ApiProviderValue = {
  kind: ApiProviderKind | null;
  name: string;
  baseUrl: string;
  wireApi: SourceWireApi;
  apiKey: string;
  /** Explicit LiteLLM namespace used for source pricing, when confirmed. */
  pricingProvider?: string | null;
  /** Explicit official family allowed as a canonical pricing fallback. */
  officialProviderFamily?: string | null;
};

export type ApiProviderDefinition = Omit<ApiProviderValue, "apiKey">;

export const providerOrder: ApiProviderKind[] = ["openai", "openrouter", "zenith", "custom"];

export const providerDefaults: Record<ApiProviderKind, ApiProviderDefinition> = {
  zenith: {
    kind: "zenith",
    name: "Zenith API",
    baseUrl: "https://api.zenithmarket.dev/v1",
    pricingProvider: null,
    officialProviderFamily: null,
    wireApi: "responses",
  },
  openai: {
    kind: "openai",
    name: "OpenAI",
    baseUrl: "https://api.openai.com/v1",
    pricingProvider: "openai",
    officialProviderFamily: "openai",
    wireApi: "responses",
  },
  openrouter: {
    kind: "openrouter",
    name: "OpenRouter",
    baseUrl: "https://openrouter.ai/api/v1",
    pricingProvider: "openrouter",
    officialProviderFamily: null,
    wireApi: "chat_completions",
  },
  custom: {
    kind: "custom",
    name: "",
    baseUrl: "",
    pricingProvider: null,
    officialProviderFamily: null,
    wireApi: "responses",
  },
};

export function defaultApiProviderValue(): ApiProviderValue {
  return {
    kind: null,
    name: "",
    baseUrl: "",
    wireApi: "responses",
    apiKey: "",
    pricingProvider: null,
    officialProviderFamily: null,
  };
}

export function selectApiProvider(value: ApiProviderValue, kind: ApiProviderKind): ApiProviderValue {
  const definition = providerDefaults[kind];
  return {
    ...definition,
    apiKey: value.apiKey,
    pricingProvider: definition.pricingProvider ?? null,
    officialProviderFamily: definition.officialProviderFamily ?? null,
  };
}

export function apiProviderReady(value: ApiProviderValue) {
  return Boolean(
    value.kind
      && value.apiKey.trim()
      && value.name.trim()
      && value.baseUrl.trim(),
  );
}

export function apiProviderSourceInput(value: ApiProviderValue) {
  return {
    name: value.name.trim(),
    baseUrl: value.baseUrl.trim(),
    apiKey: value.apiKey.trim(),
    pricingProvider: value.pricingProvider?.trim() || null,
    officialProviderFamily: value.officialProviderFamily?.trim() || null,
    wireApi: value.wireApi,
    // New sources rely on endpoint/service discovery. Persisted bindings are
    // accepted only as migration hints for existing or mixed-protocol sources.
    protocolBindings: [],
    models: [],
    allowedModels: [],
    excludedModels: [],
    draining: false,
    priority: 0,
    weight: 1,
    recoveryDelaySeconds: 0,
  };
}

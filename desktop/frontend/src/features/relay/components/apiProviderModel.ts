import type { SourceWireApi } from "../api/types";
import type { ApiKeyPageProvider } from "../../../platform/desktop";

export type ApiProviderKind = ApiKeyPageProvider | "custom";
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

export const providerOrder: ApiProviderKind[] = [
  "openai", "anthropic", "gemini", "deepseek", "groq", "mistral",
  "moonshot", "kimi", "minimax", "openrouter", "zenith", "custom",
];

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
  anthropic: {
    kind: "anthropic",
    name: "Anthropic",
    baseUrl: "https://api.anthropic.com/v1",
    pricingProvider: "anthropic",
    officialProviderFamily: "anthropic",
    wireApi: "messages",
  },
  gemini: {
    kind: "gemini",
    name: "Google Gemini",
    baseUrl: "https://generativelanguage.googleapis.com/v1beta",
    pricingProvider: "gemini",
    officialProviderFamily: "gemini",
    wireApi: "gemini",
  },
  deepseek: {
    kind: "deepseek",
    name: "DeepSeek",
    baseUrl: "https://api.deepseek.com/v1",
    pricingProvider: "deepseek",
    officialProviderFamily: "deepseek",
    wireApi: "chat_completions",
  },
  groq: {
    kind: "groq",
    name: "Groq",
    baseUrl: "https://api.groq.com/openai/v1",
    pricingProvider: "groq",
    officialProviderFamily: "groq",
    wireApi: "chat_completions",
  },
  mistral: {
    kind: "mistral",
    name: "Mistral",
    baseUrl: "https://api.mistral.ai/v1",
    pricingProvider: "mistral",
    officialProviderFamily: "mistral",
    wireApi: "chat_completions",
  },
  moonshot: {
    kind: "moonshot",
    name: "Moonshot / Kimi API",
    baseUrl: "https://api.moonshot.ai/v1",
    pricingProvider: "moonshot",
    officialProviderFamily: "moonshot",
    wireApi: "responses",
  },
  kimi: {
    kind: "kimi",
    name: "Kimi Code",
    baseUrl: "https://api.kimi.ai/coding/v1/messages",
    pricingProvider: null,
    officialProviderFamily: null,
    wireApi: "messages",
  },
  minimax: {
    kind: "minimax",
    name: "MiniMax",
    baseUrl: "https://api.minimax.io/v1",
    pricingProvider: "minimax",
    officialProviderFamily: "minimax",
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

export function selectApiProvider(providerValue: ApiProviderValue, kind: ApiProviderKind): ApiProviderValue {
  const definition = providerDefaults[kind];
  return {
    ...definition,
    apiKey: providerValue.apiKey,
    pricingProvider: definition.pricingProvider ?? null,
    officialProviderFamily: definition.officialProviderFamily ?? null,
  };
}

export function apiProviderReady(providerValue: ApiProviderValue) {
  return Boolean(
    providerValue.kind
      && providerValue.apiKey.trim()
      && providerValue.name.trim()
      && providerValue.baseUrl.trim(),
  );
}

export function apiProviderSourceInput(providerValue: ApiProviderValue) {
  return {
    name: providerValue.name.trim(),
    baseUrl: providerValue.baseUrl.trim(),
    apiKey: providerValue.apiKey.trim(),
    pricingProvider: providerValue.pricingProvider?.trim() || null,
    officialProviderFamily: providerValue.officialProviderFamily?.trim() || null,
    wireApi: providerValue.wireApi,
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

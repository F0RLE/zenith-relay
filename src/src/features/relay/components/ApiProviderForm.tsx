import { Cloud, ExternalLink, Route, Settings2, Sparkles } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { SourceProtocolBinding, SourceWireApi } from "../api/types";
import { openApiKeyPage } from "../../../platform/desktop";
import { SourceProtocolRoutingDisclosure } from "./SourceProtocolRoutingDisclosure";
import { SecretField } from "./Ui";

export type ApiProviderKind = "zenith" | "openai" | "openrouter" | "custom";
export type ApiProviderValue = {
  kind: ApiProviderKind | null;
  name: string;
  baseUrl: string;
  wireApi: SourceWireApi;
  protocolBindings: SourceProtocolBinding[];
  apiKey: string;
};

type ApiProviderDefinition = Omit<ApiProviderValue, "apiKey">;
type ApiProviderFormVariant = "source" | "onboarding";

const providerOrder: ApiProviderKind[] = ["openai", "openrouter", "zenith", "custom"];

const providerDefaults: Record<ApiProviderKind, ApiProviderDefinition> = {
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

const providerIcons = {
  zenith: Cloud,
  openai: Sparkles,
  openrouter: Route,
  custom: Settings2,
};

export function defaultApiProviderValue(): ApiProviderValue {
  return {
    kind: null,
    name: "",
    baseUrl: "",
    wireApi: "responses",
    protocolBindings: [{ wireApi: "responses", adapter: "native", reasoningMode: "disabled", modelIds: [] }],
    apiKey: "",
  };
}

function providerProtocolBindings(value: ApiProviderValue) {
  return value.protocolBindings
    .map((binding) => ({
      wireApi: binding.wireApi,
      modelIds: [...binding.modelIds],
      adapter: binding.adapter ?? "native",
      reasoningMode: binding.reasoningMode ?? "disabled",
      ...(binding.cacheWriteTtl ? { cacheWriteTtl: binding.cacheWriteTtl } : {}),
    }));
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
  const protocolBindings = providerProtocolBindings(value);
  return {
    name: value.name.trim(),
    baseUrl: value.baseUrl.trim(),
    apiKey: value.apiKey.trim(),
    wireApi: protocolBindings[0]?.wireApi ?? value.wireApi,
    protocolBindings,
    models: [],
    allowedModels: [],
    excludedModels: [],
    draining: false,
    priority: 0,
    weight: 1,
    recoveryDelaySeconds: 0,
  };
}

export function ApiProviderForm({
  value,
  onChange,
  variant = "source",
}: {
  value: ApiProviderValue;
  onChange: (value: ApiProviderValue) => void;
  variant?: ApiProviderFormVariant;
}) {
  const { t } = useTranslation();
  const onboarding = variant === "onboarding";
  const select = (kind: ApiProviderKind) => onChange({
    ...providerDefaults[kind],
    protocolBindings: providerDefaults[kind].protocolBindings.map((binding) => ({
      wireApi: binding.wireApi,
      modelIds: [...binding.modelIds],
      adapter: binding.adapter ?? "native",
      reasoningMode: binding.reasoningMode ?? "disabled",
    })),
    apiKey: value.apiKey,
  });
  const clearSelection = () => onChange({
    ...defaultApiProviderValue(),
    // Changing the provider should not make the user retype an API key they
    // have already entered. The key is still never rendered in the selector.
    apiKey: value.apiKey,
  });
  const setProtocolBindings = (protocolBindings: SourceProtocolBinding[]) => {
    onChange({
      ...value,
      protocolBindings,
      wireApi: protocolBindings[0]?.wireApi ?? value.wireApi,
    });
  };
  const selectedProvider = value.kind ? providerDefaults[value.kind] : null;
  const SelectedProviderIcon = value.kind ? providerIcons[value.kind] : null;

  return <div className={`api-provider-setup ${value.kind ? "has-selection" : "selection-only"}${onboarding ? " onboarding-api-provider" : ""}`}>
    {selectedProvider && SelectedProviderIcon
      ? <div className="api-provider-selected">
        <span className="api-provider-title">
          <SelectedProviderIcon aria-hidden />
          <strong>{selectedProvider.name || t("apiProviders.custom")}</strong>
        </span>
        {value.kind !== "custom" ? <span className="api-provider-selected-endpoint">
          <span>{t("sources.address")}</span>
          <code>{value.baseUrl}</code>
        </span> : null}
        <button type="button" className="api-provider-change" onClick={clearSelection}>{t("common.edit")}</button>
      </div>
      : <>
        {!onboarding ? <header className="api-provider-intro">
          <strong>{t("apiProviders.choose")}</strong>
          <p>{t("apiProviders.hint")}</p>
        </header> : null}
        <div className="api-provider-options" role="radiogroup" aria-label={t("apiProviders.choose")}>
          {providerOrder.map((kind) => {
            const Icon = providerIcons[kind];
            return <button key={kind} type="button" role="radio" aria-checked={false} onClick={() => select(kind)}>
              <span className="api-provider-title"><Icon aria-hidden /><strong>{providerDefaults[kind].name || t("apiProviders.custom")}</strong></span>
              <small>{t(`apiProviders.${onboarding ? "onboardingDescriptions" : "descriptions"}.${kind}`)}</small>
            </button>;
          })}
        </div>
      </>}
    {value.kind ? <div className="api-provider-configuration">
      {value.kind === "custom" ? <div className="api-provider-fields">
        <label className="relay-field"><span>{t("common.name")}</span><input value={value.name} onChange={(event) => onChange({ ...value, name: event.target.value })} required /></label>
        <label className="relay-field"><span>{t("sources.address")}</span><input type="url" value={value.baseUrl} onChange={(event) => onChange({ ...value, baseUrl: event.target.value })} placeholder="https://api.example.com/v1" required /></label>
      </div> : null}
      <div className="api-provider-key-field">
        <SecretField
          label={t(onboarding ? "apiKey.label" : "sources.apiKey")}
          value={value.apiKey}
          onChange={(apiKey) => onChange({ ...value, apiKey })}
          labelAction={value.kind !== "custom"
            ? <button type="button" className="api-key-link" onClick={() => void openApiKeyPage(value.kind as "zenith" | "openai" | "openrouter")}>
              <ExternalLink aria-hidden />
              {t("apiProviders.getKey")}
            </button>
            : undefined}
        />
      </div>
      {!onboarding ? <SourceProtocolRoutingDisclosure
        models={[]}
        value={value.protocolBindings}
        onChange={setProtocolBindings}
      /> : null}
    </div> : null}
  </div>;
}

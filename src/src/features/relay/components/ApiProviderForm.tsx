import { Cloud, ExternalLink, Route, Settings2, Sparkles, X } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { SourceProtocolBinding, SourceWireApi } from "../api/types";
import { openApiKeyPage } from "../../../platform/desktop";
import { SourceProtocolRoutingDisclosure } from "./SourceProtocolRoutingDisclosure";
import { normalizedAdapter, normalizedReasoningMode } from "../sourceProtocolBindings";
import { SecretField } from "./Ui";

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
    models: [],
    modelCatalogMode: "automatic",
    autoAssignModels: true,
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

export function ApiProviderForm({
  value,
  onChange,
  variant = "source",
  allowManualModels = true,
  showConfiguration = true,
  showRouting = true,
  showIntro = true,
  showSelectionSummary = true,
}: {
  value: ApiProviderValue;
  onChange: (value: ApiProviderValue) => void;
  variant?: ApiProviderFormVariant;
  allowManualModels?: boolean;
  /** Render only the provider picker/summary when the parent owns a multi-step flow. */
  showConfiguration?: boolean;
  /** Keep protocol routing in a separate disclosure when the form is compact. */
  showRouting?: boolean;
  /** Hide the generic picker heading when the surrounding dialog provides one. */
  showIntro?: boolean;
  /** Hide the selected-provider row when the parent already owns the step navigation. */
  showSelectionSummary?: boolean;
}) {
  const { t } = useTranslation();
  const [modelDraft, setModelDraft] = useState("");
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
    models: value.models ?? [],
    modelCatalogMode: value.modelCatalogMode ?? "automatic",
    autoAssignModels: value.autoAssignModels !== false,
  });
  const clearSelection = () => onChange({
    ...defaultApiProviderValue(),
    // Changing the provider should not make the user retype an API key they
    // have already entered. The key is still never rendered in the selector.
    apiKey: value.apiKey,
    models: value.models ?? [],
    modelCatalogMode: value.modelCatalogMode ?? "automatic",
    autoAssignModels: value.autoAssignModels !== false,
  });
  const modelCatalogMode = value.modelCatalogMode ?? ((value.models ?? []).length ? "manual" : "automatic");
  const setModelCatalogMode = (mode: "automatic" | "manual") => {
    setModelDraft("");
    onChange({
      ...value,
      modelCatalogMode: mode,
      protocolBindings: mode === "manual"
        ? value.protocolBindings
        : value.protocolBindings.map((binding) => ({ ...binding, modelIds: [] })),
    });
  };
  const addModel = () => {
    if (modelCatalogMode !== "manual") return;
    const model = modelDraft.trim();
    if (!model) return;
    const models = value.models ?? [];
    if (models.some((candidate) => candidate.toLowerCase() === model.toLowerCase())) {
      setModelDraft("");
      return;
    }
    const nextModels = [...models, model];
    const autoAssignModels = value.autoAssignModels !== false;
    const protocolBindings = autoAssignModels && value.protocolBindings.length
      ? value.protocolBindings.map((binding, index) => index === 0 && !binding.modelIds.some((candidate) => candidate.toLowerCase() === model.toLowerCase())
        ? { ...binding, modelIds: [...binding.modelIds, model] }
        : binding)
      : value.protocolBindings;
    onChange({ ...value, models: nextModels, protocolBindings });
    setModelDraft("");
  };
  const removeModel = (model: string) => onChange({
    ...value,
    models: (value.models ?? []).filter((candidate) => candidate.toLowerCase() !== model.toLowerCase()),
    protocolBindings: value.protocolBindings.map((binding) => ({
      ...binding,
      modelIds: binding.modelIds.filter((candidate) => candidate.toLowerCase() !== model.toLowerCase()),
    })),
  });
  const setAutoAssignModels = (autoAssignModels: boolean) => {
    if (!autoAssignModels || !value.protocolBindings.length) {
      onChange({ ...value, autoAssignModels });
      return;
    }
    const models = value.models ?? [];
    const assigned = new Set(value.protocolBindings.flatMap((binding) => binding.modelIds.map((model) => model.toLowerCase())));
    onChange({
      ...value,
      autoAssignModels,
      protocolBindings: value.protocolBindings.map((binding, index) => index === 0
        ? { ...binding, modelIds: [...binding.modelIds, ...models.filter((model) => !assigned.has(model.toLowerCase()))] }
        : binding),
    });
  };
  const selectedProvider = value.kind ? providerDefaults[value.kind] : null;
  const SelectedProviderIcon = value.kind ? providerIcons[value.kind] : null;

  return <div className={`api-provider-setup ${value.kind ? "has-selection" : "selection-only"}${onboarding ? " onboarding-api-provider" : ""}`}>
    {selectedProvider && SelectedProviderIcon && showSelectionSummary
      ? <div className="api-provider-selected">
        <span className="api-provider-title">
          <SelectedProviderIcon aria-hidden />
          <strong>{selectedProvider.name || t("apiProviders.custom")}</strong>
        </span>
        <button type="button" className="api-provider-change" onClick={clearSelection}>{t("common.edit")}</button>
      </div>
      : !value.kind ? <>
        {!onboarding && showIntro ? <header className="api-provider-intro">
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
      </> : null}
    {value.kind && showConfiguration ? <div className="api-provider-configuration">
      {!onboarding ? <div className="api-provider-fields">
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
      {!onboarding && allowManualModels ? <section className="source-manual-models-field" aria-labelledby="source-manual-models-label">
        <div className="source-models-toolbar">
          <strong id="source-manual-models-label">{t("sources.manualModels")}</strong>
          <div className="source-model-mode" role="radiogroup" aria-label={t("sources.modelsMode")}>
            {(["automatic", "manual"] as const).map((mode) => <label key={mode} className={modelCatalogMode === mode ? "selected" : ""}>
              <input
                type="radio"
                name="source-model-catalog-mode"
                checked={modelCatalogMode === mode}
                onChange={() => setModelCatalogMode(mode)}
              />
              <span>{t(`sources.models${mode === "automatic" ? "Automatic" : "Manual"}`)}</span>
            </label>)}
          </div>
        </div>
        {modelCatalogMode === "automatic"
          ? <small id="source-manual-models-hint">{t("sources.modelsAutomaticHint")}</small>
          : <>
            <div className="source-model-entry">
              <input
                value={modelDraft}
                onChange={(event) => setModelDraft(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Enter" || event.key === ",") {
                    event.preventDefault();
                    addModel();
                  }
                }}
                placeholder={t("sources.manualModelsPlaceholder")}
                aria-label={t("sources.manualModelInput")}
              />
              <button type="button" className="source-model-add" onClick={addModel} aria-label={t("sources.addManualModel")}>
                <span aria-hidden>+</span>
              </button>
            </div>
            {(value.models ?? []).length ? <div className="source-model-chips" aria-label={t("sources.manualModels")}>
              {(value.models ?? []).map((model) => <span className="source-model-chip" key={model}>
                <code>{model}</code>
                <button type="button" onClick={() => removeModel(model)} aria-label={t("sources.removeManualModel", { model })} title={t("common.delete")}>
                  <X aria-hidden />
                </button>
              </span>)}
            </div> : <small id="source-manual-models-hint">{t("sources.manualModelsHint")}</small>}
            <label className="source-auto-route-toggle" title={t("sources.autoAssignModelsHint")}>
              <input
                type="checkbox"
                checked={value.autoAssignModels !== false}
                onChange={(event) => setAutoAssignModels(event.target.checked)}
              />
              <span>{t("sources.autoAssignModels")}</span>
            </label>
          </>}
      </section> : null}
      {!onboarding && showRouting ? <SourceProtocolRoutingDisclosure
        models={modelCatalogMode === "manual" ? (value.models ?? []) : []}
        value={value.protocolBindings}
        showSimplePicker={allowManualModels}
        autoAssignModels={value.autoAssignModels !== false}
        onChange={(protocolBindings) => onChange({
          ...value,
          protocolBindings,
          wireApi: protocolBindings[0]?.wireApi ?? value.wireApi,
        })}
      /> : null}
    </div> : null}
  </div>;
}

import { Cloud, ExternalLink, Route, Settings2, Sparkles, X } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { openApiKeyPage } from "../../../platform/desktop";
import { SourceProtocolBindingsEditor } from "./SourceProtocolBindingsEditor";
import { SecretField } from "./Ui";
import {
  addApiProviderModel,
  clearApiProviderSelection,
  providerDefaults,
  providerOrder,
  removeApiProviderModel,
  selectApiProvider,
  setApiProviderAutoAssignModels,
  setApiProviderModelCatalogMode,
  type ApiProviderKind,
  type ApiProviderValue,
} from "./apiProviderModel";

export {
  apiProviderReady,
  apiProviderSourceInput,
  defaultApiProviderValue,
  type ApiProviderKind,
  type ApiProviderValue,
} from "./apiProviderModel";

type ApiProviderFormVariant = "source" | "onboarding";

const providerIcons = {
  zenith: Cloud,
  openai: Sparkles,
  openrouter: Route,
  custom: Settings2,
};


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
  const select = (kind: ApiProviderKind) => onChange(selectApiProvider(value, kind));
  const clearSelection = () => onChange(clearApiProviderSelection(value));
  const modelCatalogMode = value.modelCatalogMode ?? ((value.models ?? []).length ? "manual" : "automatic");
  const setModelCatalogMode = (mode: "automatic" | "manual") => {
    setModelDraft("");
    onChange(setApiProviderModelCatalogMode(value, mode));
  };
  const addModel = () => {
    const next = addApiProviderModel(value, modelDraft);
    if (next) onChange(next);
    if (next || modelDraft.trim() && (value.models ?? []).some((model) => model.toLowerCase() === modelDraft.trim().toLowerCase())) setModelDraft("");
  };
  const removeModel = (model: string) => onChange(removeApiProviderModel(value, model));
  const setAutoAssignModels = (autoAssignModels: boolean) => onChange(setApiProviderAutoAssignModels(value, autoAssignModels));
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
      {!onboarding && showRouting ? <SourceProtocolBindingsEditor
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

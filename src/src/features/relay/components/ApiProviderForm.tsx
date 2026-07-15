import { Cloud, Route, Settings2, Sparkles } from "lucide-react";
import { useTranslation } from "react-i18next";
import { OptionMenu, SecretField } from "./Ui";

export type ApiProviderKind = "zenith" | "openai" | "openrouter" | "custom";
export type ApiProviderValue = {
  kind: ApiProviderKind;
  name: string;
  baseUrl: string;
  wireApi: "responses" | "chat_completions";
  apiKey: string;
};

const providerDefaults: Record<ApiProviderKind, Omit<ApiProviderValue, "apiKey">> = {
  zenith: { kind: "zenith", name: "Zenith API", baseUrl: "https://api.zenithmarket.dev/v1", wireApi: "responses" },
  openai: { kind: "openai", name: "OpenAI", baseUrl: "https://api.openai.com/v1", wireApi: "responses" },
  openrouter: { kind: "openrouter", name: "OpenRouter", baseUrl: "https://openrouter.ai/api/v1", wireApi: "chat_completions" },
  custom: { kind: "custom", name: "", baseUrl: "", wireApi: "responses" },
};

const providerIcons = {
  zenith: Cloud,
  openai: Sparkles,
  openrouter: Route,
  custom: Settings2,
};

export function defaultApiProviderValue(): ApiProviderValue {
  return { ...providerDefaults.zenith, apiKey: "" };
}

export function apiProviderReady(value: ApiProviderValue) {
  return Boolean(value.apiKey.trim() && (value.kind === "zenith" || (value.name.trim() && value.baseUrl.trim())));
}

export function apiProviderSourceInput(value: ApiProviderValue) {
  return {
    name: value.name.trim(),
    baseUrl: value.baseUrl.trim(),
    apiKey: value.apiKey.trim(),
    wireApi: value.wireApi,
    models: [],
    allowedModels: [],
    excludedModels: [],
    draining: false,
    priority: 0,
    weight: 100,
  };
}

export function ApiProviderForm({ value, onChange }: { value: ApiProviderValue; onChange: (value: ApiProviderValue) => void }) {
  const { t } = useTranslation();
  const select = (kind: ApiProviderKind) => onChange({ ...providerDefaults[kind], apiKey: value.apiKey });

  return <div className="api-provider-setup">
    <div className="api-provider-options" role="radiogroup" aria-label={t("apiProviders.choose")}>
      {(Object.keys(providerDefaults) as ApiProviderKind[]).map((kind) => {
        const Icon = providerIcons[kind];
        return <button key={kind} type="button" role="radio" aria-checked={value.kind === kind} className={value.kind === kind ? "selected" : ""} onClick={() => select(kind)}>
          <span className="api-provider-title"><Icon aria-hidden /><strong>{providerDefaults[kind].name || t("apiProviders.custom")}</strong>{kind === "zenith" ? <em>{t("common.recommended")}</em> : null}</span>
          <small>{t(`apiProviders.descriptions.${kind}`)}</small>
        </button>;
      })}
    </div>
    {value.kind !== "zenith" ? <div className="api-provider-fields">
      <label className="relay-field"><span>{t("common.name")}</span><input value={value.name} onChange={(event) => onChange({ ...value, name: event.target.value })} required /></label>
      <label className="relay-field"><span>{t("sources.address")}</span><input type="url" value={value.baseUrl} onChange={(event) => onChange({ ...value, baseUrl: event.target.value })} placeholder="https://api.example.com/v1" required /></label>
      <div className="relay-field"><span>{t("sources.protocol")}</span><OptionMenu className="field-option-menu" label={t("sources.protocol")} value={value.wireApi} onChange={(wireApi) => onChange({ ...value, wireApi: wireApi as ApiProviderValue["wireApi"] })} options={[{ value: "responses", label: "Responses API" }, { value: "chat_completions", label: "Chat Completions" }]} /></div>
    </div> : null}
    <SecretField label={value.kind === "zenith" ? t("readyApi.key") : t("sources.apiKey")} value={value.apiKey} onChange={(apiKey) => onChange({ ...value, apiKey })} />
    <p className="form-note">{t(value.kind === "zenith" ? "apiProviders.zenithHint" : "apiProviders.localPoolHint")}</p>
  </div>;
}

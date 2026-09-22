import { Cloud, ExternalLink, Route, Settings2, Sparkles } from "lucide-react";
import { useTranslation } from "react-i18next";
import { openApiKeyPage } from "../../../platform/desktop";
import { SecretField } from "./Ui";
import {
  providerDefaults,
  providerOrder,
  selectApiProvider,
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
}: {
  value: ApiProviderValue;
  onChange: (value: ApiProviderValue) => void;
  variant?: ApiProviderFormVariant;
}) {
  const { t } = useTranslation();
  const onboarding = variant === "onboarding";
  const select = (kind: ApiProviderKind) => {
    if (kind !== value.kind) onChange(selectApiProvider(value, kind));
  };

  return <div className="api-provider-setup">
    <div className="api-provider-options" role="radiogroup" aria-label={t("apiProviders.choose")}>
      {providerOrder.map((kind, index) => {
        const Icon = providerIcons[kind];
        return <button key={kind} type="button" role="radio" aria-checked={value.kind === kind} className={value.kind === kind ? "selected" : undefined} tabIndex={value.kind === kind || (!value.kind && index === 0) ? 0 : -1} onClick={() => select(kind)} onKeyDown={(event) => {
          const offset = event.key === "ArrowRight" || event.key === "ArrowDown" ? 1 : event.key === "ArrowLeft" || event.key === "ArrowUp" ? -1 : 0;
          if (!offset && event.key !== "Home" && event.key !== "End") return;
          event.preventDefault();
          const next = event.key === "Home" ? 0 : event.key === "End" ? providerOrder.length - 1 : (index + offset + providerOrder.length) % providerOrder.length;
          const nextKind = providerOrder[next];
          if (!nextKind) return;
          select(nextKind);
          event.currentTarget.parentElement?.querySelectorAll<HTMLButtonElement>("button")[next]?.focus();
        }}>
          <span className="api-provider-title"><Icon aria-hidden /><strong>{providerDefaults[kind].name || t("apiProviders.custom")}</strong></span>
        </button>;
      })}
    </div>
    {value.kind ? <div className="api-provider-configuration">
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
      <label className="relay-field"><span>{t("sources.address")}</span><input type="url" value={value.baseUrl} onChange={(event) => onChange({ ...value, baseUrl: event.target.value })} placeholder="https://api.example.com/v1" required spellCheck={false} autoCapitalize="none" /></label>
      <label className="relay-field"><span>{t("common.name")}</span><input value={value.name} onChange={(event) => onChange({ ...value, name: event.target.value })} placeholder={t("apiProviders.namePlaceholder")} required /></label>
    </div> : !onboarding ? <p className="api-provider-empty">{t("apiProviders.hint")}</p> : null}
  </div>;
}

import { ExternalLink } from "lucide-react";
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

function nextProviderIndex(key: string, index: number, count: number) {
  if (key === "Home") return 0;
  if (key === "End") return count - 1;
  const offset = key === "ArrowRight" || key === "ArrowDown" ? 1 : key === "ArrowLeft" || key === "ArrowUp" ? -1 : 0;
  return offset ? (index + offset + count) % count : null;
}

export function ApiProviderForm({
  value,
  onChange,
}: {
  value: ApiProviderValue;
  onChange: (value: ApiProviderValue) => void;
}) {
  const { t } = useTranslation();
  const keyPageProvider = value.kind === "custom" ? null : value.kind;
  const select = (kind: ApiProviderKind) => {
    if (kind !== value.kind) onChange(selectApiProvider(value, kind));
  };

  return <div className="api-provider-setup" data-configured={Boolean(value.kind)}>
    <section className="api-provider-picker" aria-label={t("apiProviders.choose")}>
      <h3>{t("apiProviders.choose")}</h3>
    <div className="api-provider-options" role="radiogroup" aria-label={t("apiProviders.choose")}>
      {providerOrder.map((kind, index) => {
        return (
          <button
            key={kind}
            type="button"
            role="radio"
            aria-checked={value.kind === kind}
            className={value.kind === kind ? "selected" : undefined}
            tabIndex={value.kind === kind || (!value.kind && index === 0) ? 0 : -1}
            onClick={() => select(kind)}
            onKeyDown={(event) => {
              const nextProviderPosition = nextProviderIndex(event.key, index, providerOrder.length);
              if (nextProviderPosition == null) return;
              event.preventDefault();
              const nextKind = providerOrder[nextProviderPosition];
              if (!nextKind) return;
              select(nextKind);
              event.currentTarget.parentElement?.querySelectorAll<HTMLButtonElement>("button")[nextProviderPosition]?.focus();
            }}
          >
            <span className="api-provider-title">{providerDefaults[kind].name || t("apiProviders.custom")}</span>
          </button>
        );
      })}
    </div>
    </section>
    {value.kind ? <section className="api-provider-configuration" aria-label={value.name || t("apiProviders.custom")}>
      <h3>{providerDefaults[value.kind].name || t("apiProviders.custom")}</h3>
      <div className="api-provider-identity">
        <label className="relay-field">
          <span>{t("common.name")}</span>
          <input
            value={value.name}
            onChange={(event) => onChange({ ...value, name: event.target.value })}
            placeholder={t("apiProviders.namePlaceholder")}
            required
          />
        </label>
        <label className="relay-field">
          <span>{t("sources.address")}</span>
          <input
            type="url"
            value={value.baseUrl}
            onChange={(event) => onChange({ ...value, baseUrl: event.target.value })}
            placeholder="https://api.example.com/v1"
            required
            spellCheck={false}
            autoCapitalize="none"
          />
        </label>
      </div>
      <div className="api-provider-key-field">
        <SecretField
          label={t("sources.apiKey")}
          value={value.apiKey}
          onChange={(apiKey) => onChange({ ...value, apiKey })}
          labelAction={keyPageProvider
            ? <button type="button" className="api-key-link" onClick={() => void openApiKeyPage(keyPageProvider)}>
              <ExternalLink aria-hidden />
              {t("apiProviders.getKey")}
            </button>
            : undefined}
        />
      </div>
    </section> : null}
  </div>;
}

import { ChevronDown, RotateCcw } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { SourceSummary } from "../api/types";
import { groupModels, memberModelCatalog, modelIdKey, orderModelIdsBySnapshot } from "../modelGroups";
import {
  formatModelPricePlaceholder,
  parseEditableModelPrice,
} from "../modelPricing";
import { sourceModelsWithCacheWritePricing } from "../sourceProtocolBindings";
import { useRelayState } from "../state/RelayStateProvider";
import { IconButton, ToggleSwitch } from "./Ui";
import {
  removeSourcePriceDraft,
  sourcePriceModels,
  type SourcePriceDraft,
  type SourcePriceDrafts,
  updateSourcePriceDraft,
} from "./sourcePriceEditorModel";

type SourcePriceEditorProps = {
  source: SourceSummary;
  drafts: SourcePriceDrafts;
  onChange: (value: SourcePriceDrafts) => void;
  enabledModels?: readonly string[];
  onToggleModel?: (model: string) => void;
  presentation?: "disclosure" | "tab" | "member";
};

export function SourcePriceEditor({ source, drafts, onChange, enabledModels, onToggleModel, presentation = "disclosure" }: SourcePriceEditorProps) {
  const { t } = useTranslation();
  const { runtime } = useRelayState();
  const compactRows = presentation === "member" || presentation === "tab";
  const models = orderModelIdsBySnapshot(sourcePriceModels(source), runtime?.gateway.models ?? []);
  const catalogModels = new Map(
    (runtime?.gateway.models ?? []).map((model) => [modelIdKey(model.id), model]),
  );
  const catalog = memberModelCatalog(runtime?.gateway);
  const groups = groupModels(models, {
    metadata: (model) => catalog.get(modelIdKey(model)),
  });
  const detectedPrices = new Map(Object.entries(source.detectedModelPrices ?? {}).map(([model, price]) => [modelIdKey(model), price]));
  const catalogPrices = catalogModels;
  const modelSelectionEnabled = Boolean(enabledModels && onToggleModel);
  const enabledModelIds = new Set((enabledModels ?? []).map((model) => modelIdKey(model)));
  const enabledCount = modelSelectionEnabled
    ? models.filter((model) => enabledModelIds.has(modelIdKey(model))).length
    : models.length;
  const setField = (model: string, field: keyof SourcePriceDraft, value: string) => onChange(updateSourcePriceDraft(drafts, model, field, value));
  const reset = (model: string) => onChange(removeSourcePriceDraft(drafts, model));
  const title = t(modelSelectionEnabled ? "sources.modelsAndCost" : "sources.editorPricesTab");
  const hint = t(modelSelectionEnabled ? "sources.modelsAndCostHint" : "sources.apiCostHint");
  const manualOverrideCount = Object.keys(drafts).length;
  const count = modelSelectionEnabled
    ? `${t("common.enabled")}: ${enabledCount}/${models.length}`
    : t(manualOverrideCount ? "sources.manualPrices" : "sources.apiPricesInUse", { count: manualOverrideCount });
  const cacheWritePrices = (model: string) => {
    const key = modelIdKey(model);
    const inherited = detectedPrices.get(key) ?? catalogModels.get(key);
    return {
      fiveMinutes: inherited?.cacheWrite5mMicroUsdPerMillion,
      oneHour: inherited?.cacheWrite1hMicroUsdPerMillion,
    };
  };
  // A Messages route permits manual cache-write pricing. Explicit price data
  // stays visible even when the source's catalog uses another protocol.
  const cacheWriteModels = new Set(
    [
      ...sourceModelsWithCacheWritePricing(source),
      ...models.filter((model) => {
        const key = modelIdKey(model);
        const prices = cacheWritePrices(model);
        return prices.fiveMinutes != null || prices.oneHour != null
          || Boolean(drafts[key]?.cacheWrite5m.trim() || drafts[key]?.cacheWrite1h.trim());
      }),
    ].map((model) => modelIdKey(model)),
  );
  const content = (
    <div className="source-price-content">
      <div className="source-price-groups">
        {groups.map((group) => {
        const anthropicWrites = group.provider === "anthropic" && group.models.some((model) => cacheWriteModels.has(modelIdKey(model)));
        const openAiWrites = group.provider === "openai" && group.models.some((model) => {
          const key = modelIdKey(model);
          return cacheWritePrices(model).fiveMinutes != null || Boolean(drafts[key]?.cacheWrite5m.trim());
        });
        const cacheWriteKind = anthropicWrites ? "anthropic" : openAiWrites ? "openai" : "none";
        const groupEnabledCount = modelSelectionEnabled
          ? group.models.filter((model) => enabledModelIds.has(modelIdKey(model))).length
          : group.models.length;
        const writeHeadings = cacheWriteKind === "anthropic"
          ? <><span>{t("sources.cacheWrite5mPrice")}</span><span>{t("sources.cacheWrite1hPrice")}</span></>
          : cacheWriteKind === "openai"
            ? <span>{t("sources.cacheWrite30mPrice")}</span>
            : null;
        return (
          <details key={group.id} className="source-price-group" open={presentation === "member" || undefined}>
          <summary>
            <strong>{group.provider === "other" ? t("modelGroups.other") : group.label}</strong>
            <span>
              {modelSelectionEnabled
                ? `${t("common.enabled")}: ${groupEnabledCount}/${group.models.length}`
                : t("sources.groupModelsCount", { count: group.models.length })}
            </span>
            <ChevronDown aria-hidden />
          </summary>
          <div className={`source-price-table${compactRows ? " source-price-table-compact" : ""}`} data-cache-write={cacheWriteKind}>
            {compactRows ? (
              <div className="member-price-grid-head">
                <span>{t("common.model")}</span>
                <div>
                  <span>{t("sources.inputPrice")}</span>
                  <span>{t("sources.outputPrice")}</span>
                  <span>{t("sources.cachedInputPrice")}</span>
                  {writeHeadings}
                </div>
                <span />
              </div>
            ) : (
              <div className="source-price-grid-head">
                <span>{t("common.model")}</span>
                <span>{t("sources.inputPrice")}</span>
                <span>{t("sources.outputPrice")}</span>
                <span>{t("sources.cachedInputPrice")}</span>
                {writeHeadings}
                <span />
              </div>
            )}
            {group.models.map((model) => {
              const key = modelIdKey(model);
              const draft = drafts[key];
              const inherited = detectedPrices.get(key) ?? catalogPrices.get(key);
              const showAnthropicWrites = cacheWriteKind === "anthropic" && cacheWriteModels.has(key);
              const showOpenAiWrite = cacheWriteKind === "openai" && (cacheWritePrices(model).fiveMinutes != null || Boolean(draft?.cacheWrite5m.trim()));
              const writePrices = cacheWritePrices(model);
              const enabled = !modelSelectionEnabled || enabledModelIds.has(key);
              return (
                <div
                  className="source-price-row"
                  key={key}
                  data-custom-price={draft ? "true" : "false"}
                  data-member-model-id={modelSelectionEnabled ? model : undefined}
                  data-enabled={modelSelectionEnabled ? String(enabled) : undefined}
                >
                <div className="source-price-model" data-selectable={modelSelectionEnabled ? "true" : "false"}>
                  {modelSelectionEnabled ? (
                    <ToggleSwitch
                      className="member-model-toggle"
                      checked={enabled}
                      label={t(enabled ? "models.disable" : "models.enable", { model })}
                      onChange={() => onToggleModel?.(model)}
                    />
                  ) : null}
                  <code data-relay-tooltip={model}>{model}</code>
                </div>
                <div className="source-price-fields">
                  <PriceInput
                    label={t("sources.inputPriceFor", { model })}
                    caption={compactRows ? t("sources.inputPrice") : undefined}
                    value={draft?.input ?? ""}
                    placeholder={formatModelPricePlaceholder(inherited?.inputMicroUsdPerMillion)}
                    invalid={draft != null && parseEditableModelPrice(draft.input) == null}
                    onChange={(priceText) => setField(key, "input", priceText)}
                  />
                  <PriceInput
                    label={t("sources.outputPriceFor", { model })}
                    caption={compactRows ? t("sources.outputPrice") : undefined}
                    value={draft?.output ?? ""}
                    placeholder={formatModelPricePlaceholder(inherited?.outputMicroUsdPerMillion)}
                    invalid={draft != null && parseEditableModelPrice(draft.output) == null}
                    onChange={(priceText) => setField(key, "output", priceText)}
                  />
                  <PriceInput
                    label={t("sources.cachedInputPriceFor", { model })}
                    caption={compactRows ? t("sources.cachedInputPrice") : undefined}
                    value={draft?.cached ?? ""}
                    placeholder={formatModelPricePlaceholder(inherited?.cachedInputMicroUsdPerMillion)}
                    invalid={draft != null && draft.cached.trim() !== "" && parseEditableModelPrice(draft.cached) == null}
                    onChange={(priceText) => setField(key, "cached", priceText)}
                  />
                  {showAnthropicWrites ? <>
                    <PriceInput
                      label={t("sources.cacheWrite5mPriceFor", { model })}
                      caption={compactRows ? t("sources.cacheWrite5mPrice") : undefined}
                      value={draft?.cacheWrite5m ?? ""}
                      placeholder={formatModelPricePlaceholder(writePrices.fiveMinutes)}
                      invalid={draft != null && draft.cacheWrite5m.trim() !== "" && parseEditableModelPrice(draft.cacheWrite5m) == null}
                      onChange={(priceText) => setField(key, "cacheWrite5m", priceText)}
                    />
                    <PriceInput
                      label={t("sources.cacheWrite1hPriceFor", { model })}
                      caption={compactRows ? t("sources.cacheWrite1hPrice") : undefined}
                      value={draft?.cacheWrite1h ?? ""}
                      placeholder={formatModelPricePlaceholder(writePrices.oneHour)}
                      invalid={draft != null && draft.cacheWrite1h.trim() !== "" && parseEditableModelPrice(draft.cacheWrite1h) == null}
                      onChange={(priceText) => setField(key, "cacheWrite1h", priceText)}
                    />
                  </> : showOpenAiWrite ? (
                    <PriceInput
                      label={t("sources.cacheWrite30mPriceFor", { model })}
                      caption={compactRows ? t("sources.cacheWrite30mPrice") : undefined}
                      value={draft?.cacheWrite5m ?? ""}
                      placeholder={formatModelPricePlaceholder(writePrices.fiveMinutes)}
                      invalid={draft != null && draft.cacheWrite5m.trim() !== "" && parseEditableModelPrice(draft.cacheWrite5m) == null}
                      onChange={(priceText) => setField(key, "cacheWrite5m", priceText)}
                    />
                  )
                    : cacheWriteKind === "anthropic" ? <><span className="source-price-empty" aria-hidden /><span className="source-price-empty" aria-hidden /></>
                    : cacheWriteKind === "openai" ? <span className="source-price-empty" aria-hidden />
                    : null}
                </div>
                {draft ? <IconButton label={t("sources.useDefaultPrice", { model })} icon={<RotateCcw aria-hidden />} onClick={() => reset(key)} /> : <span className="source-price-action" />}
                </div>
              );
            })}
          </div>
          </details>
        );
      })}
      </div>
      {presentation !== "member" ? <small className="form-note">{t("sources.apiCostUnit")}</small> : null}
    </div>
  );
  const className = `source-price-section source-editor-panel${modelSelectionEnabled ? " source-model-configuration" : ""}`;
  if (presentation === "member") {
    return (
      <section className="source-price-section member-price-section">
        <header className="member-model-heading">
          <strong>{t("sources.apiCostUnit")}</strong>
          <span>{count}</span>
        </header>
        {content}
      </section>
    );
  }
  if (presentation === "tab") {
    return (
      <section className={`${className} source-price-tab member-price-section`}>
        <header className="source-price-tab-heading">
          <span><strong>{title}</strong><small>{hint}</small></span>
          <small className={`source-price-tab-status ${manualOverrideCount ? "source-price-tab-status-manual" : "source-price-tab-status-api"}`}>{count}</small>
        </header>
        {content}
      </section>
    );
  }
  return (
    <details className={className}>
    <summary className="source-editor-panel-summary">
      <span><strong>{title}</strong><small>{hint}</small></span>
      <small>{count}</small>
    </summary>
    {content}
    </details>
  );
}

function PriceInput({
  label,
  caption,
  value,
  placeholder,
  invalid,
  onChange,
}: {
  label: string;
  caption: string | undefined;
  value: string;
  placeholder: string;
  invalid: boolean;
  onChange: (priceText: string) => void;
}) {
  return (
    <label className="source-price-field">
    {caption ? <span className="source-price-caption">{caption}</span> : null}
    <span className="source-price-input">
      <span className="source-price-currency" aria-hidden>$</span>
      <input
        aria-label={label}
        aria-invalid={invalid || undefined}
        type="text"
        inputMode="decimal"
        autoComplete="off"
        spellCheck={false}
        value={value}
        placeholder={placeholder}
        onChange={(event) => onChange(event.target.value)}
      />
    </span>
    </label>
  );
}

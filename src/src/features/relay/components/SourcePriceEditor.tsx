import { ChevronDown, RotateCcw } from "lucide-react";
import { useState, type CSSProperties } from "react";
import { useTranslation } from "react-i18next";
import type { SourceSummary } from "../api/types";
import { groupModels, memberModelCatalog, modelIdKey, orderModelIdsBySnapshot } from "../modelGroups";
import {
  formatModelPricePlaceholder,
  parseEditableModelPrice,
} from "../modelPricing";
import { sourceModelsWithCacheWritePricing } from "../sourceProtocolBindings";
import { useRelayState } from "../state/RelayStateProvider";
import { IconButton, OptionMenu, ToggleSwitch } from "./Ui";
import {
  removeSourcePriceDraft,
  sourcePriceModels,
  sourcePriceQuote,
  sourcePriceOverride,
  sourcePriceCacheWrites,
  priceScheduleKey,
  publishedPriceBands,
  publishedPriceTiers,
  priceContextRanges,
  selectedPriceRates,
  type PriceBand,
  type PriceSchedule,
  type SourcePriceDraftField,
  type SourcePriceDrafts,
  updateSourcePriceScheduleDraft,
} from "./sourcePriceEditorModel";

type SourcePriceEditorProps = {
  source: SourceSummary;
  drafts: SourcePriceDrafts;
  onChange: (value: SourcePriceDrafts) => void;
  enabledModels?: readonly string[];
  onToggleModel?: (model: string) => void;
  presentation?: "disclosure" | "tab" | "member";
};

const cacheWriteColumns = [
  { kind: "generic", field: "cacheWrite", label: "sources.cacheWritePrice", labelFor: "sources.cacheWritePriceFor" },
  { kind: "fiveMinutes", field: "cacheWrite5m", label: "sources.cacheWrite5mPrice", labelFor: "sources.cacheWrite5mPriceFor" },
  { kind: "oneHour", field: "cacheWrite1h", label: "sources.cacheWrite1hPrice", labelFor: "sources.cacheWrite1hPriceFor" },
] as const;

export function SourcePriceEditor({ source, drafts, onChange, enabledModels, onToggleModel, presentation = "disclosure" }: SourcePriceEditorProps) {
  const { t } = useTranslation();
  const { runtime } = useRelayState();
  const [selections, setSelections] = useState<Record<string, { band: PriceBand; tier: PriceSchedule }>>({});
  const compactRows = presentation === "member" || presentation === "tab";
  const models = orderModelIdsBySnapshot(sourcePriceModels(source), runtime?.gateway.models ?? [], runtime?.gateway.modelCatalog);
  const catalog = memberModelCatalog(runtime?.gateway);
  const groups = groupModels(models, {
    metadata: (model) => catalog.get(modelIdKey(model)),
  });
  const detectedPrices = new Map(Object.entries(source.detectedModelPrices ?? {}).map(([model, price]) => [modelIdKey(model), price]));
  const resolvedPrices = new Map(Object.entries(source.resolvedModelPrices ?? {}).map(([model, price]) => [modelIdKey(model), price]));
  const messagesCacheWriteModels = new Set(sourceModelsWithCacheWritePricing(source).map(modelIdKey));
  const modelSelectionEnabled = Boolean(enabledModels && onToggleModel);
  const enabledModelIds = new Set((enabledModels ?? []).map((model) => modelIdKey(model)));
  const enabledCount = modelSelectionEnabled
    ? models.filter((model) => enabledModelIds.has(modelIdKey(model))).length
    : models.length;
  const reset = (model: string) => onChange(removeSourcePriceDraft(drafts, model));
  const title = t(modelSelectionEnabled ? "sources.modelsAndCost" : "sources.editorPricesTab");
  const manualOverrideCount = Object.keys(drafts).length;
  const count = modelSelectionEnabled
    ? `${t("common.enabled")}: ${enabledCount}/${models.length}`
    : t(manualOverrideCount ? "sources.manualPrices" : "sources.automaticPrices", { count: manualOverrideCount });
  const cacheWritePrices = (model: string) => {
    const key = modelIdKey(model);
    const writes = sourcePriceCacheWrites(sourcePriceQuote(detectedPrices.get(key), resolvedPrices.get(key)), drafts[key]);
    const manualWindows = messagesCacheWriteModels.has(key) && !writes.generic;
    return { ...writes, fiveMinutes: writes.fiveMinutes || manualWindows, oneHour: writes.oneHour || manualWindows };
  };
  const content = (
    <div className="source-price-content">
      {presentation !== "member" ? <small className="form-note">{t("sources.apiCostUnit")}</small> : null}
      <div className="source-price-groups">
        {groups.map((group) => {
        const writeColumns = cacheWriteColumns.filter((column) => group.models.some((model) => cacheWritePrices(model)[column.kind]));
        const groupEnabledCount = modelSelectionEnabled
          ? group.models.filter((model) => enabledModelIds.has(modelIdKey(model))).length
          : group.models.length;
        const writeHeadings = writeColumns.map((column) => <span key={column.field}>{t(column.label)}</span>);
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
          <div className={`source-price-table${compactRows ? " source-price-table-compact" : ""}`}
            style={{ "--source-price-columns": 3 + writeColumns.length, "--member-price-columns": 3 + writeColumns.length } as CSSProperties}>
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
              const savedDraft = drafts[key];
              const resolved = resolvedPrices.get(key);
              const quote = sourcePriceQuote(detectedPrices.get(key), resolved);
              const priceSource = resolved?.source ?? (detectedPrices.has(key) ? "provider" : "unpriced");
              const referencePrice = priceSource === "liteLlmExact" || priceSource === "liteLlmCanonical";
              const originKey = priceSource === "provider" ? "sources.priceSourceProvider"
                : priceSource === "liteLlmExact" ? "sources.priceSourceReference"
                : priceSource === "liteLlmCanonical" ? "sources.priceSourceCanonical"
                : priceSource === "manual" ? "sources.priceSourceManual" : "sources.priceSourceUnknown";
              const origin = [
                t(originKey),
                referencePrice && resolved?.stale ? t("sources.priceReferenceStale") : null,
                savedDraft && priceSource !== "manual" ? t("sources.priceManualFallback") : null,
              ].filter(Boolean).join(" · ");
              const tiers = publishedPriceTiers(quote, savedDraft);
              const previous = selections[key] ?? { band: "base", tier: "standard" };
              const tier = tiers.includes(previous.tier) ? previous.tier : "standard";
              const bands = publishedPriceBands(quote, savedDraft, tier);
              const selection = { tier, band: bands.includes(previous.band) ? previous.band : "base" as const };
              const draft = selection.band === "base" && selection.tier === "standard"
                ? savedDraft : savedDraft?.schedules?.[priceScheduleKey(selection.band, selection.tier)];
              const rates = selectedPriceRates(quote, selection.band, selection.tier);
              const setField = (_model: string, field: SourcePriceDraftField, text: string) =>
                onChange(updateSourcePriceScheduleDraft(drafts, key, selection.band, selection.tier, field, text, sourcePriceOverride(quote)));
              const select = (band: PriceBand, tier = selection.tier) =>
                setSelections((previous) => ({ ...previous, [key]: { band, tier } }));
              const contextOptions = priceContextRanges(bands).map(({ band, min, max }) => ({
                value: band,
                label: min == null ? t("sources.shortContextPrice", { limit: `${(max ?? 0) / 1000}k` })
                  : max == null ? t("sources.longContextPrice", { limit: `${min / 1000}k` })
                  : `> ${min / 1000}k · ≤ ${max / 1000}k`,
                shortLabel: min == null ? `≤ ${(max ?? 0) / 1000}k`
                  : max == null ? `> ${min / 1000}k` : `${min / 1000}–${max / 1000}k`,
              }));
              const requiredBase = selection.band === "base" && selection.tier === "standard";
              const writes = cacheWritePrices(model);
              const enabled = !modelSelectionEnabled || enabledModelIds.has(key);
              return (
                <div
                  className="source-price-row"
                  key={key}
                  data-custom-price={savedDraft ? "true" : "false"}
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
                  <code tabIndex={0} data-price-source={priceSource} data-relay-tooltip={`${model}\n${origin}`}>{model}</code>
                  {tiers.length > 1 || bands.length > 1 ? <div className="source-price-schedule">
                    {tiers.length > 1 ? <div className="source-price-picker">
                      <OptionMenu className="source-price-tier" label={t("sources.priceTierFor", { model })}
                        value={selection.tier} align="start" fitContent
                        options={tiers.map((value) => ({ value, label: { standard: "Standard", flex: "Flex", priority: "Priority", ultrafast: "Ultrafast" }[value] }))}
                        onChange={(value) => select(selection.band, value as PriceSchedule)} />
                    </div> : null}
                    {bands.length > 1 ? <div className="source-price-picker">
                      <OptionMenu className="source-price-context" label={t("sources.priceContextFor", { model })}
                        value={selection.band} align="start" fitContent options={contextOptions}
                        onChange={(value) => select(value as PriceBand)} />
                    </div> : null}
                  </div> : null}
                </div>
                <div className="source-price-fields">
                  <PriceInput
                    label={t("sources.inputPriceFor", { model })}
                    caption={compactRows ? t("sources.inputPrice") : undefined}
                    value={draft?.input ?? ""}
                    placeholder={formatModelPricePlaceholder(rates.input)}
                    invalid={draft != null && (requiredBase || draft.input.trim() !== "") && parseEditableModelPrice(draft.input) == null}
                    onChange={(priceText) => setField(key, "input", priceText)}
                  />
                  <PriceInput
                    label={t("sources.outputPriceFor", { model })}
                    caption={compactRows ? t("sources.outputPrice") : undefined}
                    value={draft?.output ?? ""}
                    placeholder={formatModelPricePlaceholder(rates.output)}
                    invalid={draft != null && (requiredBase || draft.output.trim() !== "") && parseEditableModelPrice(draft.output) == null}
                    onChange={(priceText) => setField(key, "output", priceText)}
                  />
                  <PriceInput
                    label={t("sources.cachedInputPriceFor", { model })}
                    caption={compactRows ? t("sources.cachedInputPrice") : undefined}
                    value={draft?.cached ?? ""}
                    placeholder={formatModelPricePlaceholder(rates.cacheRead)}
                    invalid={draft != null && draft.cached.trim() !== "" && parseEditableModelPrice(draft.cached) == null}
                    onChange={(priceText) => setField(key, "cached", priceText)}
                  />
                  {writeColumns.map((column) => {
                    if (!writes[column.kind]) return <span key={column.field} className="source-price-empty" aria-hidden />;
                    const value = draft?.[column.field] ?? "";
                    return <PriceInput key={column.field}
                      label={t(column.labelFor, { model })}
                      caption={compactRows ? t(column.label) : undefined}
                      value={value}
                      placeholder={formatModelPricePlaceholder(rates[column.field])}
                      invalid={value.trim() !== "" && parseEditableModelPrice(value) == null}
                      onChange={(priceText) => setField(key, column.field, priceText)}
                    />;
                  })}
                </div>
                {savedDraft ? <IconButton label={t("sources.useDefaultPrice", { model })} icon={<RotateCcw aria-hidden />} onClick={() => reset(key)} /> : <span className="source-price-action" />}
                </div>
              );
            })}
          </div>
          </details>
        );
      })}
      </div>
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
          <span><strong>{title}</strong></span>
          <small className={`source-price-tab-status ${manualOverrideCount ? "source-price-tab-status-manual" : "source-price-tab-status-api"}`}>{count}</small>
        </header>
        {content}
      </section>
    );
  }
  return (
    <details className={className}>
    <summary className="source-editor-panel-summary">
      <span><strong>{title}</strong></span>
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

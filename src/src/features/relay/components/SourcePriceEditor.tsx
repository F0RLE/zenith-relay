import { Power, RotateCcw } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { ApiModelPriceOverride, SourceSummary } from "../api/types";
import { groupModels, supportsCacheWritePricing } from "../modelGroups";
import {
  formatEditableModelPrice,
  formatModelPricePlaceholder,
  parseEditableModelPrice,
} from "../modelPricing";
import { useRelayState } from "../state/RelayStateProvider";
import { IconButton } from "./Ui";

export type SourcePriceDraft = {
  input: string;
  output: string;
  cached: string;
  cacheWrite5m: string;
  cacheWrite1h: string;
};
export type SourcePriceDrafts = Record<string, SourcePriceDraft>;

const emptyDraft = (): SourcePriceDraft => ({ input: "", output: "", cached: "", cacheWrite5m: "", cacheWrite1h: "" });

type SourcePriceEditorProps = {
  source: SourceSummary;
  drafts: SourcePriceDrafts;
  onChange: (value: SourcePriceDrafts) => void;
  enabledModels?: readonly string[];
  onToggleModel?: (model: string) => void;
};

export function SourcePriceEditor({ source, drafts, onChange, enabledModels, onToggleModel }: SourcePriceEditorProps) {
  const { t } = useTranslation();
  const { runtime } = useRelayState();
  const models = [...new Map([
    ...source.models,
    ...source.allowedModels,
    ...source.excludedModels,
    ...Object.keys(source.modelPriceOverrides ?? {}),
    ...Object.keys(source.detectedModelPrices ?? {}),
  ].map((model) => [model.toLowerCase(), model])).values()];
  const groups = groupModels(models, (model) => model);
  const detectedPrices = new Map(Object.entries(source.detectedModelPrices ?? {}).map(([model, price]) => [model.toLowerCase(), price]));
  const catalogPrices = new Map((runtime?.gateway.models ?? []).map((model) => [model.id.toLowerCase(), model]));
  const modelSelectionEnabled = Boolean(enabledModels && onToggleModel);
  const enabledModelIds = new Set((enabledModels ?? []).map((model) => model.toLowerCase()));
  const enabledCount = modelSelectionEnabled
    ? models.filter((model) => enabledModelIds.has(model.toLowerCase())).length
    : models.length;
  const setField = (model: string, field: keyof SourcePriceDraft, value: string) => onChange({
    ...drafts,
    [model]: { ...(drafts[model] ?? emptyDraft()), [field]: value },
  });
  const reset = (model: string) => {
    const next = { ...drafts };
    delete next[model];
    onChange(next);
  };
  return <details className={`source-price-section source-editor-panel${modelSelectionEnabled ? " source-model-configuration" : ""}`}>
    <summary className="source-editor-panel-summary"><span><strong>{t(modelSelectionEnabled ? "sources.modelsAndCost" : "sources.apiCost")}</strong><small>{t(modelSelectionEnabled ? "sources.modelsAndCostHint" : "sources.apiCostHint")}</small></span><small>{modelSelectionEnabled ? `${t("common.enabled")}: ${enabledCount}/${models.length}` : t("sources.groupModelsCount", { count: models.length })}</small></summary>
    <div className="source-price-content"><div className="source-price-groups">
      {groups.map((group) => {
        const cacheWrite = group.id === "anthropic";
        const groupEnabledCount = modelSelectionEnabled
          ? group.items.filter((model) => enabledModelIds.has(model.toLowerCase())).length
          : group.items.length;
        return <details key={group.id} className="source-price-group">
          <summary><strong>{t(`modelGroups.${group.id}`, { defaultValue: group.label })}</strong><span>{modelSelectionEnabled ? `${t("common.enabled")}: ${groupEnabledCount}/${group.items.length}` : t("sources.groupModelsCount", { count: group.items.length })}</span></summary>
          <div className="source-price-table" data-cache-write={cacheWrite ? "true" : "false"}>
            <div className="source-price-grid-head"><span>{t("common.model")}</span><span>{t("sources.inputPrice")}</span><span>{t("sources.outputPrice")}</span><span>{t("sources.cachedInputPrice")}</span>{cacheWrite ? <><span>{t("sources.cacheWrite5mPrice")}</span><span>{t("sources.cacheWrite1hPrice")}</span></> : null}<span /></div>
            {group.items.map((model) => {
              const key = model.toLowerCase();
              const draft = drafts[key];
              const inherited = detectedPrices.get(key) ?? catalogPrices.get(key);
              const showWrites = supportsCacheWritePricing(model);
              const enabled = !modelSelectionEnabled || enabledModelIds.has(key);
              return <div className="source-price-row" key={key} data-custom-price={draft ? "true" : "false"} data-member-model-id={modelSelectionEnabled ? model : undefined} data-enabled={modelSelectionEnabled ? String(enabled) : undefined}>
                <div className="source-price-model" data-selectable={modelSelectionEnabled ? "true" : "false"}>
                  {modelSelectionEnabled ? <IconButton className="member-model-toggle" aria-pressed={enabled} label={t(enabled ? "models.disable" : "models.enable", { model })} icon={<Power aria-hidden />} onClick={() => onToggleModel?.(model)} /> : null}
                  <code title={model}>{model}</code>
                </div>
                <PriceInput label={t("sources.inputPriceFor", { model })} value={draft?.input ?? ""} placeholder={formatModelPricePlaceholder(inherited?.inputMicroUsdPerMillion)} invalid={Boolean(draft) && parseEditableModelPrice(draft.input) == null} onChange={(value) => setField(key, "input", value)} />
                <PriceInput label={t("sources.outputPriceFor", { model })} value={draft?.output ?? ""} placeholder={formatModelPricePlaceholder(inherited?.outputMicroUsdPerMillion)} invalid={Boolean(draft) && parseEditableModelPrice(draft.output) == null} onChange={(value) => setField(key, "output", value)} />
                <PriceInput label={t("sources.cachedInputPriceFor", { model })} value={draft?.cached ?? ""} placeholder={formatModelPricePlaceholder(inherited?.cachedInputMicroUsdPerMillion ?? inherited?.inputMicroUsdPerMillion)} invalid={Boolean(draft) && draft.cached.trim() !== "" && parseEditableModelPrice(draft.cached) == null} onChange={(value) => setField(key, "cached", value)} />
                {showWrites ? <>
                  <PriceInput label={t("sources.cacheWrite5mPriceFor", { model })} value={draft?.cacheWrite5m ?? ""} placeholder={formatModelPricePlaceholder(inherited?.cacheWrite5mMicroUsdPerMillion)} invalid={Boolean(draft) && draft.cacheWrite5m.trim() !== "" && parseEditableModelPrice(draft.cacheWrite5m) == null} onChange={(value) => setField(key, "cacheWrite5m", value)} />
                  <PriceInput label={t("sources.cacheWrite1hPriceFor", { model })} value={draft?.cacheWrite1h ?? ""} placeholder={formatModelPricePlaceholder(inherited?.cacheWrite1hMicroUsdPerMillion)} invalid={Boolean(draft) && draft.cacheWrite1h.trim() !== "" && parseEditableModelPrice(draft.cacheWrite1h) == null} onChange={(value) => setField(key, "cacheWrite1h", value)} />
                </> : null}
                {draft ? <IconButton label={t("sources.useDefaultPrice", { model })} icon={<RotateCcw aria-hidden />} onClick={() => reset(key)} /> : <span className="source-price-action" />}
              </div>;
            })}
          </div>
        </details>;
      })}
    </div>
    <small className="form-note">{t("sources.apiCostUnit")}</small></div>
  </details>;
}

function PriceInput({ label, value, placeholder, invalid, onChange }: { label: string; value: string; placeholder: string; invalid: boolean; onChange: (value: string) => void }) {
  return <label className="source-price-input"><span className="sr-only">{label}</span><span aria-hidden>$</span><input aria-label={label} aria-invalid={invalid || undefined} type="text" inputMode="decimal" autoComplete="off" spellCheck={false} value={value} placeholder={placeholder} onChange={(event) => onChange(event.target.value)} /></label>;
}

export function sourcePriceDrafts(prices: Record<string, ApiModelPriceOverride>): SourcePriceDrafts {
  return Object.fromEntries(Object.entries(prices).map(([model, price]) => [model.toLowerCase(), {
    input: formatEditableModelPrice(price.inputMicroUsdPerMillion),
    output: formatEditableModelPrice(price.outputMicroUsdPerMillion),
    cached: formatEditableModelPrice(price.cachedInputMicroUsdPerMillion),
    cacheWrite5m: formatEditableModelPrice(price.cacheWrite5mMicroUsdPerMillion),
    cacheWrite1h: formatEditableModelPrice(price.cacheWrite1hMicroUsdPerMillion),
  }]));
}

export function parseSourcePriceDrafts(drafts: SourcePriceDrafts): Record<string, ApiModelPriceOverride> | null {
  const prices: Record<string, ApiModelPriceOverride> = {};
  for (const [model, draft] of Object.entries(drafts)) {
    const input = parseEditableModelPrice(draft.input);
    const output = parseEditableModelPrice(draft.output);
    const cached = optionalSourcePrice(draft.cached);
    const cacheWrite5m = optionalSourcePrice(draft.cacheWrite5m);
    const cacheWrite1h = optionalSourcePrice(draft.cacheWrite1h);
    if (input == null || output == null || cached === null || cacheWrite5m === null || cacheWrite1h === null) return null;
    prices[model] = {
      inputMicroUsdPerMillion: input,
      outputMicroUsdPerMillion: output,
      ...(cached == null ? {} : { cachedInputMicroUsdPerMillion: cached }),
      ...(cacheWrite5m == null ? {} : { cacheWrite5mMicroUsdPerMillion: cacheWrite5m }),
      ...(cacheWrite1h == null ? {} : { cacheWrite1hMicroUsdPerMillion: cacheWrite1h }),
    };
  }
  return prices;
}

function optionalSourcePrice(value: string) { return value.trim() === "" ? undefined : parseEditableModelPrice(value); }

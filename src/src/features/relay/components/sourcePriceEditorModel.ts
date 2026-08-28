import type { ApiModelPriceOverride, SourceSummary } from "../api/types";
import { formatEditableModelPrice, parseEditableModelPrice } from "../modelPricing";

export type SourcePriceDraft = {
  input: string;
  output: string;
  cached: string;
  cacheWrite5m: string;
  cacheWrite1h: string;
};

export type SourcePriceDrafts = Record<string, SourcePriceDraft>;
export type SourcePriceDraftField = keyof SourcePriceDraft;

const emptyDraft = (): SourcePriceDraft => ({ input: "", output: "", cached: "", cacheWrite5m: "", cacheWrite1h: "" });

export function sourcePriceModels(source: Pick<SourceSummary, "modelPriceOverrides" | "detectedModelPrices" | "allowedModels" | "excludedModels" | "models">) {
  return [...new Map([
    ...Object.keys(source.modelPriceOverrides ?? {}),
    ...Object.keys(source.detectedModelPrices ?? {}),
    ...source.allowedModels,
    ...source.excludedModels,
    ...source.models,
  ].map((model) => [model.toLowerCase(), model])).values()];
}

export function updateSourcePriceDraft(
  drafts: SourcePriceDrafts,
  model: string,
  field: SourcePriceDraftField,
  value: string,
): SourcePriceDrafts {
  const key = model.toLowerCase();
  return {
    ...drafts,
    [key]: { ...(drafts[key] ?? emptyDraft()), [field]: value },
  };
}

export function removeSourcePriceDraft(drafts: SourcePriceDrafts, model: string): SourcePriceDrafts {
  const next = { ...drafts };
  delete next[model.toLowerCase()];
  return next;
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

function optionalSourcePrice(value: string) {
  return value.trim() === "" ? undefined : parseEditableModelPrice(value);
}

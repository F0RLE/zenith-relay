import type { ApiModelPriceOverride, ModelSummary, SourceSummary } from "../api/types";
import { PRICE_CONTEXT_THRESHOLDS, type LongContextRates, type PriceContextBand, type TokenPrice, type TokenRateSet } from "../api/types/pricing";
import { formatEditableModelPrice, parseEditableModelPrice } from "../modelPricing";
import { modelIdKey, uniqueModelIds } from "../modelGroups";

export type SourceRateDraft = {
  input: string;
  output: string;
  cached: string;
  cacheWrite5m: string;
  cacheWrite1h: string;
};
export type PriceSchedule = "standard" | "flex" | "priority" | "ultrafast";
export type PriceBand = "base" | PriceContextBand;
export type SourcePriceDraft = SourceRateDraft & {
  schedules?: Partial<Record<string, SourceRateDraft>>;
};

export type SourcePriceDrafts = Record<string, SourcePriceDraft>;
export type SourcePriceDraftField = keyof SourceRateDraft;

const emptyDraft = (): SourcePriceDraft => ({ input: "", output: "", cached: "", cacheWrite5m: "", cacheWrite1h: "" });
const contextBands = Object.keys(PRICE_CONTEXT_THRESHOLDS) as PriceContextBand[];
const hasRates = (rates: TokenRateSet | undefined) => rates != null && Object.values(rates).some((value) => value != null);

export function sourcePriceQuote(detected: ApiModelPriceOverride | undefined, model: ModelSummary | undefined): TokenPrice | undefined {
  if (detected) {
    const {
      inputMicroUsdPerMillion: input, outputMicroUsdPerMillion: output,
      cachedInputMicroUsdPerMillion: cacheRead, cacheWrite5mMicroUsdPerMillion: cacheWrite5m,
      cacheWrite1hMicroUsdPerMillion: cacheWrite1h, ...schedules
    } = detected;
    return { input, output, cacheRead: cacheRead ?? undefined, cacheWrite5m: cacheWrite5m ?? undefined, cacheWrite1h: cacheWrite1h ?? undefined, ...schedules };
  }
  return model?.tokenPrice ?? (model?.inputMicroUsdPerMillion != null && model.outputMicroUsdPerMillion != null ? {
    input: model.inputMicroUsdPerMillion, output: model.outputMicroUsdPerMillion,
    cacheRead: model.cachedInputMicroUsdPerMillion ?? undefined,
    cacheWrite5m: model.cacheWrite5mMicroUsdPerMillion ?? undefined,
    cacheWrite1h: model.cacheWrite1hMicroUsdPerMillion ?? undefined,
  } : undefined);
}

export function sourcePriceOverride(quote: TokenPrice | undefined): ApiModelPriceOverride | undefined {
  if (!quote) return undefined;
  const { input, output, cacheRead, cacheWrite5m, cacheWrite1h, ...schedules } = quote;
  return {
    inputMicroUsdPerMillion: input, outputMicroUsdPerMillion: output,
    cachedInputMicroUsdPerMillion: cacheRead, cacheWrite5mMicroUsdPerMillion: cacheWrite5m,
    cacheWrite1hMicroUsdPerMillion: cacheWrite1h, ...schedules,
  };
}

/** A flat or unknown quote must not invent long-context tariffs. */
export function publishedPriceBands(price: TokenPrice | undefined, draft?: SourcePriceDraft): PriceBand[] {
  return ["base", ...contextBands.filter((band) =>
    Object.values(price?.[band] ?? {}).some(hasRates)
    || Object.entries(draft?.schedules ?? {}).some(([key, rates]) =>
      key.startsWith(`${band}.`) && rates && Object.values(rates).some((value) => value.trim() !== "")))];
}

export function priceContextRanges(bands: readonly PriceBand[]) {
  return bands.map((band, index) => {
    const next = bands[index + 1];
    return {
      band,
      min: band === "base" ? undefined : PRICE_CONTEXT_THRESHOLDS[band],
      max: next && next !== "base" ? PRICE_CONTEXT_THRESHOLDS[next] : undefined,
    };
  });
}

export function sourcePriceCacheWrites(price: TokenPrice | undefined, draft?: SourcePriceDraft) {
  const rates = [price, price?.flex, price?.priority, price?.ultrafast, ...contextBands.flatMap((band) => Object.values(price?.[band] ?? {}))];
  const drafts = [draft, ...Object.values(draft?.schedules ?? {})];
  return {
    fiveMinutes: rates.some((rate) => rate?.cacheWrite5m != null) || drafts.some((rate) => Boolean(rate?.cacheWrite5m.trim())),
    oneHour: rates.some((rate) => rate?.cacheWrite1h != null) || drafts.some((rate) => Boolean(rate?.cacheWrite1h.trim())),
  };
}

export function sourcePriceModels(source: Pick<SourceSummary, "modelPriceOverrides" | "detectedModelPrices" | "allowedModels" | "excludedModels" | "models">) {
  // Rust projects the complete inventory in catalog/manual order. Filters and
  // prices must not move disabled models or replace their original spelling.
  return uniqueModelIds([
    ...source.models,
    ...source.allowedModels,
    ...source.excludedModels,
    ...Object.keys(source.modelPriceOverrides ?? {}),
    ...Object.keys(source.detectedModelPrices ?? {}),
  ]);
}

export function updateSourcePriceDraft(
  drafts: SourcePriceDrafts,
  model: string,
  field: SourcePriceDraftField,
  priceText: string,
): SourcePriceDrafts {
  const key = modelIdKey(model);
  return {
    ...drafts,
    [key]: { ...(drafts[key] ?? emptyDraft()), [field]: priceText },
  };
}

export function removeSourcePriceDraft(drafts: SourcePriceDrafts, model: string): SourcePriceDrafts {
  const updatedDrafts = { ...drafts };
  delete updatedDrafts[modelIdKey(model)];
  return updatedDrafts;
}

export function sourcePriceDrafts(prices: Record<string, ApiModelPriceOverride>): SourcePriceDrafts {
  return Object.fromEntries(Object.entries(prices).map(([model, price]) => [modelIdKey(model), {
    input: formatEditableModelPrice(price.inputMicroUsdPerMillion),
    output: formatEditableModelPrice(price.outputMicroUsdPerMillion),
    cached: formatEditableModelPrice(price.cachedInputMicroUsdPerMillion),
    cacheWrite5m: formatEditableModelPrice(price.cacheWrite5mMicroUsdPerMillion),
    cacheWrite1h: formatEditableModelPrice(price.cacheWrite1hMicroUsdPerMillion),
    schedules: scheduleDrafts(price),
  }]));
}

export function parseSourcePriceDrafts(drafts: SourcePriceDrafts): Record<string, ApiModelPriceOverride> | null {
  const prices: Record<string, ApiModelPriceOverride> = {};
  for (const [model, draft] of Object.entries(drafts)) {
    const inputPrice = parseEditableModelPrice(draft.input);
    const outputPrice = parseEditableModelPrice(draft.output);
    const cached = optionalSourcePrice(draft.cached);
    const cacheWrite5m = optionalSourcePrice(draft.cacheWrite5m);
    const cacheWrite1h = optionalSourcePrice(draft.cacheWrite1h);
    if (inputPrice == null || outputPrice == null || cached === null || cacheWrite5m === null || cacheWrite1h === null) return null;
    prices[model] = {
      inputMicroUsdPerMillion: inputPrice,
      outputMicroUsdPerMillion: outputPrice,
      ...(cached == null ? {} : { cachedInputMicroUsdPerMillion: cached }),
      ...(cacheWrite5m == null ? {} : { cacheWrite5mMicroUsdPerMillion: cacheWrite5m }),
      ...(cacheWrite1h == null ? {} : { cacheWrite1hMicroUsdPerMillion: cacheWrite1h }),
    };
    for (const [key, schedule] of Object.entries(draft.schedules ?? {})) {
      if (!schedule) continue;
      const rates = parseRateDraft(schedule);
      if (rates === null) return null;
      if (!Object.keys(rates).length) continue;
      if (key === "flex" || key === "priority" || key === "ultrafast") prices[model][key] = rates;
      else {
        const [band, tier] = key.split(".");
        if (!isContextBand(band)
          || (tier !== "standard" && tier !== "flex" && tier !== "priority" && tier !== "ultrafast")) return null;
        const bands: LongContextRates = prices[model][band] ?? {};
        bands[tier] = rates;
        prices[model][band] = bands;
      }
    }
  }
  return prices;
}

export function priceScheduleKey(band: PriceBand, tier: PriceSchedule) {
  return band === "base" ? tier : `${band}.${tier}`;
}

export function rateDraft(rates: TokenRateSet): SourceRateDraft {
  return {
    input: formatEditableModelPrice(rates.input),
    output: formatEditableModelPrice(rates.output),
    cached: formatEditableModelPrice(rates.cacheRead),
    cacheWrite5m: formatEditableModelPrice(rates.cacheWrite5m),
    cacheWrite1h: formatEditableModelPrice(rates.cacheWrite1h),
  };
}

function scheduleDrafts(price: ApiModelPriceOverride) {
  const schedules: Record<string, SourceRateDraft> = {};
  for (const tier of ["flex", "priority", "ultrafast"] as const) {
    if (price[tier]) schedules[tier] = rateDraft(price[tier]);
  }
  for (const band of contextBands) {
    for (const tier of ["standard", "flex", "priority", "ultrafast"] as const) {
      if (price[band]?.[tier]) schedules[`${band}.${tier}`] = rateDraft(price[band][tier]);
    }
  }
  return schedules;
}

export function selectedPriceRates(price: TokenPrice | undefined, band: PriceBand, tier: PriceSchedule): TokenRateSet {
  if (!price) return {};
  const { input, output, cacheRead, cacheWrite5m, cacheWrite1h } = price;
  const base = tier === "standard" ? { input, output, cacheRead, cacheWrite5m, cacheWrite1h } : price[tier] ?? {};
  const above = band === "base" ? undefined : contextBands
    .filter((candidate) => PRICE_CONTEXT_THRESHOLDS[candidate] <= PRICE_CONTEXT_THRESHOLDS[band])
    .reverse().map((candidate) => price[candidate]?.[tier]).find(hasRates);
  return { ...base, ...above };
}

export function updateSourcePriceScheduleDraft(
  drafts: SourcePriceDrafts, model: string, band: PriceBand, tier: PriceSchedule,
  field: SourcePriceDraftField, text: string, inherited?: ApiModelPriceOverride,
): SourcePriceDrafts {
  const key = modelIdKey(model);
  const draft = drafts[key] ?? (inherited ? sourcePriceDrafts({ [key]: inherited })[key] : undefined) ?? emptyDraft();
  if (band === "base" && tier === "standard") {
    return updateSourcePriceDraft({ ...drafts, [key]: draft }, model, field, text);
  }
  const schedule = priceScheduleKey(band, tier);
  return {
    ...drafts,
    [key]: { ...draft, schedules: { ...draft.schedules, [schedule]: {
      ...(draft.schedules?.[schedule] ?? emptyDraft()), [field]: text,
    } } },
  };
}

function parseRateDraft(draft: SourceRateDraft): TokenRateSet | null {
  const rates: TokenRateSet = {};
  const fields = { input: "input", output: "output", cached: "cacheRead", cacheWrite5m: "cacheWrite5m", cacheWrite1h: "cacheWrite1h" } as const;
  for (const field of Object.keys(fields) as SourcePriceDraftField[]) {
    const value = optionalSourcePrice(draft[field]);
    if (value === null) return null;
    if (value !== undefined) rates[fields[field]] = value;
  }
  return rates;
}

function optionalSourcePrice(priceText: string) {
  return priceText.trim() === "" ? undefined : parseEditableModelPrice(priceText);
}

function isContextBand(band: string | undefined): band is PriceContextBand {
  return band != null && Object.hasOwn(PRICE_CONTEXT_THRESHOLDS, band);
}

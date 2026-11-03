import type { Locale } from "../../i18n/catalog";
import { formatMoney } from "../usage/model";
import type { TokenRateSet } from "../../../../src/src/features/relay/api/types/pricing";
import { PRICE_CONTEXT_THRESHOLDS } from "../../../../src/src/features/relay/api/types/pricing";
import { priceContextRanges } from "../../../../src/src/features/relay/components/sourcePriceEditorModel";
import type { PriceBand, PriceSchedule } from "../../../../src/src/features/relay/components/sourcePriceEditorModel";

/** USD micro-units per one million tokens. Missing is not free. */
export type ModelRateComponent = keyof TokenRateSet;
export type ModelRates = Partial<Record<ModelRateComponent, bigint | null>>;

/** The server qualifies matching models, context bands, tiers and cache rules. */
export interface ModelPriceComparison {
  officialRates: ModelRates;
  /** Supplied by the server, never calculated or rounded up by this UI. */
  savingsPercent?: Partial<Record<ModelRateComponent, number | null>>;
}

export interface ModelPricing {
  rates: ModelRates;
  comparison?: ModelPriceComparison;
}

/** Resolved customer offers, not raw provider quotes or proof of route support.
 * The host resolves core overlays and qualifies retail comparisons before publishing.
 * Tier identities and input-token bands reuse the Desktop/core pricing contract. */
export interface ModelPricingVariant extends ModelPricing {
  id: string;
  tier: PriceSchedule;
  contextBand: PriceBand;
}

export interface CustomerModel {
  id: string;
  name: string;
  protocols: string[];
  /** Already localized by the API; preview copy uses dictionary keys. */
  description: string;
  rates: ModelRates;
  revision: string;
  publisher?: string;
  /** Stable company identity from Relay catalog metadata, never the account/source kind. */
  catalogProvider?: string | null;
  contextTokens?: number | null;
  /** The API chooses the featured models and their incoming display order. */
  featured?: boolean;
  comparison?: ModelPriceComparison;
  pricingVariants?: readonly ModelPricingVariant[];
  defaultPricingVariantId?: string;
}

export function selectedModelVariant(model: CustomerModel, selectedId?: string) {
  const variants = model.pricingVariants ?? [];
  return variants.find(variant => variant.id === selectedId)
    ?? variants.find(variant => variant.id === model.defaultPricingVariantId) ?? variants[0];
}

export function modelTierVariant(model: CustomerModel, current: ModelPricingVariant, tierId: string) {
  const variants = (model.pricingVariants ?? []).filter(variant => variant.tier === tierId);
  return variants.find(variant => variant.contextBand === current.contextBand) ?? variants[0];
}

/** Presentation only: use the same ranges as Desktop, never infer bands from max context. */
export function modelContextRanges(model: CustomerModel, tier: PriceSchedule) {
  const bands = [...new Set((model.pricingVariants ?? []).filter(variant => variant.tier === tier).map(variant => variant.contextBand))];
  const threshold = (band: PriceBand) => band === "base" ? 0 : PRICE_CONTEXT_THRESHOLDS[band];
  return priceContextRanges(bands.sort((left, right) => threshold(left) - threshold(right)));
}

export function modelRate(value: bigint | null | undefined): bigint | null {
  return typeof value === "bigint" && value >= 0n ? value : null;
}

export function pricePresentation(model: ModelPricing, component: ModelRateComponent) {
  const price = modelRate(model.rates[component]);
  const official = modelRate(model.comparison?.officialRates[component]);
  const supplied = model.comparison?.savingsPercent?.[component];
  const comparable = price !== null && official !== null && official > price;
  const discount = comparable && typeof supplied === "number" && Number.isFinite(supplied)
    && supplied > 0 && supplied <= 100 && (supplied < 100 || price === 0n) ? supplied : null;
  return { price, official: comparable ? official : null, discount };
}

/** Preserve all six micro-USD digits without showing insignificant zeroes. */
export function formatModelPrice(value: bigint | null, locale: Locale): string {
  if (value === null) return "—";
  return `$${formatMoney(value, 6, locale).replace(/0+$/, "").replace(/[.,]$/, "")}`;
}

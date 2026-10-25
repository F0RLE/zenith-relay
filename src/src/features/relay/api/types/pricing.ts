import type { DefaultServiceTier } from "./common";

export type QuotaWindow = {
  kind: "primary" | "secondary";
  providerCycleId?: string | null;
  windowStartMs?: number | null;
  availableBasisPoints: number | null;
  explicitlyFull: boolean | null;
  resetAtMs: number | null;
  windowMinutes: number | null;
  observedAtMs: number;
};

export type SupplementalQuotaWindow = {
  id: string;
  label: string;
  serviceTier?: DefaultServiceTier | null;
  window: QuotaWindow;
};

export type QuotaSnapshot = {
  primary: QuotaWindow | null;
  secondary: QuotaWindow | null;
  supplemental?: SupplementalQuotaWindow[];
  limitReached: boolean;
  resetCreditsAvailable: number | null;
  /** Provider-reported credits in millionths of one credit; informational only. */
  availableCreditsMicroUnits?: number | null;
  /** Fresh positive or unlimited provider credits keep an exhausted account eligible. */
  providerCreditsAvailable?: boolean;
  providerCreditsUnlimited?: boolean;
  /** Complete ledger observation; passive quota headers never advance it. */
  providerCreditsObservedAtMs?: number | null;
  directBalanceMicroUsd?: number | null;
  updatedAtMs: number | null;
  error: { code: string; occurredAtMs: number } | null;
};

export type ConsumeResetCreditResponse = {
  refreshed: boolean;
  refreshError?: string;
};

export type ApiEquivalentSummary = {
  microUsd: number;
  pricedTokens: number;
  unpricedTokens: number;
};

/** Components of the same API-price estimate, not separate billed charges. */
export type ApiEquivalentBreakdown = {
  input: ApiEquivalentSummary | null;
  uncachedInput: ApiEquivalentSummary | null;
  cacheRead: ApiEquivalentSummary | null;
  cacheWrite: ApiEquivalentSummary | null;
  output: ApiEquivalentSummary | null;
};

export type PricingSourceSummary =
  | "provider"
  | "liteLlmExact"
  | "liteLlmCanonical"
  | "manual"
  | "mixed"
  | "unpriced";

export type CatalogStatus = "current" | "stale" | "updating" | "unloaded" | "error";

/** Freshness and provenance of the LiteLLM-backed API-equivalent estimate. */
export type PricingMetadata = {
  catalogRevision?: string | null;
  catalogFetchedAtMs?: number | null;
  /** Optional so snapshots produced before the pricing contract remain readable. */
  catalogStale?: boolean;
  catalogStatus?: CatalogStatus;
  priceSource?: PricingSourceSummary;
  unpricedTokens?: number;
};

export type QuotaWindowUsage = {
  recentConsumedBasisPoints?: number | null;
  recentApiEquivalent?: ApiEquivalentSummary | null;
  kind: "primary" | "secondary";
  windowStartMs: number;
  observedAtMs: number;
  windowMinutes: number;
  apiEquivalent: ApiEquivalentSummary;
};

/** Published input-token price thresholds, independent of maximum context. */
export const PRICE_CONTEXT_THRESHOLDS = {
  above100k: 100_000,
  above128k: 128_000,
  above200k: 200_000,
  above272k: 272_000,
  above512k: 512_000,
} as const;

export type PriceContextBand = keyof typeof PRICE_CONTEXT_THRESHOLDS;

export type PriceSchedules = {
  flex?: TokenRateSet | undefined;
  priority?: TokenRateSet | undefined;
  ultrafast?: TokenRateSet | undefined;
} & Partial<Record<PriceContextBand, LongContextRates | undefined>>;

export type ApiModelPriceOverride = PriceSchedules & {
  inputMicroUsdPerMillion: number;
  cachedInputMicroUsdPerMillion?: number | null | undefined;
  cacheWrite5mMicroUsdPerMillion?: number | null | undefined;
  cacheWrite1hMicroUsdPerMillion?: number | null | undefined;
  cacheWriteMicroUsdPerMillion?: number | null | undefined;
  outputMicroUsdPerMillion: number;
};

export type TokenRateSet = {
  input?: number | undefined;
  output?: number | undefined;
  cacheRead?: number | undefined;
  cacheWrite5m?: number | undefined;
  cacheWrite1h?: number | undefined;
  cacheWrite?: number | undefined;
};

export type LongContextRates = {
  standard?: TokenRateSet;
  flex?: TokenRateSet;
  priority?: TokenRateSet;
  ultrafast?: TokenRateSet;
};

export type TokenPrice = TokenRateSet & PriceSchedules & {
  input: number;
  output: number;
};

/** Query-time price evidence scoped to one source. */
export type ResolvedModelPrice = {
  quote: TokenPrice | null;
  source: Exclude<PricingSourceSummary, "mixed">;
  catalogRevision: string | null;
  catalogFetchedAtMs: number | null;
  stale: boolean;
};

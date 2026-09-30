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

export type PricingSourceSummary =
  | "provider"
  | "liteLlmExact"
  | "liteLlmCanonical"
  | "manual"
  | "mixed"
  | "unpriced";

export type CatalogStatus = "current" | "stale" | "updating" | "unloaded" | "error";

export type CatalogRefreshOutcome =
  | { kind: "updated"; revision: string }
  | { kind: "notModified"; revision: string }
  | { kind: "skipped" };

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
  kind: "primary" | "secondary";
  windowStartMs: number;
  observedAtMs: number;
  windowMinutes: number;
  apiEquivalent: ApiEquivalentSummary;
};

export type ApiModelPriceOverride = {
  inputMicroUsdPerMillion: number;
  cachedInputMicroUsdPerMillion?: number | null;
  cacheWrite5mMicroUsdPerMillion?: number | null;
  cacheWrite1hMicroUsdPerMillion?: number | null;
  outputMicroUsdPerMillion: number;
};

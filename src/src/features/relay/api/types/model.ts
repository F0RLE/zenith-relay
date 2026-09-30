import type { DefaultServiceTier } from "./common";
import type {
  CapabilityStatus,
  ProtocolFeature,
  SourceWireApi,
} from "./source";

export type ModelSummary = {
  id: string;
  protocolRoutes?: {
    clientWireApi: SourceWireApi;
    upstreamWireApi: SourceWireApi;
    features: Partial<Record<ProtocolFeature, CapabilityStatus>>;
    reasoningEfforts: string[];
  }[];
  enabled: boolean;
  memberCount: number;
  codexVisible: boolean;
  codexDisplayName: string;
  catalogProvider?: string | null;
  catalogFamily?: string | null;
  catalogName?: string | null;
  catalogReleaseDate?: string | null;
  catalogLastUpdated?: string | null;
  catalogStatus?: string | null;
  catalogReasoning?: boolean | null;
  catalogReasoningMethod?: "effort" | "toggle" | "budget_tokens" | "adaptive" | "unknown" | null;
  catalogReasoningEffortLevels?: string[];
  catalogDefaultReasoningEffort?: string | null;
  catalogToolCall?: boolean | null;
  catalogStructuredOutput?: boolean | null;
  catalogAttachment?: boolean | null;
  catalogOpenWeights?: boolean | null;
  catalogInputModalities?: string[];
  catalogOutputModalities?: string[];
  catalogContextLimit?: number | null;
  catalogInputLimit?: number | null;
  catalogOutputLimit?: number | null;
  inputMicroUsdPerMillion: number | null;
  cachedInputMicroUsdPerMillion?: number | null;
  cacheWrite5mMicroUsdPerMillion?: number | null;
  cacheWrite1hMicroUsdPerMillion?: number | null;
  outputMicroUsdPerMillion: number | null;
  imageRequestPrices?: ImageRequestPrice[];
  customPrice: boolean;
  reasoningLevels?: string[];
  reasoningSupportedLevels?: string[];
  reasoningAllowedLevels?: string[];
  reasoningConfigurable?: boolean;
  reasoningManualFallback?: boolean;
  speedSupported?: boolean;
  /** Exact speed tiers confirmed by current routes, including standard. */
  speedTiers?: DefaultServiceTier[];
  speedTier?: DefaultServiceTier;
  speedConfigurable?: boolean;
};

export type ImageRequestPrice = {
  operation: "generation" | "edit" | string;
  quality: string;
  size: string;
  microUsd: number;
};

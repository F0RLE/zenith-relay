import type { DefaultServiceTier } from "./common";
import type { TokenPrice } from "./pricing";
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
  catalogSourceModelId?: string | null;
  catalogCanonicalModelId?: string | null;
  catalogFamily?: string | null;
  catalogName?: string | null;
  catalogReleaseDate?: string | null;
  catalogLastUpdated?: string | null;
  catalogStatus?: string | null;
  catalogReasoning?: boolean | null;
  catalogReasoningMethod?: "effort" | "toggle" | "budget_tokens" | "adaptive" | "unknown" | null;
  catalogReasoningEffortLevels?: string[];
  catalogDefaultReasoningEffort?: string | null;
  catalogReasoningBudgetMinTokens?: number | null;
  catalogReasoningBudgetMaxTokens?: number | null;
  catalogReasoningBudgetDefaultTokens?: number | null;
  catalogToolCall?: boolean | null;
  catalogStructuredOutput?: boolean | null;
  catalogAttachment?: boolean | null;
  catalogOpenWeights?: boolean | null;
  catalogInputModalities?: string[];
  catalogOutputModalities?: string[];
  catalogContextLimit?: number | null;
  catalogInputLimit?: number | null;
  catalogOutputLimit?: number | null;
  catalogCapabilities?: ModelCapabilities | null;
  inputMicroUsdPerMillion: number | null;
  cachedInputMicroUsdPerMillion?: number | null;
  cacheWrite5mMicroUsdPerMillion?: number | null;
  cacheWrite1hMicroUsdPerMillion?: number | null;
  outputMicroUsdPerMillion: number | null;
  tokenPrice?: TokenPrice | null;
  imageRequestPrices?: ImageRequestPrice[];
  customPrice: boolean;
  reasoningLevels?: string[];
  reasoningSupportedLevels?: string[];
  reasoningAllowedLevels?: string[];
  reasoningConfigurable?: boolean;
  reasoningManualFallback?: boolean;
  speedSupported?: boolean;
  /** Reference speed tiers narrowed by configured routes, including standard. */
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

/** Validated reference data. Route access and observed prices remain separate. */
export type ModelCapabilities = {
  reasoning?: boolean | null;
  reasoningSource?: string | null;
  reasoningMethod?: ModelSummary["catalogReasoningMethod"];
  reasoningBudgetMinTokens?: number | null;
  reasoningBudgetMaxTokens?: number | null;
  reasoningBudgetDefaultTokens?: number | null;
  reasoningEffortLevels: string[];
  defaultReasoningEffort?: string | null;
  reasoningModes: string[];
  toolCall?: boolean | null;
  structuredOutput?: boolean | null;
  attachment?: boolean | null;
  openWeights?: boolean | null;
  temperature?: boolean | null;
  promptCaching?: boolean | null;
  parallelToolCall?: boolean | null;
  toolChoice?: boolean | null;
  streaming?: boolean | null;
  serviceTiers: DefaultServiceTier[];
  inputModalities: string[];
  outputModalities: string[];
  contextLimit?: number | null;
  inputLimit?: number | null;
  outputLimit?: number | null;
};

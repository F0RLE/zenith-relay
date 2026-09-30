import type { OperationalStatus } from "./common";
import type { ApiEquivalentSummary, ApiModelPriceOverride } from "./pricing";

export type SourceWireApi = "responses" | "chat_completions" | "messages" | "gemini";

export type SourceAdapter = "native" | "responses_to_messages" | "responses_to_gemini"
  | "responses_to_chat_completions" | "chat_completions_to_responses" | "chat_completions_to_messages"
  | "chat_completions_to_gemini" | "messages_to_responses" | "messages_to_chat_completions"
  | "messages_to_gemini" | "gemini_to_responses" | "gemini_to_chat_completions" | "gemini_to_messages";

export type CapabilityStatus = "declared" | "confirmed" | "unsupported" | "unknown";
/** Refresh evidence only; does not determine whether inference can be routed. */
export type RefreshStatus = "unknown" | "fresh" | "stale" | "unsupported";
export type CapabilityOrigin = "catalog" | "service_profile" | "endpoint_url" | "manual" | "generation_probe";
export type ProtocolFeature = "text" | "streaming" | "images" | "function_tools" | "tool_choice" | "structured_output" | "reasoning";
export type ModelEndpointCapability = {
  modelId: string;
  upstreamWireApi: SourceWireApi;
  status: CapabilityStatus;
  origin: CapabilityOrigin;
  checkedAtMs: number;
  features: Partial<Record<ProtocolFeature, CapabilityStatus>>;
  reasoningEfforts: string[];
};
export type SourceProtocolConfig = {
  revision: number;
  capabilities: ModelEndpointCapability[];
  endpointHint?: SourceWireApi | null;
};

export type SourceProbeInput = { modelId: string; wireApi: SourceWireApi; expectedRevision: number };
export type SourceProbeResult = {
  capability: ModelEndpointCapability;
  revision: number;
  httpStatus: number | null;
  errorCode: string | null;
};

export type MessagesReasoningMode = "disabled" | "budget" | "adaptive";
export type CacheWriteTtl = "provider" | "5m" | "1h";
export type DocumentedCacheRetentionMinimum = "30m";

export type SourceProtocolBinding = {
  wireApi: SourceWireApi;
  modelIds: string[];
  /**
   * Older Relay records do not have adapter metadata. The editor and runtime
   * treat an omitted value as the native passthrough.
   */
  adapter?: SourceAdapter;
  reasoningMode?: MessagesReasoningMode;
  cacheWriteTtl?: CacheWriteTtl;
};

export type SourceSummary = {
  id: string;
  name: string;
  enabled: boolean;
  inPool: boolean;
  draining: boolean;
  operationalStatus: OperationalStatus;
  baseUrl: string;
  pricingProvider?: string | null;
  officialProviderFamily?: string | null;
  wireApi: SourceWireApi;
  protocolBindings?: SourceProtocolBinding[];
  protocolConfig?: SourceProtocolConfig;
  resolvedProtocolBindings?: SourceProtocolBinding[];
  models: string[];
  allowedModels: string[];
  excludedModels: string[];
  priority: number;
  weight: number;
  recoveryDelaySeconds: number;
  modelPriceOverrides?: Record<string, ApiModelPriceOverride>;
  detectedModelPrices?: Record<string, ApiModelPriceOverride>;
  apiEquivalent: ApiEquivalentSummary;
  secretAvailable: boolean;
  lastErrorCode: string | null;
  /** Non-secret scope for cached provider observations; absent on older servers. */
  refreshRevision?: number | null;
  /** Optional on older servers; models and balance have independent freshness. */
  refreshState?: { models: RefreshStatus; balance: RefreshStatus };
  /** Host runtime cache only; reading snapshots does not poll a provider. */
  providerStats?: SourceStats | null;
};

export type SourceStats = {
  provider: "zenith" | "openrouter" | "sub2api" | "new_api" | "billing" | "deepseek" | "siliconflow" | "unsupported";
  balanceMicroUsd: number | null;
  spentMicroUsd: number | null;
  requests: number | null;
  totalTokens: number | null;
  status?: SourceStatsStatus;
  balanceKind?: "wallet" | "key_quota" | "subscription";
  balanceUnlimited?: boolean;
  amounts?: SourceStatsAmount[];
  asOfMs?: number | null;
  stale?: boolean;
  refreshError?: SourceStatsStatus | null;
};

export type SourceStatsStatus = "available" | "unsupported" | "unauthorized" | "rate_limited" | "unavailable" | "invalid_response";
export type SourceStatsAmount = {
  currency: "USD" | "CNY" | "CREDITS";
  balanceMicros: number | null;
  spentMicros: number | null;
};

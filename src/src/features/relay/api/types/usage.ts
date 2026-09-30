import type {
  DefaultServiceTier,
  ObservedServiceTier,
  RelayMode,
  ToolPolicyMode,
} from "./common";
import type { ApiEquivalentSummary, PricingMetadata } from "./pricing";

export type RoutingDiagnostics = {
  reason: "response_affinity" | "prompt_cache_affinity" | "session_affinity" | "connection_affinity" | "only_eligible" | "routing_tier" | "source_role" | "parallel_load" | "source_load" | "pool_policy" | "quota_headroom" | "adaptive_balance" | "subscription_expiry" | "subscription_plan" | "weighted_rotation" | "fair_rotation" | "fallback_attempt" | "least_recently_used" | "manual_priority" | "manual_weight" | "stable_tie_break";
  eligibleCandidates: number;
  quotaRemainingBasisPoints: number | null;
  inFlightBefore: number;
  dispatchesBefore: number;
  endpointKind?: string | null;
};

export type ToolUseDiagnostics = {
  clientToolCount: number;
  forwardedToolCount: number;
  clientSchemaBytes?: number;
  forwardedSchemaBytes?: number;
  filteredToolCount?: number;
  policyMode?: ToolPolicyMode;
  policyOutcome?: "pass_through" | "below_threshold" | "no_selection" | "unchanged" | "filtered" | "deferred";
  policyFallback?: boolean;
  deferredToolSearch?: boolean;
  toolChoice: "unspecified" | "auto" | "required" | "none" | "allowed_tools" | "specific";
  toolCallCount: number;
  textOutput: boolean;
  terminalOutput: "unknown" | "empty" | "text" | "tool_call" | "mixed";
};

export type ErrorOrigin = "provider" | "account" | "relay";
export type UpstreamErrorDetails = {
  httpStatus: number | null;
  code: string | null;
  errorType: string | null;
  message: string | null;
  redacted: boolean;
  truncated: boolean;
};
export type ReasoningEffort = "minimal" | "low" | "medium" | "high" | "xhigh" | "max" | "ultra";

/** Fields shared by local SQLite and remote Relay usage events. */
type UsageEventRecord = {
  id: number;
  requestId: string;
  attempt: number;
  routing?: RoutingDiagnostics | null;
  requestedModel: string | null;
  resolvedModel: string | null;
  requestedReasoningEffort?: ReasoningEffort | null;
  effectiveReasoningEffort?: ReasoningEffort | null;
  wireApi: "responses" | "chat_completions" | "messages" | "gemini";
  serviceTier?: DefaultServiceTier;
  appliedServiceTier?: ObservedServiceTier | null;
  success: boolean;
  httpStatus: number;
  errorCategory: string | null;
  errorOrigin?: ErrorOrigin | null;
  upstreamError?: UpstreamErrorDetails | null;
  toolUse?: ToolUseDiagnostics;
  latencyMs: number;
  ttftMs?: number | null;
  generationMs?: number | null;
  inputTokens: number | null;
  cachedInputTokens: number | null;
  cacheWriteInputTokens?: number | null;
  cacheWriteTtl?: string | null;
  reasoningTokens: number | null;
  outputTokens: number | null;
  totalTokens: number | null;
  apiEquivalent?: ApiEquivalentSummary;
};

export type LocalUsage = UsageEventRecord & {
  createdAt: string;
  sourceId: string;
  accountId?: string | null;
  clientContextId?: string | null;
  ttftMs: number | null;
  generationMs: number | null;
};

export type UsageTotals = {
  requests: number;
  successfulRequests: number;
  latencyMs: number;
  ttftMs: number;
  ttftSamples: number;
  generationMs: number;
  generationSamples: number;
  generationOutputTokens: number;
  inputTokens: number;
  cachedInputTokens: number;
  cachedInputSamples: number;
  cacheWriteInputTokens?: number;
  cacheWriteInputSamples?: number;
  reasoningTokens: number;
  outputTokens: number;
  totalTokens: number;
  speedOutputTokens: number;
  speedDurationMs: number;
  apiEquivalent: ApiEquivalentSummary;
};

export type UsageGroup = {
  key: string;
  label?: string | null;
  totals: UsageTotals;
};

export type UsageBucket = {
  startMs: number;
  totals: UsageTotals;
};

export type CacheSessionRecord = {
  clientContextId: string;
  startedAt: string;
  touchedAt: string;
  model: string | null;
  cacheWriteTtl: string | null;
};

export type LocalUsagePage = {
  events: LocalUsage[];
  total: number;
  page: number;
  pageSize: number;
  totalPages: number;
  totals: UsageTotals;
  models: UsageGroup[];
  poolMembers: UsageGroup[];
  buckets?: UsageBucket[];
  pricing?: PricingMetadata;
};

export type UsageExportRow = {
  time: string;
  success: boolean;
  model: string | null;
  requestedReasoningEffort?: ReasoningEffort | null;
  effectiveReasoningEffort?: ReasoningEffort | null;
  connection: string;
  latencyMs: number;
  ttftMs: number | null;
  inputTokens: number | null;
  cachedInputTokens: number | null;
  cacheWriteInputTokens?: number | null;
  cacheWriteTtl?: string | null;
  reasoningTokens: number | null;
  outputTokens: number | null;
  tokens: number | null;
  requestId: string | null;
  httpStatus: number | null;
  errorCategory: string | null;
  errorOrigin?: ErrorOrigin | null;
  serviceTier?: DefaultServiceTier;
  appliedServiceTier?: ObservedServiceTier | null;
};

export type SupportExportContext = {
  mode: RelayMode;
  schemaVersion: number | null;
  gatewayRunning: boolean;
  sourceCount: number;
  accountCount: number;
  automationCount: number;
  usageCount: number;
  warningCount: number;
};

export type RemoteUsage = UsageEventRecord & {
  candidateKind: "account" | "source";
  candidateHint: string;
  candidateLabel?: string | null;
  createdAtMs: number;
};

export type RemoteUsageQuery = {
  page?: number;
  pageSize?: number;
  range?: "daily" | "weekly" | "monthly" | "custom";
  fromMs?: number;
  toMs?: number;
  bucketMs?: number;
  modelQuery?: string;
  sourceOrAccountQuery?: string;
  wireApi?: "responses" | "chat_completions" | "messages" | "gemini";
  success?: boolean;
  errorCategory?: string;
  requestIdQuery?: string;
  /** Optional response projections; omitted keeps compatibility with older hosts. */
  includeEvents?: boolean;
  includeModels?: boolean;
  includePoolMembers?: boolean;
};

export type RemoteUsagePage = {
  events: RemoteUsage[];
  total: number;
  page: number;
  pageSize: number;
  totalPages: number;
  totals?: UsageTotals;
  models?: UsageGroup[];
  poolMembers?: UsageGroup[];
  buckets?: UsageBucket[];
  pricing?: PricingMetadata;
};

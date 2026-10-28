import type {
  DefaultServiceTier,
  ObservedServiceTier,
  ToolPolicyMode,
} from "./common";
import type { ApiEquivalentBreakdown, ApiEquivalentSummary, PricingMetadata } from "./pricing";

/** Local prefix comparisons and, when reported, separate provider diagnostics. */
export type CacheContextDiagnostics = {
  baseline: "first_observation" | "completed_request" | "overlapping_requests" | "unavailable" | "size_limit";
  scope: "client_session" | "cache_key" | "unavailable";
  clientChanges: CacheContextSection[];
  upstreamChanges: CacheContextSection[];
  relayChanges: CacheContextSection[];
  clientHistory: CacheHistoryDiagnostics;
  upstreamHistory: CacheHistoryDiagnostics;
  relayHistory: CacheHistoryDiagnostics;
  candidateChanged: boolean | null;
  previousCompletedAgeMs: number | null;
  provider?: { type: "cache_hit" | "comparison_response_not_found" | "unavailable" }
    | { type: "cache_miss"; reason: "model_changed" | "prompt_cache_key_changed" | "service_tier_changed"
      | "tools_changed" | "text_format_changed" | "reasoning_effort_changed" | "verbosity_changed"
      | "context_compacted" | "input_changed"; cache_missed_tokens: number; comparison_reusable_tokens?: number } | null;
};

export type CacheContextSection =
  | "model" | "tools" | "instructions" | "reasoning" | "output_format" | "verbosity"
  | "tool_choice" | "parallel_tool_calls" | "cache_key" | "cache_policy" | "service_tier"
  | "context_management" | "truncation";

export type CacheHistoryDiagnostics = {
  comparison: "not_compared" | "unchanged" | "appended" | "rewritten" | "truncated" | "continuation";
  inputItems: number | null;
  /** Compact JSON bytes, not tokens. */
  inputBytes: number | null;
  sharedPrefixItems: number | null;
  firstChangedItemKind: "developer" | "user" | "assistant" | "tool_result" | "tool_call"
    | "reasoning" | "compaction" | "additional_tools" | "configuration_update" | "other" | null;
  /** Same-request body alignment only; not evidence of provider cache reuse. */
  leadingContextAlignment?: {
    skippedClientItems: number;
    skippedUpstreamItems: number;
    sharedPrefixItems: number;
    comparison: CacheHistoryDiagnostics["comparison"];
  } | null;
};

export type CacheAffinityMember = {
  quotaRemainingBasisPoints: number | null;
  providerCreditsMicroUnits: number | null;
  providerCreditsUnlimited: boolean;
  inFlight: number;
  effectiveCapacity: number;
};

/** Local admission evidence, not a provider cache-hit or cache-miss verdict. */
export type CacheAffinityDiagnostics = {
  selection: "preferred" | "warm_alternative" | "other";
  reason: "retained" | "no_binding" | "routing_policy" | "response_owner" | "owner_unavailable" | "recovery_probe" | "parallel_load" | "quota_headroom" | "provider_credits" | "health";
  ownerBlockReason?: "already_tried" | "not_allowed" | "owner_mismatch" | "disabled" | "draining" | "route_unavailable" | "model_unavailable" | "operation_unavailable" | "auth_required" | "auth_blocked" | "quota_exhausted" | "rate_limited" | "circuit_open" | "recovery_busy" | "capacity_busy" | null;
  preferred: CacheAffinityMember | null;
  selected: CacheAffinityMember;
  bestQuotaRemainingBasisPoints: number | null;
  bestProviderCreditsMicroUnits: number | null;
  bestProviderCreditsUnlimited: boolean;
  quotaSwitchMarginBasisPoints: number;
  providerCreditSwitchMarginMicroUnits: number;
  loadAllowanceRequests: number;
};

export type RoutingDiagnostics = {
  reason: "response_affinity" | "prompt_cache_affinity" | "session_affinity" | "connection_affinity" | "only_eligible" | "routing_tier" | "source_role" | "parallel_load" | "source_load" | "pool_policy" | "quota_headroom" | "provider_credits" | "adaptive_balance" | "subscription_expiry" | "subscription_plan" | "weighted_rotation" | "fair_rotation" | "fallback_attempt" | "least_recently_used" | "manual_priority" | "manual_weight" | "stable_tie_break";
  eligibleCandidates: number;
  quotaRemainingBasisPoints: number | null;
  inFlightBefore: number;
  dispatchesBefore: number;
  endpointKind?: string | null;
  /** Upstream declaration, not independently verified model identity. */
  upstreamResponseModel?: string | null;
  transport?: {
    upstream: "http" | "websocket";
    mode: "native" | "protocol_conversion" | "basis_points";
    fallbackReason?: "basis_points_http" | "protocol_conversion" | "websocket_disabled" | "websocket_unavailable" | "websocket_slow_handshake" | "existing_http_bridge" | null;
    responseFormat?: "json" | "sse" | "websocket_frames" | null;
    handshakeMs?: number | null;
    connectionReused?: boolean | null;
    responseHeadersMs?: number | null;
    requestPreparationMs?: number | null;
    requestPayload?: {
      mode: "full" | "continuation";
      fullRequestReason?: "basis_points_transport" | "protocol_conversion" | "no_previous_response_id" | null;
      jsonBytes: number;
      /** Encoded body bytes, excluding HTTP/WebSocket framing and TLS. */
      bodyBytes: number;
      contentEncoding: "identity" | "zstd";
      compressionMs?: number | null;
      /** Wall time including preparation and blocking-pool scheduling. */
      compressionUs?: number | null;
      /** HTTP execute to headers, not upload-only time or total TTFT. */
      httpSendToHeadersMs?: number | null;
    } | null;
    requestPreparation?: {
      bodyMs?: number | null;
      /** Included in bodyMs. */
      schemaValidationMs?: number | null;
      catalogMs?: number | null;
      serializationMs?: number | null;
      attachmentsMs?: number | null;
      /** Prepared-context accounting after attachments; not all diagnostic overhead. */
      contextAccountingMs?: number | null;
    } | null;
    httpVersion?: string | null;
    upstreamResponseMs?: number | null;
    clientFirstOutputMs?: number | null;
    clientResponseReadyMs?: number | null;
    /** Optional native backend measurements; not additive Relay phases. */
    codexServerTiming?: {
      overheadMs?: number;
      inferenceMs?: number;
      engineIapiTtftMs?: number;
      engineServiceTtftMs?: number;
      engineIapiTbtMs?: number;
      engineServiceTbtMs?: number;
    } | null;
  } | null;
  cacheContext?: CacheContextDiagnostics | null;
  cacheAffinity?: CacheAffinityDiagnostics | null;
  requestTiming?: {
    durationMs: number;
    ttftMs: number | null;
    /** Logical finalization only; not another send or usage observation. */
    projectionOnly: boolean;
  } | null;
  attemptOutcome?: "completed" | "tool_discovery" | "retry_selection" | "retry_repair" | "retry_authorization" | "returned_failure" | "execution_uncertain" | "client_disconnected" | null;
};

export type RequestContextSize = {
  bodyBytes: number;
  instructionBytes: number;
  conversationBytes: number;
  toolCatalogBytes: number;
  /** Disjoint categories sum to conversationBytes; absent in legacy observations. */
  conversation?: {
    messageBytes: number;
    toolCallBytes: number;
    toolResultBytes: number;
    reasoningBytes: number;
    attachmentBytes: number;
    otherBytes: number;
    itemCount: number;
    attachmentCount: number;
  };
};

export type ToolUseDiagnostics = {
  clientToolCount: number;
  /** Post-policy catalog, before BPS prompt encoding or other transport changes. */
  forwardedToolCount: number;
  clientSchemaBytes?: number;
  forwardedSchemaBytes?: number;
  clientContext?: RequestContextSize;
  upstreamContext?: RequestContextSize;
  basisPointsCatalog?: {
    mode: "full" | "compact" | "discovery";
    selectionReason?: "requested_full" | "requested_compact" | "requested_discovery"
      | "automatic_discovery" | "below_discovery_threshold" | "no_tools"
      | "lookup_tool_present" | "dispatch_budget";
    /** Subset of upstream instructionBytes, not all generated text. */
    relayToolInstructionBytes?: number;
    fullSchemaBytes: number;
    renderedSchemaBytes: number;
    catalogPromptBytes: number;
    loadedToolCount: number;
    discoveryRounds: number;
    discoveryInputTokens?: number;
    discoveryLatencyMs?: number;
    discoveryOutputTokens?: number;
  };
  filteredToolCount?: number;
  policyMode?: ToolPolicyMode;
  policyOutcome?: "pass_through" | "below_threshold" | "no_selection" | "unchanged" | "filtered" | "deferred";
  policyFallback?: boolean;
  deferredToolSearch?: boolean;
  toolChoice: "unspecified" | "auto" | "required" | "none" | "allowed_tools" | "specific";
  toolCallCount: number;
  requestedParallelToolCalls?: boolean | null;
  upstreamToolCallCount?: number | null;
  toolCallsReadyMs?: number | null;
  textOutput: boolean;
  terminalOutput: "unknown" | "empty" | "text" | "tool_call" | "mixed";
};

export type ErrorOrigin = "provider" | "account" | "relay";
export type UsageTransport = "http" | "websocket";
export type UpstreamErrorDetails = {
  httpStatus: number | null;
  code: string | null;
  errorType: string | null;
  parameter?: string | null;
  requestId?: string | null;
  message: string | null;
  redacted: boolean;
  truncated: boolean;
};
export type ReasoningEffort = "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max" | "ultra";

/** Fields shared by local SQLite queries and runtime usage events. */
type UsageEventRecord = {
  id: number;
  requestId: string;
  attempt: number;
  routing?: RoutingDiagnostics | null;
  requestedModel: string | null;
  resolvedModel: string | null;
  requestedReasoningEffort?: ReasoningEffort | null;
  effectiveReasoningEffort?: ReasoningEffort | null;
  wireApi: "responses" | "chat_completions" | "messages" | "gemini" | "gemini_interactions";
  transport: UsageTransport;
  serviceTier?: DefaultServiceTier;
  appliedServiceTier?: ObservedServiceTier | null;
  success: boolean;
  httpStatus: number;
  errorCategory: string | null;
  errorOrigin?: ErrorOrigin | null;
  upstreamError?: UpstreamErrorDetails | null;
  imageUsage?: {
    inputTextTokens?: number | null;
    inputImageTokens?: number | null;
    cachedTextTokens?: number | null;
    cachedImageTokens?: number | null;
    outputImageTokens?: number | null;
    generatedImages: number;
    inputDetailsConsistent?: boolean | null;
  } | null;
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
  apiEquivalentBreakdown?: ApiEquivalentBreakdown;
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
  candidateKind: "account" | "source";
  candidateId: string;
  startedAt: string;
  touchedAt: string;
  model: string | null;
  cacheWriteTtl: string | null;
};

export type UsageFilterOptions = {
  models: string[];
  poolMembers: string[];
  wireApis: string[];
  transports: string[];
  errorCategories: string[];
};

export type LocalUsagePage = {
  events: LocalUsage[];
  /** Pagination for bounded sends-only reads with partial totals/counts. */
  hasMore?: boolean;
  /** Echoed by hosts that actually applied the requested workload filter. */
  workload?: UsageWorkload;
  /** Older desktop hosts can omit the projection or only provide advanced fields. */
  filterOptions?: Partial<UsageFilterOptions>;
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
  transport: UsageTransport;
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

/** Explicit task markers; unmarked requests remain in the conversation scope. */
export type UsageWorkload = "conversation" | "memory" | "background";

export type UsageQuery = {
  /** Desktop-only: false loads a bounded sends page without global statistics. */
  includeReport?: boolean;
  page?: number;
  pageSize?: number;
  range?: "daily" | "weekly" | "monthly" | "custom";
  fromMs?: number;
  toMs?: number;
  bucketMs?: number;
  modelQuery?: string;
  sourceOrAccountQuery?: string;
  /** Omitted includes all usage, including memory and other background work. */
  workload?: UsageWorkload;
  wireApi?: "responses" | "chat_completions" | "messages" | "gemini" | "gemini_interactions";
  transport?: UsageTransport;
  success?: boolean;
  errorCategory?: string;
  requestIdQuery?: string;
  requestId?: string;
  /** Optional response projections; omitted keeps compatibility with older hosts. */
  includeEvents?: boolean;
  includeModels?: boolean;
  includePoolMembers?: boolean;
  includeFilterOptions?: boolean;
};

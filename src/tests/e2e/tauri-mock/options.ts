import type { ResolvedModelPrice } from "../../../src/features/relay/api/types/pricing";
import type { AccountSummary, ApiModelPriceOverride, CacheContextDiagnostics, CacheSessionRecord, CandidateRuntimeSnapshot, ModelSummary, RelayMode, RoutingDiagnostics, SourceStats, SourceProtocolBinding, SourceProtocolConfig, UpstreamErrorDetails, UsageQuery, UsageTotals, UsageTransport, WakeTask } from "../../../src/features/relay/api/types";
import type { WeeklyCapacityReport } from "../../../src/features/relay/api/types/weeklyCapacity";

export type RuntimeMockOptions = {
  platform?: "windows" | "macos" | "linux";
  fullscreen?: boolean;
  locale?: "en" | "ru";
  onboarding?: boolean;
  mode?: RelayMode;
  /** Raw saved preference for compatibility tests, not an executable runtime mode. */
  storedMode?: string;
  legacyOwnedAccountIds?: string[];
  theme?: "system" | "light" | "dark";
  populated?: boolean;
  runtimeState?: "ready" | "error" | "invalid";
  runtimeStateDelayMs?: number;
  automation?: Partial<WakeTask>;
  diagnosticDebug?: boolean;
  updateVersion?: string;
  updateBody?: string;
  updateDate?: string;
  portableUpdateTargetMissing?: boolean;
  updateCheckError?: boolean;
  updateCheckDelayMs?: number;
  bundleType?: "nsis" | "msi" | null;
};

export type AccountMockOptions = {
  accountCount?: number;
  accountLabel?: string;
  accountRouteKinds?: Array<NonNullable<AccountSummary["oauthClientKind"]>>;
  accountRoutesGrouped?: boolean;
  accountRouteSelectError?: boolean;
  oauthCompletedRouteKind?: NonNullable<AccountSummary["oauthClientKind"]>;
  accountOAuthClientKind?: AccountSummary["oauthClientKind"];
  accountPlanType?: AccountSummary["subscription"]["planType"];
  accountQuotaMissing?: boolean;
  accountCalibrationMissing?: boolean;
  accountCalibrationMixed?: boolean;
  accountCredentialDiagnostics?: AccountSummary["credentialDiagnostics"];
  accountModels?: string[];
  accountAllowedModels?: string[];
  accountExcludedModels?: string[];
  accountHealth?: string;
  staleAccountError?: boolean;
  accountModelErrorCode?: string;
  accountCooldown?: boolean;
  accountModelCooldown?: boolean;
  accountAuthReason?: "invalid_grant" | "reused_refresh_token" | "expired_refresh_token" | "invalidated_refresh_token";
  supplementalQuota?: boolean;
  subscriptionExpiresInMs?: number;
  exhaustedQuotaWindow?: "primary" | "secondary";
  quotaAvailable?: boolean;
  resetCreditsAvailable?: number;
  providerCredits?: number;
  providerCreditsUnlimited?: boolean;
  resetCreditsError?: string;
  resetCreditsRefreshError?: string;
  quotaRefreshStatus?: "pending" | "refreshing" | "updated" | "failed" | "requires_reauth";
  freeAccountHealthy?: boolean;
  importResult?: "success" | "item_failure" | "not_found";
  importFailureCode?: string;
  importPreviewError?: boolean;
  importPreviewDelayMs?: number;
  importPreviewDeferred?: boolean;
  importConfirmDelayMs?: number;
  importDescription?: string;
  staleAccountReferences?: boolean;
  oauthCallbackBeforeStartReturns?: boolean;
  oauthCompletionDeferred?: boolean;
  oauthCompletionError?: boolean;
  distinctAccountIdentityHints?: boolean;
};

export type SourceMockOptions = {
  sourceCount?: number;
  sourceEnabled?: boolean;
  sourceRecoveryDelaySeconds?: number;
  sourceOperationalStatus?: AccountSummary["operationalStatus"];
  sourceSecretAvailable?: boolean;
  sourceDraining?: boolean;
  sourceStats?: SourceStats[];
  sourceStatsDelayMs?: number[];
  sourceStatsById?: Record<string, SourceStats>;
  sourceUpdateDelayMs?: number;
  sourceDetectedModelPrices?: Record<string, ApiModelPriceOverride>;
  sourceResolvedModelPrices?: Record<string, ResolvedModelPrice>;
  sourceErrorCode?: string;
  sourceRefreshError?: boolean;
  sourceCreateError?: string;
  sourceFollowupError?: "state" | "membership";
  catalogRefreshWarning?: "failed" | "deferred";
  sourceProtocolBindings?: SourceProtocolBinding[];
  sourceResolvedProtocolBindings?: SourceProtocolBinding[];
  sourceProtocolConfig?: SourceProtocolConfig;
};

export type UsageMockOptions = {
  weeklyCapacity?: WeeklyCapacityReport;
  weeklyCapacityError?: boolean;
  usageAccountIndex?: number;
  usagePresent?: boolean;
  usageActive?: boolean;
  usageFailure?: boolean;
  usageTiming?: { latencyMs: number; ttftMs: number | null; generationMs: number | null };
  usageUpstreamError?: UpstreamErrorDetails;
  usageUnpricedTokens?: number;
  usageCandidateKind?: "account" | "source";
  usageRequestedModel?: string;
  usageResolvedModel?: string;
  usageUpstreamResponseModel?: string;
  usageEndpointKind?: string;
  usageCacheContext?: CacheContextDiagnostics;
  usageCacheAffinity?: RoutingDiagnostics["cacheAffinity"];
  activeModelCounts?: Array<{ model: string; requestCount: number }>;
  usageToolDiagnostics?: "forwarded_text_only" | "dropped_text_only";
  usageTotalPages?: number;
  usageFilterOptionsVersion?: "full" | "advanced" | "missing";
  usageFilterHistory?: Array<{
    wireApi: NonNullable<UsageQuery["wireApi"]>;
    transport: UsageTransport;
    errorCategory: string | null;
    ageDays?: number;
    requestedModel?: string;
    resolvedModel?: string;
    accountId?: string | null;
    sourceId?: string;
    success?: boolean;
    inputTokens?: number;
    cachedInputTokens?: number | null;
    outputTokens?: number;
  }>;
  usageSummaryTotals?: Partial<UsageTotals>;
  usageAttempts?: boolean;
  usageAttemptsFail?: boolean;
  usageAttemptOutcome?: RoutingDiagnostics["attemptOutcome"];
  usageAttemptsPartial?: boolean;
  usageRepeatedRequests?: boolean;
  cachePreview?: boolean;
  usageCache?: { cachedInputTokens?: number | null; cacheWriteInputTokens?: number | null; cacheWriteTtl?: string | null };
  cacheSessions?: CacheSessionRecord[];
};

export type ProfileMockOptions = {
  profileSnapshots?: boolean;
  claudeDesktopConfigured?: boolean;
  profileSwitchError?: boolean;
  canonicalProfilePath?: boolean;
  profileSwitchBackupPrompt?: boolean;
  codexBackgroundTasksEnabled?: boolean;
  codexMemoryModel?: string | null;
  codexMemoryExtractionModel?: string | null;
  codexMemoryExtractionEnabled?: boolean;
  codexMemoryConsolidationEnabled?: boolean;
  codexMemoryModelFailure?: boolean;
  codexMemoryToggleFailure?: boolean;
  codexWebsocketsEnabled?: boolean;
  codexWebsocketsFailure?: "restart";
  basisPointsAvailable?: boolean;
  basisPointsEnabled?: boolean;
  codexTextModelsEnabled?: boolean;
  codexTextModelsFailure?: boolean;
  codexTextModelsSaveDelayMs?: number;
  codexTextModelsUnavailable?: boolean;
};

export type PoolMockOptions = {
  omittedPoolModels?: string[];
  nextCandidateId?: string | null;
  routingHeadroom?: Record<string, NonNullable<CandidateRuntimeSnapshot["resourceHeadroom"]>>;
  gatewayRunning?: boolean;
  rotationVersion?: 1 | 2 | 3;
  chatgptRetryUntilAvailable?: boolean;
  blockDegradedRoutesEnabled?: boolean;
  poolMembers?: boolean;
  mixedModels?: boolean;
  serverModelOrder?: string[];
  modelMetadata?: Record<string, Pick<ModelSummary,
    "catalogProvider" | "catalogFamily" | "catalogName" | "catalogSourceModelId"
    | "catalogReasoning" | "catalogReasoningMethod" | "catalogReasoningEffortLevels" | "catalogCapabilities"
  >>;
  modelOrderError?: boolean;
  modelOrderDelayMs?: number;
  modelContextDelayMs?: number;
  modelContextFailOnce?: boolean;
  modelSpeed?: Record<string, "standard" | "fast" | "ultrafast">;
  modelReasoning?: Record<string, string[]>;
  modelProtocolRoutes?: Record<string, ModelSummary["protocolRoutes"]>;
  modelContext?: Record<string, {
    configurable: boolean;
    policy?: { mode: "short" } | { mode: "extended" } | { mode: "custom"; tokens: number };
    contextLimit?: number;
    windows?: ModelSummary["contextWindows"];
    inputLimit?: number;
    outputLimit?: number;
  }>;
  manualReasoningFallbackModels?: string[];
};

export type ProxyMockOptions = {
  proxyCount?: number;
  commonProxyConfigured?: boolean;
  commonProxyId?: string | null;
  accountProxyRequired?: boolean;
  proxyCheckError?: string;
  proxyLocationError?: string;
  proxyCountryUnknown?: boolean;
  proxyCheckDelayMs?: number;
  proxySpeedDelayMs?: number;
  proxySpeedError?: string;
};

/** Existing flat scenario API, composed from independently owned domains. */
export type MockOptions = RuntimeMockOptions & AccountMockOptions & SourceMockOptions
  & UsageMockOptions & ProfileMockOptions & PoolMockOptions & ProxyMockOptions;
